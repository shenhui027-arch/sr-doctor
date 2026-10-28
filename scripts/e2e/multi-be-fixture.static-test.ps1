$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Static trust-boundary checks for the 1 FE + 3 BE fixture and its capture
# scripts. Pure parsing: no Docker, SQL or network.
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$fixtureDir = Join-Path $repo 'dev/starrocks-3.5.14-multi-be'
$files = @(
    (Join-Path $fixtureDir 'common.ps1'), (Join-Path $fixtureDir 'up.ps1'), (Join-Path $fixtureDir 'teardown.ps1'),
    (Join-Path $repo 'scripts/e2e/capture-oracle-corpus.ps1'), (Join-Path $repo 'scripts/e2e/capture-profile-states.ps1')
)
$checks = 0
function Check([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAIL: $Message" }
    $script:checks++
}

foreach ($file in $files) {
    $tokens = $null; $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$errors)
    Check ($errors.Count -eq 0) "$file parses"
}
$common = Get-Content -Raw (Join-Path $fixtureDir 'common.ps1')
$up = Get-Content -Raw (Join-Path $fixtureDir 'up.ps1')
$teardown = Get-Content -Raw (Join-Path $fixtureDir 'teardown.ps1')
$capture = Get-Content -Raw (Join-Path $repo 'scripts/e2e/capture-oracle-corpus.ps1')
$states = Get-Content -Raw (Join-Path $repo 'scripts/e2e/capture-profile-states.ps1')

# Images: official split images, pinned by a full registry digest.
Check ($common -match "MultiBeFeImage = 'starrocks/fe-ubuntu:3\.5\.14@sha256:[0-9a-f]{64}'") 'FE image pinned by digest'
Check ($common -match "MultiBeBeImage = 'starrocks/be-ubuntu:3\.5\.14@sha256:[0-9a-f]{64}'") 'BE image pinned by digest'
# Names stay under the dedicated prefix accepted by the capture scripts, and
# the label differs from the allin1 fixture's so neither teardown matches the other.
Check ($common -match "MultiBePrefix = 'sr-doctor-v0\.1-e2e-3514-mbe'") 'dedicated name prefix'
Check ($common -match "MultiBeLabelValue = '3\.5\.14-multi-be'") 'dedicated label value'
Check ($common -match [regex]::Escape("Join-Path `$persistent `$script:MultiBePrefix")) 'data root derived from the prefix'

# Ports: every publish binds 127.0.0.1, exactly two, and only on the FE.
$publishes = [regex]::Matches($up, "'--publish', `"([^`"]+)`"")
Check ($publishes.Count -eq 2) 'exactly two published ports'
foreach ($publish in $publishes) { Check ($publish.Groups[1].Value.StartsWith('127.0.0.1:')) "publish $($publish.Groups[1].Value) binds loopback" }
$beRun = $up.Substring($up.IndexOf('foreach ($backend in $backends) {'))
$beRun = $beRun.Substring(0, $beRun.IndexOf("`n}") + 2)
Check (-not $beRun.Contains('--publish')) 'BEs publish no host port'
Check ($up.Contains("'--security-opt', 'no-new-privileges'")) 'containers drop new privileges'
Check ($up -match "run teardown\.ps1 first") 'up.ps1 is cold-start only'
Check ($up.Contains('[switch]$LowResource')) 'low-resource profile is explicit opt-in'
Check ($up.Contains('$feMemoryMiB = if ($LowResource) { 1536 }') -and $up.Contains('$beMemoryMiB = if ($LowResource) { 1280 }')) 'low-resource container caps total 5376 MiB for three BEs'
Check ($up.Contains('$feHeapMiB = if ($LowResource) { 768 }') -and $up.Contains('$beLimitMiB = if ($LowResource) { 896 }')) 'low-resource application limits leave native-memory headroom'
Check ($up.Contains("'--memory-swap', `"`${feMemorySwapMiB}m`"") -and $up.Contains("'--memory-swap', `"`${beMemorySwapMiB}m`"")) 'memory plus swap is explicitly bounded at container creation'
# The shared `file://` directory lives under the data root (so teardown removes
# it) and is mounted at one path in the FE and every BE.
Check ($up.Contains("`$sharedFiles = Join-Path `$dataRoot 'shared-files'")) 'shared files directory under the data root'
Check ([regex]::Matches($up, "'--volume', \`$sharedFilesMount").Count -eq 2 -and $beRun.Contains("'--volume', `$sharedFilesMount")) 'shared files mounted in the FE and every BE'

# Teardown: ownership proven before removal, postconditions enforced.
Check ($teardown.IndexOf('Assert-MultiBeOwned') -lt $teardown.IndexOf("'rm', '--force'")) 'container ownership checked before rm'
Check ($teardown.Contains('-cne $script:MultiBeLabelValue')) 'network label checked before rm'
Check ($teardown.Contains('Teardown postconditions failed')) 'teardown verifies nothing is left'
Check ($teardown.Contains('Get-NetTCPConnection')) 'teardown verifies host ports are released'

# Credentials: root password only through the inherited MYSQL_PWD variable.
foreach ($text in @($up, $capture, $states, $common)) {
    Check (-not ($text -match '--password=|-p\$|IDENTIFIED BY ''')) 'no password on a command line or in plaintext SQL'
}
Check ($capture.Contains("exec -e MYSQL_PWD")) 'oracle capture passes the password via MYSQL_PWD'
Check ($states.Contains("'-e', 'MYSQL_PWD'") -and $states.Contains('exec -e MYSQL_PWD')) 'state capture passes the password via MYSQL_PWD'
Check ($common.Contains('Get-FixtureSecret')) 'password derived from the process-only secret'
Check ($up.Contains('IDENTIFIED WITH mysql_native_password AS')) 'root password set as ciphertext'

Write-Output "PASS $checks multi-BE fixture static checks (pure parse; no Docker/SQL execution)"
