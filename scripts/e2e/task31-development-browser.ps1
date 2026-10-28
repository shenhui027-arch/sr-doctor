[CmdletBinding()]
param(
    [ValidateSet('offline-local', 'connected-sql-docker', 'connected-audit-docker')][string]$Project = 'offline-local',
    [ValidateRange(1, 65535)][int]$ApiPort = 18048,
    [string]$RunnerPath = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'target/debug/examples/loopback-v2-e2e.exe'),
    [string]$FrontendDist = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'frontend/dist'),
    [string]$EvidenceRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\v02-development\browser-offline-local-20260906'),
    [switch]$RunningProfile
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($RunningProfile -and $Project -ne 'connected-sql-docker') {
    throw '-RunningProfile requires the exclusive connected-sql-docker development browser path.'
}
# The external fixture owns this identity. Inherit it without copying it into arguments or receipts.
if ($RunningProfile) {
    $hasQueryId = -not [string]::IsNullOrWhiteSpace($env:E2E_RUNNING_QUERY_ID)
    $hasHandshake = -not [string]::IsNullOrWhiteSpace($env:E2E_RUNNING_HANDSHAKE_DIR)
    if ($hasQueryId -eq $hasHandshake) { throw '-RunningProfile requires exactly one external query identity or handshake directory.' }
    if ($hasQueryId -and $env:E2E_RUNNING_QUERY_ID -notmatch '^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$') {
        throw '-RunningProfile requires a valid external query identity.'
    }
    if ($hasHandshake) {
        $handshakePath = $env:E2E_RUNNING_HANDSHAKE_DIR
        $tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\', '/')
        if (-not [IO.Path]::IsPathFullyQualified($handshakePath) -or -not (Test-Path -LiteralPath $handshakePath -PathType Container)) { throw 'Running handshake directory must exist at an absolute path.' }
        $resolvedHandshake = (Resolve-Path -LiteralPath $handshakePath).Path
        if ([IO.Path]::GetDirectoryName($resolvedHandshake).TrimEnd('\', '/') -ine $tempRoot -or [IO.Path]::GetFileName($resolvedHandshake) -notmatch '^task54-running-[0-9a-f]{32}$' -or ((Get-Item -LiteralPath $resolvedHandshake).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Running handshake directory is outside the fixed temporary boundary.' }
        if (@('ready.json', 'ready.json.tmp', 'identity.json', 'identity.json.tmp') | Where-Object { Test-Path -LiteralPath (Join-Path $resolvedHandshake $_) }) { throw 'Running handshake directory contains stale protocol files.' }
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$RunnerPath = (Resolve-Path -LiteralPath $RunnerPath).Path
$FrontendDist = (Resolve-Path -LiteralPath $FrontendDist).Path
$browserPath = (Resolve-Path (Join-Path $repoRoot 'tests\e2e\browser-e2e.mjs')).Path
$baseUrl = "http://127.0.0.1:$ApiPort"
$runToken = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + $PID
$runStartedUtc = [DateTime]::UtcNow

New-Item -ItemType Directory -Force -Path $EvidenceRoot | Out-Null

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sha256Text([string]$Value) {
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        (($sha256.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)) | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $sha256.Dispose()
    }
}

function Get-FrontendIdentity([string]$Dist) {
    $prefix = $Dist.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $files = @(Get-ChildItem -LiteralPath $Dist -Recurse -File | ForEach-Object {
        $relative = $_.FullName.Substring($prefix.Length).Replace('\', '/')
        [pscustomobject]@{
            path = $relative
            byte_length = $_.Length
            sha256 = Get-Sha256 $_.FullName
        }
    } | Sort-Object -Property path)
    if ($files.Count -eq 0) { throw 'FrontendDist contains no files.' }
    $canonical = (($files | ForEach-Object { "$($_.path)`0$($_.byte_length)`0$($_.sha256)" }) -join "`n")
    [ordered]@{
        file_count = $files.Count
        tree_sha256 = Get-Sha256Text $canonical
        latest_file_write_utc = (($files | ForEach-Object { (Get-Item -LiteralPath (Join-Path $Dist $_.path)).LastWriteTimeUtc } | Sort-Object -Descending | Select-Object -First 1).ToString('o'))
    }
}

function Wait-Api {
    for ($attempt = 1; $attempt -le 180; $attempt++) {
        if ($null -ne $script:runner -and $script:runner.HasExited) { throw 'Temporary development runner exited before readiness.' }
        try {
            $health = Invoke-WebRequest -Uri "$baseUrl/api/v1/capabilities" -Method Get -TimeoutSec 1 -ErrorAction Stop
            if ([int]$health.StatusCode -eq 200) { return $attempt }
        }
        catch { }
        Start-Sleep -Milliseconds 250
    }
    throw "Temporary development runner did not become healthy on port $ApiPort."
}

$runnerStdout = Join-Path $EvidenceRoot "runner-$runToken.stdout.log"
$runnerStderr = Join-Path $EvidenceRoot "runner-$runToken.stderr.log"
$browserLog = Join-Path $EvidenceRoot "browser-node-$runToken.log"
$launchReceiptPath = Join-Path $EvidenceRoot "development-browser-launch-$runToken.json"
$runner = $null
$nodeExit = $null
$browserReceiptPath = $null
$status = 'FAIL'
$failure = $null
$cleanup = [ordered]@{
    runner_stopped = $false
    api_port_free_after = $null
    environment_restored = $false
}
$distIdentity = Get-FrontendIdentity $FrontendDist
$distItem = Get-Item -LiteralPath $FrontendDist
$runnerHash = Get-Sha256 $RunnerPath
$browserHash = Get-Sha256 $browserPath
$port8000Before = @(Get-NetTCPConnection -LocalPort 8000 -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
$port18048Before = @(Get-NetTCPConnection -LocalPort $ApiPort -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
$previousFrontendDist = $env:DOCTOR_FRONTEND_DIST
$previousBaseUrl = $env:E2E_BASE_URL
$previousArtifactRoot = $env:E2E_ARTIFACT_ROOT
$previousChromeProfileRoot = $env:E2E_CHROME_PROFILE_ROOT
$previousFixedDockerValidated = $env:E2E_FIXED_DOCKER_VALIDATED
$receipt = [ordered]@{
    schema = 'sr-doctor.development-browser-launch/v1'
    scope = 'development'
    project = $Project
    status = 'FAIL'
    api = [ordered]@{
        base_url = $baseUrl
        loopback_port = $ApiPort
        port_8000_listeners_before = $port8000Before.Count
        port_8000_touched = $false
    }
    runner = [ordered]@{
        path = $RunnerPath
        sha256 = $runnerHash
        pid = $null
        stdout = $runnerStdout
        stderr = $runnerStderr
        readiness = $null
    }
    frontend_dist = [ordered]@{
        path = $FrontendDist
        root_last_write_utc = $distItem.LastWriteTimeUtc.ToString('o')
        latest_file_write_utc = $distIdentity.latest_file_write_utc
        file_count = $distIdentity.file_count
        tree_sha256 = $distIdentity.tree_sha256
        source_build_performed = $false
    }
    browser_script = [ordered]@{
        path = $browserPath
        sha256 = $browserHash
        running_profile_opt_in = [bool]$RunningProfile
    }
    evidence_root = $EvidenceRoot
    browser_receipt = $null
    screenshots = @()
    cleanup = $cleanup
    started_utc = $runStartedUtc.ToString('o')
}

try {
    if ($port18048Before.Count -ne 0) { throw "API port $ApiPort is already listening." }
    if (-not (Test-Path -LiteralPath (Join-Path $FrontendDist 'index.html') -PathType Leaf)) { throw 'frontend/dist/index.html is missing.' }
    if ($Project -in @('connected-sql-docker', 'connected-audit-docker')) {
        $container = @(& docker inspect 'sr-doctor-v0.1-e2e-3514-runtime' | ConvertFrom-Json)[0]
        if ($LASTEXITCODE -ne 0 -or -not $container.State.Running) { throw 'Fixed Docker fixture is not running.' }
        $expectedImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
        if ($container.Config.Image -cne $expectedImage) { throw 'Fixed Docker image digest mismatch.' }
        $bindings = @($container.NetworkSettings.Ports.'9030/tcp')
        if ($bindings.Count -ne 1 -or $bindings[0].HostIp -ne '127.0.0.1' -or $bindings[0].HostPort -ne '39230') { throw 'Fixed Docker MySQL must be loopback 39230.' }
        $receipt.fixed_docker = [ordered]@{ id = $container.Id; image = $expectedImage; mysql_endpoint = '127.0.0.1:39230'; account_scope = 'isolated_fixture_root_not_production' }
        $env:E2E_FIXED_DOCKER_VALIDATED = '3514-loopback'
    }

    $env:DOCTOR_FRONTEND_DIST = $FrontendDist
    $env:E2E_BASE_URL = $baseUrl
    $env:E2E_ARTIFACT_ROOT = $EvidenceRoot
    if ([string]::IsNullOrWhiteSpace($env:E2E_CHROME_PROFILE_ROOT)) {
        $env:E2E_CHROME_PROFILE_ROOT = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\chrome-profiles')
    }

    $script:runner = Start-Process -FilePath $RunnerPath -ArgumentList @([string]$ApiPort) -WorkingDirectory $repoRoot -WindowStyle Hidden -RedirectStandardOutput $runnerStdout -RedirectStandardError $runnerStderr -PassThru
    $runner = $script:runner
    $receipt.runner.pid = $runner.Id
    $readinessAttempts = Wait-Api
    $receipt.runner.readiness = [ordered]@{
        attempts = $readinessAttempts
        health_endpoint = "$baseUrl/api/v1/capabilities"
    }

    $browserArguments = @($browserPath, '--project', $Project)
    if ($RunningProfile) { $browserArguments += '--running-profile' }
    $nodeOutput = @(& rtk proxy node @browserArguments 2>&1)
    $nodeExit = $LASTEXITCODE
    ($nodeOutput | ForEach-Object { [string]$_ }) | Set-Content -LiteralPath $browserLog -Encoding utf8NoBOM

    $browserReceipt = Get-ChildItem -LiteralPath $EvidenceRoot -Recurse -Filter 'browser-e2e-receipt.json' -File |
        Where-Object { $_.LastWriteTimeUtc -ge $runStartedUtc } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -eq $browserReceipt) {
        throw "Browser E2E receipt was not produced (node exit $nodeExit)."
    }
    $browserReceiptPath = $browserReceipt.FullName
    $browserJson = Get-Content -LiteralPath $browserReceiptPath -Raw | ConvertFrom-Json
    $receipt.browser_receipt = [ordered]@{
        path = $browserReceiptPath
        status = [string]$browserJson.status
        assertions = @($browserJson.assertions).Count
        screenshot_count = @($browserJson.screenshots).Count
        node_exit = $nodeExit
        node_log = $browserLog
    }
    $receipt.screenshots = @($browserJson.screenshots | ForEach-Object {
        [ordered]@{
            relative_path = [string]$_.relative_path
            exists = Test-Path -LiteralPath (Join-Path $browserReceipt.DirectoryName ([string]$_.relative_path)) -PathType Leaf
            sha256 = [string]$_.sha256
        }
    })
    if ($nodeExit -eq 0 -and [string]$browserJson.status -eq 'PASS') {
        $status = 'PASS'
    }
    else {
        $failure = "browser_e2e_status_$($browserJson.status)_exit_$nodeExit"
    }
}
catch {
    $failure = $_.Exception.Message
}
finally {
    if ($null -ne $runner -and -not $runner.HasExited) {
        try {
            Stop-Process -Id $runner.Id -Force -ErrorAction Stop
            $runner.WaitForExit(5000) | Out-Null
            $cleanup.runner_stopped = $true
        }
        catch {
            $cleanup.runner_stop_error = $_.Exception.Message
        }
    }
    elseif ($null -ne $runner) {
        $cleanup.runner_stopped = $true
    }
    $cleanup.api_port_free_after = (@(Get-NetTCPConnection -LocalPort $ApiPort -State Listen -ErrorAction SilentlyContinue).Count -eq 0)
    $port8000After = @(Get-NetTCPConnection -LocalPort 8000 -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
    $receipt.api.port_8000_listeners_after = $port8000After.Count
    $receipt.api.port_8000_touched = $port8000Before.Count -ne $port8000After.Count
    [Environment]::SetEnvironmentVariable('DOCTOR_FRONTEND_DIST', $previousFrontendDist, 'Process')
    [Environment]::SetEnvironmentVariable('E2E_BASE_URL', $previousBaseUrl, 'Process')
    [Environment]::SetEnvironmentVariable('E2E_ARTIFACT_ROOT', $previousArtifactRoot, 'Process')
    [Environment]::SetEnvironmentVariable('E2E_CHROME_PROFILE_ROOT', $previousChromeProfileRoot, 'Process')
    [Environment]::SetEnvironmentVariable('E2E_FIXED_DOCKER_VALIDATED', $previousFixedDockerValidated, 'Process')
    $cleanup.environment_restored = $true
    $receipt.status = $status
    if ($null -ne $failure) { $receipt.failure = $failure }
    $receipt.cleanup = $cleanup
    $receipt.finished_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($launchReceiptPath, ($receipt | ConvertTo-Json -Depth 16), [Text.UTF8Encoding]::new($false))
}

$receipt | ConvertTo-Json -Depth 16
Write-Output "LAUNCH_RECEIPT=$launchReceiptPath"
if ($status -ne 'PASS') { exit 1 }
