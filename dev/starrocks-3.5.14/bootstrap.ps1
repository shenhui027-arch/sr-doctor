[CmdletBinding()]
param(
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514',
    [string]$DataRoot = (Join-Path ($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers')) 'sr-doctor-v0.1-e2e-3514'),
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$HttpPort = 38130,
    [ValidateRange(1024, 65535)][int]$Http2Port = 38140,
    [ValidateRange(1024, 65535)][int]$HeartbeatPort = 39120,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39130,
    [ValidateRange(30, 300)][int]$ReadyTimeoutSeconds = 180,
    # 2.5 CPUs / 5.25 GiB, no extra swap, FE heap and BE mem_limit set
    # (common.ps1). Applies only when the container is created.
    [switch]$LowResource
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$docker = Get-FixtureDockerExecutable $DockerExecutable
$secret = Get-FixtureSecret
Assert-FixtureContainerName $ContainerName
$data = Assert-FixtureDataRoot $DataRoot
if (@($HttpPort, $Http2Port, $HeartbeatPort, $MySqlPort).Count -ne @($HttpPort, $Http2Port, $HeartbeatPort, $MySqlPort | Select-Object -Unique).Count) {
    throw 'Fixture host ports must be distinct.'
}

$auditWriterPassword = Get-FixturePassword $secret 'audit_writer'
$doctorReaderPassword = Get-FixturePassword $secret 'doctor_reader'
$workloadPassword = Get-FixturePassword $secret 'workload_user'
$auditWriterPasswordHash = Get-FixtureNativePasswordHash $auditWriterPassword
$doctorReaderPasswordHash = Get-FixtureNativePasswordHash $doctorReaderPassword
$workloadPasswordHash = Get-FixtureNativePasswordHash $workloadPassword
$feMeta = Join-Path $data 'fe-meta'
$beStorage = Join-Path $data 'be-storage'
$archiveDirectory = Join-Path $data 'auditloader'
foreach ($directory in @($feMeta, $beStorage, $archiveDirectory)) {
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
}

$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $ContainerName
if ($null -eq $identity) {
    Invoke-FixtureDocker $docker @('image', 'inspect', $script:FixtureImage) -DiscardOutput
    # Created, configured, then started, so the low-resource FE/BE settings
    # are in place before the processes first read their config.
    Invoke-FixtureDocker $docker (@(
        'create', '--name', $ContainerName,
        '--label', "$script:FixtureLabelKey=$script:FixtureLabelValue"
    ) + (Get-FixtureResourceArguments -LowResource:$LowResource) + @(
        '--pids-limit', '1024', '--security-opt', 'no-new-privileges',
        '--publish', "127.0.0.1:$HttpPort`:8030", '--publish', "127.0.0.1:$Http2Port`:8040",
        '--publish', "127.0.0.1:$HeartbeatPort`:9020", '--publish', "127.0.0.1:$MySqlPort`:9030",
        '--volume', "${feMeta}:/data/deploy/starrocks/fe/meta",
        '--volume', "${beStorage}:/data/deploy/starrocks/be/storage",
        $script:FixtureImage
    )) -DiscardOutput
    if ($LowResource) { Set-FixtureLowResourceConfig -DockerExecutable $docker -ContainerName $ContainerName }
    Invoke-FixtureDocker $docker @('start', $ContainerName) -DiscardOutput
} else {
    if ($identity.Image -ne $script:FixtureImage -or $identity.Label -ne $script:FixtureLabelValue) {
        throw 'An existing container with this name is not this dedicated fixed-digest fixture.'
    }
    $running = (& $docker inspect $ContainerName --format '{{.State.Running}}').Trim()
    if ($running -ne 'true') { Invoke-FixtureDocker $docker @('start', $ContainerName) -DiscardOutput }
}

$deadline = [DateTime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
$version = $null
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $version = (Invoke-FixtureMysql $docker $ContainerName 'root' '' 'SELECT current_version();' | Select-Object -First 1).Trim()
        if ($version -match '^3\.5\.14(?:-|$)') { break }
    } catch { Start-Sleep -Seconds 2; continue }
    Start-Sleep -Seconds 2
}
if ($version -notmatch '^3\.5\.14(?:-|$)') { throw 'The dedicated fixture did not become a StarRocks 3.5.14 server before the readiness deadline.' }
# The FE answers before its first BE heartbeat succeeds, and DDL issued in that
# window fails without an alive backend. Wait, within the same deadline, until
# SHOW BACKENDS reports one alive BE (column 9, `Alive`).
$backendAlive = $false
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $backends = @(Invoke-FixtureMysql $docker $ContainerName 'root' '' 'SHOW BACKENDS;')
        if (@($backends | Where-Object { ([string]$_ -split "`t")[8] -eq 'true' }).Count -gt 0) { $backendAlive = $true; break }
    } catch { }
    Start-Sleep -Seconds 2
}
if (-not $backendAlive) { throw 'The dedicated fixture BE did not report alive before the readiness deadline.' }

$auditArchive = Join-Path $archiveDirectory 'auditloader-5.0.0.zip'
if (-not (Test-Path -LiteralPath $auditArchive -PathType Leaf)) {
    $cachedAuditArchive = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'cache\auditloader-5.0.0.zip')
    if (Test-Path -LiteralPath $cachedAuditArchive -PathType Leaf) {
        if ((Get-FixtureSha256 -Path $cachedAuditArchive) -ne $script:AuditLoaderSha256) {
            throw 'Cached AuditLoader archive SHA-256 did not match auditloader.manifest.json.'
        }
        Copy-Item -LiteralPath $cachedAuditArchive -Destination $auditArchive
    } else {
        Invoke-WebRequest -Uri $script:AuditLoaderUrl -OutFile $auditArchive -MaximumRedirection 0
    }
}
if ((Get-FixtureSha256 -Path $auditArchive) -ne $script:AuditLoaderSha256) {
    throw 'AuditLoader archive SHA-256 did not match auditloader.manifest.json.'
}

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
DISTRIBUTED BY HASH(`id`) BUCKETS 1
PROPERTIES ('replication_num'='1');
'@ + @"
CREATE USER IF NOT EXISTS doctor_reader IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral $doctorReaderPasswordHash);
CREATE USER IF NOT EXISTS audit_writer IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral $auditWriterPasswordHash);
CREATE USER IF NOT EXISTS workload_user IDENTIFIED BY PASSWORD $(ConvertTo-SqlLiteral $workloadPasswordHash);
ALTER USER doctor_reader IDENTIFIED WITH mysql_native_password AS $(ConvertTo-SqlLiteral $doctorReaderPasswordHash);
ALTER USER audit_writer IDENTIFIED WITH mysql_native_password AS $(ConvertTo-SqlLiteral $auditWriterPasswordHash);
ALTER USER workload_user IDENTIFIED WITH mysql_native_password AS $(ConvertTo-SqlLiteral $workloadPasswordHash);
GRANT SELECT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER doctor_reader;
GRANT INSERT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER audit_writer;
GRANT SELECT ON TABLE e2e_fixture.profile_rows TO USER workload_user;
"@
try {
    Invoke-FixtureMysql $docker $ContainerName 'root' '' $schemaSql | Out-Null
}
catch {
    # Do not include the submitted SQL: this batch carries only ciphertext
    # account hashes, but diagnostic output must stay credential-free too.
    throw 'Fixture schema and ciphertext-account bootstrap failed.'
}
# Verify the dedicated loader identity through the same local MySQL endpoint
# before embedding it in the fixture-only plugin configuration. This is an
# authentication check only; it does not query or change user data.
$auditWriterAuth = (Invoke-FixtureMysql $docker $ContainerName 'audit_writer' $auditWriterPassword 'SELECT 1;' | Select-Object -First 1).Trim()
if ($auditWriterAuth -ne '1') { throw 'The dedicated AuditLoader writer could not authenticate.' }
$fixtureRowCount = [int]((Invoke-FixtureMysql $docker $ContainerName 'root' '' 'SELECT COUNT(*) FROM e2e_fixture.profile_rows;' | Select-Object -First 1).Trim())
if ($fixtureRowCount -eq 0) {
    Invoke-FixtureMysql $docker $ContainerName 'root' '' "INSERT INTO e2e_fixture.profile_rows VALUES (1, 1, 'alpha'), (2, 1, 'beta'), (3, 2, 'gamma'), (4, 2, 'delta');" | Out-Null
}

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("starrocks-auditloader-" + [Guid]::NewGuid().ToString('N'))
$configuredArchive = Join-Path $tempRoot 'auditloader-configured.zip'
try {
    $contents = Join-Path $tempRoot 'contents'
    New-Item -ItemType Directory -Force -Path $contents | Out-Null
    Expand-Archive -LiteralPath $auditArchive -DestinationPath $contents -Force
    $configPath = Join-Path $contents 'plugin.conf'
    $config = Get-Content -LiteralPath $configPath -Raw
    foreach ($pair in @{
        # Docker allin1 redirects 8030 Stream Load to its local 8040 endpoint.
        # AuditLoader 5.0.0 does not retain Basic authentication on that redirect,
        # so this *fixture-only* value avoids the redirect. Production still uses
        # the official FE HTTP-port guidance and is not covered by this harness.
        max_batch_interval_sec = '5'; frontend_host_port = '127.0.0.1:8040'; database = 'starrocks_audit_db__';
        table = 'starrocks_audit_tbl__'; user = 'audit_writer'; password = $auditWriterPassword; secret_key = ''
    }.GetEnumerator()) {
        $config = [regex]::Replace($config, "(?m)^$([regex]::Escape($pair.Key))=.*$", "$($pair.Key)=$($pair.Value)")
    }
    [IO.File]::WriteAllText($configPath, $config, [Text.UTF8Encoding]::new($false))
    Compress-Archive -Path (Join-Path $contents '*') -DestinationPath $configuredArchive -CompressionLevel Optimal
    Invoke-FixtureDocker $docker @('cp', $configuredArchive, "$ContainerName`:/tmp/auditloader-configured.zip") -DiscardOutput
    $plugins = Invoke-FixtureMysql $docker $ContainerName 'root' '' 'SHOW PLUGINS;'
    if (($plugins -join "`n") -match "(?m)^AuditLoader\s") {
        Invoke-FixtureMysql $docker $ContainerName 'root' '' 'UNINSTALL PLUGIN AuditLoader;' | Out-Null
    }
    Invoke-FixtureMysql $docker $ContainerName 'root' '' 'INSTALL PLUGIN FROM "/tmp/auditloader-configured.zip";' | Out-Null
    $installed = Invoke-FixtureMysql $docker $ContainerName 'root' '' 'SHOW PLUGINS;'
    if (($installed -join "`n") -notmatch '(?m)^AuditLoader\s+AUDIT\s+.*\s+5\.0\.0\s+') {
        throw 'AuditLoader 5.0.0 was not installed in the dedicated fixture.'
    }
} finally {
    if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force }
}

$receipt = [ordered]@{
    fixture = 'sr-doctor-v0.1-e2e'; container = $ContainerName; image = $script:FixtureImage; image_digest = $script:FixtureDigest
    observed_version = $version; audit_loader_version = '5.0.0'; audit_loader_sha256 = $script:AuditLoaderSha256
    host_ports = [ordered]@{ http = $HttpPort; http2 = $Http2Port; heartbeat = $HeartbeatPort; mysql = $MySqlPort }
    data_root = $data; created_utc = [DateTime]::UtcNow.ToString('o')
    resources = Get-FixtureResourceReceipt -LowResource:$LowResource
}
$receiptPath = Join-Path $data 'fixture-bootstrap.json'
$receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $receiptPath -Encoding utf8
$receipt | ConvertTo-Json -Compress
