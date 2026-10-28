[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$HttpPort = 38530,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39530,
    [ValidateRange(60, 600)][int]$ReadyTimeoutSeconds = 300,
    # Tiny, serial topology workloads on a constrained developer machine.
    [switch]$LowResource
)

# Test infrastructure only: an isolated 1 FE + 3 BE StarRocks 3.5.14 cluster
# from the official split images, used to capture multi-BE oracle answers. It
# never stops, prunes or alters a container or network it did not create.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$docker = Get-FixtureDockerExecutable $DockerExecutable
$rootPassword = Get-MultiBeRootPassword
$rootPasswordHash = Get-FixtureNativePasswordHash $rootPassword
$feCpu = if ($LowResource) { '1' } else { '2' }
$beCpu = if ($LowResource) { '0.5' } else { '3' }
$feMemoryMiB = if ($LowResource) { 1536 } else { 2560 }
$beMemoryMiB = if ($LowResource) { 1280 } else { 3072 }
$feHeapMiB = if ($LowResource) { 768 } else { 1536 }
$beLimitMiB = if ($LowResource) { 896 } else { 2048 }
# The low-resource profile disables swap; the default retains Docker's 2x total.
$feMemorySwapMiB = if ($LowResource) { $feMemoryMiB } else { 2 * $feMemoryMiB }
$beMemorySwapMiB = if ($LowResource) { $beMemoryMiB } else { 2 * $beMemoryMiB }
if ($HttpPort -eq $MySqlPort) { throw 'Fixture host ports must be distinct.' }
$dataRoot = Get-MultiBeDataRoot
$backends = Get-MultiBeBackendNames
$names = @($script:MultiBeFeName) + $backends

# Cold start only: any leftover of ours must be torn down first, and nothing
# that is not ours may already use our names.
foreach ($name in $names) {
    if (Assert-MultiBeOwned -DockerExecutable $docker -Name $name) {
        throw "Fixture container $name already exists; run teardown.ps1 first."
    }
}
if ($null -ne (Get-MultiBeNetworkLabel -DockerExecutable $docker)) { throw "Network $script:MultiBeNetwork already exists; run teardown.ps1 first." }
if (Test-Path -LiteralPath $dataRoot) { throw "Data root $dataRoot already exists; run teardown.ps1 first." }
foreach ($port in @($HttpPort, $MySqlPort)) {
    if (Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue) { throw "Host port $port is already in use." }
}
foreach ($image in @($script:MultiBeFeImage, $script:MultiBeBeImage)) {
    Invoke-FixtureDocker $docker @('image', 'inspect', $image) -DiscardOutput
}

$feMeta = Join-Path $dataRoot 'fe-meta'
New-Item -ItemType Directory -Force -Path $feMeta | Out-Null
# One directory at the same path in the FE and every BE, standing in for the
# shared NFS mount that `file://` paths need on a multi-node cluster: the FE
# lists `FILES()` locally while each BE writes its own `INSERT INTO FILES` part.
$sharedFiles = Join-Path $dataRoot 'shared-files'
New-Item -ItemType Directory -Force -Path $sharedFiles | Out-Null
$sharedFilesMount = "${sharedFiles}:/tmp/oracle_shapes"
$label = "$script:FixtureLabelKey=$script:MultiBeLabelValue"
if (Test-MultiBeSubnetOverlap -DockerExecutable $docker) { throw "Subnet $script:MultiBeSubnet overlaps an existing Docker network." }
Invoke-FixtureDocker $docker @('network', 'create', '--driver', 'bridge', '--subnet', $script:MultiBeSubnet, '--label', $label, $script:MultiBeNetwork) -DiscardOutput

# FE: FQDN host type so BEs register by container hostname. The heap is capped
# well below the container limit (fe.conf's default -Xmx8192m would not fit).
$feCommand = 'echo ''JAVA_OPTS="-Dlog4j2.formatMsgNoLookups=true -Xmx__HEAP_MIB__m -XX:+UseG1GC -Djava.security.policy=${STARROCKS_HOME}/conf/udf_security.policy"'' >> /opt/starrocks/fe/conf/fe.conf && exec /opt/starrocks/fe/bin/start_fe.sh --host_type FQDN'
$feCommand = $feCommand.Replace('__HEAP_MIB__', [string]$feHeapMiB)
Invoke-FixtureDocker $docker @(
    'run', '--detach', '--name', $script:MultiBeFeName, '--hostname', 'mbe-fe', '--network-alias', 'mbe-fe',
    '--network', $script:MultiBeNetwork, '--label', $label, '--label', "$script:FixtureLabelKey.role=fe",
    '--cpus', $feCpu, '--memory', "${feMemoryMiB}m", '--memory-swap', "${feMemorySwapMiB}m", '--pids-limit', '1024', '--security-opt', 'no-new-privileges',
    '--publish', "127.0.0.1:$HttpPort`:8030", '--publish', "127.0.0.1:$MySqlPort`:9030",
    '--volume', "${feMeta}:/opt/starrocks/fe/meta", '--volume', $sharedFilesMount,
    '--entrypoint', '/bin/bash', $script:MultiBeFeImage, '-c', $feCommand
) -DiscardOutput

# BEs: no host ports at all; reachable only on the fixture network.
$beCommand = 'printf ''\nmem_limit = __BE_LIMIT_MIB__M\n'' >> /opt/starrocks/be/conf/be.conf && exec /opt/starrocks/be/bin/start_be.sh'
$beCommand = $beCommand.Replace('__BE_LIMIT_MIB__', [string]$beLimitMiB)
foreach ($backend in $backends) {
    $storage = Join-Path $dataRoot "$((Get-MultiBeHostname $backend).Substring(4))-storage"
    New-Item -ItemType Directory -Force -Path $storage | Out-Null
    Invoke-FixtureDocker $docker @(
        'run', '--detach', '--name', $backend, '--hostname', (Get-MultiBeHostname $backend), '--network-alias', (Get-MultiBeHostname $backend),
        '--network', $script:MultiBeNetwork, '--label', $label, '--label', "$script:FixtureLabelKey.role=be",
        '--cpus', $beCpu, '--memory', "${beMemoryMiB}m", '--memory-swap', "${beMemorySwapMiB}m", '--pids-limit', '4096', '--security-opt', 'no-new-privileges',
        '--volume', "${storage}:/opt/starrocks/be/storage", '--volume', $sharedFilesMount,
        '--entrypoint', '/bin/bash', $script:MultiBeBeImage, '-c', $beCommand
    ) -DiscardOutput
}

$deadline = [DateTime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
$version = $null
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        # current_version() needs an alive BE; the FE's own row carries its version.
        $leader = @(Invoke-MultiBeMysql $docker 'SHOW FRONTENDS;' | Where-Object { ([string]$_ -split "`t") -contains 'LEADER' })
        $version = if ($leader.Count -eq 1) { ([string]$leader[0] -split "`t")[-1].Trim() } else { $null }
        if ($version -match '^3\.5\.14-23a56ec') { break }
    } catch { }
    Start-Sleep -Seconds 2
}
if ($version -notmatch '^3\.5\.14-23a56ec') { throw 'The FE did not become a StarRocks 3.5.14-23a56ec server before the readiness deadline.' }

# Registration runs before root receives its password; the FE is reachable
# only from 127.0.0.1 and the fixture network during this window.
foreach ($backend in $backends) {
    Invoke-MultiBeMysql $docker "ALTER SYSTEM ADD BACKEND '$(Get-MultiBeHostname $backend):9050';" | Out-Null
}
Invoke-MultiBeMysql $docker "ALTER USER root IDENTIFIED WITH mysql_native_password AS $(ConvertTo-SqlLiteral $rootPasswordHash);" | Out-Null

$alive = 0
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $rows = @(Invoke-MultiBeMysql $docker 'SHOW BACKENDS;' $rootPassword)
        $alive = @($rows | Where-Object { ([string]$_ -split "`t")[8] -eq 'true' }).Count
        if ($alive -eq $backends.Count) { break }
    } catch { }
    Start-Sleep -Seconds 2
}
if ($alive -ne $backends.Count) { throw "Only $alive of $($backends.Count) BEs reported alive before the readiness deadline." }
$beVersions = @(Invoke-MultiBeMysql $docker 'SHOW BACKENDS;' $rootPassword | ForEach-Object { ([string]$_ -split "`t") | Where-Object { $_ -match '^3\.5\.14-' } | Select-Object -First 1 })
if (@($beVersions | Where-Object { $_ -notmatch '^3\.5\.14-23a56ec' }).Count -gt 0 -or $beVersions.Count -ne $backends.Count) {
    throw 'Not every BE reports StarRocks 3.5.14-23a56ec.'
}
$queryVersion = (Invoke-MultiBeMysql $docker 'SELECT current_version();' $rootPassword | Select-Object -First 1).Trim()
if ($queryVersion -notmatch '^3\.5\.14-23a56ec') { throw "Unexpected query-path version $queryVersion." }

$receipt = [ordered]@{
    fixture = 'sr-doctor-v0.6-multi-be'; frontend = $script:MultiBeFeName; backends = $backends
    network = $script:MultiBeNetwork; fe_image = $script:MultiBeFeImage; be_image = $script:MultiBeBeImage
    observed_version = $version; backends_alive = $alive
    resources = [ordered]@{
        profile = $(if ($LowResource) { 'low_resource' } else { 'default' })
        fe_cpu = $feCpu; be_cpu = $beCpu; fe_memory_mib = $feMemoryMiB; be_memory_mib = $beMemoryMiB
        fe_heap_mib = $feHeapMiB; be_mem_limit_mib = $beLimitMiB; swap_disabled = [bool]$LowResource
        total_memory_mib = $feMemoryMiB + $backends.Count * $beMemoryMiB
    }
    host_ports = [ordered]@{ http = $HttpPort; mysql = $MySqlPort; bind = '127.0.0.1' }
    data_root = $dataRoot; created_utc = [DateTime]::UtcNow.ToString('o')
}
$receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $dataRoot 'fixture-bootstrap.json') -Encoding utf8
$receipt | ConvertTo-Json -Compress -Depth 4
