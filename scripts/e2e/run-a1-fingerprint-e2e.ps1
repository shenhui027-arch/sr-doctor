[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18064,
    [switch]$SkipBuild,
    # Keep the fixture and API up this long after the HTTP checks so a browser
    # can sign in through serve-password-once.mjs (127.0.0.1:18099) and capture
    # the affected board. GET http://127.0.0.1:18099/release ends the hold.
    [ValidateRange(0, 1800)][int]$HoldForBrowserSeconds = 0
)

# A1 local acceptance on the low-resource 1 FE + 3 BE fixture. The same twelve
# tiny statements run twice through the real AuditLoader path: once without a
# digest (product fingerprints) and once with session enable_sql_digest
# (official digests). The product's groups must match the official classes,
# exact families must still come from the native digest, and no literal may
# reach a response or a log. Serial, bounded, torn down in `finally`.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be'
. (Join-Path $fixtureDir 'common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "a1-fingerprint-$stamp.json"
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$fixturePorts = @(38530, 39530)
$names = @($script:MultiBeFeName) + (Get-MultiBeBackendNames)
$stage = 'initialization'
$summary = [ordered]@{ project = 'a1-a2-fingerprint-similar-multi-be-low-resource'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null; $secret = $null; $startedApi = $null; $fixtureStarted = $false; $primaryFailure = $false; $generatedSecret = $false
$secretLiteral = 'a1_secret_literal'
$longLiteral = 'z' * 9000

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Test-PortListening([int]$Port) { $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) }
function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-FixtureState($Docker) {
    [ordered]@{
        containers_present = @($names | Where-Object { $null -ne (Get-MultiBeIdentity -DockerExecutable $Docker -Name $_) })
        network_present = ($null -ne (Get-MultiBeNetworkLabel -DockerExecutable $Docker))
        data_root_present = (Test-Path -LiteralPath (Get-MultiBeDataRoot))
        listening_ports = @(@($fixturePorts + $ApiPort) | Where-Object { Test-PortListening $_ })
    }
}
function Sorted([object[]]$Values) { @($Values | ForEach-Object { [int]$_ } | Sort-Object) -join ',' }

# Four official classes of 4, 3, 2 and 1 executions, one statement longer than
# the fingerprint prefix and one 16-term OR. Literal, case, comment, hint,
# qualifier, IN-list and LIMIT-offset variants are all inside one class each.
$cases = @(
    'SELECT id FROM t WHERE id = 1'
    'select id from t where id = 22'
    'SELECT id FROM fp_e2e.t WHERE id = -3'
    'SELECT /*+ SET_VAR(query_timeout = 9) */ id FROM t WHERE id = ''4'''
    'SELECT name FROM t WHERE id IN (1, 2) ORDER BY name LIMIT 5'
    'SELECT name FROM t WHERE id IN (3) ORDER BY name ASC LIMIT 0, 7'
    'SELECT name FROM t WHERE id IN (4, 5, 6, 7) ORDER BY name LIMIT 9'
    "SELECT count(*) FROM t WHERE name = '$secretLiteral'"
    'SELECT COUNT(*) FROM t WHERE name = "other"'
    'SELECT t.id FROM t JOIN t AS u ON t.id = u.id'
    "SELECT id FROM t WHERE name = '$longLiteral'"
    ('SELECT id FROM t WHERE ' + ((1..16 | ForEach-Object { "id = $_" }) -join ' OR '))
)

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $summary.evidence_sources = [ordered]@{ runner_sha256 = Get-Sha256 $PSCommandPath; up_sha256 = Get-Sha256 (Join-Path $fixtureDir 'up.ps1'); connected_fixture_sha256 = Get-Sha256 (Join-Path $fixtureDir 'connected-fixture.ps1') }

    $stage = 'cold_start_preconditions'
    $before = Get-FixtureState $docker
    Require ($before.containers_present.Count -eq 0 -and -not $before.network_present -and -not $before.data_root_present -and $before.listening_ports.Count -eq 0) 'A1 E2E requires a cold start.'
    Require ($null -eq (Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName 'sr-doctor-v0.1-e2e-3514')) 'The allin1 release fixture must be absent.'

    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))) {
        $bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', [Convert]::ToBase64String($bytes).Replace('+', '-').Replace('/', '_').TrimEnd('='), 'Process')
        $generatedSecret = $true
    }
    $secret = Get-FixtureSecret

    if (-not $SkipBuild) {
        $stage = 'repository_build'
        & cargo build --locked -p doctor-api
        if ($LASTEXITCODE -ne 0) { throw 'Repository API build failed.' }
        if ($HoldForBrowserSeconds -gt 0) {
            & npm --prefix (Join-Path $repoRoot 'frontend') run build
            if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
        }
    }
    $apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.git_dirty = [bool](& git -C $repoRoot status --porcelain)
    $summary.api_sha256 = Get-Sha256 $apiExecutable

    $stage = 'fixture_up'
    $fixtureStarted = $true
    $up = (& (Join-Path $fixtureDir 'up.ps1') -DockerExecutable $docker -LowResource) | ConvertFrom-Json
    $stage = 'fixture_connected_workload'
    $workload = (& (Join-Path $fixtureDir 'connected-fixture.ps1') -DockerExecutable $docker) | ConvertFrom-Json
    $summary.fixture = [ordered]@{ fe_image = $up.fe_image; be_image = $up.be_image; observed_version = $up.observed_version; backends_alive = $up.backends_alive; resources = $up.resources; audit_loader_version = $workload.audit_loader_version }

    # Tiny dedicated database: 3 rows, read only by the statements above.
    $stage = 'a1_workload'
    $root = Get-MultiBeRootPassword
    Invoke-MultiBeMysql $docker @'
CREATE DATABASE fp_e2e;
CREATE TABLE fp_e2e.t (id INT, name VARCHAR(32)) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 3 PROPERTIES ("replication_num" = "1");
INSERT INTO fp_e2e.t VALUES (1, 'a'), (2, 'b'), (3, 'c');
GRANT SELECT ON TABLE fp_e2e.t TO USER doctor_reader;
'@ $root | Out-Null
    $limits = "USE fp_e2e;`nSET query_timeout = 15;`nSET query_mem_limit = 134217728;`n"
    $statements = ($cases | ForEach-Object { "$_;" }) -join "`n"
    Invoke-MultiBeMysql $docker ($limits + $statements) $root | Out-Null
    Invoke-MultiBeMysql $docker ($limits + "SET enable_sql_digest = true;`n" + $statements) $root | Out-Null

    # AuditLoader flushes in batches; wait for both runs of every case.
    $countSql = "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'fp_e2e' AND isQuery = 1;"
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    do { Start-Sleep -Seconds 2; $audited = [int](@(Invoke-MultiBeMysql $docker $countSql $root)[0]) } while ($audited -lt 2 * $cases.Count -and [DateTime]::UtcNow -lt $deadline)
    Require ($audited -eq 2 * $cases.Count) "Expected $(2 * $cases.Count) audited queries, found $audited."
    $official = @(Invoke-MultiBeMysql $docker "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'fp_e2e' AND isQuery = 1 AND digest IS NOT NULL AND digest <> '' GROUP BY digest;" $root)
    $window = @((@(Invoke-MultiBeMysql $docker "SELECT UNIX_TIMESTAMP(MIN(``timestamp``)), UNIX_TIMESTAMP(MAX(``timestamp``)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'fp_e2e';" $root)[0]) -split "`t")
    $summary.official_classes = Sorted $official
    Require ($summary.official_classes -eq '1,1,1,2,3,4') "Unexpected official digest classes $($summary.official_classes)."

    $stage = 'api_start'
    Require (-not (Test-PortListening $ApiPort)) "API port $ApiPort is already in use."
    $previousPort = $env:DOCTOR_PORT; $previousDist = $env:DOCTOR_FRONTEND_DIST; $previousHistory = $env:DOCTOR_HISTORY
    $env:DOCTOR_PORT = [string]$ApiPort
    # History is off: these checks predate stage B; B-D have their own E2Es.
    $env:DOCTOR_HISTORY = 'off'
    $env:DOCTOR_FRONTEND_DIST = if ($HoldForBrowserSeconds -gt 0) { (Resolve-Path -LiteralPath (Join-Path $repoRoot 'frontend\dist')).Path } else { $null }
    $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-a1-$stamp.stdout.log") `
        -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-a1-$stamp.stderr.log")
    $env:DOCTOR_PORT = $previousPort; $env:DOCTOR_FRONTEND_DIST = $previousDist; $env:DOCTOR_HISTORY = $previousHistory
    $deadline = [DateTime]::UtcNow.AddSeconds(45); $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) {
        try { $capabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 }
    }
    Require ($null -ne $capabilities) 'The local Doctor API did not become ready.'

    $stage = 'a1_http_checks'
    $headers = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }
    $post = { param($Path, $Payload) Invoke-WebRequest "$apiBase$Path" -Method Post -Headers $headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 20 -Compress) -UseBasicParsing }
    $readerPassword = Get-FixturePassword -Secret $secret -Role doctor_reader
    $connection = (& $post '/connections' @{ host = '127.0.0.1'; port = $up.host_ports.mysql; username = 'doctor_reader'; password = $readerPassword; catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext' }).Content | ConvertFrom-Json
    try {
        $scope = [ordered]@{ window = [ordered]@{ start_seconds = [int64]$window[0]; end_seconds = [int64]$window[1] + 1 }; state = $null; user = $null; native_digest = $null; catalog = $null; database = 'fp_e2e'; resource_group = $null; statement_filter = 'exclude_doctor_and_non_query' }
        $id = $connection.connection_id
        $overviewRaw = (& $post "/connections/$id/audit/overview" @{ connection_id = $id; request = [ordered]@{ scope = $scope; primary = [ordered]@{ state = 'families'; sort = 'execution_count_desc'; limit = 20 }; family_top_sort = 'cumulative_query_time_desc' } }).Content
        $fingerprintRaw = (& $post "/connections/$id/audit/fingerprint-families" @{ connection_id = $id; request = [ordered]@{ scope = $scope; sort = 'execution_count_desc'; limit = 20 } }).Content
        # A2: the same window as similar-SQL clusters, then with one member split out.
        $similarRequest = { param($Detached) @{ connection_id = $id; request = [ordered]@{ scope = $scope; sort = 'execution_count_desc'; limit = 20; dimensions = [ordered]@{ tables = $true; join = $true; predicates = $true }; detached_member_keys = @($Detached); refined_cluster_keys = @() } } }
        $similarRaw = (& $post "/connections/$id/audit/similar-sql" (& $similarRequest @())).Content
        $similar = $similarRaw | ConvertFrom-Json
        $largest = @($similar.clusters | Sort-Object { [int]$_.execution_count } -Descending)[0]
        $splitRaw = (& $post "/connections/$id/audit/similar-sql" (& $similarRequest @($largest.members[0].member_key))).Content
    }
    finally {
        if ($HoldForBrowserSeconds -eq 0) { Invoke-RestMethod "$apiBase/connections/$($connection.connection_id)" -Method Delete -Headers $headers -ContentType 'application/json' | Out-Null }
    }
    $overview = $overviewRaw | ConvertFrom-Json
    $fingerprints = $fingerprintRaw | ConvertFrom-Json
    # Exact families still carry one audited `stmt` sample by design (v0.7);
    # the fingerprint list carries templates only, never a literal.
    foreach ($raw in @($overviewRaw, $fingerprintRaw)) { Require (-not $raw.Contains($readerPassword)) 'An audit response leaked a credential.' }
    Require (-not $fingerprintRaw.Contains($secretLiteral) -and -not $fingerprintRaw.Contains('z' * 64)) 'The fingerprint response leaked a literal.'
    $exact = @($overview.primary.rows)
    $groups = @($fingerprints.rows)
    $coverage = $fingerprints.coverage
    $summary.checks = [ordered]@{
        exact_family_sizes = Sorted @($exact | ForEach-Object { $_.execution_count })
        exact_families_native = (@($exact | Where-Object { $_.membership.state -ne 'exact_native_digest' }).Count -eq 0)
        missing_digest_count = $overview.coverage.missing_digest_count
        authority = $fingerprints.authority
        fingerprint_version = $fingerprints.fingerprint_version
        fingerprint_group_sizes = Sorted @($groups | ForEach-Object { $_.execution_count })
        fingerprint_keys_disjoint = (@($groups | Where-Object { -not $_.family_key.StartsWith('audit-fingerprint-v1:') }).Count -eq 0)
        templates = @($groups | ForEach-Object { $_.template })
        eligible = $coverage.eligible_event_count; read = $coverage.read_event_count; source = $coverage.source.state
        fingerprinted = $coverage.fingerprinted_event_count; partial = $coverage.partial_event_count
        unavailable = @($coverage.unavailable | ForEach-Object { "$($_.reason):$($_.count)" })
        literals_absent = $true
    }
    $c = $summary.checks
    Require ($c.exact_family_sizes -eq '1,1,1,2,3,4' -and $c.exact_families_native) 'Exact native-digest families changed.'
    Require ($c.missing_digest_count -eq [string]$cases.Count) 'Unexpected missing-digest coverage.'
    Require ($c.authority -eq 'product_unofficial' -and $c.fingerprint_keys_disjoint) 'Fingerprint groups are not marked unofficial.'
    Require ($c.fingerprint_group_sizes -eq '1,2,3,4') "Fingerprint groups $($c.fingerprint_group_sizes) do not match the official classes."
    Require ($c.eligible -eq [string]$cases.Count -and $c.read -eq [string]$cases.Count -and $c.source -eq 'complete') 'Unexpected fingerprint read coverage.'
    Require ($c.fingerprinted -eq '10' -and $c.partial -eq '1' -and (@($c.unavailable) -join ',') -eq 'massive_compound_predicate:1') 'Unexpected partial or unavailable counts.'

    $stage = 'a2_http_checks'
    Require (-not $similarRaw.Contains($secretLiteral) -and -not $similarRaw.Contains('z' * 64) -and -not $similarRaw.Contains($readerPassword)) 'The similar-SQL response leaked a literal or a credential.'
    $split = $splitRaw | ConvertFrom-Json
    $summary.a2_checks = [ordered]@{
        cluster_sizes = Sorted @($similar.clusters | ForEach-Object { @($_.members).Count })
        cluster_executions = Sorted @($similar.clusters | ForEach-Object { $_.execution_count })
        mixed_sources = @($similar.clusters | Where-Object { @($_.members.source.kind | Sort-Object -Unique).Count -eq 2 }).Count
        bases = @($similar.clusters | ForEach-Object { "$($_.basis.tables -join ',')|$($_.basis.joins -join ',')|$($_.basis.predicate_columns -join ',')" } | Sort-Object)
        independent = @($similar.independent).Count
        unavailable = Sorted @($similar.unavailable | ForEach-Object { 1 })
        unavailable_reasons = @($similar.unavailable | ForEach-Object { $_.structure.reason } | Sort-Object)
        exact_families = $similar.coverage.exact_families.state
        split_cluster_sizes = Sorted @($split.clusters | ForEach-Object { @($_.members).Count })
        split_detached = @($split.independent | Where-Object { $_.detached }).Count
    }
    $a2 = $summary.a2_checks
    Require ($a2.cluster_sizes -eq '2,2,4' -and $a2.cluster_executions -eq '2,4,14' -and $a2.mixed_sources -eq 3) "Unexpected similar clusters $($a2.cluster_sizes) / $($a2.cluster_executions)."
    Require ($a2.independent -eq 0 -and (@($a2.unavailable_reasons) -join ',') -eq 'template_unavailable,template_unavailable' -and $a2.exact_families -eq 'complete') 'Unexpected independent or unavailable similar members.'
    Require ($a2.split_cluster_sizes -eq '2,2,3' -and $a2.split_detached -eq 1) 'Splitting a member did not leave it independent for this request.'

    if ($HoldForBrowserSeconds -gt 0) {
        $stage = 'browser_hold'
        $env:A1_READER_PASSWORD = $readerPassword; $env:A1_PRODUCT_ORIGIN = "http://127.0.0.1:$ApiPort"; $env:A1_PASSWORD_PORT = '18099'; $env:A1_HOLD_SECONDS = [string]$HoldForBrowserSeconds
        try { $served = (& node (Join-Path $PSScriptRoot 'serve-password-once.mjs')) -join ' ' }
        finally { foreach ($name in 'A1_READER_PASSWORD', 'A1_PRODUCT_ORIGIN', 'A1_PASSWORD_PORT', 'A1_HOLD_SECONDS') { [Environment]::SetEnvironmentVariable($name, $null, 'Process') } }
        $summary.browser_hold = $served
    }
    $summary.status = 'PASS'
}
catch {
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message = $_.Exception.Message }
    throw
}
finally {
    $cleanupFailure = $null
    if ($startedApi -and -not $startedApi.HasExited) {
        try { Stop-Process -Id $startedApi.Id -Force -ErrorAction Stop; $startedApi.WaitForExit(5000) | Out-Null } catch { $cleanupFailure = 'api_stop' }
    }
    if ($startedApi -and $startedApi.HasExited -and $null -ne $secret) {
        $needles = @($secret, (Get-FixturePassword -Secret $secret -Role doctor_reader), $secretLiteral, ('z' * 64))
        $logs = @('stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-a1-$stamp.$_.log" } | Where-Object { Test-Path -LiteralPath $_ })
        $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 }).Count
        $summary.api_log_scan = if ($logs.Count -ne 2) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }
        if ($summary.api_log_scan -ne 'PASS' -and -not $cleanupFailure) { $cleanupFailure = 'api_log_scan' }
    }
    if ($fixtureStarted) {
        try { $summary.teardown = (& (Join-Path $fixtureDir 'teardown.ps1') -DockerExecutable $docker) | ConvertFrom-Json }
        catch { $cleanupFailure = 'fixture_teardown' }
    }
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    if ($null -ne $docker) {
        $after = Get-FixtureState $docker
        $summary.postconditions = [ordered]@{ containers_absent = ($after.containers_present.Count -eq 0); network_absent = (-not $after.network_present); data_root_absent = (-not $after.data_root_present); ports_free = ($after.listening_ports.Count -eq 0) }
        if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    }
    if ($cleanupFailure) { $summary.status = 'FAIL'; $summary.cleanup_failure = $cleanupFailure }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'A1 E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
