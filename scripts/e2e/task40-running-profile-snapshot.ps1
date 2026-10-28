param(
    [ValidateRange(1, 65535)]
    [int]$ApiPort = 18042,
    [ValidateRange(1, 65535)]
    [int]$MySqlPort = 39230,
    [string]$RunnerPath = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'target/debug/examples/loopback-v2-e2e.exe'),
    [string]$EvidenceRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\v02-development\running-snapshot-api-20260906'),
    [string]$PasswordEnv = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$containerName = 'sr-doctor-v0.1-e2e-3514-runtime'
$expectedImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$expectedRelease = '3.5.14-23a56ec'
$queryFixtureId = 'task40_session_profiled_be_scan_20_rows'
$queryMarker = 't54_' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$queryText = "SELECT /*$queryMarker*/ SUM(SLEEP(1+event_id-event_id)) FROM runtime_corpus.sales WHERE event_id BETWEEN 1 AND 20"
$querySessionSetup = 'SET enable_profile = true, pipeline_profile_level = 1, runtime_profile_report_interval = 1, query_timeout = 30'
$fixtureObservationMaximum = 10
$queryCompletionWaitMilliseconds = 35000
$profilePublicationSettleMilliseconds = 1200

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function New-Inconclusive([string]$Message) {
    throw "INCONCLUSIVE: $Message"
}

function Find-ControlledRunningQueryId([string[]]$Rows, [string]$ExpectedSql) {
    # Fixed 3.5.14 ProfileElement.toRow has five columns and truncates SQL above 128 chars.
    Require ($ExpectedSql.Length -le 128) 'Controlled SQL exceeds exact ProfileList identity boundary.'
    $matches = @($Rows | ForEach-Object {
        $columns = $_.Split("`t")
        if ($columns.Count -eq 5 -and $columns[3] -ieq 'Running' -and $columns[4] -ceq $ExpectedSql) {
            Require ($columns[0] -match '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$') 'Controlled Query ID is malformed.'
            $columns[0]
        }
    })
    Require ($matches.Count -le 1) 'Controlled SQL matched multiple Running identities.'
    if ($matches.Count -eq 1) { return $matches[0] }
    return $null
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-TextSha256([string]$Value) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Get-SafeContainerIdentity {
    $format = '{{.Id}}|{{.Config.Image}}|{{.State.Running}}|{{index .Config.Labels "com.sr-doctor.e2e"}}|{{(index (index .NetworkSettings.Ports "9030/tcp") 0).HostIp}}|{{(index (index .NetworkSettings.Ports "9030/tcp") 0).HostPort}}'
    $line = @(& docker inspect --format $format $containerName 2>$null | Select-Object -First 1)
    Require ($line.Count -eq 1) 'Fixed Docker fixture is absent.'
    $parts = ([string]$line[0]).Split('|')
    Require ($parts.Count -eq 6) 'Fixed Docker fixture identity did not have the expected safe shape.'
    Require ($parts[1] -ceq $expectedImage) 'Fixed Docker fixture image digest mismatch.'
    Require ($parts[2] -ceq 'true') 'Fixed Docker fixture is not running.'
    Require ($parts[3] -ceq '3.5.14') 'Fixed Docker fixture release label mismatch.'
    Require ($parts[4] -ceq '127.0.0.1' -and [int]$parts[5] -eq $MySqlPort) 'Fixed Docker MySQL port is not the expected loopback binding.'
    [ordered]@{
        id = $parts[0].Substring(0, [Math]::Min(12, $parts[0].Length))
        image = $parts[1]
        release_label = $parts[3]
        mysql_loopback_port = [int]$parts[5]
    }
}

function Assert-LoopbackPortFree([int]$Port) {
    $listeners = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)
    Require ($listeners.Count -eq 0) "Temporary API port $Port is already listening."
}

function Get-SafeApiErrorField($Value) {
    $text = [string]$Value
    if ($text -match '^[a-z0-9_]{1,64}$') { return $text }
    return $null
}

function Get-SafeApiErrorEnvelope([string]$Content) {
    try {
        $envelope = $Content | ConvertFrom-Json -ErrorAction Stop
        return [pscustomobject]@{
            code = Get-SafeApiErrorField $envelope.code
            stage = Get-SafeApiErrorField $envelope.stage
            message_id = Get-SafeApiErrorField $envelope.message_id
            recoverability = Get-SafeApiErrorField $envelope.recoverability
        }
    }
    catch {
        return [pscustomobject]@{ code = $null; stage = $null; message_id = $null; recoverability = $null }
    }
}

function Invoke-Api {
    param(
        [Parameter(Mandatory)][ValidateSet('Get', 'Post', 'Delete')][string]$Method,
        [Parameter(Mandatory)][string]$Path,
        $Payload = $null,
        [hashtable]$Headers = @{}
    )

    $parameters = @{
        Uri = "$script:apiBase$Path"
        Method = $Method
        Headers = $Headers
        MaximumRedirection = 0
        ErrorAction = 'Stop'
        TimeoutSec = 15
    }
    if ($Method -in @('Post', 'Delete')) { $parameters.ContentType = 'application/json' }
    if ($null -ne $Payload) { $parameters.Body = $Payload | ConvertTo-Json -Depth 32 -Compress }
    try {
        $response = Invoke-WebRequest @parameters -SkipHttpErrorCheck
        if ([int]$response.StatusCode -ge 400) {
            $safe = Get-SafeApiErrorEnvelope ([string]$response.Content)
            return [pscustomobject]@{
                status = [int]$response.StatusCode; body = $null; raw = ''
                error_code = $safe.code; error_stage = $safe.stage; error_message_id = $safe.message_id; error_recoverability = $safe.recoverability
            }
        }
        $body = if ([string]::IsNullOrWhiteSpace($response.Content)) { $null } else { $response.Content | ConvertFrom-Json }
        return [pscustomobject]@{
            status = [int]$response.StatusCode; body = $body; raw = [string]$response.Content
            error_code = $null; error_stage = $null; error_message_id = $null; error_recoverability = $null
        }
    }
    catch {
        return [pscustomobject]@{
            status = 0; body = $null; raw = ''
            error_code = $null; error_stage = $null; error_message_id = $null; error_recoverability = $null
        }
    }
}

function Wait-Runner([int]$Port, [System.Diagnostics.Process]$Process) {
    for ($attempt = 1; $attempt -le 40; $attempt++) {
        if ($Process.HasExited) { throw "Temporary runner exited before readiness (exit=$($Process.ExitCode))." }
        try {
            $health = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/api/v1/health" -Method Get -TimeoutSec 1 -ErrorAction Stop
            if ([int]$health.StatusCode -eq 200) { return $attempt }
        }
        catch { }
        Start-Sleep -Milliseconds 250
    }
    throw 'Temporary runner did not become healthy on its dedicated loopback port.'
}

$startedAtUtc = [DateTime]::UtcNow.ToString('o')
$runDirectory = Join-Path $EvidenceRoot ("running-snapshot-{0}-{1}" -f [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'), $PID)
$temporaryFiles = @()
$runner = $null
$queryProcess = $null
$connectionId = $null
$headers = @{}
$sessionDeleted = $false
$runnerStopped = $false
$queryStopped = $false
$status = 'FAIL'
$stage = 'initialize'
$failure = $null
$assertions = 0
$result = [ordered]@{}
$container = $null
$runnerSha256 = $null
$runnerReadyAttempts = 0
$password = $null
$runningCandidatesHttpCalls = 0
$runningSnapshotHttpCalls = 0
$queryFinishedNaturally = $false
$queryExitCode = $null
$fixtureObservationCalls = 0
$controlledQueryId = $null
$queryExecutions = 0

New-Item -ItemType Directory -Force -Path $runDirectory | Out-Null
$receiptPath = Join-Path $runDirectory 'receipt.json'

try {
    $stage = 'validate_fixture'
    $container = Get-SafeContainerIdentity
    $assertions++
    Assert-LoopbackPortFree $ApiPort
    $assertions++
    Require (Test-Path -LiteralPath $RunnerPath -PathType Leaf) 'Loopback runner is absent; build the current source example before this verification.'
    $runnerPath = (Resolve-Path -LiteralPath $RunnerPath).Path
    $runnerSha256 = Get-Sha256 $runnerPath
    $assertions++
    $password = if ([string]::IsNullOrEmpty($PasswordEnv)) { '' } else { [Environment]::GetEnvironmentVariable($PasswordEnv, 'Process') }
    Require ($null -ne $password) 'Configured password environment variable is unavailable.'

    $stage = 'start_loopback_runner'
    $runnerStdout = Join-Path $env:TEMP ("task40-runner-$PID.stdout")
    $runnerStderr = Join-Path $env:TEMP ("task40-runner-$PID.stderr")
    $temporaryFiles += @($runnerStdout, $runnerStderr)
    $runner = Start-Process -FilePath $runnerPath -ArgumentList @([string]$ApiPort) -WorkingDirectory (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -WindowStyle Hidden -RedirectStandardOutput $runnerStdout -RedirectStandardError $runnerStderr -PassThru
    $runnerReadyAttempts = Wait-Runner $ApiPort $runner
    $assertions++
    $script:apiBase = "http://127.0.0.1:$ApiPort/api/v1"

    $stage = 'create_isolated_api_session'
    $capabilities = Invoke-Api Get '/capabilities'
    Require ($capabilities.status -eq 200 -and -not [string]::IsNullOrWhiteSpace($capabilities.body.csrf.nonce)) 'Temporary runner capability response lacked CSRF nonce.'
    $headers['X-Doctor-CSRF'] = [string]$capabilities.body.csrf.nonce
    $session = Invoke-Api Post '/connections' @{
        host = '127.0.0.1'; port = $MySqlPort; username = 'root'; password = $password
        catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext'
    } $headers
    Require ($session.status -eq 200 -and -not [string]::IsNullOrWhiteSpace($session.body.connection_id)) 'Temporary runner could not create the isolated Docker session.'
    Require ([string]$session.body.detected_version.value -ceq $expectedRelease) 'Temporary runner did not detect the fixed StarRocks release.'
    $connectionId = [string]$session.body.connection_id
    $assertions += 2

    $stage = 'execute_one_controlled_select'
    $queryStdout = Join-Path $env:TEMP ("task40-query-$PID.stdout")
    $queryStderr = Join-Path $env:TEMP ("task40-query-$PID.stderr")
    $temporaryFiles += @($queryStdout, $queryStderr)
    # This test-only Docker CLI action is deliberately not exposed through Doctor.
    # --init-command applies only to this disposable CLI session; no GLOBAL setting is used.
    $queryProcess = Start-Process -FilePath 'docker' -ArgumentList @('exec', $containerName, 'mysql', '--host=127.0.0.1', '--port=9030', '--user=root', '--comments', '--batch', '--raw', '--skip-column-names', ('--init-command="{0}"' -f $querySessionSetup), ('--execute="{0}"' -f $queryText)) -WindowStyle Hidden -RedirectStandardOutput $queryStdout -RedirectStandardError $queryStderr -PassThru
    $queryExecutions++
    $stage = 'observe_fixture_publication_and_identity'
    # Fixture-only bounded observation; no product polling, SQL retry, raw or official reads.
    for ($observation = 0; $observation -lt $fixtureObservationMaximum; $observation++) {
        if ($queryProcess.HasExited) { break }
        $fixtureObservationCalls++
        $rows = @(& docker exec $containerName mysql '--host=127.0.0.1' '--port=9030' '--user=root' '--batch' '--raw' '--skip-column-names' '--execute=SHOW PROFILELIST LIMIT 25' 2>$null)
        Require ($LASTEXITCODE -eq 0) 'Fixture-only ProfileList observation failed.'
        $controlledQueryId = Find-ControlledRunningQueryId $rows $queryText
        if ($null -ne $controlledQueryId) { break }
        if ($observation + 1 -lt $fixtureObservationMaximum) { Start-Sleep -Milliseconds 750 }
    }
    if ($null -eq $controlledQueryId) { New-Inconclusive 'The controlled query did not publish an exactly identified Running Profile within bounded fixture observations.' }

    $stage = 'observe_running_candidates_once'
    $candidateRequest = @{ connection_id = $connectionId; limit = 25 }
    $runningCandidatesHttpCalls++
    $firstCandidates = Invoke-Api Post "/connections/$connectionId/profiles/running-candidates" $candidateRequest $headers
    Require ($firstCandidates.status -eq 200) 'Running candidates route did not return HTTP 200.'
    Require ([string]$firstCandidates.body.source -ceq 'profile_list') 'Running candidates route used an unexpected source.'
    Require ($null -eq $firstCandidates.body.profile_observed_at_utc) 'ProfileList source observation time was fabricated.'
    $target = @($firstCandidates.body.candidates | Where-Object { [string]$_.status -ieq 'running' -and [string]$_.query_id -ieq $controlledQueryId })
    $result['candidate_before_finish'] = [ordered]@{
        http_status = $firstCandidates.status; http_requests = 1; returned = [int]$firstCandidates.body.returned
        discovery_generation = [int]$firstCandidates.body.discovery_generation; profile_observed_at_utc = $firstCandidates.body.profile_observed_at_utc
        running_candidate_found = $target.Count -eq 1
    }
    if ($target.Count -ne 1) { New-Inconclusive 'The fixed Docker ProfileList did not expose the controlled query as Running during the one-shot observation.' }
    $targetQueryId = [string]$target[0].query_id
    $targetQueryHash = Get-TextSha256 $targetQueryId
    $firstGeneration = [int]$firstCandidates.body.discovery_generation
    Require ($firstGeneration -gt 0 -and -not [string]::IsNullOrWhiteSpace($targetQueryId)) 'Running candidate lacked a usable generation or query identity.'
    Require ([string]$target[0].connection_id -ceq $connectionId -and [string]$target[0].discovery_source -ceq 'profile_list' -and [long]$target[0].discovery_generation -eq $firstGeneration) 'Candidate identity differs from current session/list.'
    Require ([int]$firstCandidates.body.returned -eq @($firstCandidates.body.candidates).Count) 'Candidate returned count differs from its array.'
    $result['candidate_before_finish']['target_query_id_sha256'] = $targetQueryHash
    $assertions += 6

    $stage = 'wait_for_controlled_select_finish'
    if (-not $queryProcess.WaitForExit($queryCompletionWaitMilliseconds)) { New-Inconclusive 'The single controlled profile-enabled BE fixture did not finish inside its bounded wait.' }
    $queryFinishedNaturally = $true
    $queryExitCode = $queryProcess.ExitCode
    Require ($queryProcess.ExitCode -eq 0) 'The single controlled profile-enabled BE fixture exited unsuccessfully.'
    $assertions++

    # The query completed, but StarRocks publishes its final profile asynchronously.
    # This fixed local settle does not issue a profile read or add a retry.
    Start-Sleep -Milliseconds $profilePublicationSettleMilliseconds

    $stage = 'snapshot_observed_running_after_finish_once'
    $snapshotRequest = @{ connection_id = $connectionId; query_id = $targetQueryId; discovery_generation = $firstGeneration }
    $runningSnapshotHttpCalls++
    $snapshot = Invoke-Api Post "/connections/$connectionId/profiles/running-snapshot" $snapshotRequest $headers
    $result['snapshot_attempt'] = [ordered]@{
        http_status = $snapshot.status; http_requests = 1
        error_code = $snapshot.error_code; error_stage = $snapshot.error_stage
        error_message_id = $snapshot.error_message_id; error_recoverability = $snapshot.error_recoverability
    }
    Require ($snapshot.status -eq 200) 'The one-off Running snapshot route did not return HTTP 200 after the controlled query finished.'
    Require ([string]$snapshot.body.query.connection_id -ceq $connectionId -and [string]$snapshot.body.query.query_id -ieq $targetQueryId -and [long]$snapshot.body.query.discovery_generation -eq $firstGeneration -and [string]$snapshot.body.query.discovery_source -ceq 'profile_list' -and [string]$snapshot.body.query.status -ieq 'Running') 'Snapshot identity differs from the exact request and discovery.'
    Require ([string]$snapshot.body.list_observed_state -ieq 'running') 'Snapshot did not retain the earlier ProfileList Running observation.'
    Require ([string]$snapshot.body.profile_lifecycle -ceq 'finished') 'Snapshot parser did not report the independently fetched finished lifecycle.'
    Require ($null -eq $snapshot.body.profile_observed_at_utc -and -not [string]::IsNullOrWhiteSpace([string]$snapshot.body.read_at_utc)) 'Snapshot time fields lost unknown-source versus API-receipt separation.'
    Require ([string]$snapshot.body.input_origin -ceq 'connected_profile_fetch' -and [string]$snapshot.body.source_kind -ceq 'text_profile') 'Snapshot did not keep the existing fetched-profile acquisition shape.'
    Require ($snapshot.raw -notmatch '(?i)"(?:password|candidate_sql|profile_text|sql)"\s*:') 'Snapshot response leaked a protected source field.'
    $assertions += 7

    $stage = 'refresh_after_finish'
    $runningCandidatesHttpCalls++
    $secondCandidates = Invoke-Api Post "/connections/$connectionId/profiles/running-candidates" $candidateRequest $headers
    Require ($secondCandidates.status -eq 200) 'Post-finish Running candidates route did not return HTTP 200.'
    Require ([int]$secondCandidates.body.discovery_generation -gt $firstGeneration) 'Post-finish candidate observation did not supersede the prior discovery generation.'
    Require (@($secondCandidates.body.candidates | Where-Object { [string]$_.query_id -ieq $targetQueryId }).Count -eq 0) 'Completed controlled query remained a Running candidate.'
    $assertions += 3

    $stage = 'reject_stale_generation'
    $runningSnapshotHttpCalls++
    $stale = Invoke-Api Post "/connections/$connectionId/profiles/running-snapshot" $snapshotRequest $headers
    Require ($stale.status -eq 409) 'Old generation snapshot was not rejected after the post-finish observation.'
    $assertions++

    $result = [ordered]@{
        candidate_before_finish = [ordered]@{
            http_status = $firstCandidates.status; http_requests = 1; returned = [int]$firstCandidates.body.returned
            discovery_generation = $firstGeneration; profile_observed_at_utc = $firstCandidates.body.profile_observed_at_utc
            target_query_id_sha256 = $targetQueryHash
        }
        snapshot_after_finish = [ordered]@{
            http_status = $snapshot.status; http_requests = 1; list_observed_state = [string]$snapshot.body.list_observed_state
            profile_lifecycle = [string]$snapshot.body.profile_lifecycle; input_origin = [string]$snapshot.body.input_origin
            source_kind = [string]$snapshot.body.source_kind; read_at_utc_present = -not [string]::IsNullOrWhiteSpace([string]$snapshot.body.read_at_utc)
            profile_observed_at_utc = $snapshot.body.profile_observed_at_utc; raw_profile_returned = $false
        }
        candidate_after_finish = [ordered]@{
            http_status = $secondCandidates.status; http_requests = 1; returned = [int]$secondCandidates.body.returned
            discovery_generation = [int]$secondCandidates.body.discovery_generation; controlled_query_still_running_candidate = $false
        }
        stale_snapshot_after_refresh = [ordered]@{ http_status = $stale.status; http_requests = 1; expected_connector_io = 0 }
    }
    $status = 'PASS'
}
catch {
    $failure = $_.Exception.Message
    if ($failure.StartsWith('INCONCLUSIVE:')) { $status = 'INCONCLUSIVE' }
}
finally {
    $stageAtClose = $stage
    if ($null -ne $connectionId -and $headers.Count -gt 0) {
        try {
            $delete = Invoke-Api Delete "/connections/$connectionId" $null $headers
            $sessionDeleted = $delete.status -in @(200, 204)
            if (-not $sessionDeleted -and $status -eq 'PASS') { $status = 'FAIL'; $failure = 'Temporary API session cleanup failed.' }
        }
        catch {
            if ($status -eq 'PASS') { $status = 'FAIL'; $failure = 'Temporary API session cleanup raised an exception.' }
        }
    }
    if ($null -ne $queryProcess -and -not $queryProcess.HasExited) {
        # Allow the session-scoped 30s query timeout before stopping the local attach client.
        if (-not $queryProcess.WaitForExit($queryCompletionWaitMilliseconds)) {
            Stop-Process -Id $queryProcess.Id -Force -ErrorAction SilentlyContinue
            $queryStopped = $queryProcess.WaitForExit(5000)
        }
    }
    if ($null -ne $queryProcess -and $queryProcess.HasExited -and -not $queryStopped) {
        $queryExitCode = $queryProcess.ExitCode
        $queryFinishedNaturally = $queryExitCode -eq 0
    }
    if ($null -ne $runner -and -not $runner.HasExited) {
        Stop-Process -Id $runner.Id -Force -ErrorAction SilentlyContinue
        $runnerStopped = $runner.WaitForExit(5000)
    }
    if ($null -ne $runner -and $runner.HasExited) { $runnerStopped = $true }
    if ($null -ne $runner -and -not $runnerStopped) { $status = 'FAIL'; $failure = 'Temporary runner exit not confirmed.' }
    foreach ($path in $temporaryFiles) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
    $password = $null
    $receipt = [ordered]@{
        schema_version = 'task40-running-profile-snapshot-docker-e2e/v1'
        status = $status
        stage = $stageAtClose
        failure = $failure
        started_at_utc = $startedAtUtc
        finished_at_utc = [DateTime]::UtcNow.ToString('o')
        assertions = $assertions
        scope = 'fresh loopback development runner plus one controlled isolated-Docker SELECT; not user port 8000, Candidate, browser, production, or arbitrary SQL product capability'
        container = $container
        runner = [ordered]@{ kind = 'loopback-v2-e2e'; port = $ApiPort; sha256 = $runnerSha256; readiness_requests = $runnerReadyAttempts; user_port_8000_touched = $false }
        controlled_query = [ordered]@{
            fixture_id = $queryFixtureId; executions = $queryExecutions; raw_sql_persisted = $false; completed_naturally = $queryFinishedNaturally; client_exit_code = $queryExitCode; stopped_by_cleanup = $queryStopped
            cleanup_stop_scope = 'local_attach_client_only'; server_query_termination_verified = $queryFinishedNaturally; fixture_stop_required_if_unverified = (-not $queryFinishedNaturally -and $queryExecutions -gt 0)
            session_only_profile = [ordered]@{ enable_profile = $true; pipeline_profile_level = 1; runtime_profile_report_interval_seconds = 1; query_timeout_seconds = 30; global_configuration_changed = $false }
            post_exit_profile_publication_settle_milliseconds = $profilePublicationSettleMilliseconds
        }
        http_calls = [ordered]@{ running_candidates = $runningCandidatesHttpCalls; running_snapshot = $runningSnapshotHttpCalls; runtime_profile = 0; analyze_profile = 0; explain = 0 }
        fixture_observation = [ordered]@{ profile_list_calls = $fixtureObservationCalls; maximum_calls = $fixtureObservationMaximum; exact_controlled_identity_observed = ($null -ne $controlledQueryId); sql_sha256 = (Get-TextSha256 $queryText); product_polling = $false }
        operation_count_evidence = 'Live receipt records HTTP requests only. Current connector/router tests assert successful Running snapshot GetQueryProfile=1 and stale generation=0; no database wire trace was enabled.'
        observations = $result
        cleanup = [ordered]@{ temporary_session_deleted = $sessionDeleted; temporary_runner_stopped = $runnerStopped; database_container_left_unchanged = $true; credentials_persisted = $false }
        limitations = @('single_fixed_docker_fixture', 'no_database_wire_trace', 'no_ui_or_browser_validation', 'not_candidate_or_production_acceptance')
    }
    [IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json -Depth 16), [Text.UTF8Encoding]::new($false))
}

if ($status -eq 'PASS') {
    Write-Output "TASK40_RUNNING_SNAPSHOT_PASS assertions=$assertions receipt=$receiptPath"
    exit 0
}
if ($status -eq 'INCONCLUSIVE') {
    Write-Error "TASK40_RUNNING_SNAPSHOT_INCONCLUSIVE stage=$stage failure=$failure receipt=$receiptPath"
    exit 2
}
Write-Error "TASK40_RUNNING_SNAPSHOT_FAIL stage=$stage failure=$failure receipt=$receiptPath"
exit 1
