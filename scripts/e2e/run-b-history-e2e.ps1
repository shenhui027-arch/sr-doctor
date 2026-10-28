[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18064,
    [switch]$SkipBuild,
    # Keep the fixture and API up this long after the checks so a browser can
    # sign in through serve-password-once.mjs (127.0.0.1:18099) and capture
    # board B. GET http://127.0.0.1:18099/release ends the hold.
    [ValidateRange(0, 1800)][int]$HoldForBrowserSeconds = 0
)

# Stage B local acceptance on the low-resource 1 FE + 3 BE fixture: the
# bounded collector reads the real AuditLoader table into the local SQLite
# history with short development timings, pauses, survives a process restart
# from its watermark without re-reading a slice, and never writes a literal or
# a credential to disk. Serial, bounded, torn down in `finally`.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "b-history-$stamp.json"
$historyDir = Join-Path $runtimeRoot "b-history-$stamp"
$historyPath = Join-Path $historyDir 'history.sqlite'
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$fixturePorts = @(38530, 39530)
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)
$stage = 'initialization'
$summary = [ordered]@{ project = 'b-history-collection-multi-be-low-resource'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null; $secret = $null; $startedApi = $null; $fixtureStarted = $false; $primaryFailure = $false; $generatedSecret = $false; $apiRun = 0
$secretLiteral = 'b_history_secret_literal'
$timings = [ordered]@{ DOCTOR_COLLECTION_PERIOD_SECONDS = '5'; DOCTOR_COLLECTION_SLICE_SECONDS = '60'; DOCTOR_COLLECTION_LAG_SECONDS = '90'; DOCTOR_COLLECTION_BACKFILL_SECONDS = '600' }

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Test-PortListening([int]$Port) { $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) }
function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-FixtureState($Docker) {
    [ordered]@{
        containers_present = @($names | Where-Object { $null -ne (Get-MultiBeIdentity -DockerExecutable $Docker -Name $_) })
        network_present = ($null -ne (Get-MultiBeNetworkLabel -DockerExecutable $Docker))
        data_root_present = (Test-Path -LiteralPath (Get-MultiBeDataRoot))
        listening_ports = @(@($fixturePorts + $ApiPort) | Where-Object { Test-PortListening $_ })
    }
}
function Start-DoctorApi {
    $script:apiRun += 1
    Require (-not (Test-PortListening $ApiPort)) "API port $ApiPort is already in use."
    $saved = @{}
    $environment = [ordered]@{ DOCTOR_PORT = [string]$ApiPort; DOCTOR_HISTORY_PATH = $historyPath }
    foreach ($key in $timings.Keys) { $environment[$key] = $timings[$key] }
    $environment.DOCTOR_FRONTEND_DIST = if ($HoldForBrowserSeconds -gt 0) { (Resolve-Path -LiteralPath (Join-Path $repoRoot 'frontend\dist')).Path } else { $null }
    foreach ($key in $environment.Keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process'); [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process') }
    try {
        $script:startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-b-$stamp-$($script:apiRun).stdout.log") `
            -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-b-$stamp-$($script:apiRun).stderr.log")
    }
    finally { foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process') } }
    $deadline = [DateTime]::UtcNow.AddSeconds(45); $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) {
        try { $capabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 }
    }
    Require ($null -ne $capabilities) 'The local Doctor API did not become ready.'
    $script:headers = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }
}
function Stop-DoctorApi {
    if ($script:startedApi -and -not $script:startedApi.HasExited) { Stop-Process -Id $script:startedApi.Id -Force; $script:startedApi.WaitForExit(5000) | Out-Null }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ((Test-PortListening $ApiPort) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
}
function Invoke-Post([string]$Path, $Payload) {
    $response = Invoke-WebRequest "$apiBase$Path" -Method Post -Headers $script:headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 20 -Compress) -UseBasicParsing
    if ($response.Content) { $response.Content | ConvertFrom-Json } else { $null }
}
function Connect-Reader { (Invoke-Post '/connections' @{ host = '127.0.0.1'; port = $up.host_ports.mysql; username = 'doctor_reader'; password = $readerPassword; catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext' }).connection_id }
function Get-Status([string]$ConnectionId) { Invoke-Post '/history/status' @{ connection_id = $ConnectionId } }
# Waits until the watermark passes the newest audited hist_e2e statement.
function Wait-Watermark([string]$ConnectionId, [int64]$After, [int]$Seconds = 240) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $status = Get-Status $ConnectionId
        if ($null -ne $status.watermark_seconds -and [int64]$status.watermark_seconds -gt $After) { return $status }
        Start-Sleep -Seconds 3
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "The watermark did not pass $After (state $($status.collection.state), watermark $($status.watermark_seconds))."
}
function Invoke-Workload([string[]]$Statements, [int]$Expected) {
    $limits = "USE hist_e2e;`nSET query_timeout = 15;`nSET query_mem_limit = 134217728;`nSET enable_sql_digest = true;`n"
    Invoke-MultiBeMysql $docker ($limits + (($Statements | ForEach-Object { "$_;" }) -join "`n")) $root | Out-Null
    $countSql = "SELECT COUNT(*), UNIX_TIMESTAMP(MAX(``timestamp``)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'hist_e2e' AND isQuery = 1;"
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do { Start-Sleep -Seconds 2; $row = @((@(Invoke-MultiBeMysql $docker $countSql $root)[0]) -split "`t") } while ([int]$row[0] -lt $Expected -and [DateTime]::UtcNow -lt $deadline)
    Require ([int]$row[0] -eq $Expected) "Expected $Expected audited hist_e2e queries, found $($row[0])."
    [int64]$row[1]
}
function Get-History { (& node --no-warnings (Join-Path $PSScriptRoot 'inspect-history.mjs') $historyPath 'hist_e2e') | ConvertFrom-Json }
# The API keeps the database open; read it the way SQLite shares it.
function Read-Shared([string]$Path) {
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    try { $buffer = [IO.MemoryStream]::new(); $stream.CopyTo($buffer); [Text.Encoding]::Latin1.GetString($buffer.ToArray()) } finally { $stream.Dispose() }
}
function Test-NoSecretsOnDisk {
    $needles = @($secretLiteral, $readerPassword, $secret)
    $files = @(Get-ChildItem -LiteralPath $historyDir -File)
    $hits = @($files | Where-Object { $text = Read-Shared $_.FullName; @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 })
    [ordered]@{ files = @($files | ForEach-Object { $_.Name }); bytes = ($files | Measure-Object Length -Sum).Sum; hits = $hits.Count }
}

# Three templates with literal variants: 3 + 2 executions before the API
# starts (backfill), 4 while paused, 2 more after a restart.
$first = @('SELECT id FROM t WHERE id = 1', 'SELECT id FROM t WHERE id = 2', 'SELECT id FROM t WHERE id = 3', "SELECT count(*) FROM t WHERE name = '$secretLiteral'", "SELECT count(*) FROM t WHERE name = 'x'")
$paused = @('SELECT name FROM t WHERE id IN (1, 2)', 'SELECT name FROM t WHERE id IN (3)', 'SELECT name FROM t WHERE id IN (1, 2, 3)', 'SELECT name FROM t WHERE id IN (2)')
$restarted = @('SELECT id FROM t WHERE id = 7', 'SELECT id FROM t WHERE id = 8')

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $summary.evidence_sources = [ordered]@{ runner_sha256 = Get-Sha256 $PSCommandPath; inspector_sha256 = Get-Sha256 (Join-Path $PSScriptRoot 'inspect-history.mjs'); up_sha256 = Get-Sha256 (Join-Path $fixtureDir 'up.ps1'); connected_fixture_sha256 = Get-Sha256 (Join-Path $fixtureDir 'connected-fixture.ps1') }
    $summary.timings = $timings

    $stage = 'cold_start_preconditions'
    $before = Get-FixtureState $docker
    Require ($before.containers_present.Count -eq 0 -and -not $before.network_present -and -not $before.data_root_present -and $before.listening_ports.Count -eq 0) 'Stage B E2E requires a cold start.'
    Require ($null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName 'sr-doctor-v0.1-e2e-3514')) 'The allin1 release fixture must be absent.'
    Require (-not (Test-Path -LiteralPath $historyDir)) 'The history directory must not exist yet.'

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
        if ($HoldForBrowserSeconds -gt 0) {
            & npm --prefix (Join-Path $repoRoot 'frontend') run build
            if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
        }
    }
    $apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.git_dirty = [bool](& git -C $repoRoot status --porcelain)
    $summary.api_sha256 = Get-Sha256 $apiExecutable

    $stage = 'fixture_up'
    $fixtureStarted = $true
    $up = (& (Join-Path $fixtureDir 'up.ps1') -DockerExecutable $docker -LowResource) | ConvertFrom-Json
    $stage = 'fixture_connected_workload'
    $workload = (& (Join-Path $fixtureDir 'connected-fixture.ps1') -DockerExecutable $docker) | ConvertFrom-Json
    $summary.fixture = [ordered]@{ fe_image = $up.fe_image; be_image = $up.be_image; observed_version = $up.observed_version; backends_alive = $up.backends_alive; resources = $up.resources; audit_loader_version = $workload.audit_loader_version }

    $stage = 'b_workload_backfill'
    $root = Get-MultiBeRootPassword
    Invoke-MultiBeMysql $docker @'
CREATE DATABASE hist_e2e;
CREATE TABLE hist_e2e.t (id INT, name VARCHAR(32)) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 3 PROPERTIES ("replication_num" = "1");
INSERT INTO hist_e2e.t VALUES (1, 'a'), (2, 'b'), (3, 'c');
GRANT SELECT ON TABLE hist_e2e.t TO USER doctor_reader;
'@ $root | Out-Null
    $firstAt = Invoke-Workload $first $first.Count
    $readerPassword = Get-FixturePassword -Secret $secret -Role doctor_reader

    # RB-01: the collector starts with the session and backfills the window.
    $stage = 'rb01_collect'
    Start-DoctorApi
    $connection = Connect-Reader
    $status = Wait-Watermark $connection $firstAt
    $history = Get-History
    $summary.rb01_collect = [ordered]@{ state = $status.collection.state; target = $status.target; store = $status.store.state; templates = @($history.templates | ForEach-Object { "$($_.kind):$($_.executions)" }); runs = $history.runs; reread_slices = $history.reread_slices; production_budget_measured = $status.budget.production_budget_measured; coverage_hours = @($status.coverage).Count }
    Require ($status.collection.state -eq 'running' -and $status.target -eq "127.0.0.1:$($up.host_ports.mysql)") 'The collector is not running for the connected target.'
    Require ((@($history.templates | ForEach-Object { $_.executions }) -join ',') -eq '3,2') "Unexpected backfilled template counts $($summary.rb01_collect.templates -join ',')."
    Require (@($history.templates | Where-Object { $_.kind -ne 'official_digest' }).Count -eq 0) 'Templates should carry the official digest when the audit row has one.'
    Require (-not $status.budget.production_budget_measured -and @($status.coverage).Count -eq 24) 'Budget or coverage shape changed.'

    # RB-02: pause keeps the watermark; resume continues from it.
    $stage = 'rb02_pause_resume'
    Invoke-Post '/history/collection' @{ connection_id = $connection; action = 'pause' } | Out-Null
    $pausedStatus = Get-Status $connection
    $pausedWatermark = [int64]$pausedStatus.watermark_seconds
    $pausedAt = Invoke-Workload $paused ($first.Count + $paused.Count)
    Start-Sleep -Seconds 75
    $stillPaused = Get-Status $connection
    Require ($stillPaused.collection.state -eq 'paused' -and [int64]$stillPaused.watermark_seconds -eq $pausedWatermark) 'Collection advanced while paused.'
    Invoke-Post '/history/collection' @{ connection_id = $connection; action = 'resume' } | Out-Null
    $resumed = Wait-Watermark $connection $pausedAt
    $history = Get-History
    $summary.rb02_pause_resume = [ordered]@{ paused_state = $stillPaused.collection.state; paused_watermark_held = $true; resumed_state = $resumed.collection.state; templates = @($history.templates | ForEach-Object { "$($_.kind):$($_.executions)" }); reread_slices = $history.reread_slices }
    Require ((@($history.templates | ForEach-Object { $_.executions }) -join ',') -eq '4,3,2' -and $history.reread_slices -eq 0) "Unexpected counts after resume $($summary.rb02_pause_resume.templates -join ',')."

    # RB-02 / RB-05: a process restart forgets the session and credential; history and
    # the watermark survive, and a new session continues without re-reading.
    $stage = 'rb02_rb05_restart'
    $beforeRestart = [int64]$resumed.watermark_seconds
    Stop-DoctorApi
    Start-DoctorApi
    $cold = Get-Status $null
    Require ($cold.collection.state -eq 'stopped' -and $cold.collection.reason -eq 'not_connected' -and $cold.store.state -eq 'available' -and [int]$cold.store.templates -ge 3) 'History did not survive the restart.'
    $connection = Connect-Reader
    $reconnected = Get-Status $connection
    Require ([int64]$reconnected.watermark_seconds -ge $beforeRestart) 'The watermark did not survive the restart.'
    $restartedAt = Invoke-Workload $restarted ($first.Count + $paused.Count + $restarted.Count)
    $continued = Wait-Watermark $connection $restartedAt
    $history = Get-History
    $summary.rb02_rb05_restart_reconnect = [ordered]@{ cold_state = "$($cold.collection.state):$($cold.collection.reason)"; cold_templates = $cold.store.templates; watermark_kept = $true; templates = @($history.templates | ForEach-Object { "$($_.kind):$($_.executions)" }); reread_slices = $history.reread_slices; runs = $history.runs }
    Require ((@($history.templates | ForEach-Object { $_.executions }) -join ',') -eq '5,4,2' -and $history.reread_slices -eq 0) "Unexpected counts after restart $($summary.rb02_rb05_restart_reconnect.templates -join ',')."

    # RB-04 (local only): what one read cost on this fixture; the production
    # budget comes from G1.
    $stage = 'rb04_local_load'
    $reads = @($continued.runs | Where-Object { $_.state -in @('complete', 'partial') })
    $summary.rb04_local_load = [ordered]@{ runs_listed = $reads.Count; max_duration_ms = (@($reads | ForEach-Object { [int64]$_.duration_ms }) | Measure-Object -Maximum).Maximum; max_rows_read = (@($reads | ForEach-Object { [int64]$_.rows_read }) | Measure-Object -Maximum).Maximum; slice_row_limit = $continued.budget.slice_row_limit; query_timeout_seconds = $continued.budget.query_timeout_seconds; partial_reads = @($reads | Where-Object { $_.state -eq 'partial' }).Count; production_budget = 'pending_g1' }
    Require ($summary.rb04_local_load.max_rows_read -le $continued.budget.slice_row_limit -and $summary.rb04_local_load.max_duration_ms -lt 15000) 'A read exceeded the local budget.'

    # RB-03 / RB-05: nothing on disk holds a literal or a credential; closing
    # the session stops collection.
    $stage = 'rb03_rb05_disk'
    $disk = Test-NoSecretsOnDisk
    $summary.rb03_rb05_disk = [ordered]@{ files = $disk.files; bytes = $disk.bytes; literal_or_credential_hits = $disk.hits; templates = @($history.templates | ForEach-Object { $_.template }) }
    Require ($disk.hits -eq 0) 'The history files contain a literal or a credential.'
    Require (@($history.templates | Where-Object { $_.template -and $_.template.Contains($secretLiteral) }).Count -eq 0) 'A stored template holds a literal.'
    if ($HoldForBrowserSeconds -gt 0) {
        $stage = 'browser_hold'
        $env:A1_READER_PASSWORD = $readerPassword; $env:A1_PRODUCT_ORIGIN = "http://127.0.0.1:$ApiPort"; $env:A1_PASSWORD_PORT = '18099'; $env:A1_HOLD_SECONDS = [string]$HoldForBrowserSeconds
        try { $served = (& node (Join-Path $PSScriptRoot 'serve-password-once.mjs')) -join ' ' }
        finally { foreach ($name in 'A1_READER_PASSWORD', 'A1_PRODUCT_ORIGIN', 'A1_PASSWORD_PORT', 'A1_HOLD_SECONDS') { [Environment]::SetEnvironmentVariable($name, $null, 'Process') } }
        $summary.browser_hold = $served
    }
    Invoke-RestMethod "$apiBase/connections/$connection" -Method Delete -Headers $script:headers -ContentType 'application/json' | Out-Null
    $closed = Get-Status $connection
    $summary.rb05_close = [ordered]@{ after_close = "$($closed.collection.state):$($closed.collection.reason)" }
    Require ($closed.collection.state -eq 'stopped' -and $closed.collection.reason -eq 'not_connected') 'Collection did not stop with the session.'

    $stage = 'clear_history'
    Invoke-Post '/history/clear' @{} | Out-Null
    $cleared = Get-Status $null
    $summary.cleared = [ordered]@{ templates = $cleared.store.templates; buckets = $cleared.store.buckets }
    Require ($cleared.store.templates -eq '0' -and $cleared.store.buckets -eq '0') 'Clearing history left data behind.'
    $summary.status = 'PASS'
}
catch {
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message = $_.Exception.Message }
    throw
}
finally {
    $cleanupFailure = $null
    try { Stop-DoctorApi } catch { $cleanupFailure = 'api_stop' }
    if ($null -ne $secret -and $apiRun -gt 0) {
        $needles = @($secret, (Get-FixturePassword -Secret $secret -Role doctor_reader), $secretLiteral)
        $logs = @(1..$apiRun | ForEach-Object { $run = $_; 'stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-b-$stamp-$run.$_.log" } } | Where-Object { Test-Path -LiteralPath $_ })
        $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 }).Count
        $summary.api_log_scan = if ($logs.Count -ne 2 * $apiRun) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }
        if ($summary.api_log_scan -ne 'PASS' -and -not $cleanupFailure) { $cleanupFailure = 'api_log_scan' }
    }
    # The history file is test data: released with the fixture.
    if (Test-Path -LiteralPath $historyDir) { Remove-Item -LiteralPath $historyDir -Recurse -Force }
    if ($fixtureStarted) {
        try { $summary.teardown = (& (Join-Path $fixtureDir 'teardown.ps1') -DockerExecutable $docker) | ConvertFrom-Json }
        catch { $cleanupFailure = 'fixture_teardown' }
    }
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    if ($null -ne $docker) {
        $after = Get-FixtureState $docker
        $summary.postconditions = [ordered]@{ containers_absent = ($after.containers_present.Count -eq 0); network_absent = (-not $after.network_present); data_root_absent = (-not $after.data_root_present); ports_free = ($after.listening_ports.Count -eq 0); history_removed = (-not (Test-Path -LiteralPath $historyDir)) }
        if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    }
    if ($cleanupFailure) { $summary.status = 'FAIL'; $summary.cleanup_failure = $cleanupFailure }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Stage B E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
