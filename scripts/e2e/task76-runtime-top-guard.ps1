# Task 76 (T76-5): run the controlled-execution session guard against the
# fixed-digest StarRocks 3.5.14 allin1 fixture in three dialect phases, then
# remove the owned container and its volumes.
# Usage: pwsh -NoProfile -File scripts/e2e/task76-runtime-top-guard.ps1 [-MySqlPort 39176]
param([ValidateRange(1, 65535)][int]$MySqlPort = 39176)

$ErrorActionPreference = 'Stop'
$image = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$container = 'sr-doctor-t76-guard'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent

function Fe-Sql([string]$Sql) {
    docker exec $container mysql '-h127.0.0.1' '-P9030' '-uroot' -N -e $Sql
    if ($LASTEXITCODE -ne 0) { throw "fixture SQL failed: $Sql" }
}

if (docker ps -a --filter "name=^$container$" --format '{{.Names}}') { throw "$container already exists; remove it first." }
docker run -d --name $container --label com.sr-doctor.task=t76-guard -p "127.0.0.1:${MySqlPort}:9030" $image | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'docker run failed' }
$results = [ordered]@{}
try {
    $deadline = (Get-Date).AddMinutes(5)
    do {
        Start-Sleep -Seconds 5
        $alive = docker exec $container mysql '-h127.0.0.1' '-P9030' '-uroot' -N -e 'SHOW BACKENDS' 2>$null
    } until (($LASTEXITCODE -eq 0 -and "$alive" -match '\btrue\b') -or (Get-Date) -gt $deadline)
    if ((Get-Date) -gt $deadline) { throw 'fixture did not become ready within 5 minutes' }
    Write-Host "fixture ready: $(Fe-Sql 'SELECT current_version()')"
    # Fixture setup by the driver (the test itself only reads): one small table
    # so the profiled SELECT runs on BE and produces a runtime Profile.
    # A just-alive BE has not reported disk capacity yet, so FE briefly rejects
    # table creation ("backends without enough disk space"): retry until it lands.
    Fe-Sql 'CREATE DATABASE IF NOT EXISTS t76'
    $deadline = (Get-Date).AddMinutes(3)
    do {
        Start-Sleep -Seconds 5
        docker exec $container mysql '-h127.0.0.1' '-P9030' '-uroot' -N -e 'CREATE TABLE IF NOT EXISTS t76.t (k INT) DUPLICATE KEY(k) DISTRIBUTED BY HASH(k) BUCKETS 1 PROPERTIES ("replication_num" = "1")' 2>$null
    } until ($LASTEXITCODE -eq 0 -or (Get-Date) -gt $deadline)
    if ($LASTEXITCODE -ne 0) { throw 'fixture table was not created within 3 minutes' }
    Fe-Sql 'INSERT INTO t76.t VALUES (1), (2), (3)'
    # How the real FE renders the session snapshot the connector parses.
    Write-Host "snapshot: $(Fe-Sql 'SELECT @@session.enable_profile, @@session.pipeline_profile_level, @@session.query_timeout, @@session.sql_dialect')"

    $env:STARROCKS_PORT = "$MySqlPort"
    $env:CARGO_TARGET_DIR = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    foreach ($phase in 'starrocks', 'trino_downgrade_on', 'trino_downgrade_off') {
        if ($phase -eq 'trino_downgrade_on') { Fe-Sql "SET GLOBAL sql_dialect = 'trino'" }
        if ($phase -eq 'trino_downgrade_off') { Fe-Sql 'SET GLOBAL enable_dialect_downgrade = false' }
        $env:SR_DOCTOR_T76 = $phase
        Push-Location $repo
        try { cargo test --locked -p doctor-connector --test runtime_top_docker -- --ignored --nocapture 2>&1 | Where-Object { "$_" -notmatch 'serde attribute|ts-rs failed|^\s*$|Compiling|Finished|Running' } | ForEach-Object { Write-Host $_ } }
        finally { Pop-Location }
        $results[$phase] = $LASTEXITCODE
        Write-Host "phase $phase exit $LASTEXITCODE"
    }
}
finally {
    docker rm -f -v $container | Out-Null
    Write-Host "removed $container and its volumes"
}
$results.GetEnumerator() | ForEach-Object { "{0}={1}" -f $_.Key, $_.Value }
if (@($results.Values | Where-Object { $_ -ne 0 }).Count -gt 0) { exit 1 }
