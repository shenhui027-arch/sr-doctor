[CmdletBinding()]
param(
    # Test infrastructure only: captures real 3.5.14 Profiles in the states the
    # product must fail closed on (Level 2 unmerged, still-running snapshot)
    # from the isolated 1 FE + 3 BE fixture, plus the FE's own ANALYZE PROFILE
    # answer for each. It requires the oracle corpus dataset to be loaded
    # (capture-oracle-corpus.ps1 -Topology multi-be ran first).
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(5, 120)][int]$ProfilePollSeconds = 60,
    [ValidateRange(10, 300)][int]$RunningQueryTimeoutSeconds = 240
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be\common.ps1')
$stateRoot = Join-Path $repoRoot 'fixtures\runtime\3.5.14\profile-states'
New-Item -ItemType Directory -Force -Path $stateRoot | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$containerName = $script:MultiBeFeName
$rootPassword = Get-MultiBeRootPassword

$docker = Get-FixtureDockerExecutable $DockerExecutable
$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $containerName
if ($null -eq $identity -or $identity.Image -cne $script:MultiBeFeImage -or $identity.Label -cne $script:MultiBeLabelValue) {
    throw 'The FE container is not the dedicated fixed-digest labelled multi-BE fixture.'
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('profile-states-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
Invoke-FixtureDocker $docker @('exec', $containerName, 'mkdir', '-p', '/tmp/states') -DiscardOutput

function Use-RootPassword([scriptblock]$Body) {
    $previous = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
    [Environment]::SetEnvironmentVariable('MYSQL_PWD', $rootPassword, 'Process')
    try { & $Body } finally { [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previous, 'Process') }
}

function Get-MysqlCommand([string]$Name, [string]$Database) {
    $dbArgument = if ($Database) { " --database=$Database" } else { '' }
    "mysql --batch --skip-column-names --raw --host=127.0.0.1 --port=9030 --user=root$dbArgument < /tmp/states/$Name.sql > /tmp/states/$Name.out 2> /tmp/states/$Name.err"
}

function Send-Batch([string]$Name, [string]$Sql) {
    $hostSql = Join-Path $work "$Name.sql"
    [IO.File]::WriteAllText($hostSql, $Sql.Replace("`r`n", "`n"), $utf8)
    Invoke-FixtureDocker $docker @('cp', $hostSql, "$containerName`:/tmp/states/$Name.sql") -DiscardOutput
}

# Same shape as capture-oracle-corpus.ps1: output is redirected in the
# container and copied out, so bytes never pass through a console encoding.
# `-AllowFailure` keeps a server error as data (exit code + error file).
function Invoke-Batch {
    param([string]$Name, [string]$Sql, [string]$Database = '', [switch]$AllowFailure)
    Send-Batch $Name $Sql
    $command = Get-MysqlCommand $Name $Database
    Use-RootPassword { & $docker exec -e MYSQL_PWD $containerName sh -c $command }
    $exitCode = $LASTEXITCODE
    $hostOut = Join-Path $work "$Name.out"
    $hostErr = Join-Path $work "$Name.err"
    Invoke-FixtureDocker $docker @('cp', "$containerName`:/tmp/states/$Name.out", $hostOut) -DiscardOutput
    Invoke-FixtureDocker $docker @('cp', "$containerName`:/tmp/states/$Name.err", $hostErr) -DiscardOutput
    if ($exitCode -ne 0 -and -not $AllowFailure) {
        $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @([IO.File]::ReadAllLines($hostErr, $utf8)) -ExitCode $exitCode
        throw "Batch $Name failed (exit $exitCode; code=$($diagnostic.error_code); line=$($diagnostic.line_number))."
    }
    [pscustomobject]@{ Out = $hostOut; Err = $hostErr; ExitCode = $exitCode }
}

function Read-Text([string]$Path) { [IO.File]::ReadAllText($Path, $utf8) }

$serverVersion = (Read-Text (Invoke-Batch 'version' 'SELECT current_version();').Out).Trim()
if ($serverVersion -notmatch '^3\.5\.14-23a56ec') { throw "Unexpected server version $serverVersion." }

function Wait-ProfileLength([string]$StateId, [string]$QueryId) {
    $deadline = [DateTime]::UtcNow.AddSeconds($ProfilePollSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $text = (Read-Text (Invoke-Batch "$StateId.len" "SELECT LENGTH(get_query_profile('$QueryId'));").Out).Trim()
        if ($text -match '^[1-9][0-9]*$') { return [long]$text }
        Start-Sleep -Seconds 1
    }
    throw "State $StateId Profile was not published within $ProfilePollSeconds s."
}

# Saves get_query_profile and ANALYZE PROFILE for one query id. ANALYZE PROFILE
# is kept even when the FE refuses it: its exit code and error text are the
# official answer for that state.
function Save-State([string]$StateId, [string]$QueryId, [string]$SqlFile, [hashtable]$Extra) {
    $profile = Invoke-Batch "$StateId.profile" "SELECT get_query_profile('$QueryId');"
    $analyze = Invoke-Batch "$StateId.analyze" "ANALYZE PROFILE FROM '$QueryId';" -AllowFailure
    $profileTarget = Join-Path $stateRoot "$StateId.profile.txt"
    $analyzeTarget = Join-Path $stateRoot "$StateId.analyze.txt"
    Copy-Item -LiteralPath $profile.Out -Destination $profileTarget -Force
    if ($analyze.ExitCode -eq 0) {
        Copy-Item -LiteralPath $analyze.Out -Destination $analyzeTarget -Force
    } else {
        # Only the MySQL error line: no host paths, no credentials.
        $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @([IO.File]::ReadAllLines($analyze.Err, $utf8)) -ExitCode $analyze.ExitCode
        $line = @([IO.File]::ReadAllLines($analyze.Err, $utf8) | Where-Object { $_ -match '^ERROR ' }) | Select-Object -First 1
        [IO.File]::WriteAllText($analyzeTarget, "$line`n", $utf8)
        $Extra['analyze_error_code'] = $diagnostic.error_code
    }
    $entry = [ordered]@{
        state_id = $StateId; query_id = $QueryId
        sql_file = $SqlFile; sql_sha256 = Get-FixtureSha256 (Join-Path $stateRoot $SqlFile)
        profile_file = "$StateId.profile.txt"; profile_sha256 = Get-FixtureSha256 $profileTarget
        analyze_file = "$StateId.analyze.txt"; analyze_sha256 = Get-FixtureSha256 $analyzeTarget
        analyze_exit_code = $analyze.ExitCode
        captured_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    }
    foreach ($key in $Extra.Keys) { $entry[$key] = $Extra[$key] }
    Write-Host "captured $StateId ($QueryId)"
    $entry
}

$states = @()

# 1. Level 2: the FE does not merge isomorphic instances (needMergeProfile()
# is false), so every instance / driver is printed on its own.
foreach ($level2 in @(
    @{ id = 's01_level2_shuffle_join'; sql = 's01_level2_shuffle_join.sql' },
    @{ id = 's02_level2_multi_instance_agg'; sql = 's02_level2_multi_instance_agg.sql' }
)) {
    $caseSql = Read-Text (Join-Path $stateRoot $level2.sql)
    $batch = "SET enable_profile=true;`nSET pipeline_profile_level=2;`n$($caseSql.TrimEnd())`nSELECT last_query_id();`n"
    $run = Invoke-Batch "$($level2.id).run" $batch 'oracle_corpus'
    $queryId = ([IO.File]::ReadAllLines($run.Out, $utf8) | Select-Object -Last 1).Trim().ToLowerInvariant()
    if ($queryId -notmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') { throw "$($level2.id) returned no query id." }
    [void](Wait-ProfileLength $level2.id $queryId)
    $states += Save-State $level2.id $queryId $level2.sql @{ session = @('SET enable_profile=true', 'SET pipeline_profile_level=2'); query_state_at_capture = 'Finished' }
}

# 2. Running snapshot: a deliberately slow query (SLEEP per row) with a 1 s
# runtime report interval. Its query id is found by its unique marker in
# SHOW PROFILELIST while the row still says Running; both answers are taken
# before it finishes, then the final Profile of the same query is kept too.
# The marker is a string literal: SHOW PROFILELIST prints the statement with
# comments stripped.
$runningId = 's03_running_snapshot'
$marker = 's03_' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$runningSqlFile = "$runningId.sql"
$runningSqlTemplate = (Read-Text (Join-Path $stateRoot $runningSqlFile)).TrimEnd()
$runningSql = $runningSqlTemplate.Replace('__MARKER__', $marker)
if ($runningSql -ceq $runningSqlTemplate) { throw 'Running state SQL has no __MARKER__ placeholder.' }
$runningBatchName = "$runningId.run"
Send-Batch $runningBatchName ("SET enable_profile=true;`nSET pipeline_profile_level=1;`nSET runtime_profile_report_interval=1;`nSET query_timeout=$RunningQueryTimeoutSeconds;`n$runningSql`n")
$runningCommand = Get-MysqlCommand $runningBatchName 'oracle_corpus'
$process = Use-RootPassword {
    Start-Process -FilePath $docker -ArgumentList @('exec', '-e', 'MYSQL_PWD', $containerName, 'sh', '-c', "`"$runningCommand`"") -NoNewWindow -PassThru
}
$runningStarted = [DateTime]::UtcNow
try {
    $queryId = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($null -eq $queryId -and [DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
        $rows = [IO.File]::ReadAllLines((Invoke-Batch "$runningId.list" 'SHOW PROFILELIST LIMIT 30;').Out, $utf8)
        $hits = @($rows | ForEach-Object {
            $columns = $_ -split "`t"
            if ($columns.Count -ge 5 -and $columns[3] -ieq 'Running' -and $columns[4].Contains($marker)) { $columns[0].ToLowerInvariant() }
        })
        if ($hits.Count -gt 1) { throw 'Running marker matched more than one query.' }
        if ($hits.Count -eq 1) { $queryId = $hits[0] } else { Start-Sleep -Milliseconds 500 }
    }
    if ($null -eq $queryId) { throw 'The controlled running query was not observed as Running.' }
    Write-Host "observed Running after $([int]([DateTime]::UtcNow - $runningStarted).TotalSeconds) s"
    # Let another runtime report reach the FE.
    Start-Sleep -Seconds 2
    [void](Wait-ProfileLength $runningId $queryId)
    $listed = [IO.File]::ReadAllLines((Invoke-Batch "$runningId.list2" 'SHOW PROFILELIST LIMIT 30;').Out, $utf8)
    $stillRunning = @($listed | Where-Object { ($_ -split "`t")[0] -ieq $queryId -and ($_ -split "`t")[3] -ieq 'Running' }).Count -eq 1
    if (-not $stillRunning) { throw 'The controlled query finished before its running snapshot was taken; raise the SLEEP budget.' }
    $states += Save-State $runningId $queryId $runningSqlFile @{
        session = @('SET enable_profile=true', 'SET pipeline_profile_level=1', 'SET runtime_profile_report_interval=1', "SET query_timeout=$RunningQueryTimeoutSeconds")
        query_state_at_capture = 'Running'; sql_marker_placeholder = '__MARKER__'
    }
    Write-Host "running snapshot taken after $([int]([DateTime]::UtcNow - $runningStarted).TotalSeconds) s"
    if (-not $process.WaitForExit(($RunningQueryTimeoutSeconds + 30) * 1000)) { throw 'The controlled running query did not finish.' }
    if ($process.ExitCode -ne 0) { throw "The controlled running query failed (exit $($process.ExitCode))." }
    Start-Sleep -Seconds 2
    $finalId = 's04_running_query_final'
    [void](Wait-ProfileLength $finalId $queryId)
    $states += Save-State $finalId $queryId $runningSqlFile @{
        session = @('SET enable_profile=true', 'SET pipeline_profile_level=1', 'SET runtime_profile_report_interval=1', "SET query_timeout=$RunningQueryTimeoutSeconds")
        query_state_at_capture = 'Finished'; same_query_as = $runningId; sql_marker_placeholder = '__MARKER__'
    }
}
finally {
    if (-not $process.HasExited) { $process.Kill() }
}

$manifest = [ordered]@{
    schema = 'sr-doctor.profile-states.v1'
    purpose = 'real StarRocks 3.5.14 Profiles in states the product must fail closed on, with the FE ANALYZE PROFILE answer'
    server_version = $serverVersion
    topology = [ordered]@{ kind = 'split_fe_be'; frontends = 1; backends = $script:MultiBeBackendCount; fe_image = $script:MultiBeFeImage; be_image = $script:MultiBeBeImage }
    dataset = 'fixtures/runtime/3.5.14/oracle-corpus-multi-be (dataset.sql + dataset-*.sql)'
    states = $states
}
[IO.File]::WriteAllText((Join-Path $stateRoot 'manifest.json'), ($manifest | ConvertTo-Json -Depth 6) + "`n", $utf8)
Remove-Item -LiteralPath $work -Recurse -Force
Invoke-FixtureDocker $docker @('exec', $containerName, 'rm', '-rf', '/tmp/states') -DiscardOutput
[ordered]@{ status = 'PASS'; server_version = $serverVersion; states = $states.Count } | ConvertTo-Json -Compress
