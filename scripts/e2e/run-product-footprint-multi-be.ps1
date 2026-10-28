[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18064,
    [ValidateRange(15, 300)][int]$PhaseSeconds = 30,
    [ValidateRange(1, 4)][int]$Clients = 2,
    [ValidateRange(0, 100000)][int]$AuditSeedRows = 20000,
    [ValidateRange(1, 4)][int]$ConcurrentSessions = 2,
    [switch]$SkipBuild
)

# Product footprint on the isolated 1 FE + 3 BE fixture (v0.6-10). A logic
# check, not a performance test: small data, a light business workload, short
# phases. It asserts what the product sends to StarRocks while a business
# workload runs:
#   idle1 -> active (one engineer) -> idle2 -> concurrent (N engineers) -> idle3
# - with connections open but idle, the product sends no statement at all;
# - when used, every statement is read-only (SELECT / WITH / EXPLAIN / SHOW /
#   SET / ANALYZE PROFILE; never ANALYZE TABLE) and carries the product
#   marker, except the user-chosen Runtime Top SELECT;
# - product requests never fail with 5xx, also with concurrent sessions;
# - the business workload sees no error.
# Only statement classes and counts are read from the Audit table, never SQL.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$runDir = Join-Path $runtimeRoot "product-footprint-multi-be-$stamp"
$receiptPath = Join-Path $runtimeRoot "product-footprint-multi-be-$stamp.json"
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$clientName = "$script:MultiBePrefix-footprint"
$clientLabel = '3.5.14-multi-be-footprint'
$fixturePorts = @(38530, 39530)
$marker = '/* sr-doctor */ '
$readOnlyKeywords = @('SELECT', 'WITH', 'EXPLAIN', 'SHOW', 'SET')
# ANALYZE is read-only only as ANALYZE PROFILE (ANALYZE TABLE writes statistics).
function Test-ReadOnlyClass([string]$Words) { $first = ($Words -split ' ')[0]; ($first -in $readOnlyKeywords) -or ($Words -ceq 'ANALYZE PROFILE') }
$businessQueries = [ordered]@{
    q1_bucket_count = 'SELECT bucket, COUNT(*) FROM profile_rows GROUP BY bucket;'
    q2_payload_max = 'SELECT payload, MAX(id) FROM profile_rows WHERE bucket = 3 GROUP BY payload;'
    q3_prefix_count = "SELECT COUNT(*) FROM profile_rows WHERE payload LIKE 'p1%';"
}
$summary = [ordered]@{ project = 'product-footprint-multi-be'; status = 'FAIL'; evidence = 'logic_check_docker_not_performance_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null; $secret = $null; $startedApi = $null; $fixtureStarted = $false; $clientStarted = $false
$generatedSecret = $false; $primaryFailure = $false; $stage = 'initialization'; $jobs = @()
$previous = @{}
foreach ($name in @('STARROCKS_USER', 'STARROCKS_PASSWORD', 'DOCTOR_PORT', 'DOCTOR_HISTORY', 'DOCTOR_FRONTEND_DIST', 'MYSQL_PWD')) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$script:phaseMarks = @()

function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Test-PortListening([int]$Port) { $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) }
function Get-FootprintState($Docker) {
    $names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames) + @($clientName)
    [ordered]@{
        containers_present = @($names | Where-Object { $null -ne (Get-FixtureContainerIdentity -DockerExecutable $Docker -ContainerName $_) })
        network_present = [bool](@(& $Docker network ls --filter "name=^$($script:MultiBeNetwork)$" -q) | Where-Object { $_ })
        data_root_present = Test-Path -LiteralPath (Get-MultiBeDataRoot)
        listening_ports = @(@($fixturePorts + $ApiPort) | Where-Object { Test-PortListening $_ })
    }
}
function Invoke-Root([string]$Sql) { Invoke-MultiBeMysql $docker $Sql (Get-MultiBeRootPassword) }
function Set-Phase([string]$Phase) {
    [IO.File]::WriteAllText((Join-Path $runDir 'phase'), $Phase)
    Invoke-FixtureDocker $docker @('exec', $clientName, 'sh', '-c', "echo $Phase > /footprint/out/phase") -DiscardOutput
    $script:phaseMarks += [ordered]@{ phase = $Phase; utc = [DateTime]::UtcNow }
}
function Get-Window([string]$Phase) {
    $index = [array]::FindIndex([object[]]$script:phaseMarks, [Predicate[object]] { param($m) $m.phase -eq $Phase })
    # Two seconds of margin on each side keep clock skew out of the verdict.
    @($script:phaseMarks[$index].utc.AddSeconds(2), $script:phaseMarks[$index + 1].utc.AddSeconds(-2))
}
function Format-Utc([DateTime]$Value) { $Value.ToString('yyyy-MM-dd HH:mm:ss') }
# StrictMode-safe sum: Measure-Object yields nothing for an empty input.
function Get-CountSum($Rows) { $total = [long]0; foreach ($row in @($Rows)) { $total += [long]$row.count }; $total }

try {
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $stage = 'cold_start_preconditions'
    $before = Get-FootprintState $docker
    $allin1Absent = $null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName 'sr-doctor-v0.1-e2e-3514')
    $summary.preconditions = [ordered]@{
        containers_absent = ($before.containers_present.Count -eq 0); network_absent = (-not $before.network_present)
        data_root_absent = (-not $before.data_root_present); ports_free = ($before.listening_ports.Count -eq 0); release_allin1_fixture_absent = $allin1Absent
    }
    Require (@($summary.preconditions.Values | Where-Object { -not $_ }).Count -eq 0) 'Footprint check requires a cold start with the allin1 release fixture absent.'

    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))) {
        $bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', [Convert]::ToBase64String($bytes).Replace('+', '-').Replace('/', '_').TrimEnd('='), 'Process')
        $generatedSecret = $true
    }
    $secret = Get-FixtureSecret

    if (-not $SkipBuild) {
        $stage = 'repository_build'
        & cargo build --locked -p doctor-api
        if ($LASTEXITCODE -ne 0) { throw 'Repository API build failed.' }
    }
    $apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    Require (Test-Path -LiteralPath $apiExecutable -PathType Leaf) 'doctor-api.exe is missing.'
    $summary.repository_build = if ($SkipBuild) { 'skipped_by_request' } else { 'rebuilt_from_current_source' }
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.git_dirty = [bool](& git -C $repoRoot status --porcelain)
    $summary.api_sha256 = Get-Sha256 $apiExecutable
    New-Item -ItemType Directory -Force -Path $runDir | Out-Null

    $stage = 'fixture_up'
    $fixtureStarted = $true
    $up = (& (Join-Path $fixtureDir 'up.ps1') -DockerExecutable $docker) | ConvertFrom-Json
    $stage = 'fixture_connected'
    (& (Join-Path $fixtureDir 'connected-fixture.ps1') -DockerExecutable $docker) | ConvertFrom-Json | Out-Null

    # A small synthetic Audit history so the 30-day reads cross real partitions.
    $stage = 'audit_history_seed'
    if ($AuditSeedRows -gt 0) {
        Invoke-Root @"
INSERT INTO starrocks_audit_db__.starrocks_audit_tbl__
SELECT concat(lpad(hex(g), 8, '0'), '-0000-4000-8000-', lpad(hex(g), 12, '0')),
  seconds_sub(now(), CAST(g * 97 % 2505600 AS INT)), 'query', '10.0.0.1',
  concat('app_', CAST(g % 20 AS VARCHAR)), concat('app_', CAST(g % 20 AS VARCHAR)), 'default_wg', 'default_catalog',
  concat('db_', CAST(g % 10 AS VARCHAR)), CASE WHEN g % 50 = 0 THEN 'ERR' ELSE 'EOF' END, '',
  g % 5000, (g % 1000) * 1048576, g % 100000, g % 100, (g % 5000) * 1000000, (g % 512) * 1048576,
  CAST(g % 1000 AS INT), 1, '10.235.14.2', concat('SELECT /* app */ v FROM t', CAST(g % 200 AS VARCHAR), ' WHERE k = ', CAST(g AS VARCHAR)),
  lpad(hex(g % 200), 32, '0'), 0, 0, 0, '', '', [], 'default_warehouse'
FROM (SELECT CAST(generate_series AS BIGINT) AS g FROM TABLE(generate_series(1, $AuditSeedRows))) x;
"@ | Out-Null
    }
    $summary.audit_seed_rows = $AuditSeedRows

    $stage = 'client_container'
    Require ($null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $clientName)) 'The footprint client container already exists.'
    $clientStarted = $true
    Invoke-FixtureDocker $docker @('run', '-d', '--name', $clientName, '--network', $script:MultiBeNetwork,
        '--label', "$($script:FixtureLabelKey)=$clientLabel", '--cpus', '1', '--memory', '512m', '--pids-limit', '256',
        '--security-opt', 'no-new-privileges', '--entrypoint', 'sleep', $script:MultiBeFeImage, 'infinity') -DiscardOutput
    Invoke-FixtureDocker $docker @('exec', $clientName, 'mkdir', '-p', '/footprint/queries', '/footprint/out') -DiscardOutput
    Invoke-FixtureDocker $docker @('cp', (Join-Path $fixtureDir 'footprint\workload.sh'), "$clientName`:/footprint/workload.sh") -DiscardOutput
    foreach ($entry in $businessQueries.GetEnumerator()) {
        $path = Join-Path $runDir "$($entry.Key).sql"
        [IO.File]::WriteAllText($path, $entry.Value + "`n", [Text.UTF8Encoding]::new($false))
        Invoke-FixtureDocker $docker @('cp', $path, "$clientName`:/footprint/queries/$($entry.Key).sql") -DiscardOutput
    }

    $stage = 'api_start'
    Require (-not (Test-PortListening $ApiPort)) "API port $ApiPort is already in use."
    $env:DOCTOR_PORT = [string]$ApiPort
    # History is off: these checks predate stage B; B-D have their own E2Es.
    $env:DOCTOR_HISTORY = 'off'
    $env:DOCTOR_FRONTEND_DIST = $null
    $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $runDir 'doctor-api.stdout.log') -RedirectStandardError (Join-Path $runDir 'doctor-api.stderr.log')
    $deadline = [DateTime]::UtcNow.AddSeconds(45); $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) {
        try { $capabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 }
    }
    Require ($null -ne $capabilities) 'The local Doctor API did not become ready.'
    $nonce = $capabilities.csrf.nonce
    $headers = @{ 'X-Doctor-CSRF' = $nonce }
    $readerPassword = Get-FixturePassword $secret 'doctor_reader'
    $connections = @(1..$ConcurrentSessions | ForEach-Object {
        Invoke-RestMethod "$apiBase/connections" -Method Post -Headers $headers -ContentType 'application/json' -Body (@{
            host = '127.0.0.1'; port = $up.host_ports.mysql; username = 'doctor_reader'; password = $readerPassword
            catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext' } | ConvertTo-Json -Compress)
    })
    $readerPassword = $null

    $stage = 'workload_start'
    Set-Phase 'warmup'
    $env:MYSQL_PWD = Get-FixturePassword $secret 'workload_user'
    $feHost = Get-MultiBeHostname $script:MultiBeFeName
    foreach ($client in 0..($Clients - 1)) {
        $profileFlag = if ($client -eq 0) { '1' } else { '0' }
        Invoke-FixtureDocker $docker @('exec', '-d', '-e', 'MYSQL_PWD', $clientName, 'bash', '/footprint/workload.sh', [string]$client, '/footprint/queries', '/footprint/out', $feHost, '9030', $profileFlag) -DiscardOutput
    }
    $env:MYSQL_PWD = $previous['MYSQL_PWD']
    # Connection-time probes stay outside every measured window.
    Start-Sleep -Seconds 10

    $stage = 'phases'
    foreach ($step in @(@{ phase = 'idle1'; sessions = 0 }, @{ phase = 'active'; sessions = 1 }, @{ phase = 'idle2'; sessions = 0 },
                        @{ phase = 'concurrent'; sessions = $ConcurrentSessions }, @{ phase = 'idle3'; sessions = 0 })) {
        Set-Phase $step.phase
        $jobs = @(0..($step.sessions - 1) | Where-Object { $step.sessions -gt 0 } | ForEach-Object {
            Start-Job -FilePath (Join-Path $PSScriptRoot 'footprint-product-driver.ps1') -ArgumentList @($ApiPort, $connections[$_].connection_id, $nonce, "s$($_ + 1)", $runDir, $step.phase, 1, 1) })
        Start-Sleep -Seconds $PhaseSeconds
        if ($jobs.Count -gt 0) {
            Set-Phase "cooldown-$($step.phase)"
            $jobs | Wait-Job -Timeout 120 | Out-Null
            $jobs | Receive-Job -ErrorAction SilentlyContinue | Out-Null
            $jobs | Remove-Job -Force
            $jobs = @()
            Start-Sleep -Seconds 10
        }
    }
    Set-Phase 'done'
    Invoke-FixtureDocker $docker @('exec', $clientName, 'touch', '/footprint/out/stop') -DiscardOutput
    # AuditLoader publishes in batches; let the last ones land.
    Start-Sleep -Seconds 20

    $stage = 'collect'
    Invoke-FixtureDocker $docker @('cp', "$clientName`:/footprint/out", (Join-Path $runDir 'business')) -DiscardOutput
    $business = @(Get-ChildItem -LiteralPath (Join-Path $runDir 'business') -Filter 'client-*.csv' | ForEach-Object { Get-Content -LiteralPath $_.FullName } |
        ForEach-Object { $f = $_ -split ','; [pscustomobject]@{ phase = $f[0]; code = [int]$f[4] } })
    $product = @(Get-ChildItem -LiteralPath $runDir -Filter 'product-*.csv' | ForEach-Object { Get-Content -LiteralPath $_.FullName } |
        ForEach-Object { $f = $_ -split ','; [pscustomobject]@{ phase = $f[0]; action = $f[2]; status = [int]$f[4] } })

    $checks = [ordered]@{}
    $phases = [ordered]@{}
    foreach ($phase in @('idle1', 'active', 'idle2', 'concurrent', 'idle3')) {
        $window = Get-Window $phase
        $from = Format-Utc $window[0]; $to = Format-Utc $window[1]
        # Statement class only: marker present, first two words after the marker.
        $classes = @(Invoke-Root @"
SELECT CASE WHEN stmt LIKE '$marker%' THEN 'marked' ELSE 'unmarked' END,
  UPPER(SUBSTRING_INDEX(TRIM(REPLACE(stmt, '$marker', '')), ' ', 2)), COUNT(*)
FROM starrocks_audit_db__.starrocks_audit_tbl__
WHERE user = 'doctor_reader' AND ``timestamp`` >= '$from' AND ``timestamp`` < '$to'
GROUP BY 1, 2 ORDER BY 1, 2;
"@ | Where-Object { $_.Trim() } | ForEach-Object {
            $f = $_ -split "`t"
            # Keep keywords only: a literal (a query id, a quoted name) never reaches the receipt.
            $keyword = if ($f[1] -match '^([A-Z_]+)(?:\s+([A-Z_]+)\b(?![(''`]))?') { (@($Matches[1], $Matches[2]) | Where-Object { $_ }) -join ' ' } else { '?' }
            [pscustomobject]@{ marked = $f[0]; keyword = $keyword; count = [long]$f[2] } })
        $businessRows = @($business | Where-Object { $_.phase -eq $phase })
        $productRows = @($product | Where-Object { $_.phase -eq $phase })
        $phases[$phase] = [ordered]@{
            window_utc = "$from .. $to"
            product_statements = Get-CountSum $classes
            product_statement_classes = @($classes | ForEach-Object { "$($_.marked):$($_.keyword)=$($_.count)" })
            product_requests = $productRows.Count
            product_statuses = [ordered]@{}
            business_statements = $businessRows.Count
            business_errors = @($businessRows | Where-Object { $_.code -ne 0 }).Count
        }
        foreach ($status in @($productRows | Group-Object status)) { $phases[$phase].product_statuses[[string]$status.Name] = $status.Count }
        $phases[$phase].classes = $classes
    }

    foreach ($phase in @('idle1', 'idle2', 'idle3')) {
        $checks["$($phase)_no_product_statements"] = ($phases[$phase].product_statements -eq 0)
    }
    foreach ($phase in @('active', 'concurrent')) {
        $p = $phases[$phase]
        $checks["$($phase)_product_used"] = ($p.product_requests -gt 0 -and $p.product_statements -gt 0)
        $checks["$($phase)_read_only_keywords"] = (@($p.classes | Where-Object { -not (Test-ReadOnlyClass $_.keyword) }).Count -eq 0)
        $runtimeCalls = @($product | Where-Object { $_.phase -eq $phase -and $_.action -eq 'runtime_top' }).Count
        $unmarked = @($p.classes | Where-Object { $_.marked -eq 'unmarked' })
        $checks["$($phase)_unmarked_only_runtime_top_select"] = (@($unmarked | Where-Object { ($_.keyword -split ' ')[0] -notin @('SELECT', 'WITH') }).Count -eq 0) -and
            ((Get-CountSum $unmarked) -le $runtimeCalls)
        $checks["$($phase)_no_5xx"] = (@($product | Where-Object { $_.phase -eq $phase -and ($_.status -ge 500 -or $_.status -eq 0) }).Count -eq 0)
    }
    foreach ($phase in $phases.Keys) {
        $checks["$($phase)_business_ran_without_error"] = ($phases[$phase].business_statements -gt 0 -and $phases[$phase].business_errors -eq 0)
        $phases[$phase].Remove('classes')
    }
    $summary.phases = $phases
    $summary.checks = $checks
    $summary.parameters = [ordered]@{ phase_seconds = $PhaseSeconds; clients = $Clients; concurrent_sessions = $ConcurrentSessions; audit_seed_rows = $AuditSeedRows }
    $failed = @($checks.GetEnumerator() | Where-Object { -not $_.Value } | ForEach-Object { $_.Key })
    $summary.failed_checks = $failed
    Require ($failed.Count -eq 0) "Footprint checks failed: $($failed -join ', ')"
    $summary.status = 'PASS'
}
catch {
    $primaryFailure = $true
    $message = $_.Exception.Message
    $summary.failure = [ordered]@{ stage = $stage; message_id = 'footprint_stage_failed'; detail = $message.Substring(0, [math]::Min(300, $message.Length)) }
    throw
}
finally {
    $cleanupFailure = $null
    if ($runDir -and (Test-Path -LiteralPath $runDir)) { [IO.File]::WriteAllText((Join-Path $runDir 'phase'), 'done') }
    foreach ($job in @($jobs)) { if ($null -ne $job) { $job | Stop-Job -ErrorAction SilentlyContinue; $job | Remove-Job -Force -ErrorAction SilentlyContinue } }
    if ($startedApi -and -not $startedApi.HasExited) {
        try { Stop-Process -Id $startedApi.Id -Force -ErrorAction Stop; $startedApi.WaitForExit(5000) | Out-Null } catch { $cleanupFailure = 'api_stop' }
    }
    if ($startedApi -and $startedApi.HasExited -and $null -ne $secret) {
        $needles = @($secret, (Get-FixturePassword $secret 'doctor_reader'), (Get-FixturePassword $secret 'workload_user'), 'footprint_runtime')
        $logs = @("stdout", "stderr" | ForEach-Object { Join-Path $runDir "doctor-api.$_.log" } | Where-Object { Test-Path -LiteralPath $_ })
        $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 }).Count
        $summary.api_log_scan = [ordered]@{ status = $(if ($logs.Count -ne 2) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }); files = $logs.Count }
        if ($summary.api_log_scan.status -ne 'PASS' -and -not $cleanupFailure) { $cleanupFailure = 'api_log_scan' }
    }
    if ($clientStarted -and $null -ne $docker) {
        # Only the owned, labelled client container is removed here.
        $identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $clientName
        if ($null -ne $identity) {
            if ($identity.Image -ceq $script:MultiBeFeImage -and $identity.Label -ceq $clientLabel) {
                try { Invoke-FixtureDocker $docker @('rm', '-f', $clientName) -DiscardOutput } catch { $cleanupFailure = 'client_container_remove' }
            } else { $cleanupFailure = 'client_container_not_owned' }
        }
    }
    if ($fixtureStarted) {
        try { $summary.teardown = (& (Join-Path $fixtureDir 'teardown.ps1') -DockerExecutable $docker) | ConvertFrom-Json }
        catch { $cleanupFailure = 'fixture_teardown' }
    }
    foreach ($name in $previous.Keys) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    if ($null -ne $docker) {
        $after = Get-FootprintState $docker
        $summary.postconditions = [ordered]@{
            containers_absent = ($after.containers_present.Count -eq 0); network_absent = (-not $after.network_present)
            data_root_absent = (-not $after.data_root_present); ports_free = ($after.listening_ports.Count -eq 0)
        }
        if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    }
    if ($cleanupFailure) {
        $summary.status = 'FAIL'
        $summary.cleanup_failure = [ordered]@{ stage = $cleanupFailure; message_id = 'footprint_teardown_failed' }
    }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Footprint teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
