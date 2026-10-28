$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Static trust-boundary checks for the v0.7-01 TLS verify-full E2E. Pure
# parsing: no Docker, SQL or network.
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$runnerPath = Join-Path $repo 'scripts/e2e/run-tls-verify-full-e2e.ps1'
$fixturePath = Join-Path $repo 'dev/starrocks-3.5.14/tls-common.ps1'
$relayPath = Join-Path $repo 'scripts/e2e/tls-strip-relay.mjs'
$checks = 0
function Check([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAIL: $Message" }
    $script:checks++
}

foreach ($file in @($runnerPath, $fixturePath)) {
    $tokens = $null; $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$errors)
    Check ($errors.Count -eq 0) "$file parses"
}
$runner = Get-Content -Raw $runnerPath
$fixture = Get-Content -Raw $fixturePath
$relay = Get-Content -Raw $relayPath

# Official 3.5.14 FE settings (Config.java / SSLContextLoader.java @23a56ec).
foreach ($setting in @('ssl_keystore_location', 'ssl_keystore_password', 'ssl_key_password', 'ssl_force_secure_transport = true')) {
    Check ($fixture.Contains($setting)) "fixture sets $setting"
}
Check ($fixture.Contains("'create', '--name', `$name") -and $fixture.IndexOf("'cp', `$KeystoreDirectory") -lt $fixture.IndexOf("@('start', `$name)")) 'keystores and fe.conf are copied before the FE starts'
Check ($fixture.Contains("'--publish', `"127.0.0.1:`$MySqlPort``:9030`"") -and ([regex]::Matches($fixture, "'--publish'")).Count -eq 1) 'only the MySQL port is published, on loopback'
Check ($fixture.Contains("`$script:TlsFixtureContainerName = 'sr-doctor-v0.1-e2e-3514-tls'")) 'dedicated TLS container name'

# Private keys: generated per run, host copy removed right after the copy into the container.
Check ($runner.Contains('Remove-Item -LiteralPath $material.keystore_directory -Recurse -Force')) 'host keystores are removed after the copy'
Check ($runner.Contains('host_private_keys_removed_after_copy')) 'host key removal is recorded'
Check ($runner.Contains('[Security.Cryptography.RandomNumberGenerator]::Fill($keystoreBytes)')) 'keystore password comes from a CSPRNG'
Check ($runner.Contains("`$materialRoot = Join-Path ([IO.Path]::GetTempPath())")) 'TLS material lives in a per-run temporary directory'

# Credentials: never on a command line, never printed, cleared afterwards.
foreach ($text in @($runner, $fixture)) {
    Check (-not ($text -match '--password=|-p\$|IDENTIFIED BY ''')) 'no password on a command line or in plaintext SQL'
    Check (-not ($text -match 'Write-Host|Write-Output')) 'nothing is printed besides the final receipt'
}
Check (-not ($runner -match 'param\([^)]*(Secret|Password)')) 'no token or password is a script argument'
Check ($runner -match "ArgumentList = @\(\(Join-Path \`$PSScriptRoot 'tls-strip-relay\.mjs'\), \[string\]\`$RelayPort, \[string\]\`$MySqlPort, \`$relayReport\)" -or
    $runner.Contains("-ArgumentList @((Join-Path `$PSScriptRoot 'tls-strip-relay.mjs'), [string]`$RelayPort, [string]`$MySqlPort, `$relayReport)")) 'the relay receives only ports and a receipt path'
Check (-not ($relay -match 'console\.log')) 'the relay prints nothing'
Check ($runner.Contains("SetEnvironmentVariable('STARROCKS_E2E_SECRET', `$null, 'Process')")) 'a generated token is cleared'
Check ($runner.Contains("foreach (`$name in @('SSL_CERT_FILE', 'SSL_CERT_DIR', 'DOCTOR_PORT', 'DOCTOR_HISTORY', 'DOCTOR_FRONTEND_DIST'))") -and $runner.Contains('foreach ($name in $previous.Keys)')) 'trust-anchor environment is restored in finally'
Check ($runner.Contains("`$env:DOCTOR_HISTORY = 'off'")) 'the API runs without the local history store'
Check ($runner.Contains('$summary.response_scan') -and $runner.Contains('$summary.log_scan')) 'responses and owned logs are scanned for credentials'

# Product connections: verify_full by host name; plaintext only as the two refusal probes.
$connects = [regex]::Matches($runner, "Connect-Product \`$apis\.\w+ ([^ ]+) \`$(\w+) '(\w+)'")
Check ($connects.Count -eq 8) 'eight product connection attempts'
$plain = @($connects | Where-Object { $_.Groups[3].Value -eq 'docker_loopback_plaintext' })
Check ($plain.Count -eq 2) 'exactly two plaintext probes'
Check (@($connects | Where-Object { $_.Groups[3].Value -notin @('verify_full', 'docker_loopback_plaintext') }).Count -eq 0) 'no other TLS mode'
Check ($runner.Contains("`$requested.code -eq 'tls_required'")) 'plaintext to a host name must be refused with tls_required'
Check ($runner.Contains('[int]$wire.client_bytes_sent -eq 0')) 'nothing is sent after a stripped greeting'
foreach ($case in @('verify_full_success', 'hostname_mismatch', 'untrusted_unrelated_ca', 'untrusted_platform_store', 'downgrade_requested_plaintext', 'downgrade_stripped_greeting', 'server_force_secure_transport', 'session_discarded_after_failure', 'expired_certificate')) {
    Check ($runner.Contains("`$cases.$case = ")) "case $case is recorded"
}
Check ($runner.Contains("`$afterCode -eq 'connection_expired'")) 'a failed session is discarded'

# Resource bound: the opt-in low-resource fixture keeps the container within
# 2.5 CPUs / 5.25 GiB with no extra swap and lowers the FE heap before start.
Check ($runner.Contains('-LowResource:$LowResource') -and $fixture.Contains('Get-FixtureResourceArguments -LowResource:$LowResource') -and $fixture.Contains('Set-FixtureLowResourceConfig')) 'the low-resource switch reaches the TLS fixture'

# Isolation and teardown.
Check (-not ($runner -cmatch "'(rm|stop|kill|prune|network)'") -and -not ($fixture -cmatch "'(rm|stop|kill|prune|network)'")) 'no direct Docker removal or network command'
Check ($runner.LastIndexOf('finally {') -lt $runner.LastIndexOf("'teardown.ps1') -ContainerName `$containerName")) 'teardown of the TLS container runs in finally'
foreach ($key in @('container_absent', 'data_root_absent', 'tls_material_absent', 'ports_free')) {
    Check ($runner.Contains("$key = ")) "postcondition $key"
}

Write-Output "PASS $checks TLS verify-full E2E static checks (pure parse; no Docker/SQL execution)"
