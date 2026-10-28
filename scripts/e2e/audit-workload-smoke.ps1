param(
    [ValidateRange(1, 65535)] [int]$ApiPort = 8001,
    [ValidateRange(1, 65535)] [int]$MySqlPort = 39230,
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514[A-Za-z0-9_-]*$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514-runtime',
    [ValidateNotNullOrEmpty()] [string]$Username = 'root',
    # Empty uses the password-free Docker fixture. A named process variable is
    # read only at execution and is never printed or persisted.
    [string]$PasswordEnv = '',
    # Optional executable path recorded as a SHA-256 in the receipt. The
    # harness never starts, stops, or replaces this API process.
    [string]$ApiExecutablePath = '',
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\..\e2e\audit-workload')
)

# Synthetic source-data oracle on real StarRocks. This does not prove
# AuditLoader ingestion, Profile retrieval, business SQL, or production load.
$ErrorActionPreference = 'Stop'
$script:assertionCount = 0
$script:csrfHeaders = @{}
$connectionId = $null
$runStatus = 'FAIL'
$failure = $null
$stage = 'initializing'
$lookupSqlSha256 = $null
$capabilities = $null
$session = $null
$windowStart = 1788048000
$windowEnd = $windowStart + 3600
$fixtureDatabase = 'doctor_v2_oracle'
$fixtureTable = 'audit_workload_events'
$fixtureQualifiedTable = "$fixtureDatabase.$fixtureTable"
$fixedImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$fixedLabelName = 'com.sr-doctor.e2e'
$fixedLabelValue = '3.5.14'
$legacyFixtureColumns = @('queryId', 'timestamp', 'state', 'queryTime', 'scanBytes', 'scanRows', 'cpuCostNs', 'memCostBytes', 'returnRows', 'catalog', 'db', 'resourceGroup', 'digest', 'stmt')
$expectedColumns = $legacyFixtureColumns + @('user')
$oracle = [ordered]@{
    raw_row_count = '607'; invalid_query_id_count = '1'; exact_duplicate_row_count = '4'; conflicting_duplicate_row_count = '2'; retained_event_count = '600'; retained_family_count = '2'; missing_digest_count = '1'; missing_context_count = '1'
    query_time = [ordered]@{ sql_null = '0'; text = '600'; nonnegative = '600'; negative = '0'; aggregate = '1000598'; aggregation = 'sum' }
    scan_bytes = [ordered]@{ sql_null = '1'; text = '599'; nonnegative = '599'; negative = '0'; aggregate = '598'; aggregation = 'sum' }
    cpu_cost = [ordered]@{ sql_null = '0'; text = '600'; nonnegative = '599'; negative = '1'; aggregate = '599'; aggregation = 'sum' }
    memory_cost = [ordered]@{ sql_null = '0'; text = '600'; nonnegative = '600'; negative = '0'; aggregate = '9223372036854775807'; aggregation = 'max' }
}

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:assertionCount++
}
function Get-TextSha256([string]$Text) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { (($sha.ComputeHash([Text.UTF8Encoding]::new($false).GetBytes($Text)) | ForEach-Object { $_.ToString('x2') }) -join '') }
    finally { $sha.Dispose() }
}
function Get-FileSha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Sql-String([AllowNull()][object]$Value) {
    if ($null -eq $Value) { return 'NULL' }
    if ($Value -isnot [string]) { throw 'Fixture text literal is invalid.' }
    "'$($Value.Replace("'", "''"))'"
}
function Sql-Decimal([AllowNull()][object]$Value) {
    if ($null -eq $Value) { return 'NULL' }
    if ($Value -isnot [string] -or $Value -notmatch '^-?[0-9]+$') { throw 'Fixture metric literal is invalid.' }
    $Value
}
function New-OracleRecord(
    [string]$QueryId, [int]$EventSeconds, [string]$QueryTime,
    [AllowNull()][object]$ScanBytes = '1', [string]$ScanRows = '1', [string]$CpuCost = '1', [string]$MemoryCost = '1',
    [AllowNull()][object]$Catalog = 'catalog_a', [AllowNull()][object]$Database = 'db_a', [AllowNull()][object]$Digest = 'a0b1',
    [string]$Statement = 'SELECT audit_oracle_default',
    [string]$User = 'fixture_user_a', [ValidateSet('EOF', 'ERR')] [string]$State = 'EOF'
) {
    [pscustomobject]@{
        query_id = $QueryId; event_seconds = $EventSeconds; state = $State; query_time = $QueryTime; scan_bytes = $ScanBytes; scan_rows = $ScanRows; cpu_cost = $CpuCost; memory_cost = $MemoryCost; return_rows = '1'; catalog = $Catalog; database = $Database; resource_group = 'default_wg'; digest = $Digest; statement = $Statement; user = $User
    }
}
function Format-OracleRecord($Record) {
    "($(Sql-String $Record.query_id), FROM_UNIXTIME($($Record.event_seconds)), $(Sql-String $Record.state), $(Sql-Decimal $Record.query_time), $(Sql-Decimal $Record.scan_bytes), $(Sql-Decimal $Record.scan_rows), $(Sql-Decimal $Record.cpu_cost), $(Sql-Decimal $Record.memory_cost), $(Sql-Decimal $Record.return_rows), $(Sql-String $Record.catalog), $(Sql-String $Record.database), $(Sql-String $Record.resource_group), $(Sql-String $Record.digest), $(Sql-String $Record.statement), $(Sql-String $Record.user))"
}
function New-HalfOpenWindowCountSql(
    [ValidatePattern('^[A-Za-z0-9_]+\.[A-Za-z0-9_]+$')] [string]$QualifiedTable,
    [ValidateRange(0, [Int64]::MaxValue)] [Int64]$WindowStart,
    [ValidateRange(1, [Int64]::MaxValue)] [Int64]$WindowEnd
) {
    if ($WindowEnd -le $WindowStart) { throw 'Fixture window is invalid.' }
    "SELECT COUNT(*) AS expected_raw_window_count FROM $QualifiedTable WHERE ``timestamp`` >= FROM_UNIXTIME($WindowStart) AND ``timestamp`` < FROM_UNIXTIME($WindowEnd);"
}
function New-FixtureSql {
    $records = [Collections.Generic.List[object]]::new()
    for ($number = 1; $number -le 600; $number++) {
        $id = ('00000000-0000-0000-0000-{0:D12}' -f $number)
        $seconds = if ($number -eq 1) { $windowStart } elseif ($number -le 10) { $windowStart + 3580 + $number } else { $windowStart + 1800 + $number }
        $queryTime = if ($number -eq 1) { '999999' } else { '1' }
        $scanBytes = if ($number -eq 2) { $null } elseif ($number -eq 3) { '0' } else { '1' }
        $scanRows = if ($number -eq 4) { '9007199254740992' } else { '1' }
        $cpuCost = if ($number -eq 5) { '-1' } else { '1' }
        $memoryCost = if ($number -eq 6) { '9223372036854775807' } else { '1' }
        $digest = if ($number -eq 8) { $null } else { 'a0b1' }
        $catalog = if ($number -eq 9) { $null } elseif ($number -eq 10) { 'catalog_b' } else { 'catalog_a' }
        $database = if ($number -eq 10) { 'db_b' } else { 'db_a' }
        $statement = if ($number -eq 1) { 'SELECT audit_oracle_oldest_slowest' } else { "SELECT audit_oracle_$number" }
        $user = if ($number % 2 -eq 0) { 'fixture_user_b' } else { 'fixture_user_a' }
        $state = if ($number -in @(20, 22)) { 'ERR' } else { 'EOF' }
        $records.Add((New-OracleRecord -QueryId $id -EventSeconds $seconds -QueryTime $queryTime -ScanBytes $scanBytes -ScanRows $scanRows -CpuCost $cpuCost -MemoryCost $memoryCost -Catalog $catalog -Database $database -Digest $digest -Statement $statement -User $user -State $state))
    }
    foreach ($number in 11..14) { $records.Add($records[$number - 1]) }
    $records.Add((New-OracleRecord -QueryId '00000000-0000-0000-0000-000000000601' -EventSeconds ($windowStart + 2500) -QueryTime '77' -Statement 'SELECT audit_oracle_conflict_a'))
    $records.Add((New-OracleRecord -QueryId '00000000-0000-0000-0000-000000000601' -EventSeconds ($windowStart + 2500) -QueryTime '78' -Statement 'SELECT audit_oracle_conflict_b'))
    $records.Add((New-OracleRecord -QueryId 'not-a-valid-query-id' -EventSeconds ($windowStart + 2550) -QueryTime '1' -Statement 'SELECT audit_oracle_invalid_id'))
    # Event exactly at end is intentionally outside the half-open window.
    $records.Add((New-OracleRecord -QueryId '00000000-0000-0000-0000-000000000700' -EventSeconds $windowEnd -QueryTime '2000000' -Statement 'SELECT audit_oracle_end_boundary'))
    $ddl = @(
        '-- Synthetic source-data oracle on real StarRocks; not AuditLoader ingestion.',
        'CREATE DATABASE IF NOT EXISTS doctor_v2_oracle;',
        'DROP TABLE IF EXISTS doctor_v2_oracle.audit_workload_events;',
        'CREATE TABLE doctor_v2_oracle.audit_workload_events (',
        '  `queryId` VARCHAR(64) NOT NULL,', '  `timestamp` DATETIME NOT NULL,', '  `state` VARCHAR(16) NOT NULL,',
        '  `queryTime` BIGINT NULL,', '  `scanBytes` BIGINT NULL,', '  `scanRows` BIGINT NULL,',
        '  `cpuCostNs` BIGINT NULL,', '  `memCostBytes` BIGINT NULL,', '  `returnRows` BIGINT NULL,',
        '  `catalog` VARCHAR(256) NULL,', '  `db` VARCHAR(256) NULL,', '  `resourceGroup` VARCHAR(256) NULL,', '  `digest` VARCHAR(128) NULL,', '  `stmt` VARCHAR(4096) NULL,', '  `user` VARCHAR(256) NULL',
        ') ENGINE=OLAP', 'DUPLICATE KEY(`queryId`)', 'DISTRIBUTED BY HASH(`queryId`) BUCKETS 1', 'PROPERTIES ("replication_num" = "1");'
    ) -join "`n"
    $statements = [Collections.Generic.List[string]]::new(); $statements.Add($ddl)
    for ($offset = 0; $offset -lt $records.Count; $offset += 100) {
        $last = [Math]::Min($offset + 99, $records.Count - 1)
        $values = @($records[$offset..$last] | ForEach-Object { Format-OracleRecord $_ })
        $statements.Add("INSERT INTO doctor_v2_oracle.audit_workload_events VALUES`n$($values -join ",`n");")
    }
    $statements.Add((New-HalfOpenWindowCountSql -QualifiedTable $fixtureQualifiedTable -WindowStart $windowStart -WindowEnd $windowEnd))
    ($statements -join "`n`n") + "`n"
}
function Invoke-OracleMysql([string]$Sql, [string]$Password) {
    $previousPassword = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
    if ([string]::IsNullOrEmpty($Password)) { [Environment]::SetEnvironmentVariable('MYSQL_PWD', $null, 'Process') } else { [Environment]::SetEnvironmentVariable('MYSQL_PWD', $Password, 'Process') }
    try {
        # Keep host and port separate: compact -h127 is not portable here.
        # `docker exec -e MYSQL_PWD` forwards only the process value set above;
        # the password is neither placed in argv nor emitted in a receipt.
        $arguments = @('exec', '-i')
        if (-not [string]::IsNullOrEmpty($Password)) { $arguments += @('-e', 'MYSQL_PWD') }
        $arguments += @($ContainerName, 'mysql', '--batch', '--skip-column-names', '--raw', '-h', '127.0.0.1', '-P', '9030', '--user', $Username)
        $output = $Sql | & docker @arguments 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Fixture MySQL command failed.' }
        @($output | ForEach-Object { [string]$_ })
    }
    finally { [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previousPassword, 'Process') }
}
function Invoke-DoctorApi(
    [ValidateSet('Get', 'Post', 'Delete')] [string]$Method,
    [string]$Path, $Payload, [int[]]$ExpectedStatus = @(200)
) {
    $invoke = @{ Uri = "http://127.0.0.1:$ApiPort/api/v1$Path"; Method = $Method; ErrorAction = 'Stop'; SkipHttpErrorCheck = $true }
    if ($script:csrfHeaders.Count -gt 0) { $invoke.Headers = $script:csrfHeaders }
    if ($null -ne $Payload) { $invoke.ContentType = 'application/json'; $invoke.Body = $Payload | ConvertTo-Json -Depth 32 -Compress }
    elseif ($Method -eq 'Delete') { $invoke.ContentType = 'application/json' }
    $status = $null; $content = $null
    try { $response = Invoke-WebRequest @invoke; $status = [int]$response.StatusCode; $content = [string]$response.Content }
    catch { $httpResponse = $_.Exception.Response; if ($null -eq $httpResponse) { throw 'Doctor API request did not receive an HTTP response.' }; $status = [int]$httpResponse.StatusCode }
    $body = if ([string]::IsNullOrWhiteSpace($content)) { $null } else { $content | ConvertFrom-Json }
    if ($ExpectedStatus -notcontains $status) {
        $safeCode = [string]$body.code
        if ([string]::IsNullOrWhiteSpace($safeCode)) { $safeCode = [string]$body.error.code }
        if ($safeCode -notmatch '^[a-zA-Z0-9_]{1,80}$') { $safeCode = 'unclassified' }
        throw "Doctor API returned unexpected HTTP status $status (code=$safeCode)."
    }
    [pscustomobject]@{ status = $status; body = $body }
}
function Get-OccurrenceById($Response, [string]$QueryId) {
    $matches = @($Response.rows | Where-Object { $_.query.query_id -eq $QueryId }); Require ($matches.Count -eq 1) "Expected exactly one Top row for $QueryId."; $matches[0]
}
function Get-MetricById($Occurrence, [string]$MetricId) {
    $matches = @($Occurrence.metrics | Where-Object { $_.metric_id -eq $MetricId }); Require ($matches.Count -eq 1) "Expected exactly one metric $MetricId."; $matches[0]
}
function Assert-Coverage($Coverage) {
    foreach ($name in @('raw_row_count', 'invalid_query_id_count', 'exact_duplicate_row_count', 'conflicting_duplicate_row_count', 'retained_event_count', 'retained_family_count', 'missing_digest_count', 'missing_context_count')) { Require ([string]$Coverage.$name -eq [string]$oracle[$name]) "Coverage $name drifted from the independent oracle." }
    $expectedMetrics = @{ query_time = $oracle.query_time; scan_bytes = $oracle.scan_bytes; cpu_cost = $oracle.cpu_cost; memory_cost = $oracle.memory_cost }
    foreach ($metric in @($Coverage.metric_inputs)) {
        $expected = $expectedMetrics[$metric.metric_id]; Require ($null -ne $expected) "Unexpected coverage metric $($metric.metric_id)."
        Require ([string]$metric.sql_null_count -eq $expected.sql_null) "Coverage null count drifted for $($metric.metric_id)."
        Require ([string]$metric.text_present_count -eq $expected.text) "Coverage text count drifted for $($metric.metric_id)."
        Require ([string]$metric.nonnegative_count -eq $expected.nonnegative) "Coverage nonnegative count drifted for $($metric.metric_id)."
        Require ([string]$metric.negative_count -eq $expected.negative) "Coverage negative count drifted for $($metric.metric_id)."
        Require ([string]$metric.aggregate.aggregation -eq $expected.aggregation) "Coverage aggregation drifted for $($metric.metric_id)."
        Require ([string]$metric.aggregate.metric.input.value -eq $expected.aggregate) "Coverage aggregate input drifted for $($metric.metric_id)."
    }
    Require (@($Coverage.metric_inputs).Count -eq 4) 'Coverage metric count drifted.'
}

$startedAtUtc = [DateTime]::UtcNow.ToString('o')
$resolvedOutputDirectory = if (Test-Path -LiteralPath $OutputDirectory) { (Resolve-Path -LiteralPath $OutputDirectory).Path } else { $OutputDirectory }
$runDirectory = Join-Path $resolvedOutputDirectory ("audit-workload-{0}-{1}" -f [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'), $PID)
New-Item -ItemType Directory -Force -Path $runDirectory | Out-Null
$sourceSqlPath = Join-Path $runDirectory 'fixture-source.sql'; $sourceShaPath = Join-Path $runDirectory 'fixture-source.sha256'; $receiptPath = Join-Path $runDirectory 'receipt.json'
$sourceSql = New-FixtureSql
[IO.File]::WriteAllText($sourceSqlPath, $sourceSql, [Text.UTF8Encoding]::new($false))
$sourceSqlSha256 = Get-FileSha256 $sourceSqlPath
[IO.File]::WriteAllText($sourceShaPath, "$sourceSqlSha256  fixture-source.sql`n", [Text.UTF8Encoding]::new($false))
$apiExecutableSha256 = $null
if (-not [string]::IsNullOrWhiteSpace($ApiExecutablePath)) {
    if (-not (Test-Path -LiteralPath $ApiExecutablePath -PathType Leaf)) { throw 'ApiExecutablePath is not a file.' }
    $apiExecutableSha256 = Get-FileSha256 ((Resolve-Path -LiteralPath $ApiExecutablePath).Path)
}

try {
    $stage = 'validate_container'
    $container = (& docker inspect $ContainerName 2>$null | ConvertFrom-Json)
    Require ($container.Config.Image -eq $fixedImage) 'Container image is not the fixed StarRocks 3.5.14 digest.'
    Require ($container.Config.Labels.($fixedLabelName) -eq $fixedLabelValue) 'Container e2e label is invalid.'
    Require ([bool]$container.State.Running) 'Container is not running.'
    $bindings = @($container.NetworkSettings.Ports.'9030/tcp')
    Require (@($bindings | Where-Object { $_.HostIp -eq '127.0.0.1' -and [int]$_.HostPort -eq $MySqlPort }).Count -eq 1) 'Container has no expected loopback MySQL port binding.'
    $password = if ([string]::IsNullOrWhiteSpace($PasswordEnv)) { '' } else { [Environment]::GetEnvironmentVariable($PasswordEnv, 'Process') }
    if ($null -eq $password) { throw 'Password environment variable is not set.' }

    # Creating the explicitly named fixture database is non-destructive. Before
    # replacing its task-owned table, require an existing table to prove the
    # exact historical 14-column or current 15-column task source shape.
    Invoke-OracleMysql -Sql "CREATE DATABASE IF NOT EXISTS $fixtureDatabase;" -Password $password | Out-Null
    $stage = 'verify_existing_fixture_source'
    $existingFixture = @(Invoke-OracleMysql -Sql "SHOW TABLES FROM $fixtureDatabase LIKE '$fixtureTable';" -Password $password)
    if ($existingFixture.Count -gt 0) {
        Require ($existingFixture.Count -eq 1 -and $existingFixture[0].Trim() -eq $fixtureTable) 'Existing table is not the isolated task fixture.'
        $priorFields = @(Invoke-OracleMysql -Sql "SHOW COLUMNS FROM $fixtureQualifiedTable;" -Password $password | ForEach-Object { ($_ -split "`t")[0] })
        Require (($priorFields -join ',') -in @(($expectedColumns -join ','), ($legacyFixtureColumns -join ','))) 'Existing fixture schema does not prove task ownership; it will not be replaced.'
        $priorSource = @(Invoke-OracleMysql -Sql "SHOW CREATE TABLE $fixtureQualifiedTable;" -Password $password) -join "`n"
        Require ($priorSource -match 'DUPLICATE KEY' -and $priorSource -match 'queryId') 'Existing fixture source is not the expected duplicate-key audit oracle; it will not be replaced.'
    }
    $stage = 'seed_synthetic_fixture'; Invoke-OracleMysql -Sql $sourceSql -Password $password | Out-Null
    $schemaFields = @(Invoke-OracleMysql -Sql "SHOW COLUMNS FROM $fixtureQualifiedTable;" -Password $password | ForEach-Object { ($_ -split "`t")[0] })
    Require (($schemaFields -join ',') -eq ($expectedColumns -join ',')) 'Fixture schema is not the expected 15-column audit shape with explicit user.'
    $rawWindowRows = @(Invoke-OracleMysql -Sql (New-HalfOpenWindowCountSql -QualifiedTable $fixtureQualifiedTable -WindowStart $windowStart -WindowEnd $windowEnd) -Password $password)
    Require ($rawWindowRows.Count -eq 1 -and $rawWindowRows[0].Trim() -eq $oracle.raw_row_count) 'Independent source SQL window count drifted.'

    $stage = 'connect_api'; $capabilities = (Invoke-DoctorApi -Method Get -Path '/capabilities' -Payload $null).body
    Require ($null -ne $capabilities.csrf.nonce) 'API CSRF nonce is unavailable.'; $script:csrfHeaders = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }
    $session = (Invoke-DoctorApi -Method Post -Path '/connections' -Payload @{ host = '127.0.0.1'; port = $MySqlPort; username = $Username; password = $password; catalog = $null; database = $fixtureDatabase; audit_table = $fixtureQualifiedTable; tls_mode = 'docker_loopback_plaintext' }).body
    $connectionId = [string]$session.connection_id
    Require (-not [string]::IsNullOrWhiteSpace($connectionId)) 'API did not return a connection ID.'
    Require ($session.detected_version.value -match '^3\.5\.14(?:-|$)') 'API did not detect StarRocks 3.5.14.'
    Require ($session.capabilities.audit -eq 'available') 'API did not accept the verified audit schema.'
    Require ($session.capabilities.audit_user_filter -eq 'available') 'API did not expose the verified user filter capability.'
    # The synthetic oracle table has no isQuery column: the statement filter must not claim availability.
    Require ($session.capabilities.audit_statement_filter -eq 'unavailable') 'API claimed a statement filter without a proven isQuery column.'
    $scope = @{ window = @{ start_seconds = $windowStart; end_seconds = $windowEnd }; state = $null; user = $null; native_digest = $null; catalog = $null; database = $null; resource_group = $null; statement_filter = 'all' }

    $stage = 'top_50'; $top50 = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $scope; sort = 'query_time_desc'; limit = 50 } }).body
    Require ([int]$top50.top.requested_limit -eq 50 -and [int]$top50.top.returned_count -eq 50) 'Top 50 returned an incorrect row count.'
    Require ([string]$top50.top.full_window_count -eq $oracle.retained_event_count -and [bool]$top50.top.truncated) 'Top 50 was not based on the complete canonical window.'
    Assert-Coverage $top50.coverage
    Require ($top50.rows[0].query.query_id -eq '00000000-0000-0000-0000-000000000001') 'The oldest slowest event did not win server-side Top ordering.'
    Require (@($top50.rows | Where-Object { $_.query.query_id -eq '00000000-0000-0000-0000-000000000700' }).Count -eq 0) 'The end-boundary event leaked into the half-open Top window.'
    Require (($top50 | ConvertTo-Json -Depth 64 -Compress) -notmatch '"sql"\s*:') 'Top response leaked source SQL.'
    $q2 = Get-OccurrenceById $top50 '00000000-0000-0000-0000-000000000002'; $q3 = Get-OccurrenceById $top50 '00000000-0000-0000-0000-000000000003'; $q4 = Get-OccurrenceById $top50 '00000000-0000-0000-0000-000000000004'; $q5 = Get-OccurrenceById $top50 '00000000-0000-0000-0000-000000000005'; $q6 = Get-OccurrenceById $top50 '00000000-0000-0000-0000-000000000006'
    $q2Scan = Get-MetricById $q2 'scan_bytes'; $q3Scan = Get-MetricById $q3 'scan_bytes'; $q4Rows = Get-MetricById $q4 'scan_rows'; $q5Cpu = Get-MetricById $q5 'cpu_cost'; $q6Memory = Get-MetricById $q6 'memory_cost'
    Require ($q2Scan.input.state -eq 'sql_null' -and $q2Scan.state -eq 'missing') 'SQL NULL scanBytes did not remain typed missing.'
    Require ($q3Scan.input.state -eq 'text' -and $q3Scan.state -eq 'exact' -and $q3Scan.canonical_number.value -eq '0') 'Numeric zero scanBytes did not remain exact zero.'
    Require ($q4Rows.state -eq 'precision_unsafe' -and $q4Rows.canonical_number.value -eq '9007199254740992') 'Large scanRows did not remain precision-unsafe decimal text.'
    Require ($q5Cpu.state -eq 'negative_sentinel') 'Negative CPU sentinel was accepted as a usable metric.'
    Require ($q6Memory.state -eq 'precision_unsafe' -and $q6Memory.canonical_number.value -eq '9223372036854775807') 'Signed BIGINT maximum memory metric lost its typed precision-unsafe state.'

    $stage = 'top_500_and_sql_lookup'; $top500 = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $scope; sort = 'query_time_desc'; limit = 500 } }).body; $generationTop500 = [int]$top500.discovery_generation
    Require ([int]$top500.top.returned_count -eq 500 -and [string]$top500.top.full_window_count -eq $oracle.retained_event_count -and [bool]$top500.top.truncated) 'Top 500 did not preserve the full-window count/truncation contract.'
    Require ($top500.rows[0].query.query_id -eq '00000000-0000-0000-0000-000000000001') 'Top 500 lost the oldest slowest event.'
    Require (@($top500.rows | Where-Object { $_.query.query_id -eq '00000000-0000-0000-0000-000000000700' }).Count -eq 0) 'End-boundary event entered Top 500.'
    $lookup = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/sql" -Payload @{ connection_id = $connectionId; discovery_generation = $generationTop500; scope = $scope; query_id = '00000000-0000-0000-0000-000000000001' }).body
    Require ($lookup.query_id -eq '00000000-0000-0000-0000-000000000001' -and $lookup.sql -eq 'SELECT audit_oracle_oldest_slowest') 'Current unique audit event did not permit exact SQL lookup.'
    $lookupSqlSha256 = Get-TextSha256 ([string]$lookup.sql)

    $stage = 'families_and_stale_generation'; $families = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/families" -Payload @{ connection_id = $connectionId; request = @{ scope = $scope; sort = 'cumulative_query_time_desc'; limit = 20 } }).body
    Require ([int]$families.top.returned_count -eq 2 -and [string]$families.top.full_window_count -eq $oracle.retained_family_count -and -not [bool]$families.top.truncated) 'Family Top metadata drifted from full-window coverage.'
    Assert-Coverage $families.coverage
    Require (($families | ConvertTo-Json -Depth 64 -Compress) -notmatch '"sql"\s*:') 'Family response leaked source SQL.'
    $familyContexts = @($families.rows | ForEach-Object { "$($_.membership.context.catalog)|$($_.membership.context.database)|$($_.membership.native_digest)" } | Sort-Object)
    Require (($familyContexts -join ',') -eq 'catalog_a|db_a|a0b1,catalog_b|db_b|a0b1') 'Same digest across catalog/database contexts was merged into one family.'
    Require ([int]$families.discovery_generation -gt $generationTop500) 'Family read did not issue a new discovery generation.'
    $staleAfterFamily = Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/sql" -Payload @{ connection_id = $connectionId; discovery_generation = $generationTop500; scope = $scope; query_id = '00000000-0000-0000-0000-000000000001' } -ExpectedStatus @(409)
    Require ($staleAfterFamily.status -eq 409) 'Family generation did not revoke the prior audit SQL lookup.'

    $stage = 'family_detail_and_explicit_sql'
    $selectedFamily = @($families.rows | Where-Object { $_.membership.context.catalog -eq 'catalog_a' })
    Require ($selectedFamily.Count -eq 1) 'Expected one exact catalog_a family.'
    $detailRequest = @{ connection_id = $connectionId; discovery_generation = $families.discovery_generation; scope = $scope; family_key = $selectedFamily[0].membership.family_key }
    $detail = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/family-detail" -Payload $detailRequest).body
    Require ([int]$detail.discovery_generation -eq ([int]$families.discovery_generation + 1)) 'Family detail did not publish exactly one generation.'
    Require ($detail.family.family_key -eq $detailRequest.family_key) 'Detail changed the selected family.'
    Require ($detail.distribution.state -eq 'exact' -and $detail.distribution.population_count -eq '597' -and $detail.distribution.eligible_count -eq '597') 'Detail population is not the complete canonical family.'
    Require (@($detail.distribution.buckets).Count -eq 8) 'Detail histogram must contain eight buckets.'
    $bucketTotal = 0L
    foreach ($bucket in $detail.distribution.buckets) { $bucketTotal += [long]$bucket.count }
    Require ($bucketTotal -eq 597 -and $detail.distribution.exclusions.sql_null_count -eq '0' -and $detail.distribution.exclusions.negative_count -eq '0' -and $detail.distribution.exclusions.overflow_count -eq '0') 'Detail histogram count conservation failed.'
    Require ($detail.representatives.recent.state -eq 'selected' -and $detail.representatives.recent.occurrence.query.query_id -eq '00000000-0000-0000-0000-000000000007') 'Recent representative is not the latest eligible family event.'
    Require ($detail.representatives.slowest.state -eq 'selected' -and $detail.representatives.slowest.occurrence.query.query_id -eq '00000000-0000-0000-0000-000000000001') 'Slowest representative lost the oldest slow query.'
    Require (($detail | ConvertTo-Json -Depth 64 -Compress) -notmatch '"sql"\s*:') 'Detail response leaked SQL.'
    $staleDetail = Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/family-detail" -Payload $detailRequest -ExpectedStatus @(409)
    Require ($staleDetail.status -eq 409) 'Prior detail generation was not rejected.'
    $stage = 'family_detail_explicit_sql_lookup'
    Require ($detail.scope.native_digest -eq 'a0b1' -and $detail.scope.catalog -eq 'catalog_a' -and $detail.scope.database -eq 'db_a') 'Detail binding is not the exact selected family scope.'
    $detailSql = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/sql" -Payload @{ connection_id = $connectionId; discovery_generation = $detail.discovery_generation; scope = $detail.scope; query_id = $detail.representatives.slowest.occurrence.query.query_id }).body
    Require ($detailSql.sql -eq 'SELECT audit_oracle_oldest_slowest') 'Detail SQL lookup did not preserve the parent scope after stale rejection.'

    $stage = 'user_and_state_scope'
    $userScope = $scope.Clone(); $userScope.user = 'fixture_user_b'
    $userTop = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $userScope; sort = 'query_time_desc'; limit = 500 } }).body
    Require ($userTop.scope.user -eq 'fixture_user_b' -and [string]$userTop.top.full_window_count -eq '300' -and [int]$userTop.top.returned_count -eq 300) 'User filter did not isolate its complete retained event window.'
    Require ([string]$userTop.coverage.raw_row_count -eq '302' -and [string]$userTop.coverage.exact_duplicate_row_count -eq '2') 'User-filtered coverage was not scoped before Top.'
    $foreignScope = $userScope.Clone(); $foreignScope.user = 'fixture_user_a'
    $wrongUserLookup = Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/sql" -Payload @{ connection_id = $connectionId; discovery_generation = $userTop.discovery_generation; scope = $foreignScope; query_id = '00000000-0000-0000-0000-000000000002' } -ExpectedStatus @(404)
    Require ($wrongUserLookup.status -eq 404 -and $wrongUserLookup.body.code -eq 'query_sql_unavailable' -and $wrongUserLookup.body.details.limitation -eq 'audit_scope_mismatch') 'SQL lookup accepted a different user scope in the same generation.'
    $userFamilies = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/families" -Payload @{ connection_id = $connectionId; request = @{ scope = $userScope; sort = 'execution_count_desc'; limit = 20 } }).body
    Require ($userFamilies.scope.user -eq 'fixture_user_b' -and [string]$userFamilies.coverage.retained_event_count -eq '300' -and [int]$userFamilies.top.returned_count -eq 2) 'Family query or coverage lost the explicit user scope.'
    $errorScope = $userScope.Clone(); $errorScope.state = 'error'
    $errorTop = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $errorScope; sort = 'query_time_desc'; limit = 50 } }).body
    Require ($errorTop.scope.state -eq 'error' -and [string]$errorTop.top.full_window_count -eq '2' -and [int]$errorTop.top.returned_count -eq 2) 'Combined user/state filter did not retain exactly two error events.'
    $errorIds = @($errorTop.rows | ForEach-Object { $_.query.query_id } | Sort-Object)
    Require (($errorIds -join ',') -eq '00000000-0000-0000-0000-000000000020,00000000-0000-0000-0000-000000000022') 'Combined user/state query returned unexpected identities.'
    Require ([string]$errorTop.coverage.raw_row_count -eq '2' -and [string]$errorTop.coverage.retained_event_count -eq '2') 'Error-filtered coverage was not bound to the same scope.'
    $finishedScope = $userScope.Clone(); $finishedScope.state = 'finished'
    $finishedTop = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $finishedScope; sort = 'query_time_desc'; limit = 500 } }).body
    Require ([string]$finishedTop.top.full_window_count -eq '298' -and [int]$finishedTop.top.returned_count -eq 298) 'Finished-state mapping did not exclude ERR events.'
    $emptyScope = $scope.Clone(); $emptyScope.user = "fixture_user_b' OR 1=1 --"
    $emptyTop = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $emptyScope; sort = 'query_time_desc'; limit = 50 } }).body
    Require ([string]$emptyTop.top.full_window_count -eq '0' -and [int]$emptyTop.top.returned_count -eq 0) 'User filter was not an exact literal or the empty result was misreported.'

    $stage = 'legacy_search_stales_audit'; $freshTop = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/occurrences" -Payload @{ connection_id = $connectionId; request = @{ scope = $scope; sort = 'query_time_desc'; limit = 50 } }).body; $freshGeneration = [int]$freshTop.discovery_generation
    $legacySearch = (Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/queries/search" -Payload @{ connection_id = $connectionId; limit = 50; status = $null; digest = $null }).body
    Require ([int]$legacySearch.discovery_generation -gt $freshGeneration) 'Legacy search did not replace the audit discovery generation.'
    $staleAfterSearch = Invoke-DoctorApi -Method Post -Path "/connections/$connectionId/audit/sql" -Payload @{ connection_id = $connectionId; discovery_generation = $freshGeneration; scope = $scope; query_id = '00000000-0000-0000-0000-000000000001' } -ExpectedStatus @(404)
    Require ($staleAfterSearch.status -eq 404 -and $staleAfterSearch.body.code -eq 'query_sql_unavailable' -and $staleAfterSearch.body.details.limitation -eq 'audit_identity_not_discovered') 'Legacy search did not revoke the audit discovery.'
    $runStatus = 'PASS'
}
catch { $failure = $_.Exception.Message }
finally {
    $stageAtClose = $stage
    if ($null -ne $connectionId) {
        try { Invoke-DoctorApi -Method Delete -Path "/connections/$connectionId" -Payload $null -ExpectedStatus @(200, 204) | Out-Null }
        catch {
            if ($runStatus -eq 'PASS') {
                $runStatus = 'FAIL'
                $stage = 'cleanup_session'
                $failure = 'Doctor API connection cleanup failed.'
            }
        }
    }
    $password = $null; $finishedAtUtc = [DateTime]::UtcNow.ToString('o')
    $branch = try { (& git branch --show-current 2>$null | Select-Object -First 1).Trim() } catch { $null }
    $head = try { (& git rev-parse HEAD 2>$null | Select-Object -First 1).Trim() } catch { $null }
    $dirtyEntries = try { @(& git status --porcelain 2>$null | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) } catch { @() }
    $receipt = [ordered]@{
        schema_version = 'audit-workload-smoke.v1'; status = $runStatus; stage = $stageAtClose; failure = $failure; started_at_utc = $startedAtUtc; finished_at_utc = $finishedAtUtc; assertions = $script:assertionCount
        git = [ordered]@{ branch = $branch; head = $head; dirty = ($dirtyEntries.Count -gt 0); dirty_entry_count = $dirtyEntries.Count }
        build_identity = [ordered]@{ expected_starrocks = '3.5.14'; api_capabilities_schema_version = if ($null -ne $capabilities) { $capabilities.schema_version } else { $null }; detected_version = if ($null -ne $session) { $session.detected_version.value } else { $null }; api_executable_path = if ([string]::IsNullOrWhiteSpace($ApiExecutablePath)) { $null } else { (Resolve-Path -LiteralPath $ApiExecutablePath).Path }; api_executable_sha256 = $apiExecutableSha256 }
        container = [ordered]@{ name = $ContainerName; expected_image = $fixedImage; mysql_host_port = $MySqlPort }
        fixture = [ordered]@{ source_kind = 'synthetic_source_data_oracle_on_real_starrocks'; database = $fixtureDatabase; table = $fixtureTable; source_sql_file = 'fixture-source.sql'; source_sql_sha256 = $sourceSqlSha256; expected_window_raw_rows = $oracle.raw_row_count; expected_retained_events = $oracle.retained_event_count; expected_retained_families = $oracle.retained_family_count; preserved_after_run = $true }
        explicit_sql_lookup = [ordered]@{ sha256 = $lookupSqlSha256; raw_sql_persisted = $false }
        limitations = @('not_auditloader_ingestion', 'not_profile_or_business_sql_e2e', 'fixture_only_not_production', 'coverage_and_top_are_separate_non_atomic_reads')
    }
    [IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json -Depth 16), [Text.UTF8Encoding]::new($false))
}
if ($runStatus -ne 'PASS') { throw "Audit workload smoke failed at ${stage}: $failure" }
Write-Output "PASS assertions=$script:assertionCount receipt=$receiptPath fixture_sha256=$sourceSqlSha256"
