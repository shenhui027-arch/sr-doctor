[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)][int]$ApiPort = 18065,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39631,
    [switch]$SkipBuild,
    # Keep the fixture and API up this long after the checks so a browser can
    # sign in through serve-password-once.mjs (127.0.0.1:18099) and capture
    # boards I1-I4. GET http://127.0.0.1:18099/release ends the hold.
    [ValidateRange(0, 1800)][int]$HoldForBrowserSeconds = 0,
    # Task 52: also install the official AuditLoader (cached archive, fixed
    # SHA-256) so the audit overview and its load card (board O3) have data.
    # The reader then holds SELECT on the audit table only.
    [switch]$WithAuditLoader
)

# Task 51 (LD-05): the load workspace against official StarRocks 3.5.14
# (fixed digest) with a local Kafka (fixed digest) sharing its network.
# Real Stream Loads (clean, partly filtered, failed) and one Routine Load
# with a bad message are read through the product API by a reader with no
# grants (with -WithAuditLoader: SELECT on the audit table only). Task 52
# adds the overview (LD-06). Serial, bounded, torn down in `finally`; test
# data only.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "load-workspace-$stamp.json"
$starrocksImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$kafkaImage = 'apache/kafka:3.9.0@sha256:fbc7d7c428e3755cf36518d4976596002477e4c052d1f80b5b9eafd06d0fff2f'
$srName = 'sr-doctor-load-e2e-3514'
$kafkaName = 'sr-doctor-load-e2e-kafka'
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$stage = 'initialization'
$summary = [ordered]@{ project = 'load-workspace-l0'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o'); starrocks_image = $starrocksImage; kafka_image = $kafkaImage }
$startedApi = $null; $primaryFailure = $false; $kafkaPulled = $false
# Only containers this run created (by the id `docker run` returned) are
# removed in `finally`; anything else under the same names is left alone.
$srId = $null; $kafkaId = $null
$readerPassword = 'Ld_' + [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(10)).ToLowerInvariant()
$secretLiteral = 'load_e2e_secret_row_value'
$auditWriterPassword = 'Aw_' + [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(10)).ToLowerInvariant()
$auditLoaderSha256 = 'cd2a8aceb7943675192e80d7a6d02d675170ab3beba2d0d2f49b5bc8955a8ea2'

function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Test-PortListening([int]$Port) { $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) }
function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Invoke-InContainer([string]$Name, [string]$Script) {
    # Base64 keeps Windows line endings out of the shell script and its heredocs.
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($Script -replace "`r", '')))
    $output = & docker exec $Name sh -c "echo $encoded | base64 -d | sh" 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Command in $Name failed: $output" }
    $output
}
function Invoke-Sql([string]$Sql) { Invoke-InContainer $srName "mysql -h127.0.0.1 -P9030 -uroot -N --raw <<'SQL'`n$Sql`nSQL" }
function Invoke-Post([string]$Path, $Payload) {
    $response = Invoke-WebRequest "$apiBase$Path" -Method Post -Headers $script:headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 10 -Compress) -UseBasicParsing
    [ordered]@{ raw = $response.Content; body = ($response.Content | ConvertFrom-Json) }
}
function Container-Exists([string]$Name) { [bool](& docker ps -a --filter "name=^/$Name$" --format '{{.Names}}') }

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $summary.runner_sha256 = Get-Sha256 $PSCommandPath

    $stage = 'cold_start_preconditions'
    Require (-not (Container-Exists $srName) -and -not (Container-Exists $kafkaName)) 'Load E2E requires a cold start (containers present).'
    Require (-not (Test-PortListening $ApiPort) -and -not (Test-PortListening $MySqlPort)) 'Load E2E ports are in use.'

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
    $srId = (& docker run -d --name $srName --memory 4g -p "127.0.0.1:${MySqlPort}:9030" $starrocksImage | Select-Object -Last 1)
    if ($LASTEXITCODE -ne 0 -or $srId -notmatch '^[0-9a-f]{64}$') { $srId = $null; throw 'StarRocks container did not start.' }
    $kafkaPulled = -not [bool](& docker images -q $kafkaImage)
    $kafkaId = (& docker run -d --name $kafkaName --network "container:$srName" --memory 1g `
        -e KAFKA_NODE_ID=1 -e KAFKA_PROCESS_ROLES=broker,controller -e KAFKA_LISTENERS=PLAINTEXT://127.0.0.1:9092,CONTROLLER://127.0.0.1:9093 `
        -e KAFKA_ADVERTISED_LISTENERS=PLAINTEXT://127.0.0.1:9092 -e KAFKA_CONTROLLER_LISTENER_NAMES=CONTROLLER `
        -e KAFKA_LISTENER_SECURITY_PROTOCOL_MAP=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT -e KAFKA_CONTROLLER_QUORUM_VOTERS=1@127.0.0.1:9093 `
        -e KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR=1 -e KAFKA_NUM_PARTITIONS=2 $kafkaImage | Select-Object -Last 1)
    if ($LASTEXITCODE -ne 0 -or $kafkaId -notmatch '^[0-9a-f]{64}$') { $kafkaId = $null; throw 'Kafka container did not start.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(300)
    do { Start-Sleep -Seconds 3; $ready = (& docker exec $srName sh -c "mysql -h127.0.0.1 -P9030 -uroot -N -e 'CREATE DATABASE IF NOT EXISTS lr' 2>/dev/null && echo ok") -eq 'ok' } while (-not $ready -and [DateTime]::UtcNow -lt $deadline)
    Require $ready 'StarRocks did not become ready.'
    $summary.observed_version = (Invoke-Sql 'SELECT current_version();' | Select-Object -Last 1).Trim()

    $stage = 'workload'
    Invoke-Sql @"
CREATE TABLE lr.t (id INT NOT NULL, name VARCHAR(5), v INT NOT NULL, dt DATE) DUPLICATE KEY(id) PARTITION BY RANGE(dt) (PARTITION p1 VALUES [('2026-01-01'), ('2026-02-01'))) DISTRIBUTED BY HASH(id) BUCKETS 1 PROPERTIES ('replication_num' = '1');
CREATE TABLE lr.k (id INT, name VARCHAR(16), v INT) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 2 PROPERTIES ('replication_num' = '1');
CREATE USER load_reader IDENTIFIED BY '$readerPassword';
"@ | Out-Null
    if ($WithAuditLoader) {
        $stage = 'audit_loader'
        $auditArchive = Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'cache\auditloader-5.0.0.zip'
        Require (Test-Path -LiteralPath $auditArchive -PathType Leaf) 'The AuditLoader archive is not cached; run dev/starrocks-3.5.14/bootstrap.ps1 once.'
        Require ((Get-Sha256 $auditArchive) -eq $auditLoaderSha256) 'The cached AuditLoader archive SHA-256 does not match.'
        Invoke-Sql @"
CREATE DATABASE IF NOT EXISTS starrocks_audit_db__;
CREATE TABLE IF NOT EXISTS starrocks_audit_db__.starrocks_audit_tbl__ (
  ``queryId`` VARCHAR(64), ``timestamp`` DATETIME NOT NULL, ``queryType`` VARCHAR(12), ``clientIp`` VARCHAR(32),
  ``user`` VARCHAR(64), ``authorizedUser`` VARCHAR(64), ``resourceGroup`` VARCHAR(64), ``catalog`` VARCHAR(32),
  ``db`` VARCHAR(96), ``state`` VARCHAR(8), ``errorCode`` VARCHAR(512), ``queryTime`` BIGINT,
  ``scanBytes`` BIGINT, ``scanRows`` BIGINT, ``returnRows`` BIGINT, ``cpuCostNs`` BIGINT, ``memCostBytes`` BIGINT,
  ``stmtId`` INT, ``isQuery`` TINYINT, ``feIp`` VARCHAR(128), ``stmt`` VARCHAR(1048576), ``digest`` VARCHAR(32),
  ``planCpuCosts`` DOUBLE, ``planMemCosts`` DOUBLE, ``pendingTimeMs`` BIGINT, ``candidateMVs`` VARCHAR(65533),
  ``hitMvs`` VARCHAR(65533), ``QueriedRelations`` ARRAY<VARCHAR(65533)>, ``warehouse`` VARCHAR(32)
) ENGINE=OLAP DUPLICATE KEY (``queryId``, ``timestamp``, ``queryType``)
PARTITION BY date_trunc('day', ``timestamp``)
PROPERTIES ('replication_num'='1', 'partition_live_number'='30');
CREATE USER audit_writer IDENTIFIED BY '$auditWriterPassword';
GRANT INSERT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER audit_writer;
GRANT SELECT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER load_reader;
"@ | Out-Null
        $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('sr-doctor-load-auditloader-' + [Guid]::NewGuid().ToString('N'))
        try {
            $contents = Join-Path $tempRoot 'contents'
            New-Item -ItemType Directory -Force -Path $contents | Out-Null
            Expand-Archive -LiteralPath $auditArchive -DestinationPath $contents -Force
            $configPath = Join-Path $contents 'plugin.conf'
            $config = Get-Content -LiteralPath $configPath -Raw
            # allin1 redirects 8030 Stream Load to its local 8040, dropping Basic
            # authentication; this fixture-only value avoids the redirect.
            foreach ($pair in @{ max_batch_interval_sec = '5'; frontend_host_port = '127.0.0.1:8040'; database = 'starrocks_audit_db__'; table = 'starrocks_audit_tbl__'; user = 'audit_writer'; password = $auditWriterPassword; secret_key = '' }.GetEnumerator()) {
                $config = [regex]::Replace($config, "(?m)^$([regex]::Escape($pair.Key))=.*$", "$($pair.Key)=$($pair.Value)")
            }
            [IO.File]::WriteAllText($configPath, $config, [Text.UTF8Encoding]::new($false))
            $configured = Join-Path $tempRoot 'auditloader-configured.zip'
            Compress-Archive -Path (Join-Path $contents '*') -DestinationPath $configured -CompressionLevel Optimal
            & docker cp $configured "${srName}:/tmp/auditloader-configured.zip" | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'The AuditLoader archive could not be copied into the container.' }
            Invoke-Sql 'INSTALL PLUGIN FROM "/tmp/auditloader-configured.zip";' | Out-Null
            Require ((Invoke-Sql 'SHOW PLUGINS;') -join "`n" -match '(?m)^AuditLoader\s+AUDIT\s+.*\s+5\.0\.0\s+') 'AuditLoader 5.0.0 was not installed.'
        } finally { if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force } }
        $summary.audit_loader = [ordered]@{ version = '5.0.0'; sha256 = $auditLoaderSha256 }
        # 3.5.14 refreshes active audit plugins every 60 s.
        Start-Sleep -Seconds 65
        $stage = 'workload'
    }
    $loads = Invoke-InContainer $srName @"
printf '1\tok\t1\t2026-01-05\n2\tok2\t2\t2026-01-06\n' > /tmp/ok.tsv
printf '3\tfine\t3\t2026-01-05\nx\t$secretLiteral\t2\t2026-01-05\n4\tnul\t\\N\t2026-01-05\n5\tpart\t5\t2027-01-01\n' > /tmp/mixed.tsv
for spec in 'ld_ok:/tmp/ok.tsv:0' 'ld_partial:/tmp/mixed.tsv:0.9' 'ld_failed:/tmp/mixed.tsv:0'; do
  label=`${spec%%:*}; rest=`${spec#*:}; file=`${rest%%:*}; ratio=`${rest#*:}
  curl -s --location-trusted -u root: -H "label:`$label" -H 'strict_mode:true' -H "max_filter_ratio:`$ratio" -H 'columns: id,name,v,dt' -T `$file http://127.0.0.1:8030/api/lr/t/_stream_load | tr -d '\n' | sed 's/ //g'; echo
done
"@
    $summary.stream_loads = @($loads | ForEach-Object { if ($_ -match '"Label":"([^"]+)".*"Status":"([^"]+)".*"NumberFilteredRows":(\d+)') { "$($Matches[1]):$($Matches[2]):$($Matches[3])" } })
    Require ($summary.stream_loads -contains 'ld_ok:Success:0' -and $summary.stream_loads -contains 'ld_partial:Success:3' -and $summary.stream_loads -contains 'ld_failed:Fail:3') "Unexpected Stream Load results: $($summary.stream_loads -join ', ')"
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    do { Start-Sleep -Seconds 3; $topic = (& docker exec $kafkaName /opt/kafka/bin/kafka-topics.sh --bootstrap-server 127.0.0.1:9092 --create --if-not-exists --topic ld --partitions 2 --replication-factor 1 2>&1) } while ($LASTEXITCODE -ne 0 -and [DateTime]::UtcNow -lt $deadline)
    Require ($LASTEXITCODE -eq 0) 'Kafka topic could not be created.'
    Invoke-InContainer $kafkaName "for i in `$(seq 1 200); do echo `"`$((1000+i)),k`$i,`$i`"; done > /tmp/m.csv; echo 'bad,row,x' >> /tmp/m.csv; /opt/kafka/bin/kafka-console-producer.sh --bootstrap-server 127.0.0.1:9092 --topic ld < /tmp/m.csv" | Out-Null
    Invoke-Sql @"
CREATE ROUTINE LOAD lr.ld_kafka ON k COLUMNS TERMINATED BY ',', COLUMNS (id, name, v)
PROPERTIES ('desired_concurrent_number' = '1', 'max_error_number' = '10', 'strict_mode' = 'true', 'max_batch_interval' = '5')
FROM KAFKA ('kafka_broker_list' = '127.0.0.1:9092', 'kafka_topic' = 'ld', 'property.kafka_default_offsets' = 'OFFSET_BEGINNING');
"@ | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    do { Start-Sleep -Seconds 3; $stats = (Invoke-Sql "SELECT STATISTICS FROM information_schema.routine_load_jobs WHERE NAME = 'ld_kafka';" | Select-Object -Last 1) } while ($stats -notmatch '"loadedRows":200' -and [DateTime]::UtcNow -lt $deadline)
    Require ($stats -match '"loadedRows":200') "Routine Load did not load 200 rows: $stats"
    # Task 56: one INSERT that writes and one that strict mode cancels, for the INSERT view.
    Invoke-Sql "INSERT INTO lr.t WITH LABEL ld_insert_ok SELECT id + 100, name, v, dt FROM lr.t;" | Out-Null
    Invoke-InContainer $srName "mysql -h127.0.0.1 -P9030 -uroot -N -e `"INSERT INTO lr.t WITH LABEL ld_insert_bad (id, name, v, dt) VALUES (900, 'much-too-long', 1, '2026-01-05')`" >/dev/null 2>&1 || true" | Out-Null
    if ($WithAuditLoader) {
        $stage = 'audit_workload'
        foreach ($sql in 'SELECT COUNT(*) FROM lr.k;', 'SELECT name, SUM(v) FROM lr.k GROUP BY name ORDER BY 2 DESC LIMIT 5;', 'SELECT COUNT(*) FROM lr.t;', 'SELECT id FROM lr.k WHERE v > 100 ORDER BY id LIMIT 3;') {
            foreach ($round in 1..3) { Invoke-Sql $sql | Out-Null }
        }
        Invoke-InContainer $srName "mysql -h127.0.0.1 -P9030 -uroot -N -e 'SELECT * FROM lr.missing_table' >/dev/null 2>&1 || true" | Out-Null
        $deadline = [DateTime]::UtcNow.AddSeconds(120)
        do { Start-Sleep -Seconds 3; $audited = [int]((Invoke-Sql "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE db = 'lr' OR stmt LIKE '%lr.%';" | Select-Object -Last 1).Trim()) } while ($audited -lt 12 -and [DateTime]::UtcNow -lt $deadline)
        $summary.audit_loader.audited_rows = $audited
        Require ($audited -ge 12) "AuditLoader wrote only $audited rows."
    }

    $stage = 'api_start'
    $saved = @{}
    $environment = [ordered]@{ DOCTOR_PORT = [string]$ApiPort; DOCTOR_HISTORY = 'off' }
    $environment.DOCTOR_FRONTEND_DIST = if ($HoldForBrowserSeconds -gt 0) { (Resolve-Path -LiteralPath (Join-Path $repoRoot 'frontend\dist')).Path } else { $null }
    foreach ($key in $environment.Keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process'); [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process') }
    try {
        $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-load-$stamp.stdout.log") -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-load-$stamp.stderr.log")
    } finally { foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process') } }
    $deadline = [DateTime]::UtcNow.AddSeconds(45); $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) { try { $capabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 } }
    Require ($null -ne $capabilities) 'The local Doctor API did not become ready.'
    $script:headers = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }

    $stage = 'connect_reader_without_grants'
    $connection = (Invoke-Post '/connections' @{ host = '127.0.0.1'; port = $MySqlPort; username = 'load_reader'; password = $readerPassword; catalog = $null; database = $null; audit_table = $null; tls_mode = 'docker_loopback_plaintext' }).body
    $summary.connection_state = $connection.state
    Require ($connection.state -eq 'ready') "Connection is not ready: $($connection.state)"
    $id = $connection.connection_id

    $stage = 'ld01_routine_jobs'
    $routine = Invoke-Post "/connections/$id/loads/routine-jobs" @{ connection_id = $id }
    $job = @($routine.body.jobs | Where-Object { $_.name -eq 'ld_kafka' })[0]
    $summary.routine = [ordered]@{
        support = $routine.body.support; state = "$($job.state):$($job.state_class)"; topic = $job.topic
        partitions = @($job.partitions | ForEach-Object { "p$($_.partition) consumed=$($_.consumed) latest=$($_.latest) lag=$($_.lag)" })
        loaded = $job.statistics.loaded_rows; errors = $job.statistics.error_rows; max_error_number = $job.max_error_number; has_error_log = $job.has_error_log
        leaks_broker_list = $routine.raw.Contains('9092')
    }
    Require ($routine.body.support -eq 'full' -and $job.state_class -in @('running', 'scheduling') -and $job.topic -eq 'ld') 'The Routine Load job is not read as running.'
    Require ($job.partitions.Count -eq 2 -and $job.statistics.loaded_rows -eq '200' -and $job.statistics.error_rows -eq '1' -and $job.max_error_number -eq '10') 'Routine Load partitions or counters are wrong.'
    Require (-not $summary.routine.leaks_broker_list) 'The broker list left the server.'

    $stage = 'ld02_records'
    $records = Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = $null; outcome = $null; table = $null; include_system = $false }
    $byLabel = @{}; foreach ($record in $records.body.records) { $byLabel[$record.label] = $record }
    $summary.records = [ordered]@{
        matched = $records.body.matched; hidden_system = $records.body.hidden_system; fe_time_zone = $records.body.fe_time_zone
        rows = @($records.body.records | ForEach-Object { "$($_.kind):$($_.label):$($_.state):loaded=$($_.loaded_rows):filtered=$($_.filtered_rows):phases=$($_.begin_txn_ms)/$($_.plan_ms)/$($_.receive_data_ms):total=$($_.total_seconds):log=$($_.has_error_log)" })
    }
    Require ($byLabel.ContainsKey('ld_ok') -and $byLabel['ld_ok'].state -eq 'FINISHED' -and $byLabel['ld_ok'].loaded_rows -eq '2' -and $null -ne $byLabel['ld_ok'].plan_ms) 'The clean Stream Load is missing or incomplete.'
    Require ($byLabel['ld_partial'].filtered_rows -eq '3' -and $byLabel['ld_partial'].has_error_log) 'The partly filtered Stream Load lacks its filtered rows or error log.'
    Require ($byLabel['ld_failed'].state_class -eq 'cancelled' -and $byLabel['ld_failed'].error_message -like '*too many filtered rows*') 'The failed Stream Load is not shown as failed.'
    Require (@($records.body.records | Where-Object { $_.kind -eq 'ROUTINE_LOAD' }).Count -ge 1) 'No Routine Load task appears among the records.'
    $failedOnly = (Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = 'stream_load'; outcome = 'failed'; table = 'LR.T'; include_system = $false }).body
    Require (@($failedOnly.records | ForEach-Object { $_.label }) -join ',' -eq 'ld_failed') 'The failed/stream/table filters do not narrow to the failed load.'

    $stage = 'ld03_error_log'
    $log = Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $byLabel['ld_partial'].job_id }
    $summary.error_log = [ordered]@{ state = $log.body.state; categories = @($log.body.categories | ForEach-Object { "$($_.category):$($_.column):$($_.target_type):$($_.count)" }); entries = $log.body.entries.Count; converted = @($log.body.entries | Where-Object { $_.row_converted }).Count }
    Require ($log.body.state -eq 'available' -and $log.body.entries.Count -eq 3) 'The error log of the partly filtered load is not read.'
    Require (@($log.body.categories | ForEach-Object { $_.category }) -contains 'null_in_non_nullable' -and @($log.body.categories | ForEach-Object { $_.category }) -contains 'out_of_partition') 'Error categories are missing.'
    Require ($log.raw.Contains($secretLiteral)) 'The rejected row (business data) is not shown in the error log.'
    $routineLog = Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $job.job_id }
    $summary.routine_error_log = "$($routineLog.body.state):$($routineLog.body.entries.Count)"
    # The BE removes error logs after 48 hours; remove one now to see the product say so.
    $failedFiles = Invoke-InContainer $srName "ls /data/deploy/starrocks/be/storage/error_log/"
    Invoke-InContainer $srName "rm -f /data/deploy/starrocks/be/storage/error_log/error_log_*" | Out-Null
    $purged = Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $byLabel['ld_failed'].job_id }
    $summary.purged_error_log = $purged.body.state
    $summary.error_files_before_purge = @($failedFiles).Count
    Require ($purged.body.state -eq 'purged') 'A removed error log is not reported as purged.'
    $invalid = try { Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = '1 OR 1=1' } | Out-Null; 'accepted' } catch { [int]$_.Exception.Response.StatusCode }
    $summary.invalid_job_id = $invalid
    Require ($invalid -eq 400) 'A non-numeric job id was accepted.'

    $stage = 'ld06_overview'
    # Read the records again next to the overview: StarRocks may have added a task since LD-02.
    $recordsNow = (Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = $null; outcome = $null; table = $null; include_system = $false }).body
    $overview = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = $null; include_system = $false }).body
    $lastHour = $overview.hours[-1]
    $kafkaLags = @($overview.routine.lags | Where-Object { $_.job_name -eq 'ld_kafka' })
    $summary.overview = [ordered]@{
        loads = $overview.summary.loads; failed = $overview.summary.failed; failed_share_permyriad = $overview.failed_share_permyriad
        filtered_rows = $overview.summary.filtered_rows; filtered_rows_unknown = $overview.summary.filtered_rows_unknown; filtered_share_permyriad = $overview.summary.filtered_share_permyriad; outside_hours = $overview.records_outside_hours; hidden_system = $overview.hidden_system
        hours = $overview.hours.Count; last_hour = "$($lastHour.hour) loads=$($lastHour.loads) failed=$($lastHour.failed) height=$($lastHour.loads_permyriad)/$($lastHour.failed_permyriad)"; axis = $overview.hour_axis_ticks -join ','
        by_kind = @($overview.by_kind | ForEach-Object { "$($_.kind):$($_.loads)/$($_.failed)" })
        tables = @($overview.tables | ForEach-Object { "$($_.table):$($_.filtered_rows):$($_.width_permyriad)" })
        phases = "begin=$($overview.phases.begin_txn.samples) plan=$($overview.phases.plan.samples) bins=" + (@($overview.phases.bins | ForEach-Object { "$($_.begin_txn)/$($_.plan)" }) -join ',')
        routine = "running=$($overview.routine.running) paused=$($overview.routine.paused) lags=" + (@($overview.routine.lags | ForEach-Object { "$($_.job_name):p$($_.partition)=$($_.lag)" }) -join ',')
    }
    Require ($overview.support -eq 'full' -and $overview.summary.loads -eq $recordsNow.matched) 'The overview does not count the same records as the records tab.'
    Require ([int]$overview.summary.failed -ge 1 -and $null -ne $overview.failed_share_permyriad) 'The overview misses the failed Stream Load.'
    Require ($overview.hours.Count -ge 72 -and $overview.hours.Count -le 96 -and [int]$lastHour.loads -ge 1 -and $overview.hour_axis_ticks.Count -eq 3) 'The overview hours are not 72-96 buckets ending at the newest record.'
    Require ((@($overview.hours | ForEach-Object { [int]$_.loads } | Measure-Object -Sum).Sum + [int]$overview.records_outside_hours) -eq [int]$overview.summary.loads) 'Hour buckets do not add up to the load count.'
    Require ($overview.tables.Count -ge 1 -and $overview.tables[0].table -eq 'lr.t' -and $overview.tables[0].width_permyriad -eq 10000) 'The filtered-rows ranking does not lead with lr.t.'
    Require ($overview.routine.running -eq '1' -and $kafkaLags.Count -ge 1) 'The overview misses the running Routine Load or its lag.'
    Require ([int]$overview.phases.begin_txn.samples -ge 3) 'The phase distribution misses the Stream Loads.'
    if ($WithAuditLoader) { Require ([int]$overview.hidden_system -ge 1) 'AuditLoader loads are not hidden as system loads.' }
    $hourRecords = (Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = $null; outcome = $null; table = $null; include_system = $false; created_hour = $lastHour.hour }).body
    $summary.overview.last_hour_records = $hourRecords.matched
    Require ($hourRecords.matched -eq $lastHour.loads) 'The hour filter does not match the hour bucket.'
    $badHour = try { Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = $null; outcome = $null; table = $null; include_system = $false; created_hour = "$($lastHour.hour):00" } | Out-Null; 'accepted' } catch { [int]$_.Exception.Response.StatusCode }
    Require ($badHour -eq 400) 'A malformed hour was accepted.'

    $stage = 'ld09_types'
    # Task 56: each type shows only its own measures, from the same reads.
    $byGroup = @{}; foreach ($group in $overview.groups) { $byGroup[$group.group] = $group }
    $streamView = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = 'stream_load'; include_system = $false }).body
    $insertView = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = 'insert'; include_system = $false }).body
    $routineView = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = 'routine_load'; include_system = $false }).body
    $kafkaHealth = @($routineView.routine_detail.jobs | Where-Object { $_.name -eq 'ld_kafka' })[0]
    $insertRecords = (Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = 'insert'; outcome = $null; table = $null; include_system = $false; created_hour = $null }).body
    $summary.types = [ordered]@{
        groups = @($overview.groups | ForEach-Object { "$($_.group):$($_.loads)/$($_.failed) sink=$($_.sink_rows) p95=$($_.total_p95_seconds)" }); all_loads = $overview.all_loads
        stream = "bytes=$($streamView.stream.received_bytes) unit=$($streamView.volume_unit) receive=$($streamView.phases.receive_data.samples) publish_p95=$($streamView.stream.publish_p95_seconds) reasons=" + (@($streamView.stream.failure_reasons | ForEach-Object { "$($_.reason):$($_.loads)" }) -join ',') + " sources=" + (@($streamView.stream.sources | ForEach-Object { "$($_.client_ip)/$($_.user):$($_.loads)" }) -join ',')
        insert = "loads=$($insertView.summary.loads) failed=$($insertView.summary.failed) scan=$($insertView.batch.scan_rows)/$($insertView.batch.scan_bytes) samples=$($insertView.batch.duration.samples) reasons=" + (@($insertView.batch.failure_reasons | ForEach-Object { "$($_.reason):$($_.loads)" }) -join ',') + " slowest=" + (@($insertView.batch.slowest | ForEach-Object { "$($_.label):$($_.total_seconds)=$($_.execute_seconds)+$($_.publish_seconds)/$($_.timeout_seconds)" }) -join ',')
        insert_records = @($insertRecords.records | ForEach-Object { "$($_.label):$($_.state):scan=$($_.scan_rows)/$($_.scan_bytes):sink=$($_.loaded_rows):commit=$($_.commit_time):timeout=$($_.timeout_seconds):priority=$($_.priority):error=$($_.error_message)" })
        routine = "held_tasks=$($routineView.routine_detail.held_tasks) task_samples=$($routineView.routine_detail.tasks.samples) ld_kafka committed=$($kafkaHealth.committed_tasks) aborted=$($kafkaHealth.aborted_tasks) avg_ms=$($kafkaHealth.average_task_ms) rate=$($kafkaHealth.load_rows_rate) error_share=$($kafkaHealth.error_share_permyriad) desired=$($kafkaHealth.desired_tasks)"
    }
    Require ($overview.all_loads -eq $overview.summary.loads -and $byGroup.ContainsKey('stream_load') -and $byGroup.ContainsKey('insert') -and $byGroup.ContainsKey('routine_load')) 'The overview does not split the records into Stream Load, INSERT and Routine Load.'
    Require ($null -eq $overview.stream -and $null -eq $overview.batch -and $null -eq $overview.routine_detail) 'The all-types overview carries a type detail.'
    Require ($streamView.volume_unit -eq 'bytes' -and [long]$streamView.stream.received_bytes -gt 0 -and @($streamView.stream.failure_reasons | Where-Object { $_.reason -eq 'filtered_ratio' }).Count -eq 1 -and @($streamView.stream.sources).Count -ge 1 -and [int]$streamView.phases.receive_data.samples -ge 3) 'The Stream Load view misses bytes, the filtered-ratio failure, sources or the receive phase.'
    Require ($null -eq $streamView.batch -and $null -eq $streamView.routine_detail -and @($streamView.hours | ForEach-Object { @($_.segments | Where-Object { $_.group -ne 'stream_load' }).Count } | Measure-Object -Sum).Sum -eq 0) 'The Stream Load view mixes in other types.'
    Require ($insertView.summary.loads -eq '2' -and $insertView.summary.failed -eq '1' -and $insertView.volume_unit -eq 'rows' -and [int]$insertView.batch.duration.samples -ge 1 -and @($insertView.batch.slowest).Count -ge 1 -and @($insertView.batch.failure_reasons).Count -eq 1) 'The INSERT view misses the two INSERTs, their timings or the failure.'
    Require ($null -ne $insertView.batch.slowest[0].execute_seconds -and $null -ne $insertView.batch.slowest[0].timeout_seconds -and $null -ne $insertView.batch.slowest[0].timeout_share_permyriad) 'The finished INSERT lacks its execute time or timeout share.'
    Require ([int]$routineView.routine_detail.held_tasks -ge 1 -and [int]$routineView.routine_detail.tasks.samples -ge 1 -and [int]$kafkaHealth.committed_tasks -ge 1 -and $null -ne $kafkaHealth.average_task_ms -and $null -ne $kafkaHealth.load_rows_rate -and $null -ne $kafkaHealth.error_share_permyriad -and $kafkaHealth.desired_tasks -eq '1') 'The Routine Load view misses the job health or its tasks.'
    Require (@($routineView.hours | ForEach-Object { @($_.segments | Where-Object { $_.group -ne 'routine_load' }).Count } | Measure-Object -Sum).Sum -eq 0) 'The Routine Load view mixes in other types.'

    if ($HoldForBrowserSeconds -gt 0) {
        $stage = 'browser_hold'
        # A fresh partly filtered load, so the held page has an error log to open.
        Invoke-InContainer $srName "curl -s --location-trusted -u root: -H 'label:ld_partial_view' -H 'strict_mode:true' -H 'max_filter_ratio:0.9' -H 'columns: id,name,v,dt' -T /tmp/mixed.tsv http://127.0.0.1:8030/api/lr/t/_stream_load" | Out-Null
        Invoke-RestMethod "$apiBase/connections/$id" -Method Delete -Headers $script:headers -ContentType 'application/json' | Out-Null
        $env:A1_READER_PASSWORD = $readerPassword; $env:A1_PRODUCT_ORIGIN = "http://127.0.0.1:$ApiPort"; $env:A1_PASSWORD_PORT = '18099'; $env:A1_HOLD_SECONDS = [string]$HoldForBrowserSeconds
        try { $summary.browser_hold = (& node (Join-Path $PSScriptRoot 'serve-password-once.mjs')) -join ' ' }
        finally { foreach ($name in 'A1_READER_PASSWORD', 'A1_PRODUCT_ORIGIN', 'A1_PASSWORD_PORT', 'A1_HOLD_SECONDS') { [Environment]::SetEnvironmentVariable($name, $null, 'Process') } }
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
    if ($startedApi -and -not $startedApi.HasExited) { Stop-Process -Id $startedApi.Id -Force; $startedApi.WaitForExit(5000) | Out-Null }
    $logs = @('stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-load-$stamp.$_.log" } | Where-Object { Test-Path -LiteralPath $_ })
    $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); $text.Contains($readerPassword) -or $text.Contains($secretLiteral) -or $text.Contains($auditWriterPassword) }).Count
    $summary.api_log_scan = if ($logs.Count -eq 0) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }
    if ($summary.api_log_scan -eq 'FAIL') { $cleanupFailure = 'api_log_scan' }
    foreach ($ownedId in @($kafkaId, $srId) | Where-Object { $_ }) { & docker rm -f $ownedId 2>$null | Out-Null }
    if ($kafkaPulled) { & docker rmi $kafkaImage 2>$null | Out-Null }
    $summary.postconditions = [ordered]@{ containers_absent = (@(@($kafkaId, $srId) | Where-Object { $_ -and (& docker ps -aq --no-trunc --filter "id=$_") }).Count -eq 0); ports_free = (-not (Test-PortListening $ApiPort) -and -not (Test-PortListening $MySqlPort)) }
    if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    if ($cleanupFailure) { $summary.status = 'FAIL'; $summary.cleanup_failure = $cleanupFailure }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'Load E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
