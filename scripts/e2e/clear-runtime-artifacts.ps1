[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string]$RunDirectory
)

$ErrorActionPreference = 'Stop'
$artifactRoot = [IO.Path]::GetFullPath((Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\browser'))
$resolvedRunDirectory = [IO.Path]::GetFullPath($RunDirectory)

if (-not $resolvedRunDirectory.StartsWith($artifactRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "RunDirectory must be below $artifactRoot."
}
if (-not (Test-Path -LiteralPath $resolvedRunDirectory -PathType Container)) {
    throw 'RunDirectory does not exist.'
}
if ($PSCmdlet.ShouldProcess($resolvedRunDirectory, 'Remove owned browser E2E evidence directory')) {
    Remove-Item -LiteralPath $resolvedRunDirectory -Recurse -Force
}
