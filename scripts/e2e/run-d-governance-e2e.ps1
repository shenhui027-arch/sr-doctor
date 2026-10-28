[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18064,
    [switch]$SkipBuild,
    # Keep the fixture and API up this long after the checks so a browser can
    # sign in through serve-password-once.mjs (127.0.0.1:18099) and capture
    # boards D (alerts, governance). GET http://127.0.0.1:18099/release ends the hold.
    [ValidateRange(0, 1800)][int]$HoldForBrowserSeconds = 0
)

# Stage D local acceptance on the low-resource 1 FE + 3 BE fixture. A
# synthetic week-old baseline (test-only seed) sits under the real workload
# of the same official digest, so the collector's data raises one in-app
# alert (RD-01); the governance queue refuses hand transitions outside its
# edges (RD-02) and only the comparator may close an item (RD-03). Nothing
# on disk holds a literal or a credential. Serial, bounded, torn down in
# `finally`.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "d-governance-$stamp.json"
$historyDir = Join-Path $runtimeRoot "d-governance-$stamp"
$historyPath = Join-Path $historyDir 'history.sqlite'
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$fixturePorts = @(38530, 39530)
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)
$stage = 'initialization'
$summary = [ordered]@{ project = 'd-alerts-governance-multi-be-low-resource'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null; $secret = $null; $startedApi = $null; $fixtureStarted = $false; $primaryFailure = $false; $generatedSecret = $false; $apiRun = 0
$secretLiteral = 'd_governance_secret_literal'
$timings = [ordered]@{ DOCTOR_COLLECTION_PERIOD_SECONDS = '5'; DOCTOR_COLLECTION_SLICE_SECONDS = '300'; DOCTOR_COLLECTION_LAG_SECONDS = '90'; DOCTOR_COLLECTION_BACKFILL_SECONDS = '600' }

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
            -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-d-$stamp-$($script:apiRun).stdout.log") `
            -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-d-$stamp-$($script:apiRun).stderr.log")
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
# Waits until the watermark passes the newest audited gov_e2e statement.
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
    $limits = "USE gov_e2e;`nSET query_timeout = 15;`nSET query_mem_limit = 134217728;`nSET enable_sql_digest = true;`nSET enable_profile = true;`nSET pipeline_profile_level = 1;`n"
    Invoke-FixtureMysql $docker $script:MultiBeFeName 'workload_user' (Get-FixturePassword -Secret $secret -Role workload_user) ($limits + (($Statements | ForEach-Object { "$_;" }) -join "`n")) | Out-Null
    $countSql = "SELECT COUNT(*), UNIX_TIMESTAMP(MAX(``timestamp``)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'gov_e2e' AND isQuery = 1;"
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do { Start-Sleep -Seconds 2; $row = @((@(Invoke-MultiBeMysql $docker $countSql $root)[0]) -split "`t") } while ([int]$row[0] -lt $Expected -and [DateTime]::UtcNow -lt $deadline)
    Require ([int]$row[0] -eq $Expected) "Expected $Expected audited gov_e2e queries, found $($row[0])."
    [int64]$row[1]
}
function Get-History { (& node --no-warnings (Join-Path $PSScriptRoot 'inspect-history.mjs') $historyPath 'gov_e2e') | ConvertFrom-Json }
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
    Require ($before.containers_present.Count -eq 0 -and -not $before.network_present -and -not $before.data_root_present -and $before.listening_ports.Count -eq 0) 'Stage D E2E requires a cold start.'
    Require ($null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName 'sr-doctor-v0.1-e2e-3514')) 'The allin1 release fixture must be absent.'
    Require (-not (Test-Path -LiteralPath $historyDir)) 'The history directory must not exist yet.'
    Require (Test-Path -LiteralPath (Join-Path $repoRoot 'target\debug\examples\seed_baseline.exe')) 'Build the seed example first (cargo build -p doctor-history --example seed_baseline).'

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
CREATE DATABASE gov_e2e;
CREATE TABLE gov_e2e.t (id INT, name VARCHAR(32)) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 3 PROPERTIES ("replication_num" = "1");
INSERT INTO gov_e2e.t VALUES (1, 'a'), (2, 'b'), (3, 'c');
GRANT SELECT ON TABLE gov_e2e.t TO USER doctor_reader;
GRANT SELECT ON TABLE gov_e2e.t TO USER workload_user;
'@ $root | Out-Null
    $firstAt = Invoke-Workload $first $first.Count
    $readerPassword = Get-FixturePassword -Secret $secret -Role doctor_reader

    # A week-old baseline of 1 ms and 1 scanned byte for the same official
    # digest (synthetic, test-only), then the real workload is collected on
    # top: a high-severity regression with more work per execution.
    $stage = 'seed_baseline'
    $context = @((@(Invoke-MultiBeMysql $docker "SELECT DISTINCT LOWER(digest), catalog, db FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'gov_e2e' AND isQuery = 1 AND stmt LIKE 'SELECT id FROM t WHERE id%';" $root)[0]) -split "`t")
    Require ($context.Count -eq 3 -and $context[0] -match '^[0-9a-f]{8,128}$') 'The workload has no single official digest.'
    New-Item -ItemType Directory -Force -Path $historyDir | Out-Null
    $seed = (& (Join-Path $repoRoot 'target\debug\examples\seed_baseline.exe') $historyPath "127.0.0.1:$($up.host_ports.mysql)" $context[0] $context[1] $context[2] $up.observed_version 50 4 1 1) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Seeding the baseline failed.' }
    $summary.seeded = [ordered]@{ hours = $seed.seeded_hours; per_hour = 5; query_ms = 1; scan_bytes = 1; engine = $up.observed_version; synthetic = $true }

    $stage = 'collect'
    Start-DoctorApi
    $connection = Connect-Reader
    # One 300 s slice plus the 90 s lag and a period: wait up to 600 s.
    $status = Wait-Watermark $connection $firstAt 600

    # RD-01 live: one alert for the template however often it is evaluated.
    $stage = 'rd01_alerts'
    $null = Invoke-Post '/history/alerts' @{ target = $null }
    $alertsRaw = (Invoke-WebRequest "$apiBase/history/alerts" -Method Post -Headers $script:headers -ContentType 'application/json' -Body '{"target":null}' -UseBasicParsing).Content
    $alerts = $alertsRaw | ConvertFrom-Json
    $mine = @($alerts.alerts | Where-Object { $_.template -like 'SELECT id FROM t WHERE id = ?' })
    $statusAfter = Get-Status $connection
    $summary.rd01_alerts = [ordered]@{
        alerts_for_template = $mine.Count
        state = $mine[0].state; severity = $mine[0].severity; confidence = $mine[0].confidence; evidence = $mine[0].evidence; workload = $mine[0].workload
        change = "$($mine[0].baseline_median.canonical_number.value)$($mine[0].baseline_median.canonical_unit) -> $($mine[0].current_median.canonical_number.value)$($mine[0].current_median.canonical_unit)"
        exemplar_real = ($mine[0].exemplar_query_id -match '^[0-9a-f-]{36}$')
        open_alerts_badge = $statusAfter.open_alerts
        channels = @($alerts.channels | ForEach-Object { "$($_.name):$($_.available)" })
    }
    Require ($mine.Count -eq 1 -and $mine[0].state -eq 'open' -and $mine[0].severity -in @('high', 'medium')) 'The regression did not raise exactly one open alert.'
    Require ($summary.rd01_alerts.exemplar_real -and $statusAfter.open_alerts -eq '1') 'The alert lacks a real exemplar or the badge count.'
    Require (-not $alertsRaw.Contains($secretLiteral) -and -not $alertsRaw.Contains($readerPassword)) 'The alerts response leaked a literal or a credential.'
    $alert = $mine[0]

    # RD-02 / RD-03 live: hand transitions only along the allowed edges; a
    # change makes it pending; the comparator, not the user, decides.
    $stage = 'rd02_rd03_governance'
    $item = Invoke-Post '/governance/create' @{ target = $alert.target; template_key = $alert.template_key; origin = 'alert'; alert_id = $alert.alert_id; title = $null }
    $rejected = @()
    foreach ($to in 'closed', 'pending_verification', 'in_progress') {
        try { $null = Invoke-Post '/governance/transition' @{ item_id = $item.item_id; to = $to; note = $null }; $rejected += "$($to):accepted" }
        catch { $rejected += "$($to):$([int]$_.Exception.Response.StatusCode)" }
    }
    $null = Invoke-Post '/governance/transition' @{ item_id = $item.item_id; to = 'triaged'; note = 'Seen in the alert list' }
    $null = Invoke-Post '/governance/transition' @{ item_id = $item.item_id; to = 'in_progress'; note = $null }
    $pinned = Invoke-Post '/governance/pin' @{ item_id = $item.item_id }
    $changed = Invoke-Post '/governance/change' @{ item_id = $item.item_id; note = 'Refreshed table statistics outside this tool'; change_seconds = $null }
    try { $null = Invoke-Post '/governance/transition' @{ item_id = $item.item_id; to = 'closed'; note = $null }; $manualClose = 'accepted' } catch { $manualClose = [int]$_.Exception.Response.StatusCode }
    $verified = Invoke-Post '/governance/verify' @{ item_id = $item.item_id }
    $summary.rd02_rd03_governance = [ordered]@{
        created = "$($item.state):$($item.origin):alert#$($item.alert_id)"
        illegal = $rejected
        pinned = [ordered]@{ executions = $pinned.snapshot.executions; median = "$($pinned.snapshot.query_time_median.canonical_number.value)ns"; template = $pinned.snapshot.template }
        after_change = $changed.state
        manual_close = $manualClose
        verification = "$($verified.verification.outcome.state)"
        state_after_verify = $verified.state
        events = @($verified.events | ForEach-Object { "$($_.kind):$($_.actor)" })
    }
    Require ((@($rejected) -join ',') -eq 'closed:409,pending_verification:409,in_progress:409' -and $manualClose -eq 409) 'An illegal transition was accepted.'
    Require ($changed.state -eq 'pending_verification' -and $verified.state -eq 'pending_verification' -and $verified.verification.outcome.state -eq 'insufficient_samples') 'Verification closed without an after window.'
    Require ($pinned.snapshot.template -eq 'SELECT id FROM t WHERE id = ?') 'The pinned snapshot holds an unexpected template.'
    $null = Invoke-RestMethod "$apiBase/history/alerts/action" -Method Post -Headers $script:headers -ContentType 'application/json' -Body (@{ alert_id = $alert.alert_id; action = 'acknowledge' } | ConvertTo-Json -Compress)
    $acked = @((Invoke-Post '/history/alerts' @{ target = $null }).alerts | Where-Object { $_.alert_id -eq $alert.alert_id })[0]
    $summary.rd01_alerts.after_acknowledge = "$($acked.state):item#$($acked.governance_item_id)"
    Require ($acked.state -eq 'acknowledged' -and $acked.governance_item_id -eq $item.item_id) 'The alert was not acknowledged or not linked.'

    # Disk: no literal, no credential, in history, snapshot or records.
    $stage = 'disk'
    $disk = Test-NoSecretsOnDisk
    $summary.disk = [ordered]@{ files = $disk.files; bytes = $disk.bytes; literal_or_credential_hits = $disk.hits }
    Require ($disk.hits -eq 0) 'The history files contain a literal or a credential.'

    if ($HoldForBrowserSeconds -gt 0) {
        $stage = 'browser_hold'
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
        $logs = @(1..$apiRun | ForEach-Object { $run = $_; 'stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-d-$stamp-$run.$_.log" } } | Where-Object { Test-Path -LiteralPath $_ })
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
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Stage D E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
