[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [string]$OutputPath = (Join-Path $PSScriptRoot 'official-digests.json')
)

# A1 black-box oracle: runs the synthetic statements of cases.json once each,
# serially, on a fresh low-resource 1 FE + 3 BE StarRocks 3.5.14 fixture with
# session `enable_sql_digest = true`, then reads the official digest of every
# statement back from fe.audit.log by QueryId. Only case ids, digests and
# states are written; statement text, query ids and the secret are not. The
# fixture is torn down in `finally`, whatever happens.
$ErrorActionPreference = 'Stop'
$fixtureRoot = Join-Path $PSScriptRoot '..\..\..\..\..\dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureRoot 'common.ps1')

$docker = Get-FixtureDockerExecutable $DockerExecutable
$spec = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'cases.json') -Raw -Encoding utf8 | ConvertFrom-Json
$ids = @($spec.cases | ForEach-Object { $_.id })
if (($ids | Sort-Object -Unique).Count -ne $ids.Count) { throw 'cases.json has duplicate ids.' }
foreach ($case in $spec.cases) {
    if ($case.id -notmatch '^[a-z0-9_]+$') { throw "Case id $($case.id) is not [a-z0-9_]." }
    if ($case.sql.Contains('|')) { throw "Case $($case.id) contains '|', which the audit log uses as a separator." }
}

$generatedSecret = $false
$summary = [ordered]@{ schema = 'a1-official-digests-v1'; started_at = [DateTime]::UtcNow.ToString('o') }
try {
    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))) {
        $bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', [Convert]::ToBase64String($bytes).Replace('+', '-').Replace('/', '_').TrimEnd('='), 'Process')
        $generatedSecret = $true
    }
    & (Join-Path $fixtureRoot 'up.ps1') -DockerExecutable $docker -LowResource | Out-Null
    $password = Get-MultiBeRootPassword
    Invoke-MultiBeMysql $docker (($spec.setup | ForEach-Object { "$_;" }) -join "`n") $password | Out-Null

    # One session: the limits hold for every case; a failing case leaves
    # last_query_id() on the previous marker, which the audit check rejects.
    $batch = [Text.StringBuilder]::new()
    [void]$batch.AppendLine('SET enable_sql_digest = true;')
    [void]$batch.AppendLine('SET query_timeout = 15;')
    [void]$batch.AppendLine('SET query_mem_limit = 134217728;')
    foreach ($case in $spec.cases) {
        [void]$batch.AppendLine($case.sql)
        [void]$batch.AppendLine(';')
        [void]$batch.AppendLine("SELECT 'A1CASE', '$($case.id)', last_query_id();")
    }
    $previousMysqlPassword = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
    [Environment]::SetEnvironmentVariable('MYSQL_PWD', $password, 'Process')
    try {
        $output = $batch.ToString() | & $docker exec -i -e MYSQL_PWD $script:MultiBeFeName mysql --batch --skip-column-names --raw --comments --force --host=127.0.0.1 --port=9030 --user=root "--database=$($spec.database)" 2>&1
    }
    finally {
        [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previousMysqlPassword, 'Process')
    }
    $caseQueryIds = @{}
    $clientErrors = 0
    foreach ($line in @($output | ForEach-Object { [string]$_ })) {
        if ($line -match '^A1CASE\t([a-z0-9_]+)\t([0-9a-f-]{36})$') { $caseQueryIds[$Matches[1]] = $Matches[2] }
        elseif ($line -match '^ERROR ') { $clientErrors += 1 }
    }

    Start-Sleep -Seconds 3
    $audit = @{}
    $auditLines = @(Invoke-FixtureDocker $docker @('exec', $script:MultiBeFeName, 'cat', '/opt/starrocks/fe/log/fe.audit.log'))
    foreach ($line in $auditLines) {
        $text = [string]$line
        if ($text -notmatch '\|QueryId=([0-9a-f-]{36})') { continue }
        $queryId = $Matches[1]
        $digest = if ($text -match '\|Digest=([0-9a-fA-F]*)') { $Matches[1] } else { $null }
        $state = if ($text -match '\|State=([A-Z_]+)') { $Matches[1] } else { $null }
        $audit[$queryId] = [ordered]@{ digest = $digest; state = $state; marker = $text.Contains("'A1CASE'") }
    }

    $results = foreach ($case in $spec.cases) {
        $queryId = $caseQueryIds[$case.id]
        $entry = if ($queryId) { $audit[$queryId] } else { $null }
        $status = if (-not $queryId) { 'no_marker' }
            elseif (-not $entry) { 'not_in_audit_log' }
            elseif ($entry.marker) { 'statement_failed' }
            elseif ([string]::IsNullOrEmpty($entry.digest)) { 'no_digest' }
            else { 'ok' }
        [ordered]@{ id = $case.id; status = $status; state = if ($entry) { $entry.state } else { $null }; digest = if ($status -eq 'ok') { $entry.digest.ToLowerInvariant() } else { $null } }
    }
    $summary.fe_image = $script:MultiBeFeImage
    $summary.case_count = $spec.cases.Count
    $summary.ok_count = @($results | Where-Object { $_.status -eq 'ok' }).Count
    $summary.client_error_lines = $clientErrors
    $summary.cases = @($results)
}
finally {
    try { & (Join-Path $fixtureRoot 'teardown.ps1') -DockerExecutable $docker -Confirm:$false | Out-Null; $summary.teardown = 'ok' }
    catch { $summary.teardown = "failed: $($_.Exception.Message)" }
    $left = @(& $docker ps -a --filter "name=$script:MultiBePrefix" --format '{{.Names}}')
    $summary.containers_left = $left.Count
    $summary.network_left = @(& $docker network ls --filter "name=$script:MultiBeNetwork" --format '{{.Name}}').Count
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    $summary.finished_at = [DateTime]::UtcNow.ToString('o')
    $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding utf8
}
