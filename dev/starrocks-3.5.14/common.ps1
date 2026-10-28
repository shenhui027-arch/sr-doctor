Set-StrictMode -Version Latest

$script:FixtureImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$script:FixtureDigest = 'sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$script:FixtureLabelKey = 'com.sr-doctor.e2e'
$script:FixtureLabelValue = '3.5.14'
$script:AuditLoaderSha256 = 'cd2a8aceb7943675192e80d7a6d02d675170ab3beba2d0d2f49b5bc8955a8ea2'
$script:AuditLoaderUrl = 'https://releases.starrocks.io/resources/auditloader.zip'

function Get-FixtureDockerExecutable {
    param([string]$DockerExecutable = $env:DOCKER_EXE)
    if ([string]::IsNullOrWhiteSpace($DockerExecutable)) {
        $knownDocker = [IO.Path]::Combine([string]$env:ProgramFiles, 'Docker\Docker\resources\bin\docker.exe')
        if (Test-Path -LiteralPath $knownDocker -PathType Leaf) {
            $DockerExecutable = $knownDocker
        }
        else {
            $dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
            if ($null -eq $dockerCommand) {
                throw 'Docker CLI was not found. Set DOCKER_EXE or install Docker CLI.'
            }
            $DockerExecutable = $dockerCommand.Source
        }
    }
    if (-not (Test-Path -LiteralPath $DockerExecutable -PathType Leaf)) {
        throw 'Docker CLI was not found. Set DOCKER_EXE to the Docker Desktop CLI.'
    }
    (Resolve-Path -LiteralPath $DockerExecutable).Path
}

function Get-FixtureSecret {
    $Secret = [Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process')
    if ([string]::IsNullOrWhiteSpace($Secret) -or $Secret -notmatch '^[A-Za-z0-9_-]{24,64}$') {
        throw 'Set STARROCKS_E2E_SECRET to a 24-64 character [A-Za-z0-9_-] process-only token.'
    }
    $Secret
}

function Get-FixturePassword {
    param(
        [Parameter(Mandatory)][string]$Secret,
        [Parameter(Mandatory)][ValidateSet('audit_writer', 'doctor_reader', 'workload_user')][string]$Role
    )
    "sr3514-$Role-$Secret"
}

function Get-FixtureNativePasswordHash {
    param([Parameter(Mandatory)][string]$Password)
    # SHA1.HashData is unavailable in Windows PowerShell 5.1, which is still
    # a supported host for the repeatable E2E scripts. Keep the double-SHA1
    # native-password calculation explicit and dispose the compatibility API.
    $sha1 = [Security.Cryptography.SHA1]::Create()
    try {
        $first = $sha1.ComputeHash([Text.Encoding]::UTF8.GetBytes($Password))
        '*' + (($sha1.ComputeHash($first) | ForEach-Object { $_.ToString('X2') }) -join '')
    }
    finally {
        $sha1.Dispose()
    }
}

function Get-FixtureSha256 {
    param([Parameter(Mandatory)][string]$Path)
    $sha256 = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        (($sha256.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
}

function Assert-FixtureContainerName {
    param([string]$ContainerName)
    if ($ContainerName -notmatch '^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$') {
        throw 'ContainerName must use the dedicated sr-doctor-v0.1-e2e-3514 prefix.'
    }
}

function Assert-FixtureDataRoot {
    param([string]$DataRoot)
    $persistentRoot = [IO.Path]::GetFullPath(($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers')))
    $resolved = [IO.Path]::GetFullPath($DataRoot)
    if (-not $resolved.StartsWith($persistentRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "DataRoot must be below $persistentRoot."
    }
    $resolved
}

function Invoke-FixtureDocker {
    param(
        [Parameter(Mandatory)][string]$DockerExecutable,
        [Parameter(Mandatory)][string[]]$Arguments,
        [switch]$DiscardOutput
    )
    $output = & $DockerExecutable @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Docker command failed (exit $LASTEXITCODE)."
    }
    if (-not $DiscardOutput) { return $output }
}

# Container limits for the allin1 fixture. The default is the original v0.x
# sizing; -LowResource keeps the whole FE+BE container inside 2.5 CPUs and
# 5.25 GiB with no extra swap (memory-swap equal to memory).
$script:FixtureLowResource = [ordered]@{ profile = 'low_resource'; cpus = '2.5'; memory_mib = 5376; fe_heap_mib = 1024; be_mem_limit_mib = 3072; swap_disabled = $true }
function Get-FixtureResourceArguments {
    param([switch]$LowResource)
    if (-not $LowResource) { return @('--cpus', '4', '--memory', '6g') }
    $memory = "$($script:FixtureLowResource.memory_mib)m"
    @('--cpus', $script:FixtureLowResource.cpus, '--memory', $memory, '--memory-swap', $memory)
}
function Get-FixtureResourceReceipt {
    param([switch]$LowResource)
    if ($LowResource) { return $script:FixtureLowResource }
    [ordered]@{ profile = 'default'; cpus = '4'; memory_mib = 6144; fe_heap_mib = 8192; be_mem_limit_mib = $null; swap_disabled = $false }
}

# For a created, not yet started container: the FE reads fe.conf and the BE
# be.conf only at start. A later JAVA_OPTS line overrides the image default
# (-Xmx8192m), and an explicit BE mem_limit replaces the 90%-of-container
# default so FE and BE together stay below the container limit.
function Set-FixtureLowResourceConfig {
    param([Parameter(Mandatory)][string]$DockerExecutable, [Parameter(Mandatory)][string]$ContainerName)
    $staging = Join-Path ([IO.Path]::GetTempPath()) ('starrocks-lowres-' + [Guid]::NewGuid().ToString('N'))
    try {
        New-Item -ItemType Directory -Force -Path $staging | Out-Null
        $heap = $script:FixtureLowResource.fe_heap_mib
        $edits = @(
            @{ path = '/data/deploy/starrocks/fe/conf/fe.conf'; text = "`n# Low-resource fixture: FE heap within the container limit.`nJAVA_OPTS=`"-Dlog4j2.formatMsgNoLookups=true -Xmx${heap}m -XX:+UseG1GC -XX:ErrorFile=`${LOG_DIR}/hs_err_pid%p.log -Djava.security.policy=`${STARROCKS_HOME}/conf/udf_security.policy`"`n" }
            @{ path = '/data/deploy/starrocks/be/conf/be.conf'; text = "`n# Low-resource fixture: explicit BE memory limit.`nmem_limit = $($script:FixtureLowResource.be_mem_limit_mib)M`n" }
        )
        foreach ($edit in $edits) {
            $local = Join-Path $staging ([IO.Path]::GetFileName($edit.path))
            Invoke-FixtureDocker $DockerExecutable @('cp', "${ContainerName}:$($edit.path)", $local) -DiscardOutput
            [IO.File]::AppendAllText($local, $edit.text, [Text.UTF8Encoding]::new($false))
            Invoke-FixtureDocker $DockerExecutable @('cp', $local, "${ContainerName}:$($edit.path)") -DiscardOutput
        }
    }
    finally {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    }
}

function Get-FixtureContainerIdentity {
    param(
        [Parameter(Mandatory)][string]$DockerExecutable,
        [Parameter(Mandatory)][string]$ContainerName
    )
    # Do not embed a quoted label key in a Go template: Windows PowerShell 5.1
    # strips the nested quotes while invoking native commands. Read the labels
    # as JSON instead, then resolve the dedicated label in PowerShell.
    # A missing dedicated container is expected before bootstrap and after
    # teardown. Windows PowerShell turns its native stderr into an error record
    # when callers use ErrorActionPreference=Stop, so suppress only this probe.
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $image = & $DockerExecutable inspect $ContainerName --format '{{.Config.Image}}' 2>$null
        $imageExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($imageExitCode -ne 0) { return $null }
    try {
        $ErrorActionPreference = 'Continue'
        $labelsJson = & $DockerExecutable inspect $ContainerName --format '{{json .Config.Labels}}' 2>$null
        $labelsExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($labelsExitCode -ne 0) { throw 'Docker inspect could not read fixture labels.' }
    $labels = $labelsJson | ConvertFrom-Json
    $label = $null
    if ($null -ne $labels) {
        $labelProperty = $labels.PSObject.Properties[$script:FixtureLabelKey]
        if ($null -ne $labelProperty) { $label = $labelProperty.Value }
    }
    [pscustomobject]@{
        Image = $image.Trim()
        Label = $label
    }
}

function Get-FixtureMysqlDiagnostic {
    param([object[]]$OutputLines, [int]$ExitCode)
    $diagnostic = [ordered]@{ classification = 'native_command_failed'; exit_code = $ExitCode; error_code = $null; sqlstate = $null; line_number = $null }
    foreach ($line in $OutputLines) {
        if ([string]$line -cmatch '^ERROR ([0-9]{1,5}) \(([0-9A-Z]{5})\)(?: at line ([0-9]{1,9}))?:') {
            $diagnostic.classification = 'mysql_server_error'
            $diagnostic.error_code = [int]$Matches[1]
            $diagnostic.sqlstate = $Matches[2]
            if ($Matches[3]) { $diagnostic.line_number = [int]$Matches[3] }
            break
        }
    }
    return $diagnostic
}

function Invoke-FixtureMysql {
    param(
        [Parameter(Mandatory)][string]$DockerExecutable,
        [Parameter(Mandatory)][string]$ContainerName,
        [Parameter(Mandatory)][string]$Username,
        [AllowEmptyString()][string]$Password,
        [Parameter(Mandatory)][string]$Sql
    )
    # mysql reads the SQL from standard input; docker exec needs -i to retain it.
    $arguments = @('exec', '-i')
    $previousMysqlPassword = $null
    if (-not [string]::IsNullOrEmpty($Password)) {
        # Docker inherits this process variable for `-e MYSQL_PWD`, avoiding a
        # secret-bearing NAME=value argument in the host process list.
        $previousMysqlPassword = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
        [Environment]::SetEnvironmentVariable('MYSQL_PWD', $Password, 'Process')
        $arguments += @('-e', 'MYSQL_PWD')
    }
    $arguments += @($ContainerName, 'mysql', '--batch', '--skip-column-names', '--raw', '--host=127.0.0.1', '--port=9030', "--user=$Username")
    try {
        $output = $Sql | & $DockerExecutable @arguments 2>&1
        if ($LASTEXITCODE -ne 0) {
            $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @($output) -ExitCode $LASTEXITCODE
            $failure = [InvalidOperationException]::new("Fixture MySQL command failed (exit $LASTEXITCODE; class=$($diagnostic.classification); code=$($diagnostic.error_code); sqlstate=$($diagnostic.sqlstate); line=$($diagnostic.line_number)).")
            $failure.Data['fixture_mysql'] = $diagnostic
            throw $failure
        }
        @($output)
    }
    finally {
        if (-not [string]::IsNullOrEmpty($Password)) {
            [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previousMysqlPassword, 'Process')
        }
    }
}

function ConvertTo-SqlLiteral {
    param([Parameter(Mandatory)][string]$Value)
    "'" + $Value.Replace("'", "''") + "'"
}
