[CmdletBinding()]
param(
    # One official release to smoke, e.g. 3.4.10. Each run creates its own
    # container from starrocks/allin1-ubuntu:<Release>, captures Profiles for a
    # few fixed queries, then removes the container, its volumes and the image.
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Release,
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39631,
    [ValidateRange(60, 1800)][int]$ReadyTimeoutSeconds = 600,
    # Where captures are written; one folder per release.
    [string]$OutputRoot = (Join-Path $PSScriptRoot '..\..\fixtures\runtime\version-smoke'),
    # Keep the image after the run (default: removed, to bound disk use).
    [switch]$KeepImage
)

$ErrorActionPreference = 'Stop'
$docker = if ($DockerExecutable) { $DockerExecutable } else { (Get-Command docker).Source }
$image = "starrocks/allin1-ubuntu:$Release"
$name = "sr-doctor-vsmoke-$($Release.Replace('.', '-'))"
$label = 'sr-doctor.version-smoke'
$out = Join-Path ([IO.Path]::GetFullPath($OutputRoot)) $Release
$utf8 = [Text.UTF8Encoding]::new($false)
# The low-resource budget of every fixture in this repository.
$cpus = '2.5'; $memory = '5376m'; $feHeapMiB = 1024; $beMemLimitMiB = 3072

function Invoke-Docker {
    param([string[]]$Arguments, [switch]$AllowFailure)
    $previous = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = & $docker @Arguments 2>&1
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previous }
    if ($code -ne 0 -and -not $AllowFailure) { throw "docker $($Arguments[0]) failed ($code): $($output | Select-Object -Last 3)" }
    [pscustomobject]@{ Code = $code; Output = ($output | ForEach-Object { "$_" }) -join "`n" }
}

# One SQL batch as root inside the container; the result is copied out with
# `docker cp`, so captured bytes never pass through a console encoding.
function Invoke-Sql {
    param([Parameter(Mandatory)][string]$Tag, [Parameter(Mandatory)][string]$Sql, [string]$Database = '', [switch]$AllowFailure)
    $local = Join-Path $script:work "$Tag.sql"
    [IO.File]::WriteAllText($local, $Sql.Replace("`r`n", "`n"), $utf8)
    Invoke-Docker @('cp', $local, "${name}:/tmp/vsmoke/$Tag.sql") | Out-Null
    $db = if ($Database) { " --database=$Database" } else { '' }
    $run = Invoke-Docker @('exec', $name, 'bash', '-c', "mysql --batch --skip-column-names --raw --host=127.0.0.1 --port=9030 --user=root$db < /tmp/vsmoke/$Tag.sql > /tmp/vsmoke/$Tag.out 2> /tmp/vsmoke/$Tag.err") -AllowFailure
    $result = Join-Path $script:work "$Tag.out"
    Invoke-Docker @('cp', "${name}:/tmp/vsmoke/$Tag.out", $result) | Out-Null
    if ($run.Code -ne 0) {
        $err = Join-Path $script:work "$Tag.err"
        Invoke-Docker @('cp', "${name}:/tmp/vsmoke/$Tag.err", $err) -AllowFailure | Out-Null
        $message = if (Test-Path $err) { (Get-Content -Raw $err).Trim() } else { 'unknown error' }
        if ($AllowFailure) { return [pscustomobject]@{ Ok = $false; Text = $message } }
        throw "SQL batch $Tag failed: $message"
    }
    [pscustomobject]@{ Ok = $true; Text = [IO.File]::ReadAllText($result, $utf8) }
}

function Remove-Smoke {
    Invoke-Docker @('rm', '-f', '-v', $name) -AllowFailure | Out-Null
    if (-not $KeepImage) { Invoke-Docker @('image', 'rm', '-f', $image) -AllowFailure | Out-Null }
}

$script:work = Join-Path ([IO.Path]::GetTempPath()) ('vsmoke-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $script:work, $out | Out-Null
$receipt = [ordered]@{ release = $Release; image = $image; started_utc = [DateTime]::UtcNow.ToString('o'); resources = [ordered]@{ cpus = $cpus; memory = $memory; fe_heap_mib = $feHeapMiB; be_mem_limit_mib = $beMemLimitMiB; swap = 'disabled' }; queries = @() }
try {
    $existing = Invoke-Docker @('inspect', $name, '--format', "{{index .Config.Labels `"$label`"}}") -AllowFailure
    if ($existing.Code -eq 0) {
        if ($existing.Output.Trim() -ne $Release) { throw "A container named $name exists and is not this smoke fixture." }
        Invoke-Docker @('rm', '-f', '-v', $name) | Out-Null
    }
    Write-Host "==> pull $image"
    Invoke-Docker @('pull', $image) | Out-Null
    $receipt.image_digest = (Invoke-Docker @('image', 'inspect', $image, '--format', '{{index .RepoDigests 0}}')).Output.Trim()

    Write-Host "==> create $name (cpus $cpus, memory $memory, no swap)"
    Invoke-Docker @('create', '--name', $name, '--label', "$label=$Release", '--cpus', $cpus, '--memory', $memory, '--memory-swap', $memory,
        '--pids-limit', '1024', '--security-opt', 'no-new-privileges', '--publish', "127.0.0.1:${MySqlPort}:9030", $image) | Out-Null
    # FE heap and BE mem_limit inside the container limit, set before first start.
    foreach ($edit in @(
        @{ path = '/data/deploy/starrocks/fe/conf/fe.conf'; text = "`nJAVA_OPTS=`"-Xmx${feHeapMiB}m -XX:+UseG1GC`"`n" },
        @{ path = '/data/deploy/starrocks/be/conf/be.conf'; text = "`nmem_limit = ${beMemLimitMiB}M`n" })) {
        $local = Join-Path $script:work ([IO.Path]::GetFileName($edit.path))
        $copied = Invoke-Docker @('cp', "${name}:$($edit.path)", $local) -AllowFailure
        if ($copied.Code -ne 0) { throw "This image has no $($edit.path); the low-resource settings cannot be applied." }
        [IO.File]::AppendAllText($local, $edit.text, $utf8)
        Invoke-Docker @('cp', $local, "${name}:$($edit.path)") | Out-Null
    }
    Invoke-Docker @('start', $name) | Out-Null
    Invoke-Docker @('exec', $name, 'mkdir', '-p', '/tmp/vsmoke') | Out-Null

    Write-Host "==> wait for FE and BE (up to $ReadyTimeoutSeconds s)"
    $deadline = [DateTime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
    $ready = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        $probe = Invoke-Sql -Tag 'ready' -Sql "SELECT current_version();`nSHOW BACKENDS;" -AllowFailure
        if ($probe.Ok -and $probe.Text -match "(?m)^$([regex]::Escape($Release))" -and $probe.Text -match "`ttrue`t") { $ready = $true; break }
        Start-Sleep -Seconds 5
    }
    if (-not $ready) { throw "StarRocks $Release did not become ready." }
    $receipt.current_version = ((Invoke-Sql -Tag 'version' -Sql 'SELECT current_version();').Text -split "`n")[0].Trim()

    Write-Host '==> dataset'
    # VALUES plus self-doubling inserts: works on every audited line (no generate_series before 3.1).
    $values = (0..9 | ForEach-Object { "($_, $($_ % 4), $($_ * 7))" }) -join ', '
    $dataset = @"
CREATE DATABASE IF NOT EXISTS vsmoke;
USE vsmoke;
CREATE TABLE fact (k INT, g INT, v BIGINT) DUPLICATE KEY(k) DISTRIBUTED BY HASH(k) BUCKETS 4 PROPERTIES ("replication_num" = "1");
CREATE TABLE dim (g INT, name VARCHAR(32)) DUPLICATE KEY(g) DISTRIBUTED BY HASH(g) BUCKETS 1 PROPERTIES ("replication_num" = "1");
INSERT INTO dim VALUES (0, 'zero'), (1, 'one'), (2, 'two'), (3, 'three');
INSERT INTO fact VALUES $values;
"@ + ((1..15 | ForEach-Object { "INSERT INTO fact SELECT k + $([math]::Pow(2, $_ - 1) * 10), (k + $_) % 4, v + $_ FROM fact;" }) -join "`n")
    Invoke-Sql -Tag 'dataset' -Sql $dataset | Out-Null

    $queries = [ordered]@{
        join_agg = 'SELECT d.name, count(*), sum(f.v) FROM fact f JOIN dim d ON f.g = d.g GROUP BY d.name ORDER BY d.name'
        scan_topn = 'SELECT k, v FROM fact WHERE v > 100 ORDER BY v DESC LIMIT 10'
        shuffle_join = 'SELECT count(*) FROM fact a JOIN [shuffle] fact b ON a.k = b.k WHERE a.g = 1'
    }
    foreach ($id in $queries.Keys) {
        Write-Host "==> query $id"
        $run = Invoke-Sql -Tag "q-$id" -Database 'vsmoke' -Sql "SET enable_profile = true;`nSET pipeline_profile_level = 1;`n$($queries[$id]);`nSELECT last_query_id();" -AllowFailure
        $entry = [ordered]@{ id = $id; sql = $queries[$id]; executed = $run.Ok }
        if (-not $run.Ok) { $entry.error = $run.Text; $receipt.queries += $entry; continue }
        $queryId = (($run.Text.Trim() -split "`n") | Select-Object -Last 1).Trim()
        $entry.query_id = $queryId
        # Profiles report asynchronously; poll get_query_profile(), then the FE REST API before 3.0.2.
        $profile = $null
        for ($attempt = 0; $attempt -lt 20 -and -not $profile; $attempt++) {
            Start-Sleep -Seconds 2
            $fetched = Invoke-Sql -Tag "p-$id" -Sql "SELECT get_query_profile('$queryId');" -AllowFailure
            if ($fetched.Ok -and $fetched.Text -match 'Summary:') { $profile = $fetched.Text; $entry.source = 'get_query_profile'; break }
            $rest = Invoke-Docker @('exec', $name, 'bash', '-c', "curl -s -u root: 'http://127.0.0.1:8030/api/profile?query_id=$queryId' > /tmp/vsmoke/rest-$id.json") -AllowFailure
            if ($rest.Code -eq 0) {
                $restLocal = Join-Path $script:work "rest-$id.json"
                Invoke-Docker @('cp', "${name}:/tmp/vsmoke/rest-$id.json", $restLocal) | Out-Null
                # 2.5 answers with the plain text Profile; later releases wrap it in JSON.
                $body = [IO.File]::ReadAllText($restLocal, $utf8)
                try { $json = $body | ConvertFrom-Json } catch { $json = $null }
                $text = if ($json.data.profile) { $json.data.profile } elseif ($json.profile) { $json.profile } elseif (-not $json) { $body } else { $null }
                if ($text -and $text -match 'Summary:') { $profile = $text; $entry.source = 'fe_rest_api_profile' }
            }
        }
        if (-not $profile) { $entry.profile = 'unavailable'; $receipt.queries += $entry; continue }
        # --raw keeps the Profile's own newlines; nothing is unescaped here.
        $profile = $profile.TrimEnd() + "`n"
        [IO.File]::WriteAllText((Join-Path $out "$id.profile.txt"), $profile, $utf8)
        $entry.profile = "$id.profile.txt"
        # The official analysis, where the release has it (ExplainAnalyzer, 3.1+).
        $analyzed = Invoke-Sql -Tag "a-$id" -Sql "ANALYZE PROFILE FROM '$queryId';" -AllowFailure
        if ($analyzed.Ok -and $analyzed.Text.Trim()) {
            [IO.File]::WriteAllText((Join-Path $out "$id.analyze.txt"), $analyzed.Text, $utf8)
            $entry.analyze = "$id.analyze.txt"
        }
        $receipt.queries += $entry
    }
    $receipt.status = if (@($receipt.queries | Where-Object { $_.profile -and $_.profile -ne 'unavailable' }).Count -gt 0) { 'captured' } else { 'no_profile' }
}
catch {
    $receipt.status = 'failed'
    $receipt.error = $_.Exception.Message
}
finally {
    Write-Host "==> remove container$(if (-not $KeepImage) { ', volumes and image' } else { ' and volumes' })"
    Remove-Smoke
    Remove-Item -LiteralPath $script:work -Recurse -Force -ErrorAction SilentlyContinue
    $receipt.finished_utc = [DateTime]::UtcNow.ToString('o')
    $receipt.removed = [ordered]@{ container = $true; volumes = $true; image = -not $KeepImage }
    [IO.File]::WriteAllText((Join-Path $out 'capture.json'), ($receipt | ConvertTo-Json -Depth 6), $utf8)
    Write-Host "==> $Release $($receipt.status)"
}
if ($receipt.status -eq 'failed') { exit 1 }
