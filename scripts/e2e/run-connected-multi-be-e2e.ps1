[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18063,
    # Skip the incremental `cargo build --locked -p doctor-api`; the hashed
    # binary in the receipt then describes whatever target\debug holds.
    [switch]$SkipBuild,
    # Also drive the connected browser E2E (tests/e2e/browser-e2e.mjs,
    # project connected-readonly-docker) through the same owned API, after the
    # Audit statement check so its own statements do not enter that count.
    [switch]$Browser,
    # Start the fixture with up.ps1 -LowResource (1 FE 1.5 GiB + 3 BE 1.25 GiB,
    # 2.5 CPUs, no extra swap) instead of the default sizes.
    [switch]$LowResource
)

# Connected read-only E2E against the isolated 1 FE + 3 BE fixture
# (dev/starrocks-3.5.14-multi-be). HTTP only, no browser: it reuses
# connected-smoke.ps1 unchanged, adds one distributed Runtime Top query and an
# Audit-table check of what the product sent. It never touches the allin1
# release fixture and refuses to run while that fixture exists.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "connected-multi-be-$stamp.json"
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$fixturePorts = @(38530, 39530)
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)
$stage = 'initialization'
$summary = [ordered]@{ project = 'connected-readonly-multi-be'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null
$secret = $null
$startedApi = $null
$fixtureStarted = $false
$primaryFailure = $false
$generatedSecret = $false
$previous = @{}
foreach ($name in @('STARROCKS_USER', 'STARROCKS_PASSWORD', 'DOCTOR_PORT', 'DOCTOR_HISTORY', 'DOCTOR_FRONTEND_DIST', 'E2E_BASE_URL', 'E2E_DB_PORT', 'E2E_BASELINE_QUERY_ID', 'E2E_CANDIDATE_QUERY_ID', 'E2E_ARTIFACT_ROOT')) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }

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

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $summary.evidence_sources = [ordered]@{
        runner_sha256 = Get-Sha256 $PSCommandPath
        connected_smoke_sha256 = Get-Sha256 (Join-Path $PSScriptRoot 'connected-smoke.ps1')
        connected_fixture_sha256 = Get-Sha256 (Join-Path $fixtureDir 'connected-fixture.ps1')
        up_sha256 = Get-Sha256 (Join-Path $fixtureDir 'up.ps1')
        teardown_sha256 = Get-Sha256 (Join-Path $fixtureDir 'teardown.ps1')
    }

    # Cold start, and never alongside the allin1 release fixture.
    $stage = 'cold_start_preconditions'
    $before = Get-FixtureState $docker
    $allin1Absent = $null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName 'sr-doctor-v0.1-e2e-3514')
    $summary.preconditions = [ordered]@{
        multi_be_containers_absent = ($before.containers_present.Count -eq 0); multi_be_network_absent = (-not $before.network_present)
        multi_be_data_root_absent = (-not $before.data_root_present); ports_free = ($before.listening_ports.Count -eq 0)
        release_allin1_fixture_absent = $allin1Absent
    }
    Require (@($summary.preconditions.Values | Where-Object { -not $_ }).Count -eq 0) 'Multi-BE connected E2E requires a cold start with the allin1 release fixture absent.'

    # The fixture token lives only in this process. Generate one when the
    # caller did not, and clear it in `finally` either way.
    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))) {
        $bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', [Convert]::ToBase64String($bytes).Replace('+', '-').Replace('/', '_').TrimEnd('='), 'Process')
        $generatedSecret = $true
    }
    $secret = Get-FixtureSecret
    $summary.secret_source = if ($generatedSecret) { 'generated_in_process' } else { 'caller_process_environment' }

    if (-not $SkipBuild) {
        $stage = 'repository_build'
        & cargo build --locked -p doctor-api
        if ($LASTEXITCODE -ne 0) { throw 'Repository API build failed.' }
        if ($Browser) {
            & npm --prefix (Join-Path $repoRoot 'frontend') run build
            if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
        }
    }
    $frontendDist = Join-Path $repoRoot 'frontend\dist'
    if ($Browser) { Require (Test-Path -LiteralPath (Join-Path $frontendDist 'index.html') -PathType Leaf) 'frontend\dist is missing; build the frontend first.' }
    $apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    Require (Test-Path -LiteralPath $apiExecutable -PathType Leaf) 'doctor-api.exe is missing.'
    $summary.repository_build = if ($SkipBuild) { 'skipped_by_request' } else { 'rebuilt_from_current_source' }
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.api_sha256 = Get-Sha256 $apiExecutable

    $stage = 'fixture_up'
    $fixtureStarted = $true
    $up = (& (Join-Path $fixtureDir 'up.ps1') -DockerExecutable $docker -LowResource:$LowResource) | ConvertFrom-Json
    $stage = 'fixture_connected_workload'
    $workload = (& (Join-Path $fixtureDir 'connected-fixture.ps1') -DockerExecutable $docker) | ConvertFrom-Json
    $summary.fixture = [ordered]@{
        frontend = $up.frontend; backends = $up.backends; network = $up.network; subnet = $script:MultiBeSubnet
        fe_image = $up.fe_image; be_image = $up.be_image; observed_version = $up.observed_version; backends_alive = $up.backends_alive
        host_ports = $up.host_ports; resources = $up.resources; audit_loader_version = $workload.audit_loader_version; audit_loader_sha256 = $workload.audit_loader_sha256
        audit_loader_frontend_host_port = $workload.audit_loader_frontend_host_port; profile_table_tablets = $workload.profile_table_tablets
        audit_rows = $workload.audit_rows; unavailable_profile_audit_rows = $workload.unavailable_profile_audit_rows
    }

    $stage = 'api_start'
    Require (-not (Test-PortListening $ApiPort)) "API port $ApiPort is already in use."
    $env:DOCTOR_PORT = [string]$ApiPort
    # History is off: these checks predate stage B; B-D have their own E2Es.
    $env:DOCTOR_HISTORY = 'off'
    $env:DOCTOR_FRONTEND_DIST = if ($Browser) { (Resolve-Path -LiteralPath $frontendDist).Path } else { $null }
    $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-multi-be-$stamp.stdout.log") `
        -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-multi-be-$stamp.stderr.log")
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) {
        try { $capabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 }
    }
    Require ($null -ne $capabilities) 'The local Doctor API did not become ready.'

    # Only doctor_reader reaches the product, through the process environment.
    $env:STARROCKS_USER = 'doctor_reader'
    $env:STARROCKS_PASSWORD = Get-FixturePassword -Secret $secret -Role doctor_reader
    $stage = 'profile_capture'
    $profilePath = Join-Path $runtimeRoot "connected-multi-be-baseline-$stamp.profile"
    $capture = (& (Join-Path $PSScriptRoot 'capture-runtime-profile.ps1') -QueryId $workload.baseline_query_id -OutputPath $profilePath -ContainerName $script:MultiBeFeName -DockerExecutable $docker) | ConvertFrom-Json
    $summary.profile_hash = $capture.source_hash

    $stage = 'connected_http_smoke'
    $smoke = (& (Join-Path $PSScriptRoot 'connected-smoke.ps1') -QueryId $workload.baseline_query_id -CandidateQueryId $workload.candidate_query_id -ProfilePath $capture.output_path -MySqlPort $up.host_ports.mysql -ApiPort $ApiPort) | ConvertFrom-Json
    # connected-smoke accepts graph `complete` or `partial`; a merged multi-BE
    # Level 1 Profile must be complete.
    Require ($smoke.status -eq 'PASS') 'connected-smoke did not pass.'
    Require ($smoke.detected_version -match '^3\.5\.14-23a56ec') "Unexpected detected version $($smoke.detected_version)."
    Require ($smoke.graph_state -eq 'complete' -and $smoke.summary_state -eq 'available') 'The multi-BE Profile graph is not complete or its execution summary is not available.'
    Require ($smoke.official_share_nodes_matched -gt 0) 'No official ANALYZE PROFILE share was compared.'
    Require ($smoke.forbidden_statement_status -eq 422 -and $smoke.runtime_top_forbidden_statement_status -eq 422) 'A forbidden statement was not rejected with 422.'
    $summary.connected_evidence = $smoke

    # Runtime Top on a query whose scan, local aggregation and shuffle span all
    # three BEs (connected-smoke's own Runtime Top reads information_schema).
    $stage = 'distributed_runtime_top'
    $headers = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }
    $post = { param($Path, $Payload) Invoke-RestMethod "$apiBase$Path" -Method Post -Headers $headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 20 -Compress) }
    $connection = & $post '/connections' @{
        host = '127.0.0.1'; port = $up.host_ports.mysql; username = $env:STARROCKS_USER; password = $env:STARROCKS_PASSWORD
        catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext'
    }
    try {
        $runtimeSql = 'SELECT payload, COUNT(*) AS mbe_runtime_count FROM e2e_fixture.profile_rows GROUP BY payload'
        $runtime = & $post "/connections/$($connection.connection_id)/runtime-profile" @{ connection_id = $connection.connection_id; candidate_sql = $runtimeSql }
        $report = $runtime.outcome.report
        Require ($runtime.state -eq 'fetched' -and $null -ne $report) 'Distributed Runtime Top did not return an analyzed Profile.'
        Require ($report.profile_context.profile_level -eq 'level_1' -and $report.profile_context.snapshot_kind -eq 'final' -and $report.profile_context.merge_state -eq 'merged') 'Distributed Runtime Top Profile is not a final merged Level 1 Profile.'
        Require ($report.graph_view.state -eq 'complete' -and $report.execution_summary.state -eq 'available') 'Distributed Runtime Top graph is not complete or its summary is not available.'
        $serialized = $runtime | ConvertTo-Json -Depth 100 -Compress
        Require (-not $serialized.Contains($runtimeSql) -and -not $serialized.Contains('mbe_runtime_count')) 'Distributed Runtime Top leaked the submitted SQL.'
        Require (-not $serialized.Contains($env:STARROCKS_PASSWORD)) 'Distributed Runtime Top leaked a credential.'
        $summary.distributed_runtime_top = [ordered]@{
            query_id = $runtime.query.query_id; analysis_state = $report.analysis_state
            profile_level = $report.profile_context.profile_level; snapshot_kind = $report.profile_context.snapshot_kind; merge_state = $report.profile_context.merge_state
            graph_state = $report.graph_view.state; graph_nodes = @($report.graph_view.nodes).Count; graph_edges = @($report.graph_view.edges).Count
            exchange_nodes = @($report.graph_view.nodes | Where-Object { [string]$_.plan_node_type -match 'EXCHANGE' }).Count
            fragments = @($report.graph_view.nodes | ForEach-Object { $_.fragment_id } | Where-Object { $_ } | Sort-Object -Unique).Count
            summary_state = $report.execution_summary.state; summary_nodes = @($report.execution_summary.nodes).Count
            operator_rows = @($report.operator_rows).Count; findings = @($report.findings).Count
        }
    }
    finally {
        Invoke-RestMethod "$apiBase/connections/$($connection.connection_id)" -Method Delete -Headers $headers -ContentType 'application/json' | Out-Null
    }

    # What the product actually sent, as recorded by the FE's AuditLoader: the
    # topology-only path issued EXPLAIN COSTS and nothing else, and neither
    # forbidden DELETE reached the server.
    # Read as root, so this lookup is not itself a doctor_reader Audit row.
    $stage = 'audit_statement_check'
    $auditSql = "SELECT stmt FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE user = 'doctor_reader' AND (stmt LIKE '%EXPLAIN%' OR stmt LIKE '%forbidden_fixture%');"
    $rows = @(); $explainRows = @()
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        $rows = @(Invoke-MultiBeMysql $docker $auditSql (Get-MultiBeRootPassword) | ForEach-Object { ([string]$_).Trim() } | Where-Object { $_ })
        $explainRows = @($rows | Where-Object { $_ -match 'EXPLAIN' })
        if ($explainRows.Count -ge 1) { break }
        Start-Sleep -Seconds 2
    }
    # Only the statement keywords are kept, never the submitted SQL.
    $explainModes = @($explainRows | ForEach-Object { if ($_ -cmatch '^(/\* sr-doctor \*/ )?(EXPLAIN(?: [A-Z]+)?)\b') { "$(if ($Matches[1]) { 'marked' } else { 'unmarked' }):$($Matches[2])" } else { 'other' } } | Sort-Object -Unique)
    $summary.audit_statement_check = [ordered]@{ explain_rows = $explainRows.Count; explain_modes = $explainModes; forbidden_statements_at_server = @($rows | Where-Object { $_ -match 'forbidden_fixture' }).Count }
    Require ($explainRows.Count -ge 1) 'AuditLoader did not record the topology-only EXPLAIN.'
    Require ($explainModes.Count -eq 1 -and $explainModes[0] -ceq 'marked:EXPLAIN COSTS') 'The product sent an EXPLAIN other than EXPLAIN COSTS.'
    Require ($summary.audit_statement_check.forbidden_statements_at_server -eq 0) 'A forbidden statement reached the server.'

    if ($Browser) {
        # The multi-BE fixture mirrors the allin1 one (same database, tables,
        # markers and doctor_reader password derivation), so the connected
        # browser project runs unchanged; only the MySQL port differs.
        $stage = 'browser_e2e'
        $env:E2E_BASE_URL = "http://127.0.0.1:$ApiPort"
        $env:E2E_DB_PORT = [string]$up.host_ports.mysql
        $env:E2E_BASELINE_QUERY_ID = $workload.baseline_query_id
        $env:E2E_CANDIDATE_QUERY_ID = $workload.candidate_query_id
        $browserRoot = Join-Path $runtimeRoot "connected-multi-be-browser-$stamp"
        $env:E2E_ARTIFACT_ROOT = $browserRoot
        $browserOutput = @(& node (Join-Path $repoRoot 'tests\e2e\browser-e2e.mjs') --project connected-readonly-docker)
        $browserExit = $LASTEXITCODE
        $browserReceipts = @(Get-ChildItem -LiteralPath $browserRoot -Filter 'browser-e2e-receipt.json' -Recurse -File -ErrorAction SilentlyContinue)
        $browserReceipt = if ($browserReceipts.Count -eq 1) { Get-Content -LiteralPath $browserReceipts[0].FullName -Raw | ConvertFrom-Json } else { $null }
        $summary.browser = [ordered]@{
            exit_code = $browserExit
            status = if ($browserReceipt) { $browserReceipt.status } else { 'NOT_RUN' }
            assertions = if ($browserReceipt) { @($browserReceipt.assertions).Count } else { 0 }
            screenshots = if ($browserReceipt) { @($browserReceipt.screenshots).Count } else { 0 }
            receipt_path = if ($browserReceipts.Count -eq 1) { $browserReceipts[0].FullName } else { $null }
            receipt_sha256 = if ($browserReceipts.Count -eq 1) { Get-Sha256 $browserReceipts[0].FullName } else { $null }
        }
        Require ($browserExit -eq 0 -and $summary.browser.status -eq 'PASS') 'The connected browser E2E did not pass on the multi-BE cluster.'
    }

    $summary.status = 'PASS'
}
catch {
    # The receipt names only the bounded stage: no command output, SQL,
    # credentials or Profile bytes.
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message_id = 'e2e_stage_failed' }
    throw
}
finally {
    $cleanupFailure = $null
    if ($startedApi -and -not $startedApi.HasExited) {
        try { Stop-Process -Id $startedApi.Id -Force -ErrorAction Stop; $startedApi.WaitForExit(5000) | Out-Null } catch { $cleanupFailure = 'api_stop' }
    }
    if ($startedApi -and $startedApi.HasExited -and $null -ne $secret) {
        # The owned API logs must not contain the token, the reader password or
        # any submitted SQL; only the verdict and byte counts are recorded.
        $needles = @($secret, (Get-FixturePassword -Secret $secret -Role doctor_reader), 'mbe_runtime_count', 'p11_runtime_count', 'e2e_baseline', 'e2e_candidate')
        $logs = @("stdout", "stderr" | ForEach-Object { Join-Path $runtimeRoot "doctor-api-multi-be-$stamp.$_.log" } | Where-Object { Test-Path -LiteralPath $_ })
        $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 }).Count
        $summary.api_log_scan = [ordered]@{ status = $(if ($logs.Count -ne 2) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }); files = $logs.Count; bytes = @($logs | ForEach-Object { (Get-Item -LiteralPath $_).Length }) }
        if ($summary.api_log_scan.status -ne 'PASS' -and -not $cleanupFailure) { $cleanupFailure = 'api_log_scan' }
    }
    if ($fixtureStarted) {
        try { $summary.teardown = (& (Join-Path $fixtureDir 'teardown.ps1') -DockerExecutable $docker) | ConvertFrom-Json }
        catch { $cleanupFailure = 'fixture_teardown' }
    }
    foreach ($name in $previous.Keys) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    if ($null -ne $docker) {
        $after = Get-FixtureState $docker
        $summary.postconditions = [ordered]@{
            containers_absent = ($after.containers_present.Count -eq 0); network_absent = (-not $after.network_present)
            data_root_absent = (-not $after.data_root_present); ports_free = ($after.listening_ports.Count -eq 0)
        }
        if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    }
    if ($cleanupFailure) {
        $summary.status = 'FAIL'
        $summary.cleanup_failure = [ordered]@{ stage = $cleanupFailure; message_id = 'e2e_teardown_failed' }
    }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Multi-BE teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
