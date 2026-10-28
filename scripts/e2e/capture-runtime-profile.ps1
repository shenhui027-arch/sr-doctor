param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$')]
    [string]$QueryId,
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputPath,
    [ValidateNotNullOrEmpty()]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514',
    [ValidateNotNullOrEmpty()]
    [string]$Username = $env:STARROCKS_USER,
    [ValidatePattern('^[A-Za-z_][A-Za-z0-9_]*$')]
    [string]$PasswordEnvironmentVariable = 'STARROCKS_PASSWORD',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [int]$MySqlPort = 9030
)

$ErrorActionPreference = 'Stop'

function Get-LocalSha256 {
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

function ConvertFrom-HexStringCompat {
    param([Parameter(Mandatory)][string]$Hex)
    if (($Hex.Length % 2) -ne 0) { throw 'Profile hex payload has an odd length.' }
    $bytes = New-Object byte[] ($Hex.Length / 2)
    for ($index = 0; $index -lt $bytes.Length; $index++) {
        $bytes[$index] = [Convert]::ToByte($Hex.Substring($index * 2, 2), 16)
    }
    $bytes
}
if ($MySqlPort -lt 1 -or $MySqlPort -gt 65535) {
    throw 'Invalid MySQL port.'
}
$password = [Environment]::GetEnvironmentVariable($PasswordEnvironmentVariable)
if ([string]::IsNullOrWhiteSpace($Username) -or [string]::IsNullOrEmpty($password)) {
    throw "Set STARROCKS_USER and $PasswordEnvironmentVariable before capturing a Profile."
}
if (Test-Path -LiteralPath $OutputPath) {
    throw 'OutputPath already exists; choose a new runtime-artifact path.'
}
if ([string]::IsNullOrWhiteSpace($DockerExecutable)) {
    $knownDocker = [IO.Path]::Combine([string]$env:ProgramFiles, 'Docker\Docker\resources\bin\docker.exe')
    if (Test-Path -LiteralPath $knownDocker -PathType Leaf) {
        $DockerExecutable = $knownDocker
    }
    else {
        $dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
        if ($null -eq $dockerCommand) { throw 'Docker CLI was not found. Set DOCKER_EXE or install Docker CLI.' }
        $DockerExecutable = $dockerCommand.Source
    }
}
if (-not (Test-Path -LiteralPath $DockerExecutable -PathType Leaf)) { throw 'Docker CLI was not found.' }
$DockerExecutable = (Resolve-Path -LiteralPath $DockerExecutable).Path

$parent = Split-Path -Parent $OutputPath
if ([string]::IsNullOrWhiteSpace($parent)) {
    throw 'OutputPath must include a parent directory.'
}
$artifactRoot = [IO.Path]::GetFullPath((Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e'))
$resolvedOutputPath = [IO.Path]::GetFullPath($OutputPath)
if (-not $resolvedOutputPath.StartsWith($artifactRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "OutputPath must be below $artifactRoot."
}
New-Item -ItemType Directory -Force -Path $parent | Out-Null

# `mysql --raw` still adds its own row delimiter. Request HEX so this harness
# reconstructs only the database field bytes before the offline-equivalence check.
$previousMysqlPassword = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
try {
    [Environment]::SetEnvironmentVariable('MYSQL_PWD', $password, 'Process')
    # Docker inherits the current process value for `-e NAME`; do not pass a
    # secret-bearing NAME=value argument that would appear in the host process list.
    $hex = & $DockerExecutable exec -e MYSQL_PWD $ContainerName mysql `
            --batch --skip-column-names --raw --host=127.0.0.1 --port=$MySqlPort `
            --user=$Username --execute="SELECT/**/HEX(get_query_profile('$QueryId'))"
    if ($LASTEXITCODE -ne 0) {
        throw "get_query_profile failed with exit $LASTEXITCODE."
    }
}
finally {
    [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previousMysqlPassword, 'Process')
}
$hex = ($hex -join '').Trim()
if ($hex -notmatch '^(?:[0-9A-Fa-f]{2})+$') {
    throw 'get_query_profile returned no usable Profile bytes.'
}
[IO.File]::WriteAllBytes($resolvedOutputPath, (ConvertFrom-HexStringCompat -Hex $hex))

$item = Get-Item -LiteralPath $resolvedOutputPath
[pscustomobject]@{
    query_id = $QueryId
    output_path = $item.FullName
    byte_length = $item.Length
    source_hash = Get-LocalSha256 -Path $item.FullName
} | ConvertTo-Json -Compress
