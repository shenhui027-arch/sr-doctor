$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Static trust-boundary checks for the multi-BE connected E2E. Pure parsing:
# no Docker, SQL or network.
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$runnerPath = Join-Path $repo 'scripts/e2e/run-connected-multi-be-e2e.ps1'
$fixturePath = Join-Path $repo 'dev/starrocks-3.5.14-multi-be/connected-fixture.ps1'
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

# Credentials: never on a command line, never plaintext SQL, cleared afterwards.
foreach ($text in @($runner, $fixture)) {
    Check (-not ($text -match '--password=|-p\$|IDENTIFIED BY ''')) 'no password on a command line or in plaintext SQL'
    Check (-not ($text -match 'Write-Host|Write-Output \$env:STARROCKS_PASSWORD')) 'credentials are never printed'
}
Check ($fixture.Contains('IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral (Get-FixtureNativePasswordHash')) 'fixture roles use native-password ciphertext'
Check (-not ($runner -match 'param\([^)]*Secret')) 'the fixture token is not a script argument'
Check ($runner.Contains("SetEnvironmentVariable('STARROCKS_E2E_SECRET', `$null, 'Process')")) 'a generated token is cleared'
Check ($runner.Contains('foreach ($name in $previous.Keys)')) 'STARROCKS_USER/PASSWORD are restored in finally'
Check ($runner.Contains("`$summary.api_log_scan")) 'owned API logs are scanned for leaks'

# Plaintext only to the loopback FE port published by up.ps1.
$connections = [regex]::Matches($runner, "host = '([^']+)'; port = ([^;]+); username")
Check ($connections.Count -eq 1 -and $connections[0].Groups[1].Value -eq '127.0.0.1') 'the only product connection targets 127.0.0.1'
Check ($runner.Contains("tls_mode = 'docker_loopback_plaintext'")) 'plaintext mode is the docker loopback mode'

# Isolation: cold start, never alongside the allin1 release fixture, only the
# multi-BE scripts create or remove Docker objects.
Check ($runner.Contains("Get-FixtureContainerIdentity -DockerExecutable `$docker -ContainerName 'sr-doctor-v0.1-e2e-3514'")) 'refuses to run while the allin1 release fixture exists'
foreach ($text in @($runner, $fixture)) {
    Check (-not ($text -cmatch "'(rm|stop|kill|prune|network)'")) 'no direct Docker removal or network command'
}
Check ($runner.LastIndexOf('finally {') -lt $runner.LastIndexOf("'teardown.ps1'")) 'teardown runs in finally'
Check ($runner.Contains('containers_absent') -and $runner.Contains('network_absent') -and $runner.Contains('data_root_absent') -and $runner.Contains('ports_free')) 'postconditions cover containers, network, data root and ports'
Check ($fixture.Contains('Assert-MultiBeOwned')) 'fixture provisioning only targets the owned FE'

# Product-path coverage reuses the release smoke unchanged.
Check ($runner.Contains("'connected-smoke.ps1'")) 'connected-smoke.ps1 is reused'
Check ($runner.Contains("`$smoke.graph_state -eq 'complete'")) 'multi-BE graph must be complete'
Check ($runner.Contains("EXPLAIN COSTS")) 'topology-only path is checked at the server'

# Optional browser stage: after the Audit statement check (so its statements
# do not enter that count), env restored in finally, password never passed.
Check ($runner.IndexOf("`$stage = 'browser_e2e'") -gt $runner.IndexOf("`$stage = 'audit_statement_check'")) 'browser stage runs after the Audit statement check'
foreach ($name in @('E2E_BASE_URL', 'E2E_DB_PORT', 'E2E_BASELINE_QUERY_ID', 'E2E_CANDIDATE_QUERY_ID', 'E2E_ARTIFACT_ROOT')) {
    Check ($runner -match "foreach \(\`$name in @\([^)]*'$name'") "$name is restored in finally"
}
Check (-not ($runner -match 'browser-e2e\.mjs[^\r\n]*(password|PASSWORD|secret)')) 'no credential on the browser command line'

Write-Output "PASS $checks multi-BE connected E2E static checks (pure parse; no Docker/SQL execution)"
