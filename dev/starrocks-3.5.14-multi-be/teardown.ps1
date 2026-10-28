[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [int[]]$HostPorts = @(38530, 39530)
)

# Removes exactly the 1 FE + 3 BE fixture this directory creates: each
# container only after its fixed-digest image and dedicated label are
# verified, the network only with the dedicated label, and the data root only
# at its single derived path. It then proves that nothing is left.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$docker = Get-FixtureDockerExecutable $DockerExecutable
$dataRoot = Get-MultiBeDataRoot
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)

foreach ($name in $names) {
    if ((Assert-MultiBeOwned -DockerExecutable $docker -Name $name) -and $PSCmdlet.ShouldProcess($name, 'Remove multi-BE fixture container')) {
        Invoke-FixtureDocker $docker @('rm', '--force', '--volumes', $name) -DiscardOutput
    }
}
$networkLabel = Get-MultiBeNetworkLabel -DockerExecutable $docker
if ($null -ne $networkLabel) {
    if ($networkLabel -cne $script:MultiBeLabelValue) { throw "Refusing to remove $script:MultiBeNetwork`: it does not carry the fixture label." }
    if ($PSCmdlet.ShouldProcess($script:MultiBeNetwork, 'Remove multi-BE fixture network')) {
        Invoke-FixtureDocker $docker @('network', 'rm', $script:MultiBeNetwork) -DiscardOutput
    }
}
if ((Test-Path -LiteralPath $dataRoot -PathType Container) -and $PSCmdlet.ShouldProcess($dataRoot, 'Remove multi-BE fixture persistent data')) {
    Remove-Item -LiteralPath $dataRoot -Recurse -Force
}
if ($WhatIfPreference) { return }

$left = [ordered]@{
    containers = @($names | Where-Object { $null -ne (Get-MultiBeIdentity -DockerExecutable $docker -Name $_) })
    network = ($null -ne (Get-MultiBeNetworkLabel -DockerExecutable $docker))
    data_root = (Test-Path -LiteralPath $dataRoot)
    listening_ports = @($HostPorts | Where-Object { Get-NetTCPConnection -LocalPort $_ -State Listen -ErrorAction SilentlyContinue })
}
if ($left.containers.Count -gt 0 -or $left.network -or $left.data_root -or $left.listening_ports.Count -gt 0) {
    throw "Teardown postconditions failed: $($left | ConvertTo-Json -Compress)"
}
[ordered]@{ status = 'PASS'; removed = $names; network = $script:MultiBeNetwork; data_root = $dataRoot; ports_released = $HostPorts } | ConvertTo-Json -Compress
