[CmdletBinding(SupportsShouldProcess)]
param(
    # A non-default fixture (for example the v0.4 oracle corpus capture) must
    # still use the dedicated prefix accepted by bootstrap.ps1. Its data root is
    # always derived from the same name, so one teardown can never reach a
    # directory that belongs to another fixture.
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514',
    [string]$DockerExecutable = $env:DOCKER_EXE
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

Assert-FixtureContainerName $ContainerName
$containerName = $ContainerName
$persistentRoot = [IO.Path]::GetFullPath(($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers')))
$dataRoot = [IO.Path]::GetFullPath((Join-Path $persistentRoot $containerName))
if (-not $dataRoot.StartsWith($persistentRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    $dataRoot -ne (Join-Path $persistentRoot $containerName)) {
    throw 'The fixture data root did not resolve to the dedicated E-drive directory.'
}

$docker = Get-FixtureDockerExecutable $DockerExecutable
$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $containerName
if ($null -ne $identity) {
    if ($identity.Image -ne $script:FixtureImage -or $identity.Label -ne $script:FixtureLabelValue) {
        throw 'Refusing teardown: the named container is not this fixed-digest labelled fixture.'
    }
    if ($PSCmdlet.ShouldProcess($containerName, 'Remove dedicated Docker E2E container')) {
        Invoke-FixtureDocker $docker @('rm', '--force', $containerName) -DiscardOutput
    }
}
if ((Test-Path -LiteralPath $dataRoot -PathType Container) -and $PSCmdlet.ShouldProcess($dataRoot, 'Remove dedicated Docker E2E persistent data')) {
    Remove-Item -LiteralPath $dataRoot -Recurse -Force
}
