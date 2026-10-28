[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18064,
    [switch]$SkipBuild,
    # Keep the fixture and API up this long after the checks so a browser can
    # sign in through serve-password-once.mjs (127.0.0.1:18099) and capture
    # boards B and C. GET http://127.0.0.1:18099/release ends the hold.
    [ValidateRange(0, 1800)][int]$HoldForBrowserSeconds = 0
)

# Stage C local acceptance on the low-resource 1 FE + 3 BE fixture: the
# collector fills the local history from the real AuditLoader table, then the
# trend API must leave every template unscored (no baseline yet, RC-03),
# keep not-collected hours apart from 0, keep slow executions by query ID,
# and a kept execution must reach its Profile through the ordinary
# discovery-then-fetch path. Serial, bounded, torn down in `finally`.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "c-trends-$stamp.json"
$historyDir = Join-Path $runtimeRoot "c-trends-$stamp"
$historyPath = Join-Path $historyDir 'history.sqlite'
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$fixturePorts = @(38530, 39530)
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)
$stage = 'initialization'
$summary = [ordered]@{ project = 'c-trends-multi-be-low-resource'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null; $secret = $null; $startedApi = $null; $fixtureStarted = $false; $primaryFailure = $false; $generatedSecret = $false; $apiRun = 0
$secretLiteral = 'c_trends_secret_literal'
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
            -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-c-$stamp-$($script:apiRun).stdout.log") `
            -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-c-$stamp-$($script:apiRun).stderr.log")
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
# Waits until the watermark passes the newest audited trend_e2e statement.
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
    $limits = "USE trend_e2e;`nSET query_timeout = 15;`nSET query_mem_limit = 134217728;`nSET enable_sql_digest = true;`nSET enable_profile = true;`nSET pipeline_profile_level = 1;`n"
    Invoke-FixtureMysql $docker $script:MultiBeFeName 'workload_user' (Get-FixturePassword -Secret $secret -Role workload_user) ($limits + (($Statements | ForEach-Object { "$_;" }) -join "`n")) | Out-Null
    $countSql = "SELECT COUNT(*), UNIX_TIMESTAMP(MAX(``timestamp``)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'trend_e2e' AND isQuery = 1;"
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do { Start-Sleep -Seconds 2; $row = @((@(Invoke-MultiBeMysql $docker $countSql $root)[0]) -split "`t") } while ([int]$row[0] -lt $Expected -and [DateTime]::UtcNow -lt $deadline)
    Require ([int]$row[0] -eq $Expected) "Expected $Expected audited trend_e2e queries, found $($row[0])."
    [int64]$row[1]
}
function Get-History { (& node --no-warnings (Join-Path $PSScriptRoot 'inspect-history.mjs') $historyPath 'trend_e2e') | ConvertFrom-Json }
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

# Two templates: 12 executions with literal variants, and 3 carrying a
# literal that must never reach the history file.
$first = @((1..12 | ForEach-Object { "SELECT id FROM t WHERE id = $_" }) + @("SELECT count(*) FROM t WHERE name = '$secretLiteral'", "SELECT count(*) FROM t WHERE name = 'x'", "SELECT count(*) FROM t WHERE name = 'y'"))

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $summary.evidence_sources = [ordered]@{ runner_sha256 = Get-Sha256 $PSCommandPath; inspector_sha256 = Get-Sha256 (Join-Path $PSScriptRoot 'inspect-history.mjs'); up_sha256 = Get-Sha256 (Join-Path $fixtureDir 'up.ps1'); connected_fixture_sha256 = Get-Sha256 (Join-Path $fixtureDir 'connected-fixture.ps1') }
    $summary.timings = $timings

    $stage = 'cold_start_preconditions'
    $before = Get-FixtureState $docker
    Require ($before.containers_present.Count -eq 0 -and -not $before.network_present -and -not $before.data_root_present -and $before.listening_ports.Count -eq 0) 'Stage C E2E requires a cold start.'
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

    $stage = 'c_workload'
    $root = Get-MultiBeRootPassword
    Invoke-MultiBeMysql $docker @'
CREATE DATABASE trend_e2e;
CREATE TABLE trend_e2e.t (id INT, name VARCHAR(32)) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 3 PROPERTIES ("replication_num" = "1");
INSERT INTO trend_e2e.t VALUES (1, 'a'), (2, 'b'), (3, 'c');
GRANT SELECT ON TABLE trend_e2e.t TO USER doctor_reader;
GRANT SELECT ON TABLE trend_e2e.t TO USER workload_user;
'@ $root | Out-Null
    $firstAt = Invoke-Workload $first $first.Count
    $readerPassword = Get-FixturePassword -Secret $secret -Role doctor_reader

    # Collect: the session's collector backfills the workload.
    $stage = 'collect'
    Start-DoctorApi
    $connection = Connect-Reader
    $status = Wait-Watermark $connection $firstAt
    $history = Get-History
    $summary.collected = [ordered]@{ state = $status.collection.state; templates = @($history.templates | ForEach-Object { "$($_.kind):$($_.executions)" }); reread_slices = $history.reread_slices }
    Require ((@($history.templates | ForEach-Object { $_.executions }) -join ',') -eq '12,3') "Unexpected collected counts $($summary.collected.templates -join ',')."

    # RC-03 live: no baseline yet, so nothing is scored.
    $stage = 'rc03_trends'
    $trendsRaw = (Invoke-WebRequest "$apiBase/history/trends" -Method Post -Headers $script:headers -ContentType 'application/json' -Body (@{ target = $null; current_hours = 24; baseline_hours = 168; limit = 100 } | ConvertTo-Json -Compress) -UseBasicParsing).Content
    $trends = $trendsRaw | ConvertFrom-Json
    $mine = @($trends.rows | Where-Object { $_.database -eq 'trend_e2e' })
    $summary.rc03_trends = [ordered]@{
        target = $trends.target
        rows = @($mine | ForEach-Object { "$($_.verdict.state):$($_.verdict.reason)" })
        confidence_absent = (@($mine | Where-Object { $null -ne $_.confidence }).Count -eq 0)
        change_absent = (@($mine | Where-Object { $null -ne $_.change }).Count -eq 0)
        counts = $trends.counts
        calibrated = $trends.rule.calibrated
        evidence = @($mine | ForEach-Object { $_.evidence })
    }
    Require ($trends.target -eq "127.0.0.1:$($up.host_ports.mysql)") 'Trends picked the wrong target.'
    Require ($mine.Count -eq 2 -and (@($mine | Where-Object { $_.verdict.state -ne 'unscored' -or $_.verdict.reason -ne 'insufficient_baseline' }).Count -eq 0)) 'A template without a baseline was scored.'
    Require ($summary.rc03_trends.confidence_absent -and $summary.rc03_trends.change_absent -and $trends.counts.regression -eq '0' -and -not $trends.rule.calibrated) 'Unscored rows must carry no confidence, change or regression.'
    Require (-not $trendsRaw.Contains($secretLiteral) -and -not $trendsRaw.Contains($readerPassword)) 'The trends response leaked a literal or a credential.'

    # Series: hours before collection are not collected (null), never 0.
    $stage = 'series'
    $top = @($mine | Where-Object { $_.template -like 'SELECT id FROM t WHERE id = ?' })[0]
    $seriesRaw = (Invoke-WebRequest "$apiBase/history/series" -Method Post -Headers $script:headers -ContentType 'application/json' -Body (@{ target = $trends.target; template_key = $top.template_key; current_hours = 24; baseline_hours = 168; points = 48 } | ConvertTo-Json -Compress) -UseBasicParsing).Content
    $series = $seriesRaw | ConvertFrom-Json
    $points = @($series.points)
    $collected = @($points | Where-Object { $null -ne $_.executions })
    $summary.series = [ordered]@{
        points = $points.Count
        not_collected_null = @($points | Where-Object { $_.coverage -eq 'not_collected' -and $null -eq $_.executions }).Count
        collected_hours = $collected.Count
        executions = (@($collected | ForEach-Object { [int64]$_.executions }) | Measure-Object -Sum).Sum
        coverage = @($points | Select-Object -Last 3 | ForEach-Object { $_.coverage })
        exemplars = @($series.exemplars | ForEach-Object { "$($_.query_time.canonical_number.value)$($_.query_time.canonical_unit)" })
    }
    Require ($points.Count -eq 48 -and $summary.series.not_collected_null -ge 40 -and $summary.series.executions -eq 12) 'The series invented or lost executions.'
    Require (@($series.exemplars).Count -ge 3 -and @($series.exemplars | Where-Object { $_.query_id -notmatch '^[0-9a-fA-F-]{8,64}$' }).Count -eq 0) 'Exemplars are missing or malformed.'

    # A kept execution reaches its Profile through discovery, then fetch.
    $stage = 'exemplar_profile'
    $exemplar = @($series.exemplars)[0]
    $scope = [ordered]@{ window = [ordered]@{ start_seconds = [int64]$exemplar.event_seconds; end_seconds = [int64]$exemplar.event_seconds + 1 }; state = $null; user = $null; native_digest = $null; catalog = $top.catalog; database = $top.database; resource_group = $null; statement_filter = 'exclude_doctor_and_non_query' }
    $occurrences = Invoke-Post "/connections/$connection/audit/occurrences" @{ connection_id = $connection; request = [ordered]@{ scope = $scope; sort = 'query_time_desc'; limit = 50 } }
    Require (@($occurrences.rows | Where-Object { $_.query.query_id -eq $exemplar.query_id }).Count -eq 1) 'The exemplar was not found in its audit second.'
    $profile = Invoke-Post "/connections/$connection/profiles/fetch" @{ connection_id = $connection; query_id = $exemplar.query_id; discovery_generation = $occurrences.discovery_generation; kind = 'baseline' }
    $summary.exemplar_profile = [ordered]@{ state = $profile.state; origin = $profile.input_origin; query_matches = ($profile.query.query_id -eq $exemplar.query_id) }
    Require ($profile.state -eq 'fetched' -and $summary.exemplar_profile.query_matches) 'The exemplar Profile was not fetched.'

    # Disk: query IDs and templates only; no literal, no credential.
    $stage = 'disk'
    $disk = Test-NoSecretsOnDisk
    $summary.disk = [ordered]@{ files = $disk.files; bytes = $disk.bytes; literal_or_credential_hits = $disk.hits; templates = @($history.templates | ForEach-Object { $_.template }) }
    Require ($disk.hits -eq 0) 'The history files contain a literal or a credential.'

    if ($HoldForBrowserSeconds -gt 0) {
        $stage = 'browser_hold'
        # Let the browser session collect: this runner's session steps aside.
        Invoke-RestMethod "$apiBase/connections/$connection" -Method Delete -Headers $script:headers -ContentType 'application/json' | Out-Null
        $env:A1_READER_PASSWORD = $readerPassword; $env:A1_PRODUCT_ORIGIN = "http://127.0.0.1:$ApiPort"; $env:A1_PASSWORD_PORT = '18099'; $env:A1_HOLD_SECONDS = [string]$HoldForBrowserSeconds
        try { $served = (& node (Join-Path $PSScriptRoot 'serve-password-once.mjs')) -join ' ' }
        finally { foreach ($name in 'A1_READER_PASSWORD', 'A1_PRODUCT_ORIGIN', 'A1_PASSWORD_PORT', 'A1_HOLD_SECONDS') { [Environment]::SetEnvironmentVariable($name, $null, 'Process') } }
        $summary.browser_hold = $served
    }
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
        $logs = @(1..$apiRun | ForEach-Object { $run = $_; 'stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-c-$stamp-$run.$_.log" } } | Where-Object { Test-Path -LiteralPath $_ })
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
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Stage C E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
