Set-StrictMode -Version Latest

# Shared helpers (Docker CLI resolution, SHA-256, MySQL diagnostics) come
# from the single-node fixture so both topologies fail the same way.
. (Join-Path $PSScriptRoot '..\starrocks-3.5.14\common.ps1')

# Official split FE / BE images of StarRocks 3.5.14, pinned by registry digest.
$script:MultiBeFeImage = 'starrocks/fe-ubuntu:3.5.14@sha256:3d786dba52ea5e72a5815c438b8c45e041c647730f69c6728783f900e6798127'
$script:MultiBeBeImage = 'starrocks/be-ubuntu:3.5.14@sha256:ab6eb2aea5b323bddbb59e04c65dde436a3ff07e9018c92caa67d3f564a6a732'
$script:MultiBeLabelValue = '3.5.14-multi-be'
$script:MultiBePrefix = 'sr-doctor-v0.1-e2e-3514-mbe'
$script:MultiBeNetwork = "$script:MultiBePrefix-net"
$script:MultiBeFeName = "$script:MultiBePrefix-fe"
$script:MultiBeBackendCount = 3
$script:MultiBePersistentRoot = ($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers'))
# Explicit subnet: on a host with many projects Docker's predefined address
# pools can be exhausted. up.ps1 refuses it if any existing network overlaps.
$script:MultiBeSubnet = '10.235.14.0/24'

function Test-MultiBeSubnetOverlap {
    param([Parameter(Mandatory)][string]$DockerExecutable)
    $toRange = {
        param([string]$Cidr)
        $address, $bits = $Cidr.Split('/')
        $parsed = [Net.IPAddress]::Parse($address)
        if ($parsed.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork) { return $null }
        $bytes = $parsed.GetAddressBytes(); [Array]::Reverse($bytes)
        $start = [BitConverter]::ToUInt32($bytes, 0)
        $size = [uint64]1 -shl (32 - [int]$bits)
        $start = [uint64]($start - ($start % $size))
        @($start, ($start + $size - 1))
    }
    $ours = & $toRange $script:MultiBeSubnet
    $ids = @(& $DockerExecutable network ls -q)
    if ($LASTEXITCODE -ne 0) { throw 'Docker network listing failed.' }
    if ($ids.Count -eq 0) { return $false }
    $subnets = (& $DockerExecutable network inspect @ids --format '{{range .IPAM.Config}}{{.Subnet}} {{end}}') -split '\s+' | Where-Object { $_ }
    foreach ($subnet in $subnets) {
        $theirs = & $toRange $subnet
        if ($null -ne $theirs -and $theirs[0] -le $ours[1] -and $ours[0] -le $theirs[1]) { return $true }
    }
    $false
}

function Get-MultiBeBackendNames {
    @(1..$script:MultiBeBackendCount | ForEach-Object { "$script:MultiBePrefix-be$_" })
}

function Get-MultiBeHostname([string]$Name) {
    # Container names carry `v0.1`; a dotted hostname would be split into
    # host + domain, so every node gets a dot-free network alias instead.
    'mbe-' + $Name.Substring($script:MultiBePrefix.Length + 1)
}

function Get-MultiBeDataRoot {
    $persistent = [IO.Path]::GetFullPath($script:MultiBePersistentRoot)
    $root = [IO.Path]::GetFullPath((Join-Path $persistent $script:MultiBePrefix))
    if (-not $root.StartsWith($persistent + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "The multi-BE data root did not resolve below $script:MultiBePersistentRoot."
    }
    $root
}

function Get-MultiBeRootPassword {
    # Derived from the process-only STARROCKS_E2E_SECRET; never printed or stored.
    "sr3514-mbe-root-$(Get-FixtureSecret)"
}

function Get-MultiBeImageFor([string]$Name) {
    if ($Name -ceq $script:MultiBeFeName) { return $script:MultiBeFeImage }
    $script:MultiBeBeImage
}

function Get-MultiBeIdentity {
    param([Parameter(Mandatory)][string]$DockerExecutable, [Parameter(Mandatory)][string]$Name)
    Get-FixtureContainerIdentity -DockerExecutable $DockerExecutable -ContainerName $Name
}

function Assert-MultiBeOwned {
    # A container that exists under one of our names must be exactly ours:
    # fixed-digest image and the dedicated label. Anything else is refused.
    param([Parameter(Mandatory)][string]$DockerExecutable, [Parameter(Mandatory)][string]$Name)
    $identity = Get-MultiBeIdentity -DockerExecutable $DockerExecutable -Name $Name
    if ($null -eq $identity) { return $false }
    if ($identity.Image -cne (Get-MultiBeImageFor $Name) -or $identity.Label -cne $script:MultiBeLabelValue) {
        throw "Refusing to touch $Name`: it is not the fixed-digest labelled multi-BE fixture."
    }
    $true
}

function Get-MultiBeNetworkLabel {
    param([Parameter(Mandatory)][string]$DockerExecutable)
    $previous = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $labels = & $DockerExecutable network inspect $script:MultiBeNetwork --format '{{json .Labels}}' 2>$null
        $exitCode = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $previous }
    if ($exitCode -ne 0) { return $null }
    $parsed = $labels | ConvertFrom-Json
    $property = if ($null -ne $parsed) { $parsed.PSObject.Properties[$script:FixtureLabelKey] } else { $null }
    if ($null -eq $property) { return '' }
    [string]$property.Value
}

function Invoke-MultiBeMysql {
    # Runs SQL as root inside the FE container. The password travels through
    # the inherited MYSQL_PWD variable (`docker exec -e MYSQL_PWD`), never on a
    # command line; an empty password is only used before it is first set.
    param(
        [Parameter(Mandatory)][string]$DockerExecutable,
        [Parameter(Mandatory)][string]$Sql,
        [AllowEmptyString()][string]$Password = ''
    )
    Invoke-FixtureMysql $DockerExecutable $script:MultiBeFeName 'root' $Password $Sql
}
