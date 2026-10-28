[CmdletBinding()]
param(
    [ValidateRange(1, 65535)][int]$ApiPort = 18048,
    [string]$RunnerPath = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'target/debug/examples/loopback-v2-e2e.exe'),
    [string]$FrontendDist = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'frontend/dist'),
    [string]$EvidenceRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\v02-development\task54-running-browser')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$containerName = 'sr-doctor-v0.1-e2e-3514-runtime'
$containerId = 'c62e80a6286f7de4259ce902b655022365058cfbc8c11f22e060e0083a876e81'
$image = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$queryMarker = 't54_' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$queryText = "SELECT /*$queryMarker*/ SUM(SLEEP(1+event_id-event_id)) FROM runtime_corpus.sales WHERE event_id BETWEEN 1 AND 20"
$querySetup = 'SET enable_profile = true, pipeline_profile_level = 1, runtime_profile_report_interval = 1, query_timeout = 30'
$runDirectory = Join-Path $EvidenceRoot ([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + $PID)
$previousQueryId = $env:E2E_RUNNING_QUERY_ID
$previousHandshake = $env:E2E_RUNNING_HANDSHAKE_DIR
$handshakeDirectory = Join-Path ([IO.Path]::GetFullPath($env:TEMP)) ('task54-running-' + [Guid]::NewGuid().ToString('N'))
$handshakeCreated = $false
$identityFileUsed = $false
$identityFileRemoved = $false
$browserProcess = $null
$browserNaturalExit = $false
$forcedCleanup = $false
$browserStartedTicks = $null
$browserReadyUtc = $null
$identityFoundUtc = $null
$startedContainer = $false
$queryProcess = $null
$queryId = $null
$queryExecutions = 0
$queryStartedUtc = $null
$browserLaunchUtc = $null
$observations = 0
$readinessQueries = 0
$status = 'FAIL'
$stage = 'preflight'
$failure = $null
$browserExit = $null
$queryExit = $null
$naturalCompletion = $false
$containerRestored = $false
$temporaryFiles = @()
$startedUtc = [DateTime]::UtcNow.ToString('o')

function Require([bool]$Condition, [string]$Code) { if (-not $Condition) { throw $Code } }
function TextHash([string]$Value) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
function Inspect-FixedContainer {
    $json = @(& docker inspect $containerName 2>$null)
    Require ($LASTEXITCODE -eq 0) 'fixture_inspect_failed'
    $value = @($json -join "`n" | ConvertFrom-Json)[0]
    Require ($value.Id -ceq $containerId -and $value.Config.Image -ceq $image) 'fixture_identity_mismatch'
    Require ($value.Config.Labels.'com.sr-doctor.e2e' -ceq '3.5.14') 'fixture_label_mismatch'
    # HostConfig remains inspectable while the fixed fixture is exited.
    $bindings = @($value.HostConfig.PortBindings.'9030/tcp')
    Require ($bindings.Count -eq 1 -and $bindings[0].HostIp -ceq '127.0.0.1' -and $bindings[0].HostPort -ceq '39230') 'fixture_binding_mismatch'
    return $value
}
function Find-ExactRunning([string[]]$Rows) {
    $found = @($Rows | ForEach-Object {
        $columns = $_.Split("`t")
        if ($columns.Count -eq 5 -and $columns[3] -ieq 'Running' -and $columns[4] -ceq $queryText) {
            Require ($columns[0] -match '^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$') 'fixture_query_identity_invalid'
            $columns[0]
        }
    })
    Require ($found.Count -le 1) 'fixture_query_identity_ambiguous'
    if ($found.Count -eq 1) { return $found[0] }
    return $null
}

New-Item -ItemType Directory -Force -Path $runDirectory | Out-Null
try {
    Require ($ApiPort -ne 8000) 'user_port_8000_forbidden'
    Require ($queryText.Length -le 128) 'fixture_sql_exceeds_exact_identity_boundary'
    Require (Test-Path -LiteralPath $RunnerPath -PathType Leaf) 'runner_missing'
    Require (Test-Path -LiteralPath (Join-Path $FrontendDist 'index.html') -PathType Leaf) 'frontend_missing'
    Require (@(Get-NetTCPConnection -LocalPort $ApiPort -State Listen -ErrorAction SilentlyContinue).Count -eq 0) 'api_port_busy'
    $fixture = Inspect-FixedContainer
    # This coordinator has exactly one admitted initial lifecycle: the known exited fixture.
    Require (-not $fixture.State.Running -and $fixture.State.Status -ceq 'exited') 'fixture_not_exited'
    $stage = 'start_fixed_fixture'
    $startedContainer = $true
    $null = & docker start $containerId 2>$null
    Require ($LASTEXITCODE -eq 0) 'fixture_start_failed'
    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        $null = & docker exec $containerId mysqladmin --host=127.0.0.1 --port=9030 --user=root ping 2>$null
        if ($LASTEXITCODE -eq 0) { $ready = $true; break }
        Start-Sleep -Seconds 1
    }
    Require $ready 'fixture_readiness_timeout'
    $fixture = Inspect-FixedContainer
    Require ([bool]$fixture.State.Running) 'fixture_not_running'
    $stage = 'verify_release_and_backend_readiness'
    $readinessQueries++
    $version = @(& docker exec $containerId mysql --host=127.0.0.1 --port=9030 --user=root --connect-timeout=2 --batch --raw --skip-column-names '--init-command=SET query_timeout=3' '--execute=SELECT current_version()' 2>$null)
    Require ($LASTEXITCODE -eq 0 -and $version.Count -eq 1 -and [string]$version[0] -ceq '3.5.14-23a56ec') 'fixture_release_mismatch'
    $backendReady = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        $readinessQueries++
        $backends = @(& docker exec $containerId mysql --host=127.0.0.1 --port=9030 --user=root --connect-timeout=2 --batch --raw '--init-command=SET query_timeout=3' '--execute=SHOW BACKENDS' 2>$null)
        if ($LASTEXITCODE -eq 0 -and $backends.Count -gt 1) {
            $columns = ([string]$backends[0]).Split("`t")
            $aliveIndex = [Array]::IndexOf($columns, 'Alive')
            if ($aliveIndex -ge 0) {
                $aliveRows = @($backends | Select-Object -Skip 1 | Where-Object {
                    $cells = ([string]$_).Split("`t")
                    $cells.Count -eq $columns.Count -and $cells[$aliveIndex] -ieq 'true'
                })
                if ($aliveRows.Count -eq $backends.Count - 1) { $backendReady = $true; break }
            }
        }
        if ($attempt -lt 29) { Start-Sleep -Seconds 1 }
    }
    Require $backendReady 'fixture_backend_readiness_timeout'
    $readinessQueries++
    $fixtureRows = @(& docker exec $containerId mysql --host=127.0.0.1 --port=9030 --user=root --connect-timeout=2 --batch --raw --skip-column-names '--init-command=SET query_timeout=3' '--execute=SELECT COUNT(*) FROM runtime_corpus.sales WHERE event_id BETWEEN 1 AND 20' 2>$null)
    Require ($LASTEXITCODE -eq 0 -and $fixtureRows.Count -eq 1 -and ([string]$fixtureRows[0]).Trim() -ceq '20') 'fixture_expected_exactly_twenty_rows'
    $stage = 'browser_ready_handshake'
    Require (-not (Test-Path -LiteralPath $handshakeDirectory)) 'handshake_directory_exists'
    New-Item -ItemType Directory -Path $handshakeDirectory | Out-Null
    $handshakeCreated = $true
    $env:E2E_RUNNING_QUERY_ID = $null
    $env:E2E_RUNNING_HANDSHAKE_DIR = $handshakeDirectory
    $wrapper = Join-Path $PSScriptRoot 'task31-development-browser.ps1'
    $browserStdout = Join-Path $handshakeDirectory 'wrapper.stdout'
    $browserStderr = Join-Path $handshakeDirectory 'wrapper.stderr'
    $browserLaunchUtc = [DateTime]::UtcNow.ToString('o')
    $browserProcess = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -ArgumentList @('-NoProfile', '-File', ('"{0}"' -f $wrapper), '-Project', 'connected-sql-docker', '-RunningProfile', '-ApiPort', [string]$ApiPort, '-RunnerPath', ('"{0}"' -f $RunnerPath), '-FrontendDist', ('"{0}"' -f $FrontendDist), '-EvidenceRoot', ('"{0}"' -f $runDirectory)) -WindowStyle Hidden -RedirectStandardOutput $browserStdout -RedirectStandardError $browserStderr -PassThru
    $browserStartedTicks = $browserProcess.StartTime.ToUniversalTime().Ticks
    $readyPath = Join-Path $handshakeDirectory 'ready.json'
    $readyDeadline = [DateTime]::UtcNow.AddSeconds(180)
    while (-not (Test-Path -LiteralPath $readyPath -PathType Leaf)) {
        Require (-not $browserProcess.HasExited) 'browser_exited_before_ready'
        Require ([DateTime]::UtcNow -lt $readyDeadline) 'browser_ready_timeout'
        Start-Sleep -Milliseconds 200
    }
    $readyReceipt = Get-Content -LiteralPath $readyPath -Raw | ConvertFrom-Json
    Require ($readyReceipt.schema -ceq 'task54-running-handshake/v1') 'handshake_schema_invalid'
    $parsedReady = [DateTimeOffset]::MinValue
    # ConvertFrom-Json can produce a UTC DateTime. Casting it to string loses Kind/offset.
    if ($readyReceipt.ready_at_utc -is [DateTime]) {
        Require ($readyReceipt.ready_at_utc.Kind -ne [DateTimeKind]::Unspecified) 'handshake_time_invalid'
        $parsedReady = [DateTimeOffset]::new($readyReceipt.ready_at_utc)
    } else {
        Require ([DateTimeOffset]::TryParse([string]$readyReceipt.ready_at_utc, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AssumeUniversal, [ref]$parsedReady)) 'handshake_time_invalid'
    }
    $browserReadyUtc = $parsedReady.ToUniversalTime().ToString('o')
    Require (-not $browserProcess.HasExited) 'browser_exited_after_ready'
    $stage = 'single_controlled_select'
    $stdout = Join-Path $env:TEMP ("task54-query-$PID-$queryMarker.stdout")
    $stderr = Join-Path $env:TEMP ("task54-query-$PID-$queryMarker.stderr")
    $temporaryFiles = @($stdout, $stderr)
    # Only this external coordinator executes the fixture SELECT, once; no retry.
    $queryStartedUtc = [DateTime]::UtcNow.ToString('o')
    $queryProcess = Start-Process -FilePath 'docker' -ArgumentList @('exec', $containerId, 'mysql', '--host=127.0.0.1', '--port=9030', '--user=root', '--comments', '--batch', '--raw', '--skip-column-names', ('--init-command="{0}"' -f $querySetup), ('--execute="{0}"' -f $queryText)) -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    $queryExecutions++
    $stage = 'bounded_exact_identity_observation'
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        if ($queryProcess.HasExited) { break }
        $observations++
        $rows = @(& docker exec $containerId mysql --host=127.0.0.1 --port=9030 --user=root --batch --raw --skip-column-names '--execute=SHOW PROFILELIST LIMIT 25' 2>$null)
        Require ($LASTEXITCODE -eq 0) 'fixture_observation_failed'
        $queryId = Find-ExactRunning $rows
        if ($null -ne $queryId) { break }
        if ($attempt -lt 9) { Start-Sleep -Milliseconds 750 }
    }
    if ($null -eq $queryId) { throw 'INCONCLUSIVE_running_window_not_observed' }
    $stage = 'publish_identity_and_wait_browser'
    $identityFoundUtc = [DateTime]::UtcNow.ToString('o')
    $identityTemp = Join-Path $handshakeDirectory 'identity.json.tmp'
    $identityPath = Join-Path $handshakeDirectory 'identity.json'
    [IO.File]::WriteAllText($identityTemp, (@{ schema = 'task54-running-handshake/v1'; query_id = $queryId; observed_at_utc = $identityFoundUtc } | ConvertTo-Json -Compress), [Text.UTF8Encoding]::new($false))
    $identityFileUsed = $true
    [IO.File]::Move($identityTemp, $identityPath)
    Require ($browserProcess.WaitForExit(240000)) 'browser_completion_timeout'
    $browserExit = $browserProcess.ExitCode
    if ($browserExit -ne 0) {
        # CDP assertion detail lives in the browser receipt, not the launch summary log.
        $browserReceipts = @(Get-ChildItem -LiteralPath $runDirectory -Recurse -Filter 'browser-e2e-receipt.json' -File)
        $missed = $false
        foreach ($path in $browserReceipts) {
            $browserReceipt = Get-Content -LiteralPath $path.FullName -Raw | ConvertFrom-Json
            $errorProperty = $browserReceipt.PSObject.Properties['error']
            if ($null -ne $errorProperty -and [string]$errorProperty.Value -match 'External Running candidate missing: INCONCLUSIVE|Expected exactly one external Running candidate; absence is INCONCLUSIVE') { $missed = $true }
        }
        if ($missed) { throw 'INCONCLUSIVE_running_window_missed_by_browser' }
        throw 'browser_acceptance_failed'
    }
    $status = 'PASS'
}
catch {
    # Never persist arbitrary native stderr, SQL, query identity, or credentials.
    $failure = if ($_.Exception.Message -match '^(?:INCONCLUSIVE_)?[a-z0-9_]+$') { $_.Exception.Message } else { 'coordinator_exception' }
    if ($failure.StartsWith('INCONCLUSIVE_')) { $status = 'INCONCLUSIVE' }
}
finally {
    [Environment]::SetEnvironmentVariable('E2E_RUNNING_QUERY_ID', $previousQueryId, 'Process')
    [Environment]::SetEnvironmentVariable('E2E_RUNNING_HANDSHAKE_DIR', $previousHandshake, 'Process')
    if ($null -ne $browserProcess -and -not $browserProcess.HasExited) {
        # Allow the browser's bounded handshake timeout and task31 finally to finish normally.
        if (-not $browserProcess.WaitForExit(120000)) {
            $liveProcess = Get-Process -Id $browserProcess.Id -ErrorAction SilentlyContinue
            if ($null -ne $liveProcess -and $liveProcess.StartTime.ToUniversalTime().Ticks -eq $browserStartedTicks -and $liveProcess.Path -ieq (Join-Path $PSHOME 'pwsh.exe')) {
                $forcedCleanup = $true
                $null = & taskkill /PID $browserProcess.Id /T /F 2>$null
            }
            $status = 'FAIL'; $failure = 'browser_cleanup_forced'
        }
    }
    $browserNaturalExit = $null -ne $browserProcess -and $browserProcess.HasExited -and -not $forcedCleanup
    if ($null -ne $queryProcess) {
        if (-not $queryProcess.HasExited) { $null = $queryProcess.WaitForExit(35000) }
        if ($queryProcess.HasExited) { $queryExit = $queryProcess.ExitCode; $naturalCompletion = $queryExit -eq 0 }
        else { Stop-Process -Id $queryProcess.Id -Force -ErrorAction SilentlyContinue }
    }
    if ($startedContainer) {
        try {
            $fixture = Inspect-FixedContainer
            if ($fixture.State.Running) {
                $null = & docker stop --time 10 $containerId 2>$null
                Require ($LASTEXITCODE -eq 0) 'fixture_stop_failed'
            }
            $fixture = Inspect-FixedContainer
            $containerRestored = -not $fixture.State.Running -and $fixture.State.Status -ceq 'exited'
        } catch { $containerRestored = $false }
        if (-not $containerRestored) { $status = 'FAIL'; $failure = 'fixture_restore_unconfirmed' }
    }
    foreach ($path in $temporaryFiles) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
    if ($handshakeCreated) {
        try {
            $tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\', '/')
            $resolvedHandshake = (Resolve-Path -LiteralPath $handshakeDirectory).Path
            Require ([IO.Path]::GetDirectoryName($resolvedHandshake).TrimEnd('\', '/') -ieq $tempRoot -and [IO.Path]::GetFileName($resolvedHandshake) -match '^task54-running-[0-9a-f]{32}$') 'handshake_cleanup_path_invalid'
            Require (((Get-Item -LiteralPath $resolvedHandshake).Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) 'handshake_cleanup_reparse_point'
            foreach ($name in @('ready.json', 'ready.json.tmp', 'identity.json', 'identity.json.tmp', 'wrapper.stdout', 'wrapper.stderr')) {
                Remove-Item -LiteralPath (Join-Path $resolvedHandshake $name) -Force -ErrorAction SilentlyContinue
            }
            [IO.Directory]::Delete($resolvedHandshake, $false)
            $identityFileRemoved = -not (Test-Path -LiteralPath $resolvedHandshake)
        } catch { $status = 'FAIL'; $failure = 'handshake_cleanup_unconfirmed' }
    }
    if ($status -eq 'PASS' -and -not $naturalCompletion) { $status = 'FAIL'; $failure = 'query_natural_completion_unconfirmed' }
    $receipt = [ordered]@{
        schema = 'task54-running-browser-coordinator/v1'; status = $status; stage = $stage; failure = $failure
        started_utc = $startedUtc; finished_utc = [DateTime]::UtcNow.ToString('o')
        container_id_sha256 = TextHash $containerId; image = $image
        query = [ordered]@{ executions = $queryExecutions; sql_sha256 = TextHash $queryText; id_sha256 = $(if ($queryId) { TextHash $queryId } else { $null }); row_count = 20; sleep_seconds = 1; timeout_seconds = 30; query_started_utc = $queryStartedUtc; completed_naturally = $naturalCompletion; client_exit_code = $queryExit }
        fixture_observation = [ordered]@{ calls = $observations; maximum = 10; limit = 25; product_polling = $false }
        readiness_queries = $readinessQueries; browser_exit = $browserExit; browser_launch_utc = $browserLaunchUtc; browser_ready_utc = $browserReadyUtc; identity_found_utc = $identityFoundUtc; api_port = $ApiPort; user_port_8000_touched = $false
        cleanup = [ordered]@{ container_restored_exited = $containerRestored; query_environment_restored = $true; browser_exited_naturally = $browserNaturalExit; forced_process_cleanup_attempted = $forcedCleanup; temporary_identity_file_used = $identityFileUsed; temporary_identity_file_removed = $identityFileRemoved; raw_sql_or_credentials_persisted = $false }
        limitations = @('development_only', 'one_select_no_retry', 'handshake_is_fixture_only_not_product_polling', 'not_candidate_acceptance')
    }
    $receiptPath = Join-Path $runDirectory 'task54-coordinator-receipt.json'
    [IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
}
Write-Output "TASK54_RUNNING_BROWSER_$status receipt=$receiptPath"
if ($status -eq 'PASS') { exit 0 }
if ($status -eq 'INCONCLUSIVE') { exit 2 }
exit 1