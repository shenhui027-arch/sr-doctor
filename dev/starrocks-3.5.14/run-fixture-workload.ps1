[CmdletBinding()]
param(
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [switch]$EnableDigestFixture,
    [switch]$AuditReleaseAcceptance,
    [ValidateRange(5, 120)][int]$PollTimeoutSeconds = 30,
    [ValidateRange(65, 120)][int]$PluginWarmupSeconds = 65,
    [string]$ReceiptPath = (Join-Path (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e') ("workload-" + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '.json'))
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

function Assert-WorkspaceReceiptPath {
    param([Parameter(Mandatory)][string]$Path)
    $root = [IO.Path]::GetFullPath((Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e'))
    $fullPath = [IO.Path]::GetFullPath($Path)
    if (-not $fullPath.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "ReceiptPath must be below $root."
    }
    $fullPath
}

function Get-ProfileIdByMarker {
    param([Parameter(Mandatory)][string]$Marker)
    $rows = Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username 'root' -Password '' -Sql 'SHOW PROFILELIST;'
    $row = $rows | Where-Object { $_ -like "*$Marker*" } | Select-Object -First 1
    if ($null -eq $row) { throw "Profile list has no profile for $Marker." }
    $queryId = ($row -split "`t", 2)[0].Trim()
    if ($queryId -notmatch '^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$') {
        throw 'Profile list returned an invalid query ID.'
    }
    $queryId.ToLowerInvariant()
}

# Fixed single-FE fixture only. Never used by product connections.
function Get-FixtureDigestSetting {
    $rows = @(Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username root -Password '' -Sql 'ADMIN SHOW FRONTEND CONFIG LIKE "enable_sql_digest";')
    if ($rows.Count -ne 1) { throw 'digest_config_row_invalid' }
    $cells = $rows[0] -split "`t"
    # StarRocks 23a56ec517db763ea9bc7ee50b19357acdee0303:
    # ConfigBase.getConfigInfo / AdminShowConfigStmt: Key, AliasNames, Value, Type, IsMutable, Comment.
    if ($cells.Count -ne 6 -or $cells[0] -cne 'enable_sql_digest' -or $cells[1] -cne '[]' -or
        $cells[2] -cnotin @('true', 'false') -or $cells[3] -cne 'boolean' -or $cells[4] -cne 'true') {
        throw 'digest_config_value_invalid'
    }
    $cells[2]
}

function Invoke-DigestFixtureWorkload {
    param([scriptblock]$Action, [switch]$Enabled, [System.Collections.IDictionary]$Evidence)
    if (-not $Enabled) { & $Action; return }
    $identity = Get-FixtureContainerIdentity $docker $ContainerName
    if ($null -eq $identity -or $identity.Image -cne $script:FixtureImage -or $identity.Label -cne $script:FixtureLabelValue) {
        throw 'digest_fixture_identity_invalid'
    }
    $original = Get-FixtureDigestSetting
    $Evidence.original = $original
    try {
        # Restore even if the enable call fails after FE applied the change.
        $Evidence.enable_attempted = $true
        Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username root -Password '' -Sql 'ADMIN SET FRONTEND CONFIG ("enable_sql_digest" = "true");' | Out-Null
        $Evidence.enabled_readback = Get-FixtureDigestSetting
        if ($Evidence.enabled_readback -cne 'true') { throw 'digest_enable_readback_failed' }
        & $Action
    }
    finally {
        $Evidence.restore_attempted = $true
        Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username root -Password '' -Sql "ADMIN SET FRONTEND CONFIG (`"enable_sql_digest`" = `"$original`");" | Out-Null
        $Evidence.restored_readback = Get-FixtureDigestSetting
        $Evidence.restore_verified = $Evidence.restored_readback -ceq $original
        if (-not $Evidence.restore_verified) { throw 'digest_restore_readback_failed' }
    }
}
$docker = Get-FixtureDockerExecutable $DockerExecutable
$secret = Get-FixtureSecret
Assert-FixtureContainerName $ContainerName
$workloadPassword = Get-FixturePassword -Secret $secret -Role workload_user
$readerPassword = Get-FixturePassword -Secret $secret -Role doctor_reader
$resolvedReceiptPath = Assert-WorkspaceReceiptPath $ReceiptPath
if (Test-Path -LiteralPath $resolvedReceiptPath) { throw 'ReceiptPath already exists.' }
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $resolvedReceiptPath) | Out-Null

# In 3.5.14 the FE AuditEventProcessor refreshes active plugins on a 60-second
# cadence. A workload sent immediately after INSTALL can therefore be consumed
# before AuditLoader is active. Wait out that documented refresh window. The
# fixture's AuditLoader config has max_batch_interval_sec=5, so the bounded
# query below is the post-workload flush proof rather than a second blind wait.
$plugins = Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username 'root' -Password '' -Sql 'SHOW PLUGINS;'
if (($plugins -join "`n") -notmatch '(?m)^AuditLoader\s+AUDIT\s+.*\s+5\.0\.0\s+') {
    throw 'AuditLoader 5.0.0 is not installed in the dedicated fixture.'
}
Start-Sleep -Seconds $PluginWarmupSeconds

# This is fixture-only external workload. Product APIs never receive or execute these statements.
$workloads = @(
    [ordered]@{ label = 'baseline'; marker = 'e2e_baseline'; sql = 'SELECT bucket, COUNT(*) AS e2e_baseline FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket ORDER BY bucket;' },
    [ordered]@{ label = 'candidate'; marker = 'e2e_candidate'; sql = 'SELECT bucket, COUNT(*) AS e2e_candidate FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket;' }
)
$digestEvidence = [ordered]@{ requested = [bool]$EnableDigestFixture; original = $null; enable_attempted = $false; enabled_readback = $null; restore_attempted = $false; restored_readback = $null; restore_verified = $false }
$queryIds = [ordered]@{}
$f05State = [ordered]@{ query_id = $null; stage = 'not_started' }
$missingProfileDigest = $null

if ($AuditReleaseAcceptance -and -not $EnableDigestFixture) { throw 'F05 requires the digest fixture.' }
$auditVisibilityClock = [Diagnostics.Stopwatch]::StartNew()
try {
Invoke-DigestFixtureWorkload -Enabled:$EnableDigestFixture -Evidence $digestEvidence -Action {
foreach ($workload in $workloads) {
    # Level 1 is the official 3.5.14 default and asks FE to merge the final
    # profile. It also retains the embedded Topology used by the product's
    # evidence-backed plan projection. Level 2 remains covered by parser
    # regression tests and deliberately has no inferred runtime edges.
    $sessionSql = "SET enable_profile=true;`nSET pipeline_profile_level=1;`n$($workload.sql)"
    if ($EnableDigestFixture) { $sessionSql = "USE e2e_fixture;`n$sessionSql" }
    Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username 'workload_user' -Password $workloadPassword -Sql $sessionSql | Out-Null
    $queryIds[$workload.label] = Get-ProfileIdByMarker $workload.marker
}

if ($AuditReleaseAcceptance) {
    # One new structural family, never a rerun of baseline/candidate. Both
    # profile switches are session-only and disappear when this client exits.
    $f05State.stage = 'session_batch'
    $missingSql = @'
SET enable_profile=false;
SET big_query_profile_threshold='0s';
USE e2e_fixture;
SELECT payload, MIN(id) AS f05_min_id FROM e2e_fixture.profile_rows GROUP BY payload ORDER BY payload;
SELECT last_query_id();
'@
    $f05State.query_id = (Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username workload_user -Password $workloadPassword -Sql $missingSql | Select-Object -Last 1).Trim().ToLowerInvariant()
    $f05State.stage = 'identity_validation'
    if ($f05State.query_id -notmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') { throw 'F05 query identity invalid.' }
    $f05State.stage = 'session_batch_completed'
}

}
}
catch {
    # Preserve setting/restoration facts even on failure, without native error text.
    $mysqlDiagnostic = $_.Exception.Data['fixture_mysql']
    if ($f05State.stage -eq 'session_batch' -and $mysqlDiagnostic -and $mysqlDiagnostic.line_number -in 1,2,3,4,5) {
        $f05State.stage = @('disable_profile', 'disable_big_query_profile_threshold', 'select_database', 'unique_query', 'read_last_query_identity')[$mysqlDiagnostic.line_number - 1]
    }
    [ordered]@{ status = 'INCONCLUSIVE'; fixture = 'external_workload_only'; digest_fixture = $digestEvidence; reason = 'fixture_workload_or_digest_setup_failed'; f05_stage = $f05State.stage; mysql_diagnostic = $mysqlDiagnostic } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $resolvedReceiptPath -Encoding utf8
    throw
}

# Produce one real, terminally failed Audit row so the product must expose the
# typed `query_not_finished`/unavailable branch. The failure happens in this
# external fixture process, never through a product endpoint, and is expected.
$unavailableProbeObserved = $false
try {
    Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username 'workload_user' -Password $workloadPassword -Sql 'SELECT /* e2e_unavailable_probe */ * FROM e2e_fixture.table_that_does_not_exist;' | Out-Null
}
catch {
    $unavailableProbeObserved = $true
}
if (-not $unavailableProbeObserved) {
    throw 'The controlled unavailable-Profile probe unexpectedly succeeded.'
}

foreach ($queryId in $queryIds.Values) {
    $profileLength = (Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username doctor_reader -Password $readerPassword -Sql "SELECT LENGTH(get_query_profile('$queryId'));" | Select-Object -First 1).Trim()
    if ($profileLength -notmatch '^[1-9][0-9]*$') { throw 'The read-only role could not retrieve a non-empty fixture Profile.' }
}

$auditObservationCalls = 0
$f05Observation = [ordered]@{ row_count = 0; has_digest = $false; expected_scope_matches = $false }
if ($AuditReleaseAcceptance -and $f05State.query_id -notmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') { throw 'F05 identity missing before Audit observation.' }
$auditCount = 0
$unavailableProbeAuditCount = 0
$baselineLiteral = ConvertTo-SqlLiteral $queryIds.baseline
$candidateLiteral = ConvertTo-SqlLiteral $queryIds.candidate
$deadline = [DateTime]::UtcNow.AddSeconds($PollTimeoutSeconds)
while ([DateTime]::UtcNow -lt $deadline) {
    $auditObservationCalls++
    $auditCount = [int]((Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username doctor_reader -Password $readerPassword -Sql "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE queryId IN ($baselineLiteral, $candidateLiteral);" | Select-Object -First 1).Trim())
    # The audit plugin may normalize/redact the failed statement text. Bind the
    # proof to the dedicated workload user and non-EOF state instead of relying
    # on retention of the SQL marker.
    $unavailableProbeAuditCount = [int]((Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username doctor_reader -Password $readerPassword -Sql "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE user = 'workload_user' AND state <> 'EOF';" | Select-Object -First 1).Trim())
    if ($AuditReleaseAcceptance) {
        $missingRows = @(Invoke-FixtureMysql -DockerExecutable $docker -ContainerName $ContainerName -Username doctor_reader -Password $readerPassword -Sql "SELECT digest, user = 'workload_user', db = 'e2e_fixture', state = 'EOF' FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE queryId = '$($f05State.query_id)' LIMIT 2;")
        $f05Observation.row_count = $missingRows.Count
        if ($missingRows.Count -gt 1) { throw 'F05 duplicate Audit identity.' }
        if ($missingRows.Count -eq 1) {
            $fields = ([string]$missingRows[0]).Split("`t")
            $f05Observation.has_digest = $fields[0] -match '^[0-9a-fA-F]{32}$'
            $f05Observation.expected_scope_matches = $fields.Count -eq 4 -and $fields[1] -eq '1' -and $fields[2] -eq '1' -and $fields[3] -eq '1'
            if ($f05Observation.has_digest -and $f05Observation.expected_scope_matches) { $missingProfileDigest = $fields[0].ToLowerInvariant() }
        }
    }
    if ($auditCount -ge 2 -and $unavailableProbeAuditCount -ge 1 -and (-not $AuditReleaseAcceptance -or $missingProfileDigest)) { break }
    Start-Sleep -Seconds 2
}
if ($auditCount -lt 2) { throw 'AuditLoader did not publish both fixture query IDs before the polling deadline.' }
if ($unavailableProbeAuditCount -lt 1) { throw 'AuditLoader did not publish the controlled unavailable-Profile row before the polling deadline.' }

if ($AuditReleaseAcceptance -and -not $missingProfileDigest) {
    [ordered]@{ status = 'INCONCLUSIVE'; reason = 'f05_audit_observation_timeout'; f05_stage = 'audit_observation'; f05_observation = $f05Observation; digest_fixture = $digestEvidence } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $resolvedReceiptPath -Encoding utf8
    throw 'F05 Audit digest not observed within deadline.'
}
$auditVisibilityClock.Stop()
$receipt = [ordered]@{
    status = 'PASS'; fixture = 'external_workload_only'; container = $ContainerName
    digest_fixture = $digestEvidence; workload_database = $(if ($EnableDigestFixture) { 'e2e_fixture' } else { $null })
    baseline_query_id = $queryIds.baseline; candidate_query_id = $queryIds.candidate
    missing_profile_fixture = $(if ($AuditReleaseAcceptance) { 'one_finished_query_profile_collection_disabled' } else { 'not_requested' })
    audit_rows = $auditCount; plugin_warmup_seconds = $PluginWarmupSeconds
    unavailable_profile_probe = 'terminal_error_audit_row'; unavailable_profile_audit_rows = $unavailableProbeAuditCount
    audit_visibility_observation_calls = $auditObservationCalls; audit_visibility_upper_bound_ms = $auditVisibilityClock.ElapsedMilliseconds
    audit_visibility_measurement = 'from_before_first_target_execution_to_all_required_rows_observed_not_exact_ingestion_latency'
    audit_loader_max_batch_interval_seconds = 5; audit_poll_timeout_seconds = $PollTimeoutSeconds
    created_utc = [DateTime]::UtcNow.ToString('o')
}
$receipt | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $resolvedReceiptPath -Encoding utf8
# Only the parent process receives these short-lived identities. The persisted
# workload receipt does not add raw F05 Query IDs or digests.
$receipt.missing_profile_query_id = $f05State.query_id
$receipt.missing_profile_digest = $missingProfileDigest
$receipt | ConvertTo-Json -Compress
