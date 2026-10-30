[CmdletBinding()]
param(
    # One official release to smoke, e.g. 3.4.10: starrocks/allin1-ubuntu:<Release>.
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Release,
    [ValidateRange(1024, 65535)][int]$ApiPort = 18066,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39632,
    [ValidateRange(60, 1800)][int]$ReadyTimeoutSeconds = 600,
    # Extra Stream Loads after the checks, to time the reads with a nearly
    # full FE manager. Keep the total under stream_load_task_keep_max_num
    # (1000): above it StarRocks drops every finished task at once. 0 skips.
    [ValidateRange(0, 3000)][int]$RecordVolume = 0,
    # Keep the StarRocks image after the run (default: removed, to bound disk use).
    [switch]$KeepImage,
    # Keep the Kafka image for the next release in a series.
    [switch]$KeepKafkaImage
)

# Load workspace smoke for one supported release line (tasks 51-52): the
# same real Stream Loads (clean, partly filtered, failed) and Kafka Routine
# Load as the 3.5.14 E2E, read through the product API by a reader with no
# grants. Expectations follow `load_support`: 3.3+ full; 3.2 Routine Load
# without lag or error logs, no load records. Low-resource container like
# run-version-smoke.ps1; container, volumes and image removed afterwards.
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "load-version-smoke-$Release-$stamp.json"
$starrocksImage = "starrocks/allin1-ubuntu:$Release"
$kafkaImage = 'apache/kafka:3.9.0@sha256:fbc7d7c428e3755cf36518d4976596002477e4c052d1f80b5b9eafd06d0fff2f'
$srName = "sr-doctor-load-vsmoke-$($Release.Replace('.', '-'))"
$kafkaName = "$srName-kafka"
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$cpus = '2.5'; $memory = '5376m'; $feHeapMiB = 1024; $beMemLimitMiB = 3072
$parts = $Release.Split('.') | ForEach-Object { [int]$_ }
$full = ($parts[0] -gt 3) -or ($parts[0] -eq 3 -and $parts[1] -ge 3)
$stage = 'initialization'
$summary = [ordered]@{ project = 'load-version-smoke'; release = $Release; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o'); starrocks_image = $starrocksImage; kafka_image = $kafkaImage; expected = if ($full) { 'full' } else { 'routine_without_lag_no_records' }; resources = [ordered]@{ cpus = $cpus; memory = $memory; fe_heap_mib = $feHeapMiB; be_mem_limit_mib = $beMemLimitMiB } }
$startedApi = $null; $primaryFailure = $false; $srId = $null; $kafkaId = $null; $kafkaPulled = $false
$readerPassword = 'Lv_' + [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(10)).ToLowerInvariant()
$secretLiteral = 'load_vsmoke_secret_row_value'

function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Test-PortListening([int]$Port) { $null -ne (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) }
function Invoke-InContainer([string]$Name, [string]$Script) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($Script -replace "`r", '')))
    $output = & docker exec $Name sh -c "echo $encoded | base64 -d | sh" 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Command in $Name failed: $output" }
    $output
}
function Invoke-Sql([string]$Sql) { Invoke-InContainer $srName "mysql -h127.0.0.1 -P9030 -uroot -N --raw <<'SQL'`n$Sql`nSQL" }
function Invoke-Post([string]$Path, $Payload) {
    try {
        $response = Invoke-WebRequest "$apiBase$Path" -Method Post -Headers $script:headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 10 -Compress) -UseBasicParsing
    } catch {
        # Keep the product's failure body (code, stage, message id); it carries no credentials.
        $detail = $_.ErrorDetails.Message
        $exception = [Exception]::new("$($_.Exception.Message) $detail")
        $exception.Data['status'] = [int]$_.Exception.Response.StatusCode
        throw $exception
    }
    [ordered]@{ raw = $response.Content; body = ($response.Content | ConvertFrom-Json) }
}
function Container-Exists([string]$Name) { [bool](& docker ps -a --filter "name=^/$Name$" --format '{{.Names}}') }

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $stage = 'cold_start_preconditions'
    Require (-not (Container-Exists $srName) -and -not (Container-Exists $kafkaName)) 'Smoke requires a cold start (containers present).'
    Require (-not (Test-PortListening $ApiPort) -and -not (Test-PortListening $MySqlPort)) 'Smoke ports are in use.'
    $apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    Require (Test-Path -LiteralPath $apiExecutable) 'Build doctor-api first (cargo build --locked -p doctor-api).'
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.git_dirty = [bool](& git -C $repoRoot status --porcelain)

    $stage = 'fixture_up'
    & docker pull $starrocksImage | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not pull $starrocksImage." }
    $summary.image_digest = (& docker image inspect $starrocksImage --format '{{index .RepoDigests 0}}').Trim()
    $srId = (& docker create --name $srName --cpus $cpus --memory $memory --memory-swap $memory --pids-limit 1024 --security-opt no-new-privileges -p "127.0.0.1:${MySqlPort}:9030" $starrocksImage | Select-Object -Last 1)
    if ($LASTEXITCODE -ne 0 -or $srId -notmatch '^[0-9a-f]{64}$') { $srId = $null; throw 'StarRocks container could not be created.' }
    $work = Join-Path ([IO.Path]::GetTempPath()) ('load-vsmoke-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    foreach ($edit in @(
        @{ path = '/data/deploy/starrocks/fe/conf/fe.conf'; text = "`nJAVA_OPTS=`"-Xmx${feHeapMiB}m -XX:+UseG1GC`"`n" },
        @{ path = '/data/deploy/starrocks/be/conf/be.conf'; text = "`nmem_limit = ${beMemLimitMiB}M`n" })) {
        $local = Join-Path $work ([IO.Path]::GetFileName($edit.path))
        & docker cp "${srId}:$($edit.path)" $local | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "This image has no $($edit.path)." }
        [IO.File]::AppendAllText($local, $edit.text, [Text.UTF8Encoding]::new($false))
        & docker cp $local "${srId}:$($edit.path)" | Out-Null
    }
    Remove-Item -LiteralPath $work -Recurse -Force
    & docker start $srId | Out-Null
    $kafkaPulled = -not [bool](& docker images -q $kafkaImage)
    $kafkaId = (& docker run -d --name $kafkaName --network "container:$srName" --memory 1g `
        -e KAFKA_NODE_ID=1 -e KAFKA_PROCESS_ROLES=broker,controller -e KAFKA_LISTENERS=PLAINTEXT://127.0.0.1:9092,CONTROLLER://127.0.0.1:9093 `
        -e KAFKA_ADVERTISED_LISTENERS=PLAINTEXT://127.0.0.1:9092 -e KAFKA_CONTROLLER_LISTENER_NAMES=CONTROLLER `
        -e KAFKA_LISTENER_SECURITY_PROTOCOL_MAP=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT -e KAFKA_CONTROLLER_QUORUM_VOTERS=1@127.0.0.1:9093 `
        -e KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR=1 -e KAFKA_NUM_PARTITIONS=2 $kafkaImage | Select-Object -Last 1)
    if ($LASTEXITCODE -ne 0 -or $kafkaId -notmatch '^[0-9a-f]{64}$') { $kafkaId = $null; throw 'Kafka container did not start.' }
    $deadline = [DateTime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
    do {
        Start-Sleep -Seconds 5
        $ready = (& docker exec $srName sh -c "mysql -h127.0.0.1 -P9030 -uroot -N -e 'SHOW BACKENDS' 2>/dev/null | grep -c true" 2>$null) -match '^[1-9]'
        if ($ready) { $ready = (& docker exec $srName sh -c "mysql -h127.0.0.1 -P9030 -uroot -N -e 'CREATE DATABASE IF NOT EXISTS lr' 2>/dev/null && echo ok") -eq 'ok' }
    } while (-not $ready -and [DateTime]::UtcNow -lt $deadline)
    Require $ready "StarRocks $Release did not become ready."
    $summary.observed_version = (Invoke-Sql 'SELECT current_version();' | Select-Object -Last 1).Trim()

    $stage = 'workload'
    Invoke-Sql @"
CREATE TABLE lr.t (id INT NOT NULL, name VARCHAR(5), v INT NOT NULL, dt DATE) DUPLICATE KEY(id) PARTITION BY RANGE(dt) (PARTITION p1 VALUES [('2026-01-01'), ('2026-02-01'))) DISTRIBUTED BY HASH(id) BUCKETS 1 PROPERTIES ('replication_num' = '1');
CREATE TABLE lr.k (id INT, name VARCHAR(16), v INT) DUPLICATE KEY(id) DISTRIBUTED BY HASH(id) BUCKETS 2 PROPERTIES ('replication_num' = '1');
CREATE USER load_reader IDENTIFIED BY '$readerPassword';
"@ | Out-Null
    $loads = Invoke-InContainer $srName @"
printf '1\tok\t1\t2026-01-05\n2\tok2\t2\t2026-01-06\n' > /tmp/ok.tsv
printf '3\tfine\t3\t2026-01-05\nx\t$secretLiteral\t2\t2026-01-05\n4\tnul\t\\N\t2026-01-05\n5\tpart\t5\t2027-01-01\n' > /tmp/mixed.tsv
for spec in 'ld_ok:/tmp/ok.tsv:0' 'ld_partial:/tmp/mixed.tsv:0.9' 'ld_failed:/tmp/mixed.tsv:0'; do
  label=`${spec%%:*}; rest=`${spec#*:}; file=`${rest%%:*}; ratio=`${rest#*:}
  curl -s --location-trusted -u root: -H 'Expect:100-continue' -H "label:`$label" -H 'strict_mode:true' -H "max_filter_ratio:`$ratio" -H 'columns: id,name,v,dt' -T `$file http://127.0.0.1:8030/api/lr/t/_stream_load | tr -d '\n' | sed 's/ //g'; echo
done
"@
    # The raw responses carry only status, counts and the error URL, never rows.
    $summary.stream_load_responses = @($loads | ForEach-Object { "$_".Substring(0, [Math]::Min(600, "$_".Length)) })
    $summary.stream_loads = @($loads | ForEach-Object { if ($_ -match '"Label":"([^"]+)".*"Status":"([^"]+)".*"NumberFilteredRows":(\d+)') { "$($Matches[1]):$($Matches[2]):$($Matches[3])" } })
    Require ($summary.stream_loads -contains 'ld_ok:Success:0' -and $summary.stream_loads -contains 'ld_failed:Fail:3') "Unexpected Stream Load results: $($summary.stream_loads -join ', ')"
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    do { Start-Sleep -Seconds 3; & docker exec $kafkaName /opt/kafka/bin/kafka-topics.sh --bootstrap-server 127.0.0.1:9092 --create --if-not-exists --topic ld --partitions 2 --replication-factor 1 2>&1 | Out-Null } while ($LASTEXITCODE -ne 0 -and [DateTime]::UtcNow -lt $deadline)
    Require ($LASTEXITCODE -eq 0) 'Kafka topic could not be created.'
    Invoke-InContainer $kafkaName "for i in `$(seq 1 200); do echo `"`$((1000+i)),k`$i,`$i`"; done > /tmp/m.csv; echo 'bad,row,x' >> /tmp/m.csv; /opt/kafka/bin/kafka-console-producer.sh --bootstrap-server 127.0.0.1:9092 --topic ld < /tmp/m.csv" | Out-Null
    # Right after start a low-resource BE may not count as alive for task
    # slots yet ("we only support 0*16 tasks"); retry a bounded number of times.
    $created = $false
    for ($attempt = 1; $attempt -le 12 -and -not $created; $attempt++) {
        try {
            Invoke-Sql @"
CREATE ROUTINE LOAD lr.ld_kafka ON k COLUMNS TERMINATED BY ',', COLUMNS (id, name, v)
PROPERTIES ('desired_concurrent_number' = '1', 'max_error_number' = '10', 'strict_mode' = 'true', 'max_batch_interval' = '5')
FROM KAFKA ('kafka_broker_list' = '127.0.0.1:9092', 'kafka_topic' = 'ld', 'property.kafka_default_offsets' = 'OFFSET_BEGINNING');
"@ | Out-Null
            $created = $true
        } catch {
            if ($_.Exception.Message -notmatch 'routine load tasks' -or $attempt -eq 12) { throw }
            Start-Sleep -Seconds 10
        }
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(180)
    do { Start-Sleep -Seconds 3; $stats = (Invoke-Sql "SHOW ROUTINE LOAD FOR lr.ld_kafka\G" | Out-String) } while ($stats -notmatch '"loadedRows":200' -and [DateTime]::UtcNow -lt $deadline)
    Require ($stats -match '"loadedRows":200') "Routine Load did not load 200 rows."

    $stage = 'api_start'
    $saved = @{}
    $environment = [ordered]@{ DOCTOR_PORT = [string]$ApiPort; DOCTOR_HISTORY = 'off' }
    foreach ($key in $environment.Keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process'); [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process') }
    try {
        $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-load-vsmoke-$Release-$stamp.stdout.log") -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-load-vsmoke-$Release-$stamp.stderr.log")
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

    $stage = 'routine_jobs'
    $routine = Invoke-Post "/connections/$id/loads/routine-jobs" @{ connection_id = $id }
    $job = @($routine.body.jobs | Where-Object { $_.name -eq 'ld_kafka' })[0]
    $summary.routine = [ordered]@{
        support = $routine.body.support; jobs = @($routine.body.jobs).Count; state = "$($job.state):$($job.state_class)"; topic = $job.topic
        partitions = @($job.partitions | ForEach-Object { "p$($_.partition) consumed=$($_.consumed) latest=$($_.latest) lag=$($_.lag)" })
        loaded = $job.statistics.loaded_rows; errors = $job.statistics.error_rows; has_error_log = $job.has_error_log
        summary = "running=$($routine.body.summary.running) paused=$($routine.body.summary.paused) error_rows=$($routine.body.summary.error_rows)/unknown=$($routine.body.summary.error_rows_unknown) lags=$(@($routine.body.summary.lags).Count)"
        limitations = $routine.body.limitations; leaks_broker_list = $routine.raw.Contains('9092')
    }
    Require ($null -ne $job) 'The Routine Load job is not visible to the reader.'
    Require ($job.state_class -in @('running', 'scheduling') -and $job.statistics.loaded_rows -eq '200') 'The Routine Load job is not decoded as running with 200 rows.'
    Require (-not $summary.routine.leaks_broker_list) 'The broker list left the server.'
    if ($full) {
        Require ($routine.body.support -eq 'full' -and $job.has_error_log -and @($job.partitions | Where-Object { $null -ne $_.lag }).Count -ge 1) 'Expected lag and an error log entry on 3.3+.'
    } else {
        Require ($routine.body.support -eq 'without_lag' -and -not $job.has_error_log -and @($job.partitions | Where-Object { $null -ne $_.lag }).Count -eq 0) 'Expected no lag and no error log entry on 3.2.'
    }

    $stage = 'records'
    $records = (Invoke-Post "/connections/$id/loads/records" @{ connection_id = $id; kind = $null; outcome = $null; table = $null; include_system = $false; created_hour = $null }).body
    $byLabel = @{}; foreach ($record in $records.records) { $byLabel[$record.label] = $record }
    $summary.records = [ordered]@{ support = $records.support; matched = $records.matched; hidden_system = $records.hidden_system; fe_time_zone = $records.fe_time_zone
        rows = @($records.records | Select-Object -First 8 | ForEach-Object { "$($_.kind):$($_.label):$($_.state):loaded=$($_.loaded_rows):filtered=$($_.filtered_rows):phases=$($_.begin_txn_ms)/$($_.plan_ms)/$($_.receive_data_ms)" }) }
    if ($full) {
        Require ($records.support -eq 'full' -and $byLabel.ContainsKey('ld_ok') -and $null -ne $byLabel['ld_ok'].plan_ms) 'The clean Stream Load is missing or lacks phase times.'
        Require ($byLabel['ld_partial'].filtered_rows -eq '3' -and $byLabel['ld_failed'].state_class -eq 'cancelled') 'Filtered or failed Stream Loads are wrong.'
        Require (@($records.records | Where-Object { $_.kind -eq 'ROUTINE_LOAD' }).Count -ge 1) 'No Routine Load task appears among the records.'
    } else {
        Require ($records.support -eq 'unsupported_version' -and @($records.records).Count -eq 0) 'Expected load records to be unsupported on 3.2.'
    }

    $stage = 'overview'
    $timings = @()
    for ($round = 0; $round -lt 3; $round++) {
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $overview = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = $null; include_system = $false }).body
        $watch.Stop(); $timings += [int]$watch.ElapsedMilliseconds
    }
    $summary.overview = [ordered]@{ support = $overview.support; routine_support = $overview.routine.support; loads = $overview.summary.loads; failed = $overview.summary.failed
        hours = @($overview.hours).Count; coverage = (@($overview.hours | Group-Object coverage | ForEach-Object { "$($_.Name)=$($_.Count)" }) -join ','); tables = @($overview.tables | ForEach-Object { "$($_.table):$($_.filtered_rows)" })
        phases = "begin=$($overview.phases.begin_txn.samples) p95=$($overview.phases.begin_txn.p95_ms)"; lags = @($overview.routine.lags | ForEach-Object { "$($_.job_name):p$($_.partition)=$($_.lag)" }); api_ms = $timings }
    if ($full) {
        Require ($overview.summary.loads -eq $records.matched -and @($overview.hours).Count -ge 72 -and @($overview.routine.lags).Count -ge 1) 'The overview does not match the records or lacks lag.'
    } else {
        Require ($overview.support -eq 'unsupported_version' -and @($overview.hours).Count -eq 0 -and $overview.routine.support -eq 'without_lag' -and $overview.routine.running -in @('0', '1')) 'The 3.2 overview should carry Routine Load only.'
    }

    $stage = 'error_log'
    if ($full) {
        $log = (Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $byLabel['ld_partial'].job_id }).body
        $summary.error_log = [ordered]@{ state = $log.state; entries = @($log.entries).Count; categories = @($log.categories | ForEach-Object { "$($_.category):$($_.column):$($_.count)" }) }
        Require ($log.state -eq 'available' -and @($log.entries).Count -eq 3) 'The error log of the partly filtered load is not read.'
        $routineLog = (Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $job.job_id }).body
        $summary.routine_error_log = "$($routineLog.state):$(@($routineLog.entries).Count)"
    } else {
        $refused = try { Invoke-Post "/connections/$id/loads/error-log" @{ connection_id = $id; job_id = $job.job_id } | Out-Null; 'accepted' } catch { $_.Exception.Data['status'] }
        $summary.error_log = "3.2 request answered $refused (entry hidden in the product)"
    }

    if ($RecordVolume -gt 0) {
        $stage = 'record_volume'
        Invoke-InContainer $srName "for i in `$(seq 1 $RecordVolume); do curl -s --location-trusted -u root: -H 'Expect:100-continue' -H `"label:vol_`$i`" -H 'columns: id,name,v,dt' -T /tmp/ok.tsv http://127.0.0.1:8030/api/lr/t/_stream_load > /dev/null; done" | Out-Null
        $summary.record_volume = [ordered]@{ stream_loads_added = $RecordVolume; loads_rows_seen_by_root = (Invoke-Sql 'SELECT COUNT(*) FROM information_schema.loads;' | Select-Object -Last 1).Trim() }
        $overview = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = $null; include_system = $false }).body
        $timings = @()
        for ($round = 0; $round -lt 3; $round++) {
            $watch = [Diagnostics.Stopwatch]::StartNew()
            $overview = (Invoke-Post "/connections/$id/loads/overview" @{ connection_id = $id; kind = $null; include_system = $false }).body
            $watch.Stop(); $timings += [int]$watch.ElapsedMilliseconds
        }
    }

    if ($RecordVolume -gt 0) {
        $stage = 'read_cost'
        # Server-side time of the two fixed reads, run as the reader, three times each.
        $sqlTimes = Invoke-InContainer $srName @"
for q in 'SELECT COUNT(*) FROM (SELECT ID, LABEL, PROFILE_ID, DB_NAME, TABLE_NAME, USER, STATE, TYPE, FILTERED_ROWS, UNSELECTED_ROWS, SINK_ROWS, CAST(RUNTIME_DETAILS AS VARCHAR), CAST(CREATE_TIME AS VARCHAR), CAST(LOAD_FINISH_TIME AS VARCHAR), ERROR_MSG, TRACKING_SQL FROM information_schema.loads ORDER BY ID DESC LIMIT 2000) t' 'SELECT COUNT(*) FROM information_schema.routine_load_jobs'; do
  for r in 1 2 3; do s=`$(date +%s%N); mysql -h127.0.0.1 -P9030 -uload_reader -p'$readerPassword' -N -e "`$q" > /dev/null 2>&1; e=`$(date +%s%N); echo `$(( (e - s) / 1000000 )); done
done
"@
        $times = @($sqlTimes | ForEach-Object { [int]$_ })
        $stats = (& docker stats --no-stream --format '{{.CPUPerc}} {{.MemUsage}}' $srName)
        $summary.read_cost = [ordered]@{ loads_read_ms = $times[0..2]; routine_jobs_read_ms = $times[3..5]; overview_api_ms = $timings; records_in_overview = $overview.summary.loads; at_retention_limit = $overview.at_retention_limit; container_after = $stats }
    }

    $summary.status = 'PASS'
}
catch {
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message = $_.Exception.Message }
}
finally {
    if ($startedApi -and -not $startedApi.HasExited) { Stop-Process -Id $startedApi.Id -Force; $startedApi.WaitForExit(5000) | Out-Null }
    $logs = @('stdout', 'stderr' | ForEach-Object { Join-Path $runtimeRoot "doctor-api-load-vsmoke-$Release-$stamp.$_.log" } | Where-Object { Test-Path -LiteralPath $_ })
    $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); $text.Contains($readerPassword) -or $text.Contains($secretLiteral) }).Count
    $summary.api_log_scan = if ($logs.Count -eq 0) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }
    if ($summary.api_log_scan -eq 'FAIL') { $summary.status = 'FAIL' }
    foreach ($ownedId in @($kafkaId, $srId) | Where-Object { $_ }) { & docker rm -f -v $ownedId 2>$null | Out-Null }
    if (-not $KeepImage) { & docker image rm -f $starrocksImage 2>$null | Out-Null }
    if ($kafkaPulled -and -not $KeepKafkaImage) { foreach ($imageId in @(& docker images -q $kafkaImage)) { & docker rmi -f $imageId 2>$null | Out-Null } }
    $summary.postconditions = [ordered]@{ containers_absent = (@(@($kafkaId, $srId) | Where-Object { $_ -and (& docker ps -aq --no-trunc --filter "id=$_") }).Count -eq 0); image_removed = ($KeepImage -or -not [bool](& docker images -q $starrocksImage)); ports_free = (-not (Test-PortListening $ApiPort) -and -not (Test-PortListening $MySqlPort)) }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
if ($primaryFailure) { exit 1 }
