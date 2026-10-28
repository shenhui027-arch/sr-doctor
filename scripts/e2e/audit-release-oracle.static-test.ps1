$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$path = Join-Path $PSScriptRoot 'audit-release-oracle.ps1'
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($path,[ref]$tokens,[ref]$errors)
if ($errors.Count) { throw 'AST invalid' }
# Load only pure function definitions; no top-level native invocation.
$ast.FindAll({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst]},$false) | ForEach-Object { . ([scriptblock]::Create($_.Extent.Text)) }
$count=0
function Check([bool]$Ok,[string]$Name) { if(-not $Ok){throw $Name}; $script:count++ }
function Reject([scriptblock]$Action,[string]$Code) { $caught=$false; try{& $Action}catch{ $caught=$_.Exception.Message -ceq $Code }; Check $caught $Code }
$scope=[pscustomobject]@{window=[pscustomobject]@{start_seconds=100;end_seconds=3700};user="x'; DROP TABLE t;--";statement_filter='all'}
$sql=New-AuditOracleSql $scope 'query_time_desc'
Check ($sql.Contains('>= FROM_UNIXTIME(100)') -and $sql.Contains('< FROM_UNIXTIME(3700)')) 'half-open bounds'
Check ($sql.EndsWith('LIMIT 501;')) 'sentinel cap'
Check (-not $sql.Contains('DROP') -and $sql.Contains('CAST(UNHEX(')) 'filter hex quoting'
# stmt leaves the database only as the SHA-256 of the product's 240-character prefix plus a truncation flag.
function Get-RawStmtProjection([string]$Text) { $Text.Substring(0, $Text.IndexOf(' FROM ')).Replace('SHA2(LEFT(`stmt`, 240), 256)', '').Replace("CASE WHEN CHAR_LENGTH(``stmt``) > 240 THEN '1' ELSE '0' END", '') }
Check ($sql.Contains('SHA2(LEFT(`stmt`, 240), 256)') -and $sql.Contains("CASE WHEN CHAR_LENGTH(``stmt``) > 240 THEN '1' ELSE '0' END")) 'statement prefix hash projection'
Check (-not (Get-RawStmtProjection $sql).Contains('stmt')) 'no raw SQL projection'
Check (-not $sql.Contains('isQuery') -and (New-AuditOracleHiddenCountSql $scope) -ceq '') 'all statements: no statement predicate or hidden read'
# D2: the same statement-class predicate as the product, restated independently.
$filtered=[pscustomobject]@{window=[pscustomobject]@{start_seconds=100;end_seconds=3700};user='workload_user';statement_filter='exclude_doctor_and_non_query'}
$fsql=New-AuditOracleSql $filtered 'query_time_desc'
Check ($fsql.Contains(" AND ``isQuery`` = 1 AND (``stmt`` IS NULL OR (``stmt`` NOT LIKE '/* sr-doctor */ %' AND ``stmt`` NOT LIKE '/* starrocks-doctor */ %')) ORDER BY")) 'statement filter predicate'
Check (-not (Get-RawStmtProjection $fsql).Contains('stmt')) 'statement filter reads stmt only in WHERE'
$hsql=New-AuditOracleHiddenCountSql $filtered
Check ($hsql.Contains("COUNT(CASE WHEN ``isQuery`` IS NULL OR ``isQuery`` <> 1 THEN 1 END)") -and $hsql.Contains("COUNT(CASE WHEN ``isQuery`` = 1 AND (``stmt`` LIKE '/* sr-doctor */ %' OR ``stmt`` LIKE '/* starrocks-doctor */ %') THEN 1 END)")) 'hidden counts disjoint by isQuery'
Check ($hsql.Contains('`user` = CAST(UNHEX(') -and -not $hsql.Contains('NOT LIKE')) 'hidden counts use base filters without the statement filter'
Reject {New-AuditOracleSql ([pscustomobject]@{window=[pscustomobject]@{start_seconds=100;end_seconds=3700}}) 'query_time_desc'} 'scope_statement_filter_invalid'
Reject {New-AuditOracleSql ([pscustomobject]@{window=[pscustomobject]@{start_seconds=100;end_seconds=3700};statement_filter='hide'}) 'query_time_desc'} 'scope_statement_filter_invalid'
$markerSource = Get-Content (Join-Path $PSScriptRoot '../../backend/crates/doctor-connector/src/audit_workload.rs') -Raw
Check ($markerSource.Contains('pub const DOCTOR_STATEMENT_MARKER: &str = "/* sr-doctor */ ";')) 'oracle marker matches product marker'
Check ($markerSource.Contains('pub const LEGACY_DOCTOR_STATEMENT_MARKER: &str = "/* starrocks-doctor */ ";')) 'oracle legacy marker matches product legacy marker'
Reject {New-AuditOracleSql $scope 'evil'} 'unsupported_oracle_sort'
$scope.window.end_seconds=3701
Reject {New-AuditOracleSql $scope 'query_time_desc'} 'oracle_window_outside_one_hour'
$scope.window.end_seconds=100
Reject {New-AuditOracleSql $scope 'query_time_desc'} 'oracle_window_outside_one_hour'
$one='["00000000-0000-0000-0000-000000000001","2","3","4","5","6","7","EOF","100",null,"0"]'
$two='["00000000-0000-0000-0000-000000000002","1","3","4","5","6","7","ERR","100",null,"0"]'
$f=Convert-AuditOracleRows @($one,$two) 1
foreach ($entry in @(@('EOF','finished'), @('ERR','error'), @('OK','unknown'), @('NOOP','unknown'))) {
    $mapped = Convert-AuditOracleRows @($one.Replace('"EOF"', ('"' + $entry[0] + '"'))) 1
    Check ($mapped.rows[0].status -ceq $entry[1] -and $mapped.retained_event_count -ceq '1') "official state retained $($entry[0])"
    Check ($mapped.error_count -ceq $(if ($entry[0] -ceq 'ERR') {'1'} else {'0'})) "official error count $($entry[0])"
}
foreach ($invalid in @('FINISHED','RUNNING','ok','EOF ','untrusted-state')) {
    Reject { Convert-AuditOracleRows @($one.Replace('"EOF"', ('"' + $invalid + '"'))) 1 } 'cohort_unknown_state'
}
Reject { Convert-AuditOracleRows @($one.Replace('"EOF"','null')) 1 } 'cohort_unknown_state'
Check ($f.retained_event_count -ceq '2' -and $f.error_count -ceq '1') 'full cohort counts not Top counts'
$stmtHash='a'*64
$hashed=Convert-AuditOracleRows @($one.Replace('"100",null,"0"]',('"100","' + $stmtHash + '","1"]'))) 1
Check ($hashed.rows[0].statement_sample.state -ceq 'text' -and $hashed.rows[0].statement_sample.sha256 -ceq $stmtHash -and $hashed.rows[0].statement_sample.truncated) 'statement prefix hash and truncation'
Check ($f.rows[0].statement_sample.state -ceq 'sql_null' -and -not $f.rows[0].statement_sample.Contains('sha256')) 'NULL stmt stays sql_null'
Reject {Convert-AuditOracleRows @($one.Replace('"100",null,"0"]','"100","SELECT 1","0"]')) 1} 'cohort_statement_hash_invalid'
Reject {Convert-AuditOracleRows @($one.Replace('"100",null,"0"]','"100",null,"2"]')) 1} 'cohort_statement_flag_invalid'
Reject {Convert-AuditOracleRows @($one.Replace(',null,"0"]',']')) 1} 'source_projection_invalid'
Check ($f.top.truncated -and $f.top.returned_count -eq 1) 'Top truncation'
Check ($f.rows[0].metrics[0].value -ceq '2000000' -and $f.dom_rows[0].metric_raw[1] -ceq '3 item') 'exact units'
Check ($f.rows[0].query_id_sha256.Length -eq 64 -and ($f|ConvertTo-Json -Depth 10) -notmatch '00000000-0000') 'hash-only identities'
Reject {Convert-AuditOracleRows @($one,$one) 1} 'cohort_duplicate_identity'
Reject {Convert-AuditOracleRows @($one.Replace('"2"','null')) 1} 'cohort_nonexact_metric'
Reject {Convert-AuditOracleRows @($one.Replace('"2"','"-1"')) 1} 'cohort_nonexact_metric'
Reject {Convert-AuditOracleRows @() 1} 'cohort_empty_or_cap_reached'
Reject {Convert-AuditOracleRows (@($one)*501) 1} 'cohort_empty_or_cap_reached'
$source=Get-Content $path -Raw
Check ($source.Contains('source_changed_between_reads') -and $source.Contains('source_timezone_mismatch')) 'stability and timezone guards'
Check ($source.Contains('SET query_timeout=5; SET exec_mem_limit=67108864;')) 'bounded DB settings'
Check ($source.Contains('fixture_identity_invalid') -and $source.Contains('3.5.14-23a56ec')) 'fixed fixture and version'
Check ($source.Contains("status='INCONCLUSIVE'") -and $source.Contains('exit 2')) 'fail-closed oracle'
Check (-not ($source -match '\b(Start-Process|Start-Sleep|docker start|docker stop|INSERT INTO|CREATE TABLE)\b')) 'no fixture mutation or retry'
$workloadPath = Join-Path $PSScriptRoot '../../dev/starrocks-3.5.14/run-fixture-workload.ps1'
$tokens=$null; $errors=$null
[System.Management.Automation.Language.Parser]::ParseFile($workloadPath,[ref]$tokens,[ref]$errors) | Out-Null
Check ($errors.Count -eq 0) 'workload AST'
$workload = Get-Content $workloadPath -Raw
Check ($workload.IndexOf('$auditVisibilityClock =') -lt $workload.IndexOf('foreach ($workload in $workloads)')) 'visibility conservative upper bound begins before targets'
Check ($workload.Contains('$auditObservationCalls++') -and $workload.Contains('not_exact_ingestion_latency')) 'observation counter and honest latency label'
# Load only the two digest helpers; all Docker/MySQL calls below are local mocks.
$wa=[System.Management.Automation.Language.Parser]::ParseFile($workloadPath,[ref]$tokens,[ref]$errors)
$wa.FindAll({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -in @('Get-FixtureDigestSetting','Invoke-DigestFixtureWorkload')},$false) | ForEach-Object { . ([scriptblock]::Create($_.Extent.Text)) }
$docker='mock'; $ContainerName='sr-doctor-v0.1-e2e-3514'
$script:FixtureImage='fixed-digest'; $script:FixtureLabelValue='3.5.14'
function Get-FixtureContainerIdentity { [pscustomobject]@{Image=$script:mockImage;Label='3.5.14'} }
function Invoke-FixtureMysql {
    param($DockerExecutable,$ContainerName,$Username,$Password,$Sql)
    $script:commands.Add($Sql)
    if ($Sql.StartsWith('ADMIN SHOW')) {
        if ($script:badConfig) { return 'invalid' }
        if ($null -ne $script:configRowOverride) { return $script:configRowOverride }
        # Official fixed-commit ConfigBase six-column shape, including AliasNames.
        return "enable_sql_digest`t[]`t$script:setting`tboolean`ttrue`tEnable the sql digest feature, building a parameterized digest for each sql in the query detail"
    }
    if ($Sql -match '= "(true|false)"') {
        $script:setting=$Matches[1]
        if ($script:failEnable -and $script:commands.Count -eq 2) { throw 'mock_enable_failed' }
        if ($script:failRestore -and $script:commands.Count -ge 4) { $script:setting='true' }
        return
    }
    throw 'unexpected_mock_sql'
}
function Reset-DigestMock([string]$Original='false') {
    $script:setting=$Original; $script:mockImage='fixed-digest'; $script:badConfig=$false; $script:configRowOverride=$null
    $script:failEnable=$false; $script:failRestore=$false; $script:runs=0
    $script:commands=[Collections.Generic.List[string]]::new()
}
Reset-DigestMock
$e=[ordered]@{}
Invoke-DigestFixtureWorkload -Evidence $e -Action { $script:runs++ }
Check ($runs -eq 1 -and $commands.Count -eq 0 -and $e.Count -eq 0) 'default path no config reads or writes'
foreach ($original in @('false','true')) {
    Reset-DigestMock $original
    $e=[ordered]@{}
    Invoke-DigestFixtureWorkload -Enabled -Evidence $e -Action { Check ($setting -ceq 'true') 'enabled during workload'; $script:runs++ }
    Check ($runs -eq 1 -and $setting -ceq $original -and $e.restore_verified -and $commands.Count -eq 5) 'exact original restored and read back'
}
Reset-DigestMock
$e=[ordered]@{}
Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence $e -Action { throw 'mock_workload_failed' } } 'mock_workload_failed'
Check ($e.restore_verified -and $setting -ceq 'false') 'workload failure restores'
Reset-DigestMock
$script:failEnable=$true
$e=[ordered]@{}
Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence $e -Action { $script:runs++ } } 'mock_enable_failed'
Check ($runs -eq 0 -and $e.restore_verified -and $setting -ceq 'false') 'ambiguous enable failure restores'
Reset-DigestMock
$script:mockImage='other-image'
Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence ([ordered]@{}) -Action { $script:runs++ } } 'digest_fixture_identity_invalid'
Check ($commands.Count -eq 0 -and $runs -eq 0) 'identity gate before SQL'
Reset-DigestMock
$script:badConfig=$true
Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence ([ordered]@{}) -Action { $script:runs++ } } 'digest_config_value_invalid'
Check ($commands.Count -eq 1 -and $runs -eq 0) 'invalid original never mutates'
Reset-DigestMock
$script:failRestore=$true
$e=[ordered]@{}
Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence $e -Action {} } 'digest_restore_readback_failed'
Check (-not $e.restore_verified) 'restore mismatch fails closed'
Check ($workload.Contains('[switch]$EnableDigestFixture') -and $workload.Contains('if ($EnableDigestFixture) { $sessionSql = "USE e2e_fixture;`n$sessionSql" }')) 'explicit opt in same-session database'
Check (([regex]::Matches($workload,"label = '(baseline|candidate)';")).Count -eq 2 -and ([regex]::Matches($workload,[regex]::Escape('-Sql $sessionSql'))).Count -eq 1) 'two original targets single loop execution site'
Check ($workload.Contains("SELECT bucket, COUNT(*) AS e2e_baseline FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket ORDER BY bucket;") -and $workload.Contains("SELECT bucket, COUNT(*) AS e2e_candidate FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket;")) 'target SQL unchanged'
Check ($workload.Contains("status = 'INCONCLUSIVE'") -and $workload.Contains('digest_fixture = $digestEvidence')) 'digest failure receipt retained'
# Reject guessed/latest-documentation layouts instead of scanning for a boolean.
foreach ($row in @(
    "enable_sql_digest`tfalse`tboolean`ttrue",
    "enable_sql_digest`tfalse`tboolean`ttrue`tcomment",
    "enable_sql_digest`t[]`tfalse`tboolean`ttrue",
    "enable_sql_digest`t[]`tfalse`tboolean`ttrue`tcomment`textra",
    "other_key`t[]`tfalse`tboolean`ttrue`tcomment",
    "enable_sql_digest`t[other]`tfalse`tboolean`ttrue`tcomment",
    "enable_sql_digest`t[]`tunknown`tboolean`ttrue`tcomment",
    "enable_sql_digest`t[]`tfalse`tstring`ttrue`tcomment",
    "enable_sql_digest`t[]`tfalse`tboolean`tfalse`tcomment"
)) {
    Reset-DigestMock
    $script:configRowOverride=$row
    Reject { Invoke-DigestFixtureWorkload -Enabled -Evidence ([ordered]@{}) -Action { $script:runs++ } } 'digest_config_value_invalid'
    Check ($commands.Count -eq 1 -and $runs -eq 0) 'malformed official shape never mutates'
}

# Fixed 23a56ec grammar accepts CAST/UNHEX, not MySQL CONVERT(... USING ...).
Check (-not $sql.Contains('CONVERT(') -and $sql.Contains("AS VARCHAR)")) 'fixed grammar hex expression'
$unicodeScope=[pscustomobject]@{window=[pscustomobject]@{start_seconds=100;end_seconds=3700};user="用户'\";statement_filter='all'}
$unicodeSql=New-AuditOracleSql $unicodeScope 'query_time_desc'
$hex=([Text.Encoding]::UTF8.GetBytes($unicodeScope.user)|ForEach-Object {$_.ToString('x2')}) -join ''
Check ($unicodeSql.Contains("CAST(UNHEX('$hex') AS VARCHAR)") -and -not $unicodeSql.Contains($unicodeScope.user)) 'UTF8 and quote filter remains data'
foreach ($stage in @('read_observation','validate_scope','fixture_identity','version_read','first_cohort_read','second_cohort_read','compare_reads','parse_timezone','convert_rows','assemble_result')) {
    try { throw 'sensitive-sentinel SQL password identity' } catch { $failure=Get-OracleFailure $_ $stage }
    Check ($failure.failure_stage -ceq $stage -and $failure.exception_type -ceq 'System.Management.Automation.RuntimeException') 'value-free stage and type'
    Check (($failure | ConvertTo-Json) -notmatch 'sensitive|password') 'diagnostics omit exception payload'
}
try { throw [FormatException]::new('sensitive-sentinel') } catch { $failure=Get-OracleFailure $_ 'sensitive-stage' }
Check ($failure.failure_stage -ceq 'unknown_stage' -and $failure.exception_type -ceq 'System.FormatException') 'unknown stage redacted and format type classified'

Write-Output "AUDIT_ORACLE_STATIC_PASS checks=$count"
