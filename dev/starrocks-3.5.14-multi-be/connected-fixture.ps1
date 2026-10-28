[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(5, 120)][int]$PollTimeoutSeconds = 30,
    [ValidateRange(65, 120)][int]$PluginWarmupSeconds = 65
)

# Test infrastructure only: turns a cluster started by up.ps1 into the
# connected-mode fixture that dev/starrocks-3.5.14/bootstrap.ps1 and
# run-fixture-workload.ps1 provide for allin1: AuditLoader 5.0.0, the
# doctor_reader / audit_writer / workload_user roles, a table spread over all
# three BEs, two finished Profiles and one terminally failed Audit row. It
# touches only the FE that up.ps1 created and writes only below its data root.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$docker = Get-FixtureDockerExecutable $DockerExecutable
$secret = Get-FixtureSecret
$rootPassword = Get-MultiBeRootPassword
$fe = $script:MultiBeFeName
if (-not (Assert-MultiBeOwned -DockerExecutable $docker -Name $fe)) { throw 'The multi-BE FE is not running; run up.ps1 first.' }
$dataRoot = Get-MultiBeDataRoot
if (-not (Test-Path -LiteralPath (Join-Path $dataRoot 'fixture-bootstrap.json') -PathType Leaf)) { throw 'The multi-BE cluster was not created by up.ps1.' }

$passwords = @{}
foreach ($role in @('audit_writer', 'doctor_reader', 'workload_user')) { $passwords[$role] = Get-FixturePassword $secret $role }
function Invoke-Root([string]$Sql) { Invoke-MultiBeMysql $docker $Sql $rootPassword }
function Invoke-As([string]$Role, [string]$Sql) { Invoke-FixtureMysql $docker $fe $Role $passwords[$Role] $Sql }

# Same pinned archive and cache as the allin1 bootstrap; the copy lives in the
# multi-BE data root, so teardown.ps1 removes it with everything else.
$archiveDirectory = Join-Path $dataRoot 'auditloader'
New-Item -ItemType Directory -Force -Path $archiveDirectory | Out-Null
$auditArchive = Join-Path $archiveDirectory 'auditloader-5.0.0.zip'
$cachedAuditArchive = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'cache\auditloader-5.0.0.zip')
if (Test-Path -LiteralPath $cachedAuditArchive -PathType Leaf) {
    Copy-Item -LiteralPath $cachedAuditArchive -Destination $auditArchive
} else {
    Invoke-WebRequest -Uri $script:AuditLoaderUrl -OutFile $auditArchive -MaximumRedirection 0
}
if ((Get-FixtureSha256 -Path $auditArchive) -ne $script:AuditLoaderSha256) {
    throw 'AuditLoader archive SHA-256 did not match auditloader.manifest.json.'
}

# Schema identical to the allin1 bootstrap except: the Profile table has six
# buckets and 3000 rows so its tablets land on every BE, and doctor_reader may
# also SELECT it, so Runtime Top can run one distributed query.
$schemaSql = @'
CREATE DATABASE IF NOT EXISTS starrocks_audit_db__;
CREATE TABLE IF NOT EXISTS starrocks_audit_db__.starrocks_audit_tbl__ (
  `queryId` VARCHAR(64), `timestamp` DATETIME NOT NULL, `queryType` VARCHAR(12), `clientIp` VARCHAR(32),
  `user` VARCHAR(64), `authorizedUser` VARCHAR(64), `resourceGroup` VARCHAR(64), `catalog` VARCHAR(32),
  `db` VARCHAR(96), `state` VARCHAR(8), `errorCode` VARCHAR(512), `queryTime` BIGINT,
  `scanBytes` BIGINT, `scanRows` BIGINT, `returnRows` BIGINT, `cpuCostNs` BIGINT, `memCostBytes` BIGINT,
  `stmtId` INT, `isQuery` TINYINT, `feIp` VARCHAR(128), `stmt` VARCHAR(1048576), `digest` VARCHAR(32),
  `planCpuCosts` DOUBLE, `planMemCosts` DOUBLE, `pendingTimeMs` BIGINT, `candidateMVs` VARCHAR(65533),
  `hitMvs` VARCHAR(65533), `QueriedRelations` ARRAY<VARCHAR(65533)>, `warehouse` VARCHAR(32)
) ENGINE=OLAP DUPLICATE KEY (`queryId`, `timestamp`, `queryType`)
PARTITION BY date_trunc('day', `timestamp`)
PROPERTIES ('replication_num'='1', 'partition_live_number'='30');
CREATE DATABASE IF NOT EXISTS e2e_fixture;
CREATE TABLE IF NOT EXISTS e2e_fixture.profile_rows (
  `id` INT NOT NULL, `bucket` INT NOT NULL, `payload` VARCHAR(32) NOT NULL
) ENGINE=OLAP DUPLICATE KEY (`id`)
DISTRIBUTED BY HASH(`id`) BUCKETS 6
PROPERTIES ('replication_num'='1');
INSERT INTO e2e_fixture.profile_rows SELECT generate_series, generate_series % 7, concat('p', generate_series % 13) FROM TABLE(generate_series(1, 3000));
'@ + @"
CREATE USER IF NOT EXISTS doctor_reader IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral (Get-FixtureNativePasswordHash $passwords.doctor_reader));
CREATE USER IF NOT EXISTS audit_writer IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral (Get-FixtureNativePasswordHash $passwords.audit_writer));
CREATE USER IF NOT EXISTS workload_user IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral (Get-FixtureNativePasswordHash $passwords.workload_user));
GRANT SELECT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER doctor_reader;
GRANT SELECT ON TABLE e2e_fixture.profile_rows TO USER doctor_reader;
GRANT INSERT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER audit_writer;
GRANT SELECT ON TABLE e2e_fixture.profile_rows TO USER workload_user;
"@
try { Invoke-Root $schemaSql | Out-Null }
catch { throw 'Multi-BE schema and ciphertext-account bootstrap failed.' }
if ((Invoke-As 'audit_writer' 'SELECT 1;' | Select-Object -First 1).Trim() -ne '1') { throw 'The dedicated AuditLoader writer could not authenticate.' }

# Evidence that the Profile table really spans every BE.
$placement = [ordered]@{}
foreach ($line in @(Invoke-Root "SELECT b.BE_ID, COUNT(*) FROM information_schema.be_tablets b JOIN information_schema.tables_config t ON b.TABLE_ID = t.TABLE_ID WHERE t.TABLE_SCHEMA = 'e2e_fixture' AND t.TABLE_NAME = 'profile_rows' GROUP BY b.BE_ID ORDER BY b.BE_ID;")) {
    $cells = ([string]$line) -split "`t"
    if ($cells.Count -eq 2) { $placement["be_$($cells[0])"] = [int]$cells[1] }
}
if ($placement.Count -ne $script:MultiBeBackendCount) { throw 'The Profile table tablets do not span every BE.' }

# AuditLoader keeps the official FE HTTP endpoint. Unlike allin1 there is no
# local 8040 on the FE, so the fixture does not override frontend_host_port.
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('starrocks-auditloader-mbe-' + [Guid]::NewGuid().ToString('N'))
$configuredArchive = Join-Path $tempRoot 'auditloader-configured.zip'
try {
    $contents = Join-Path $tempRoot 'contents'
    New-Item -ItemType Directory -Force -Path $contents | Out-Null
    Expand-Archive -LiteralPath $auditArchive -DestinationPath $contents -Force
    $configPath = Join-Path $contents 'plugin.conf'
    $config = Get-Content -LiteralPath $configPath -Raw
    foreach ($pair in @{
        max_batch_interval_sec = '5'; frontend_host_port = '127.0.0.1:8030'; database = 'starrocks_audit_db__';
        table = 'starrocks_audit_tbl__'; user = 'audit_writer'; password = $passwords.audit_writer; secret_key = ''
    }.GetEnumerator()) {
        $config = [regex]::Replace($config, "(?m)^$([regex]::Escape($pair.Key))=.*$", "$($pair.Key)=$($pair.Value)")
    }
    [IO.File]::WriteAllText($configPath, $config, [Text.UTF8Encoding]::new($false))
    Compress-Archive -Path (Join-Path $contents '*') -DestinationPath $configuredArchive -CompressionLevel Optimal
    Invoke-FixtureDocker $docker @('cp', $configuredArchive, "$fe`:/tmp/auditloader-configured.zip") -DiscardOutput
    Invoke-Root 'INSTALL PLUGIN FROM "/tmp/auditloader-configured.zip";' | Out-Null
    if (((Invoke-Root 'SHOW PLUGINS;') -join "`n") -notmatch '(?m)^AuditLoader\s+AUDIT\s+.*\s+5\.0\.0\s+') {
        throw 'AuditLoader 5.0.0 was not installed in the multi-BE FE.'
    }
} finally {
    if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force }
}

# 3.5.14 refreshes active audit plugins every 60 s (see run-fixture-workload.ps1).
Start-Sleep -Seconds $PluginWarmupSeconds

function Get-ProfileIdByMarker([string]$Marker) {
    # The FE publishes a finished Profile asynchronously; on the low-resource
    # profile that can take a few seconds. Bounded polling, never a rerun.
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $row = @(Invoke-Root 'SHOW PROFILELIST;') | Where-Object { $_ -like "*$Marker*" } | Select-Object -First 1
        if ($null -eq $row) { Start-Sleep -Seconds 1 }
    } while ($null -eq $row -and [DateTime]::UtcNow -lt $deadline)
    if ($null -eq $row) { throw "Profile list has no profile for $Marker." }
    $queryId = (([string]$row) -split "`t", 2)[0].Trim()
    if ($queryId -notmatch '^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$') { throw 'Profile list returned an invalid query ID.' }
    $queryId.ToLowerInvariant()
}

# Fixture-only external workload, same SQL as the allin1 workload.
$queryIds = [ordered]@{}
foreach ($workload in @(
    [ordered]@{ label = 'baseline'; marker = 'e2e_baseline'; sql = 'SELECT bucket, COUNT(*) AS e2e_baseline FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket ORDER BY bucket;' },
    [ordered]@{ label = 'candidate'; marker = 'e2e_candidate'; sql = 'SELECT bucket, COUNT(*) AS e2e_candidate FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket;' }
)) {
    Invoke-As 'workload_user' "SET enable_profile=true;`nSET pipeline_profile_level=1;`n$($workload.sql)" | Out-Null
    $queryIds[$workload.label] = Get-ProfileIdByMarker $workload.marker
}
$unavailableProbeObserved = $false
try { Invoke-As 'workload_user' 'SELECT /* e2e_unavailable_probe */ * FROM e2e_fixture.table_that_does_not_exist;' | Out-Null }
catch { $unavailableProbeObserved = $true }
if (-not $unavailableProbeObserved) { throw 'The controlled unavailable-Profile probe unexpectedly succeeded.' }

foreach ($queryId in $queryIds.Values) {
    $profileLength = (Invoke-As 'doctor_reader' "SELECT LENGTH(get_query_profile('$queryId'));" | Select-Object -First 1).Trim()
    if ($profileLength -notmatch '^[1-9][0-9]*$') { throw 'The read-only role could not retrieve a non-empty fixture Profile.' }
}

$auditCount = 0; $unavailableAuditCount = 0
$idList = (@($queryIds.Values) | ForEach-Object { ConvertTo-SqlLiteral $_ }) -join ', '
$deadline = [DateTime]::UtcNow.AddSeconds($PollTimeoutSeconds)
while ([DateTime]::UtcNow -lt $deadline) {
    $auditCount = [int]((Invoke-As 'doctor_reader' "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE queryId IN ($idList);" | Select-Object -First 1).Trim())
    $unavailableAuditCount = [int]((Invoke-As 'doctor_reader' "SELECT COUNT(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE user = 'workload_user' AND state <> 'EOF';" | Select-Object -First 1).Trim())
    if ($auditCount -ge 2 -and $unavailableAuditCount -ge 1) { break }
    Start-Sleep -Seconds 2
}
if ($auditCount -lt 2) { throw 'AuditLoader did not publish both fixture query IDs before the polling deadline.' }
if ($unavailableAuditCount -lt 1) { throw 'AuditLoader did not publish the controlled unavailable-Profile row before the polling deadline.' }

[ordered]@{
    status = 'PASS'; fixture = 'multi_be_connected_external_workload_only'; frontend = $fe
    audit_loader_version = '5.0.0'; audit_loader_sha256 = $script:AuditLoaderSha256; audit_loader_frontend_host_port = '127.0.0.1:8030'
    profile_table_tablets = $placement
    baseline_query_id = $queryIds.baseline; candidate_query_id = $queryIds.candidate
    audit_rows = $auditCount; unavailable_profile_audit_rows = $unavailableAuditCount
    plugin_warmup_seconds = $PluginWarmupSeconds; audit_poll_timeout_seconds = $PollTimeoutSeconds
} | ConvertTo-Json -Compress -Depth 4
