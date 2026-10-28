[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BrowserReceipt,
    [Parameter(Mandatory)][string]$OutputPath,
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514',
    [string]$DockerExecutable = $env:DOCKER_EXE
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../dev/starrocks-3.5.14/common.ps1')

function Assert-Oracle([bool]$Condition, [string]$Code) {
    if (-not $Condition) { throw $Code }
}
function Get-OracleFailure($ErrorRecord, [string]$Stage) {
    $stages = @('read_observation','validate_scope','fixture_identity','version_read','first_cohort_read','second_cohort_read','compare_reads','parse_timezone','convert_rows','assemble_result')
    $type = $ErrorRecord.Exception.GetType().FullName
    $types = @('System.Management.Automation.RuntimeException','System.Management.Automation.PropertyNotFoundException','System.Management.Automation.ParameterBindingException','System.Management.Automation.MethodInvocationException','System.ArgumentException','System.FormatException','System.IO.IOException')
    # Persist metadata only, never Message/ErrorDetails/TargetObject/stack/native output.
    [ordered]@{
        failure_stage = $(if ($Stage -cin $stages) { $Stage } else { 'unknown_stage' })
        exception_type = $(if ($type -cin $types) { $type } else { 'other_exception' })
    }
}
function Get-OracleHash([string]$Text) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { (($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)) | ForEach-Object { $_.ToString('x2') }) -join '') }
    finally { $sha.Dispose() }
}
# Independent restatement of the product statement-class filter (D2): FE
# `isQuery` (official 3.5.14 ConnectContext.isQueryStmt, written 1/0 by
# AuditLoader 5.0.0) and the fixed tool statement marker prefix on `stmt`.
function Get-AuditOracleStatementMarker { '/* sr-doctor */ ' }
# Earlier releases tagged their own statements with this marker; audit history keeps them.
function Get-AuditOracleLegacyStatementMarker { '/* starrocks-doctor */ ' }
function Get-AuditOracleMarkedPredicate {
    '(`stmt` LIKE ''' + (Get-AuditOracleStatementMarker) + '%'' OR `stmt` LIKE ''' + (Get-AuditOracleLegacyStatementMarker) + '%'')'
}
function Get-AuditOracleUnmarkedPredicate {
    '(`stmt` NOT LIKE ''' + (Get-AuditOracleStatementMarker) + '%'' AND `stmt` NOT LIKE ''' + (Get-AuditOracleLegacyStatementMarker) + '%'')'
}
function Get-AuditOracleStatementFilter($Scope) {
    $property = $Scope.PSObject.Properties['statement_filter']
    Assert-Oracle ($null -ne $property -and $property.Value -is [string] -and $property.Value -cin @('all','exclude_doctor_and_non_query')) 'scope_statement_filter_invalid'
    $property.Value
}
function New-AuditOracleBaseWhere($Scope) {
    $allowed = @('window','state','user','catalog','database','resource_group','native_digest','statement_filter')
    foreach ($key in $Scope.PSObject.Properties.Name) { Assert-Oracle ($key -cin $allowed) 'scope_field_invalid' }
    foreach ($key in $Scope.window.PSObject.Properties.Name) { Assert-Oracle ($key -cin @('start_seconds','end_seconds')) 'window_field_invalid' }
    $start = $Scope.window.start_seconds; $end = $Scope.window.end_seconds
    Assert-Oracle ($start -is [long] -or $start -is [int]) 'window_invalid'
    Assert-Oracle ($end -is [long] -or $end -is [int]) 'window_invalid'
    Assert-Oracle ($start -ge 0 -and $end -gt $start -and $end -le [uint32]::MaxValue -and ($end - $start) -le 3600) 'oracle_window_outside_one_hour'
    $where = @("``timestamp`` >= FROM_UNIXTIME($start)", "``timestamp`` < FROM_UNIXTIME($end)")
    $columns = @{ user='user'; catalog='catalog'; database='db'; resource_group='resourceGroup'; native_digest='digest'; state='state' }
    foreach ($key in $columns.Keys) {
        $property = $Scope.PSObject.Properties[$key]
        if ($null -eq $property -or $null -eq $property.Value) { continue }
        $value = $property.Value
        Assert-Oracle ($value -is [string] -and $value.Length -gt 0 -and [Text.Encoding]::UTF8.GetByteCount($value) -le 256 -and $value -notmatch '[\x00-\x1f]') 'scope_filter_invalid'
        if ($key -eq 'state') {
            Assert-Oracle ($value -cin @('finished','error')) 'scope_state_invalid'
            $value = if ($value -ceq 'finished') { 'EOF' } else { 'ERR' }
        }
        # Hex text literals keep arbitrary exact filters out of SQL syntax.
        $hex = ([Text.Encoding]::UTF8.GetBytes($value) | ForEach-Object { $_.ToString('x2') }) -join ''
        $where += "``$($columns[$key])`` = CAST(UNHEX('$hex') AS VARCHAR)"
    }
    $where
}
function New-AuditOracleHiddenCountSql($Scope) {
    if ((Get-AuditOracleStatementFilter $Scope) -ceq 'all') { return '' }
    $where = New-AuditOracleBaseWhere $Scope
    # COUNT(CASE ...) over the unfiltered base cohort; disjoint by isQuery.
    'SELECT JSON_ARRAY(CAST(COUNT(CASE WHEN `isQuery` IS NULL OR `isQuery` <> 1 THEN 1 END) AS STRING), CAST(COUNT(CASE WHEN `isQuery` = 1 AND ' + (Get-AuditOracleMarkedPredicate) + ' THEN 1 END) AS STRING)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE ' + ($where -join ' AND ') + '; '
}
function New-AuditOracleSql($Scope, [string]$Sort) {
    Assert-Oracle ($Sort -ceq 'query_time_desc') 'unsupported_oracle_sort'
    $where = @(New-AuditOracleBaseWhere $Scope)
    if ((Get-AuditOracleStatementFilter $Scope) -ceq 'exclude_doctor_and_non_query') {
        $where += '`isQuery` = 1 AND (`stmt` IS NULL OR ' + (Get-AuditOracleUnmarkedPredicate) + ')'
    }
    # Read the entire bounded cohort, not just the displayed Top. `stmt` leaves the
    # database only as the SHA-256 of the product's 240-character prefix, never as text.
    'SELECT JSON_ARRAY(LOWER(`queryId`), CAST(`queryTime` AS STRING), CAST(`scanRows` AS STRING), CAST(`returnRows` AS STRING), CAST(`scanBytes` AS STRING), CAST(`cpuCostNs` AS STRING), CAST(`memCostBytes` AS STRING), `state`, CAST(UNIX_TIMESTAMP(`timestamp`) AS STRING), SHA2(LEFT(`stmt`, 240), 256), CASE WHEN CHAR_LENGTH(`stmt`) > 240 THEN ''1'' ELSE ''0'' END) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE ' + ($where -join ' AND ') + ' ORDER BY `queryTime` DESC, `timestamp` DESC, LOWER(`queryId`) ASC LIMIT 501;'
}
function Convert-AuditOracleRows([object[]]$Lines, [int]$Limit) {
    Assert-Oracle ($Limit -ge 1 -and $Limit -le 100) 'top_limit_invalid'
    Assert-Oracle ($Lines.Count -gt 0 -and $Lines.Count -lt 501) 'cohort_empty_or_cap_reached'
    $ids = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $rows = @(); $dom = @(); $errors = 0
    $names = @('query_time','scan_rows','return_rows','scan_bytes','cpu_cost','memory_cost')
    $units = @('ns','item','item','byte','ns','byte')
    foreach ($line in $Lines) {
        $cells = @($line | ConvertFrom-Json)
        Assert-Oracle ($cells.Count -eq 11) 'source_projection_invalid'
        Assert-Oracle ($cells[0] -is [string] -and $cells[0] -cmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') 'cohort_invalid_identity'
        Assert-Oracle ($ids.Add($cells[0])) 'cohort_duplicate_identity'
        # QueryState.MysqlStateType at fixed 23a56ec: NOOP/OK/EOF/ERR.
        # Match the product's typed mapping: OK/NOOP are retained as unknown,
        # not promoted to finished and not dropped from the cohort.
        Assert-Oracle ($cells[7] -cin @('EOF','ERR','OK','NOOP')) 'cohort_unknown_state'
        $queryStatus = switch -CaseSensitive ($cells[7]) { 'EOF' { 'finished' }; 'ERR' { 'error' }; default { 'unknown' } }
        if ($cells[7] -ceq 'ERR') { $errors++ }
        $metrics = @(); $raw = @()
        for ($i = 0; $i -lt 6; $i++) {
            Assert-Oracle ($cells[$i+1] -is [string] -and $cells[$i+1] -cmatch '^(0|[1-9][0-9]*)$') 'cohort_nonexact_metric'
            $number = [Numerics.BigInteger]::Parse($cells[$i+1], [Globalization.CultureInfo]::InvariantCulture)
            if ($i -eq 0) { $number *= 1000000 }
            Assert-Oracle ($number -le [uint64]::MaxValue) 'cohort_metric_overflow'
            $value = $number.ToString([Globalization.CultureInfo]::InvariantCulture)
            $state = if ($number -gt 9007199254740991) { 'precision_unsafe' } else { 'exact' }
            $metrics += [ordered]@{ metric_id=$names[$i]; state=$state; value=$value; unit=$units[$i] }
            $raw += "$value $($units[$i])"
        }
        Assert-Oracle ($cells[10] -cin @('0','1')) 'cohort_statement_flag_invalid'
        if ($null -eq $cells[9]) {
            $statement = [ordered]@{ state='sql_null' }
        } else {
            Assert-Oracle ($cells[9] -is [string] -and $cells[9] -cmatch '^[0-9a-f]{64}$') 'cohort_statement_hash_invalid'
            $statement = [ordered]@{ state='text'; sha256=$cells[9]; truncated=($cells[10] -ceq '1') }
        }
        if ($rows.Count -lt $Limit) {
            $hash = Get-OracleHash $cells[0]
            $rows += [ordered]@{ query_id_sha256=$hash; status=$queryStatus; metrics=$metrics; statement_sample=$statement }
            $dom += [ordered]@{ query_id_sha256=$hash; metric_raw=$raw }
        }
    }
    [ordered]@{ retained_event_count=[string]$Lines.Count; error_count=[string]$errors; rows=$rows; dom_rows=$dom; top=[ordered]@{ full_window_count=[string]$Lines.Count; requested_limit=$Limit; returned_count=$rows.Count; truncated=($Lines.Count -gt $Limit) } }
}

Assert-Oracle (-not (Test-Path -LiteralPath $OutputPath)) 'output_exists'
$clock = [Diagnostics.Stopwatch]::StartNew()
$result = [ordered]@{ schema='audit-release-oracle/v1'; source='fixed_3.5.14_auditloader_independent_sql'; status='INCONCLUSIVE'; cohort_complete=$false }
$queries = 0
$stage = 'read_observation'
try {
    Assert-Oracle ((Get-Item -LiteralPath $BrowserReceipt).Length -le 4194304) 'browser_receipt_too_large'
    $observation = (Get-Content -LiteralPath $BrowserReceipt -Raw | ConvertFrom-Json).audit_release
    Assert-Oracle ($observation.schema -ceq 'audit-release-observation/v1' -and $observation.transport -ceq 'real_loopback_api') 'observation_invalid'
    $body = $observation.overview.body
    $stage = 'validate_scope'
    $sql = New-AuditOracleSql $body.scope $body.sort
    $hiddenSql = New-AuditOracleHiddenCountSql $body.scope
    $hiddenLines = if ($hiddenSql) { 1 } else { 0 }
    $limit = [int]$body.top.requested_limit
    Assert-Oracle ($limit -ge 1 -and $limit -le 100) 'top_limit_invalid'
    $stage = 'fixture_identity'
    Assert-FixtureContainerName $ContainerName
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $identity = Get-FixtureContainerIdentity $docker $ContainerName
    Assert-Oracle ($null -ne $identity -and $identity.Image -ceq $script:FixtureImage -and $identity.Label -ceq $script:FixtureLabelValue) 'fixture_identity_invalid'
    $password = Get-FixturePassword -Secret (Get-FixtureSecret) -Role doctor_reader
    # Per-session limits; no global changes, no target workload, no DDL/DML.
    $settings = 'SET query_timeout=5; SET exec_mem_limit=67108864; '
    $queries++
    $stage = 'version_read'
    $version = @(Invoke-FixtureMysql $docker $ContainerName doctor_reader $password ($settings + 'SELECT current_version();'))
    Assert-Oracle ($version.Count -eq 1 -and $version[0] -ceq '3.5.14-23a56ec') 'fixture_version_invalid'
    $zoneSql = 'SELECT JSON_ARRAY(@@time_zone, @@system_time_zone, CAST(TIME_TO_SEC(TIMEDIFF(NOW(), UTC_TIMESTAMP())) AS STRING)); '
    $queries++
    $stage = 'first_cohort_read'
    $before = @(Invoke-FixtureMysql $docker $ContainerName doctor_reader $password ($settings + $zoneSql + $hiddenSql + $sql))
    $queries++
    $stage = 'second_cohort_read'
    $after = @(Invoke-FixtureMysql $docker $ContainerName doctor_reader $password ($settings + $zoneSql + $hiddenSql + $sql))
    $stage = 'compare_reads'
    Assert-Oracle (($before -join "`n") -ceq ($after -join "`n")) 'source_changed_between_reads'
    Assert-Oracle ($before.Count -ge 2 + $hiddenLines) 'source_empty'
    $stage = 'parse_timezone'
    $zone = @($before[0] | ConvertFrom-Json)
    Assert-Oracle ($zone.Count -eq 3) 'source_timezone_invalid'
    $timeZone = [ordered]@{ session_time_zone=$zone[0]; system_time_zone=$zone[1]; effective_time_zone=$(if ($zone[0] -ieq 'SYSTEM') {$zone[1]} else {$zone[0]}); current_utc_offset_seconds=[int]$zone[2]; source_ref='connection:session_time_zone' }
    foreach ($key in $timeZone.Keys) { Assert-Oracle ($timeZone[$key] -ceq $body.audit_timestamp_time_zone.$key) 'source_timezone_mismatch' }
    $stage = 'convert_rows'
    $statementFilter = [ordered]@{ state = 'not_applied' }
    if ($hiddenLines -eq 1) {
        $hidden = @($before[1] | ConvertFrom-Json)
        Assert-Oracle ($hidden.Count -eq 2 -and @($hidden | Where-Object { $_ -is [string] -and $_ -cmatch '^(0|[1-9][0-9]*)$' }).Count -eq 2) 'hidden_count_invalid'
        $statementFilter = [ordered]@{ state = 'applied'; hidden_non_query_row_count = $hidden[0]; hidden_doctor_row_count = $hidden[1] }
    }
    $facts = Convert-AuditOracleRows @($before | Select-Object -Skip (1 + $hiddenLines)) $limit
    $facts.statement_filter = $statementFilter
    $stage = 'assemble_result'
    foreach ($key in $facts.Keys) { $result[$key] = $facts[$key] }
    $result.scope = $body.scope; $result.sort = $body.sort; $result.audit_timestamp_time_zone = $timeZone
    $result.source_projection_sha256 = Get-OracleHash ($before -join "`n")
    $result.status = 'PASS'; $result.cohort_complete = $true
} catch {
    $failure = Get-OracleFailure $_ $stage
    foreach ($key in $failure.Keys) { $result[$key] = $failure[$key] }
    # Never persist native error text, SQL, source rows, session tokens or credentials.
    $result.reason = if ($_.Exception.Message -cmatch '^[a-z][a-z0-9_]+$') { $_.Exception.Message } else { 'oracle_capture_failed' }
} finally {
    $password = $null; $sql = $null; $before = $null; $after = $null
    $clock.Stop()
    $result.resources = [ordered]@{ elapsed_ms=$clock.ElapsedMilliseconds; mysql_sessions=$queries; query_timeout_seconds=5; exec_mem_limit_bytes=67108864; source_row_cap=501; oracle_window_max_seconds=3600; actual_database_peak_memory='NOT_MEASURED'; query_limits_observed='NOT_MEASURED' }
    $result.limitations = @('A05_one_hour_fixture_only_not_product_window_limit','unique_exact_cohort_only','stable_two_read_observation_not_atomic_snapshot','AuditLoader_installation_and_ingestion_proven_by_external_coordinator','not_A06_resource_acceptance')
    $result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $OutputPath -Encoding utf8
}
if ($result.status -ne 'PASS') { exit 2 }
