[CmdletBinding()]
param(
    [int]$Port = 0,
    [string]$DockerExecutable = $env:DOCKER_EXE
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$composeFile = Join-Path $repoRoot 'deploy/local/compose.yaml'
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
$project = "sr-doctor-m06-$stamp-$PID".ToLowerInvariant()
$receiptPath = Join-Path $runtimeRoot "docker-app-$stamp.json"
$browserArtifactRoot = Join-Path $runtimeRoot "docker-browser-$stamp"
$stage = 'initialization'
$containerId = $null
$composeStarted = $false
$failure = $null
$teardownFailures = @()
$resourcePostconditions = [ordered]@{}
$previousDoctorPort = $env:DOCTOR_PORT
$previousPublicPort = $env:DOCTOR_PUBLIC_PORT
$previousBaseUrl = $env:E2E_BASE_URL
$previousArtifactRoot = $env:E2E_ARTIFACT_ROOT
$summary = [ordered]@{
    schema_version = 'sr-doctor.docker-e2e/v1'
    project = $project
    status = 'FAIL'
    started_utc = [DateTime]::UtcNow.ToString('o')
    assertions = @()
    compose = [ordered]@{}
    http = [ordered]@{}
    browser = [ordered]@{}
}

function Resolve-DockerExecutable {
    if (-not [string]::IsNullOrWhiteSpace($DockerExecutable)) {
        if (-not (Test-Path -LiteralPath $DockerExecutable -PathType Leaf)) {
            throw "Docker executable is missing: $DockerExecutable"
        }
        return (Resolve-Path -LiteralPath $DockerExecutable).Path
    }
    $command = Get-Command docker -ErrorAction SilentlyContinue
    if ($null -eq $command) { throw 'Docker CLI was not found. Set DOCKER_EXE or install Docker Desktop.' }
    return $command.Source
}

function Add-Assertion([string]$Name) {
    $summary.assertions += $Name
}

function Invoke-Docker([string[]]$Arguments) {
    $output = @(& $script:docker @Arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Docker command failed (exit $LASTEXITCODE): docker $($Arguments -join ' ')"
    }
    return $output
}

function Invoke-Compose([string[]]$Arguments) {
    $allArguments = @('compose', '--project-name', $project, '--file', $composeFile) + $Arguments
    return Invoke-Docker $allArguments
}

function Test-TcpPortOccupied([int]$SelectedPort) {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $pending = $client.BeginConnect('127.0.0.1', $SelectedPort, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne(500)) { return $false }
        try { $client.EndConnect($pending); return $client.Connected }
        catch { return $false }
        finally { $pending.AsyncWaitHandle.Dispose() }
    }
    finally { $client.Dispose() }
}

function Wait-TcpPortFree([int]$SelectedPort) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-TcpPortOccupied $SelectedPort)) { return }
        Start-Sleep -Milliseconds 200
    }
    throw "TCP port 127.0.0.1:$SelectedPort remained occupied after Docker teardown."
}

function Wait-Http([string]$Url, [int]$ExpectedStatus = 200) {
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 3
            if ([int]$response.StatusCode -eq $ExpectedStatus) { return $response }
        }
        catch { }
        Start-Sleep -Milliseconds 300
    }
    throw "HTTP endpoint did not return $ExpectedStatus before timeout: $Url"
}

function Get-HeaderHostResponse([string]$Url, [string]$HostName) {
    $client = [Net.Http.HttpClient]::new()
    try {
        $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Get, $Url)
        $request.Headers.Host = $HostName
        $response = $client.Send($request)
        return [pscustomobject]@{ StatusCode = [int]$response.StatusCode }
    }
    finally { $client.Dispose() }
}

try {
    $stage = 'preconditions'
    if ($Port -eq 0) {
        $Port = if ([string]::IsNullOrWhiteSpace($env:DOCTOR_PORT)) { 8000 } else { [int]$env:DOCTOR_PORT }
    }
    if ($Port -lt 1 -or $Port -gt 65535) { throw 'Port must be between 1 and 65535.' }
    if (-not (Test-Path -LiteralPath $composeFile -PathType Leaf)) { throw "Compose file is missing: $composeFile" }
    $script:docker = Resolve-DockerExecutable
    $summary.port = $Port
    if (Test-TcpPortOccupied $Port) { throw "TCP port 127.0.0.1:$Port is already occupied." }
    Add-Assertion 'host_port_free_before_start'

    $env:DOCTOR_PORT = [string]$Port
    $env:DOCTOR_PUBLIC_PORT = [string]$Port
    $stage = 'compose_config'
    $configText = ((Invoke-Compose @('config', '--format', 'json')) -join [Environment]::NewLine)
    $config = $configText | ConvertFrom-Json
    $service = $config.services.doctor
    if ($null -eq $service) { throw 'Compose service doctor is missing.' }
    $ports = @($service.ports)
    if ($ports.Count -ne 1) { throw 'Compose must expose exactly one application port.' }
    $portBinding = $ports[0]
    $hostIp = [string]$portBinding.host_ip
    $published = [int]$portBinding.published
    $target = [int]$portBinding.target
    if ($hostIp -cne '127.0.0.1' -or $published -ne $Port -or $target -ne 8000) {
        throw "Compose port binding is not loopback $Port`:8000: $hostIp`:$published`:$target"
    }
    if ($null -ne $service.volumes -and @($service.volumes).Count -ne 0) { throw 'Compose must not declare persistent volumes.' }
    if ($null -ne $service.privileged -and $service.privileged) { throw 'Compose must not enable privileged mode.' }
    $serviceEnvironment = @($service.environment.PSObject.Properties | ForEach-Object { $_.Name })
    if ($serviceEnvironment | Where-Object { $_ -match 'PASSWORD|SECRET|TOKEN|CREDENTIAL' }) {
        throw 'Compose environment contains a credential-shaped variable.'
    }
    $summary.compose.config = [ordered]@{
        host_ip = $hostIp
        published_port = $published
        target_port = $target
        volumes = 0
        environment_keys = $serviceEnvironment
    }
    Add-Assertion 'compose_loopback_port_and_container_target'
    Add-Assertion 'compose_has_no_persistent_volumes_or_credentials'

    $stage = 'docker_build'
    Invoke-Compose @('build', '--pull') | Out-Null
    Add-Assertion 'docker_build_pull'
    $stage = 'docker_start'
    # Mark the project before `up`: a partially-created Compose project must
    # still be cleaned when Docker reports a start failure.
    $composeStarted = $true
    Invoke-Compose @('up', '-d') | Out-Null
    $containerId = ((Invoke-Compose @('ps', '-q', 'doctor')) -join '').Trim()
    if ($containerId -notmatch '^[0-9a-f]{12,64}$') { throw 'Compose did not return a valid doctor container id.' }
    $inspect = (Invoke-Docker @('inspect', $containerId) -join [Environment]::NewLine) | ConvertFrom-Json
    $container = $inspect[0]
    $imageInspect = (Invoke-Docker @('image', 'inspect', $container.Config.Image) -join [Environment]::NewLine) | ConvertFrom-Json
    $image = $imageInspect[0]
    $imageRepoDigests = @($image.RepoDigests | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) })
    $hostConfig = $container.HostConfig
    $runtimeUser = ((Invoke-Docker @('exec', $containerId, 'id', '-u')) -join '').Trim()
    $runtimeEnvironment = @($container.Config.Env)
    $publishedPort = @($hostConfig.PortBindings.'8000/tcp')[0]
    $publishedHostIp = [string]$publishedPort.HostIp
    $publishedHostPort = [int]$publishedPort.HostPort
    $mounts = @($container.Mounts)
    $nonTmpfsMount = @($mounts | Where-Object { $_.Type -ne 'tmpfs' -or $_.Destination -cne '/tmp' })
    $capDrop = @($hostConfig.CapDrop | ForEach-Object { ([string]$_).ToUpperInvariant() })
    $securityOptions = @($hostConfig.SecurityOpt | ForEach-Object { [string]$_ })
    if ($publishedHostIp -cne '127.0.0.1' -or $publishedHostPort -ne $Port) { throw 'Docker port binding is not fixed to 127.0.0.1 and the selected host port.' }
    if ([string]::IsNullOrWhiteSpace($container.Config.User) -or $container.Config.User -match '^(0|root)$' -or $runtimeUser -eq '0') { throw 'Docker container is running as root.' }
    if (-not $hostConfig.ReadonlyRootfs) { throw 'Docker root filesystem is not read-only.' }
    if (-not ($capDrop -contains 'ALL')) { throw 'Docker container does not drop all capabilities.' }
    if (-not ($securityOptions -contains 'no-new-privileges:true')) { throw 'Docker container does not enable no-new-privileges.' }
    if ($nonTmpfsMount.Count -ne 0) { throw 'Docker container has a non-tmpfs mount.' }
    if ($runtimeEnvironment | Where-Object { $_ -match 'PASSWORD|SECRET|TOKEN|CREDENTIAL' }) { throw 'Docker environment contains a credential-shaped variable.' }
    $summary.compose.container = [ordered]@{
        id = $containerId.Substring(0, [Math]::Min(12, $containerId.Length))
        image_id = [string]$container.Image
        image = [string]$container.Config.Image
        repo_digests = $imageRepoDigests
        user = [string]$container.Config.User
        runtime_uid = $runtimeUser
        published_host_ip = $publishedHostIp
        published_host_port = $publishedHostPort
        read_only_rootfs = [bool]$hostConfig.ReadonlyRootfs
        cap_drop = $capDrop
        security_opt = $securityOptions
        mounts = @($mounts | ForEach-Object { [ordered]@{ type = [string]$_.Type; destination = [string]$_.Destination } })
    }
    Add-Assertion 'container_loopback_binding_verified'
    Add-Assertion 'container_non_root_verified'
    Add-Assertion 'container_read_only_drop_all_no_new_privileges_verified'
    Add-Assertion 'container_has_no_persistent_mounts'

    $stage = 'windows_http_probe'
    $apiPath = "/api/v1/capabilities"
    $loopbackUrl = "http://127.0.0.1:$Port$apiPath"
    $loopback = Wait-Http $loopbackUrl
    # Docker Desktop publishes this service on IPv4 loopback only. Probe the
    # Windows localhost Host header over that exact published socket so an
    # IPv6-first resolver cannot turn a valid URL check into a timeout.
    $localhost = Get-HeaderHostResponse $loopbackUrl "localhost:$Port"
    if ($localhost.StatusCode -ne 200) { throw "Localhost Host header returned HTTP $($localhost.StatusCode) instead of 200." }
    $homeResponse = Wait-Http "http://127.0.0.1:$Port/"
    $badHost = Get-HeaderHostResponse $loopbackUrl 'evil.example'
    $badHostStatus = $badHost.StatusCode
    if ($badHostStatus -ne 403) { throw "Bad Host request returned HTTP $badHostStatus instead of 403." }
    $summary.http = [ordered]@{
        loopback_capabilities = [int]$loopback.StatusCode
        localhost_capabilities = [int]$localhost.StatusCode
        localhost_probe = '127.0.0.1 socket with Host localhost'
        loopback_home = [int]$homeResponse.StatusCode
        bad_host = $badHostStatus
        home_contains_root = $homeResponse.Content -match 'id="root"'
    }
    if (-not $summary.http.home_contains_root) { throw 'Docker home page did not contain the frontend root element.' }
    Add-Assertion 'windows_127_0_0_1_capabilities'
    Add-Assertion 'windows_localhost_capabilities'
    Add-Assertion 'windows_home_page'
    Add-Assertion 'bad_host_forbidden'

    $stage = 'browser_e2e'
    New-Item -ItemType Directory -Force -Path $browserArtifactRoot | Out-Null
    $env:E2E_BASE_URL = "http://127.0.0.1:$Port"
    $env:E2E_ARTIFACT_ROOT = $browserArtifactRoot
    $browserOutput = @(& rtk npm run test:e2e -- --project offline-local 2>&1)
    if ($LASTEXITCODE -ne 0) { throw 'Offline browser E2E against the Docker-published URL failed.' }
    $browserReceipt = Get-ChildItem -LiteralPath $browserArtifactRoot -Recurse -Filter browser-e2e-receipt.json -File | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
    if ($null -eq $browserReceipt) { throw 'Docker browser E2E did not produce a browser receipt.' }
    $browserJson = Get-Content -LiteralPath $browserReceipt.FullName -Raw | ConvertFrom-Json
    if ($browserJson.status -ne 'PASS') { throw 'Docker browser E2E receipt is not PASS.' }
    $summary.browser = [ordered]@{
        project = 'offline-local'
        base_url = $env:E2E_BASE_URL
        status = [string]$browserJson.status
        receipt = $browserReceipt.FullName
        assertions = @($browserJson.assertions).Count
        screenshots = @($browserJson.screenshots).Count
    }
    Add-Assertion 'offline_browser_e2e_through_docker_url'
    $summary.status = 'PASS'
}
catch {
    $failure = $_
    $summary.failure = [ordered]@{ stage = $stage; message_id = 'docker_e2e_stage_failed' }
}
finally {
    $stage = 'teardown'
    if ($composeStarted) {
        try { Invoke-Compose @('down', '--volumes', '--remove-orphans') | Out-Null }
        catch { $teardownFailures += 'compose_down_failed' }
        try {
            $remainingContainers = ((Invoke-Docker @('ps', '-aq', '--filter', "label=com.docker.compose.project=$project")) -join '').Trim()
            $remainingNetworks = ((Invoke-Docker @('network', 'ls', '-q', '--filter', "label=com.docker.compose.project=$project")) -join '').Trim()
            if (-not [string]::IsNullOrWhiteSpace($remainingContainers)) { $teardownFailures += 'compose_container_remained' }
            if (-not [string]::IsNullOrWhiteSpace($remainingNetworks)) { $teardownFailures += 'compose_network_remained' }
            $resourcePostconditions = [ordered]@{
                containers = [string]::IsNullOrWhiteSpace($remainingContainers)
                networks = [string]::IsNullOrWhiteSpace($remainingNetworks)
            }
        }
        catch { $teardownFailures += 'compose_resource_postcondition_failed' }
    }
    try { Wait-TcpPortFree $Port; Add-Assertion 'host_port_free_after_teardown' }
    catch { $teardownFailures += 'host_port_remained_occupied' }
    if ($teardownFailures.Count -gt 0) {
        $summary.status = 'FAIL'
        $summary.teardown = [ordered]@{ status = 'FAIL'; failures = $teardownFailures; resources_absent = $resourcePostconditions }
        if ($null -eq $summary.failure) { $summary.failure = [ordered]@{ stage = 'teardown'; message_id = 'docker_e2e_teardown_failed' } }
    }
    else { $summary.teardown = [ordered]@{ status = 'PASS'; compose_down = $composeStarted; resources_absent = $resourcePostconditions } }
    $env:DOCTOR_PORT = $previousDoctorPort
    $env:DOCTOR_PUBLIC_PORT = $previousPublicPort
    $env:E2E_BASE_URL = $previousBaseUrl
    $env:E2E_ARTIFACT_ROOT = $previousArtifactRoot
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false))
}

Write-Output ($summary | ConvertTo-Json -Compress -Depth 12)
if ($summary.status -ne 'PASS') { exit 1 }
