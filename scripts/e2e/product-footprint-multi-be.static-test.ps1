$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Static trust-boundary checks for the multi-BE product-footprint check
# (v0.6-10). Pure parsing: no Docker, SQL or network.
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$runnerPath = Join-Path $repo 'scripts/e2e/run-product-footprint-multi-be.ps1'
$driverPath = Join-Path $repo 'scripts/e2e/footprint-product-driver.ps1'
$workloadPath = Join-Path $repo 'dev/starrocks-3.5.14-multi-be/footprint/workload.sh'
$checks = 0
function Check([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAIL: $Message" }
    $script:checks++
}

foreach ($file in @($runnerPath, $driverPath)) {
    $tokens = $null; $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$errors)
    Check ($errors.Count -eq 0) "$file parses"
}
$runner = Get-Content -Raw $runnerPath
$driver = Get-Content -Raw $driverPath
$workload = [IO.File]::ReadAllText($workloadPath)
Check (-not $workload.Contains("`r")) 'workload.sh keeps LF line endings for the Linux container'

# Credentials: never on a command line or printed; cleared and restored.
foreach ($text in @($runner, $driver, $workload)) {
    Check (-not ($text -match '--password=|-p\$|IDENTIFIED BY ''')) 'no password on a command line or in plaintext SQL'
    Check (-not ($text -match 'Write-Host|echo \$MYSQL_PWD')) 'credentials are never printed'
}
Check ($runner.Contains("'exec', '-d', '-e', 'MYSQL_PWD'")) 'business clients receive the password only through inherited MYSQL_PWD'
Check (-not ($workload -match 'MYSQL_PWD=')) 'workload.sh never sets MYSQL_PWD'
Check ($runner.Contains("SetEnvironmentVariable('STARROCKS_E2E_SECRET', `$null, 'Process')")) 'a generated token is cleared'
Check ($runner.Contains('foreach ($name in $previous.Keys)')) 'process environment is restored in finally'
Check ($runner.Contains('$env:MYSQL_PWD = $previous[''MYSQL_PWD'']')) 'MYSQL_PWD is restored right after the clients start'
Check ($runner.Contains('$summary.api_log_scan')) 'owned API logs are scanned for leaks'

# The product is driven only through its own loopback HTTP API.
Check (-not ($driver -match 'mysql|Invoke-MultiBeMysql|docker')) 'the product driver never talks to StarRocks or Docker directly'
Check ($driver.Contains('http://127.0.0.1:$ApiPort/api/v1')) 'the product driver targets the loopback API only'
Check ([regex]::Matches($runner, "host = '([^']+)'; port = ").Count -eq 1 -and $runner.Contains("host = '127.0.0.1'; port = ")) 'the only product connection targets 127.0.0.1'
Check ($runner.Contains("tls_mode = 'docker_loopback_plaintext'")) 'plaintext mode is the docker loopback mode'

# What is asserted, and that SQL text is never read back.
Check ($runner.Contains('_no_product_statements') -and $runner.Contains('_read_only_keywords') -and $runner.Contains('_unmarked_only_runtime_top_select') -and $runner.Contains('_no_5xx')) 'idle silence, read-only classes, marker and 5xx checks are all asserted'
Check ($runner.Contains("`$readOnlyKeywords = @('SELECT', 'WITH', 'EXPLAIN', 'SHOW', 'SET')")) 'the read-only keyword set is explicit'
Check ($runner -match "UPPER\(SUBSTRING_INDEX\(TRIM\(REPLACE\(stmt, '\`$marker', ''\)\), ' ', 2\)\)") 'only the first two words of each statement are read from the Audit table'
Check ($runner.Contains("(`$Words -ceq 'ANALYZE PROFILE')")) 'ANALYZE is allowed only as ANALYZE PROFILE, never ANALYZE TABLE'
Check (-not ($runner -match 'SELECT\s+stmt\b|SELECT `stmt`')) 'the Audit statement text is never selected'

# Isolation: cold start, small footprint, only owned containers removed.
Check ($runner.Contains("Get-FixtureContainerIdentity -DockerExecutable `$docker -ContainerName 'sr-doctor-v0.1-e2e-3514'")) 'refuses to run while the allin1 release fixture exists'
$removals = [regex]::Matches($runner, "'rm', '-f', ([^)]+)\)")
Check ($removals.Count -eq 1 -and $removals[0].Groups[1].Value.Trim() -eq '$clientName') 'only the client container is removed directly'
Check ($runner.Contains("`$identity.Label -ceq `$clientLabel") -and $runner.Contains("`$identity.Image -ceq `$script:MultiBeFeImage")) 'the client container is verified by image digest and label before removal'
Check ($runner.IndexOf("'rm', '-f', `$clientName") -lt $runner.IndexOf("'teardown.ps1'")) 'the client container leaves the fixture network before teardown'
Check ($runner.LastIndexOf('finally {') -lt $runner.LastIndexOf("'teardown.ps1'")) 'teardown runs in finally'
Check ($runner.Contains("'--cpus', '1', '--memory', '512m'")) 'the client container is small and resource-limited'
Check ($runner -match '\[ValidateRange\(0, 100000\)\]\[int\]\$AuditSeedRows = 20000') 'the Audit seed stays small (logic check, not a load test)'
Check ($runner.Contains('containers_absent') -and $runner.Contains('network_absent') -and $runner.Contains('data_root_absent') -and $runner.Contains('ports_free')) 'postconditions cover containers, network, data root and ports'
Check ($runner.Contains("evidence = 'logic_check_docker_not_performance_not_production'")) 'the receipt labels the evidence as a logic check, not performance or production'

Write-Output "PASS $checks multi-BE product-footprint static checks (pure parse; no Docker/SQL execution)"
