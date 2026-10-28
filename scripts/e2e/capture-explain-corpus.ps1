[CmdletBinding()]
param(
    # Test infrastructure only: captures official 3.5.14 `EXPLAIN COSTS` text for
    # the SQL of the runtime oracle corpus from an isolated, already bootstrapped
    # allin1 fixture, so the offline test can compare it with each case's runtime
    # Profile Topology. EXPLAIN plans only; no corpus query is executed.
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514-explain-oracle',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [string[]]$CaseIds = @(),
    # The runtime corpus whose SQL is planned. Its datasets are loaded unless
    # -SkipDataset, which runs only each dataset's closing row-count SELECT.
    [ValidatePattern('^oracle-corpus(?:-[a-z0-9]+)*$')][string]$CorpusName = 'oracle-corpus',
    [switch]$SkipDataset
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $repoRoot 'dev\starrocks-3.5.14\common.ps1')
$corpusRoot = Join-Path $repoRoot "fixtures\runtime\3.5.14\$CorpusName"
if (-not (Test-Path -LiteralPath $corpusRoot -PathType Container)) { throw "Corpus directory $CorpusName does not exist." }
$explainRoot = Join-Path $repoRoot "fixtures\explain\starrocks-3.5.14\$CorpusName"
New-Item -ItemType Directory -Force -Path $explainRoot | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)

$docker = Get-FixtureDockerExecutable $DockerExecutable
Assert-FixtureContainerName $ContainerName
$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $ContainerName
if ($null -eq $identity -or $identity.Image -cne $script:FixtureImage -or $identity.Label -cne $script:FixtureLabelValue) {
    throw 'The named container is not the dedicated fixed-digest labelled fixture.'
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('explain-corpus-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'mkdir', '-p', '/tmp/oracle') -DiscardOutput

# Same transport as capture-oracle-corpus.ps1: the batch runs as fixture root
# inside the container and its output is copied out byte-for-byte.
function Invoke-ContainerBatch {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Sql, [string]$Database = '')
    $hostSql = Join-Path $work "$Name.sql"
    [IO.File]::WriteAllText($hostSql, $Sql.Replace("`r`n", "`n"), $utf8)
    Invoke-FixtureDocker $docker @('cp', $hostSql, "$ContainerName`:/tmp/oracle/$Name.sql") -DiscardOutput
    $dbArgument = if ($Database) { " --database=$Database" } else { '' }
    & $docker exec $ContainerName sh -c "mysql --batch --skip-column-names --raw --host=127.0.0.1 --port=9030 --user=root$dbArgument < /tmp/oracle/$Name.sql > /tmp/oracle/$Name.out 2> /tmp/oracle/$Name.err"
    $exitCode = $LASTEXITCODE
    $hostOut = Join-Path $work "$Name.out"
    Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/oracle/$Name.out", $hostOut) -DiscardOutput
    if ($exitCode -ne 0) {
        $hostErr = Join-Path $work "$Name.err"
        Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/oracle/$Name.err", $hostErr) -DiscardOutput
        $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @([IO.File]::ReadAllLines($hostErr, $utf8)) -ExitCode $exitCode
        throw "Batch $Name failed (exit $exitCode; code=$($diagnostic.error_code); line=$($diagnostic.line_number))."
    }
    $hostOut
}

function Read-TableCounts([string]$CountsOut) {
    $counts = [ordered]@{}
    foreach ($line in [IO.File]::ReadAllLines($CountsOut, $utf8)) {
        $cells = $line -split "`t"
        if ($cells.Count -eq 2 -and $cells[1] -match '^[0-9]+$') { $counts[$cells[0]] = [long]$cells[1] }
    }
    $counts
}

$serverVersion = (([IO.File]::ReadAllText((Invoke-ContainerBatch 'version' 'SELECT current_version();'), $utf8)).Trim())
if ($serverVersion -notmatch '^3\.5\.14-23a56ec') { throw "Unexpected server version $serverVersion." }

# The runtime corpus datasets, byte-identical: `ANALYZE FULL ... WITH SYNC MODE`
# makes the statistics (and so the cost-based plan) a function of these files.
$runtimeManifest = Get-Content -LiteralPath (Join-Path $corpusRoot 'manifest.json') -Raw | ConvertFrom-Json -DateKind String
# `dataset.sql` first: supplementary `dataset-*.sql` files build on its tables
# (the order of capture-oracle-corpus.ps1).
$datasets = @()
$datasetFiles = @(Get-Item -LiteralPath (Join-Path $corpusRoot 'dataset.sql')) +
    @(Get-ChildItem -LiteralPath $corpusRoot -Filter 'dataset-*.sql' | Sort-Object Name)
foreach ($file in $datasetFiles) {
    $batchName = [IO.Path]::GetFileNameWithoutExtension($file.Name)
    $countsOut = if ($SkipDataset) {
        $countSql = @([IO.File]::ReadAllLines($file.FullName, $utf8) | Where-Object { $_.Trim() }) | Select-Object -Last 1
        Invoke-ContainerBatch "$batchName.counts" $countSql 'oracle_corpus'
    } else {
        Invoke-ContainerBatch $batchName ([IO.File]::ReadAllText($file.FullName, $utf8))
    }
    $datasets += [ordered]@{ dataset_file = $file.Name; dataset_sha256 = Get-FixtureSha256 $file.FullName; table_rows = Read-TableCounts $countsOut }
}
$expectedRows = [ordered]@{}
foreach ($table in $runtimeManifest.table_rows.PSObject.Properties) { $expectedRows[$table.Name] = [long]$table.Value }
foreach ($supplement in @($runtimeManifest.supplementary_datasets)) {
    foreach ($table in $supplement.table_rows.PSObject.Properties) { $expectedRows[$table.Name] = [long]$table.Value }
}
foreach ($dataset in $datasets) {
    foreach ($table in $dataset.table_rows.GetEnumerator()) {
        if ($expectedRows[$table.Key] -ne $table.Value) { throw "Table $($table.Key) row count differs from the runtime corpus manifest." }
    }
}

$caseFiles = @(Get-ChildItem -LiteralPath $corpusRoot -Filter 'c*.sql' | Sort-Object Name)
if ($CaseIds.Count -gt 0) { $caseFiles = @($caseFiles | Where-Object { $CaseIds -contains $_.BaseName }) }
$cases = @()
foreach ($caseFile in $caseFiles) {
    $caseId = $caseFile.BaseName
    $runtimeCase = @($runtimeManifest.cases | Where-Object { $_.case_id -ceq $caseId })
    if ($runtimeCase.Count -ne 1 -or $runtimeCase[0].sql_sha256 -cne (Get-FixtureSha256 $caseFile.FullName)) {
        throw "Case $caseId SQL does not match the runtime corpus manifest."
    }
    # Corpus SQL is one statement per line: session `SET`s, then the query. The
    # same `SET`s precede `EXPLAIN COSTS` in the same session, as in the run.
    $statements = @([IO.File]::ReadAllLines($caseFile.FullName, $utf8) | Where-Object { $_.Trim() })
    # A case answered by EXPLAIN ANALYZE (spill, see capture-oracle-corpus.ps1)
    # is planned as the query it wraps.
    $query = $statements[-1].Trim() -replace '^(?i)EXPLAIN ANALYZE\s+', ''
    $preamble = @($statements | Select-Object -SkipLast 1)
    if ($query -match '^(?i)SET\s' -or @($preamble | Where-Object { $_ -notmatch '^(?i)SET\s' }).Count -gt 0) {
        throw "Case $caseId is not session SETs followed by one query."
    }
    $batch = (@($preamble) + "EXPLAIN COSTS $query") -join "`n"
    $explainOut = Invoke-ContainerBatch "$caseId.explain" "$batch`n" 'oracle_corpus'
    $target = Join-Path $explainRoot "$caseId.costs.txt"
    Copy-Item -LiteralPath $explainOut -Destination $target -Force
    $lineCount = [IO.File]::ReadAllLines($target, $utf8).Count
    if ($lineCount -eq 0) { throw "Case $caseId returned no EXPLAIN rows." }
    $cases += [ordered]@{
        case_id = $caseId
        sql_file = "../../../runtime/3.5.14/$CorpusName/$caseId.sql"; sql_sha256 = Get-FixtureSha256 $caseFile.FullName
        explain_file = "$caseId.costs.txt"; explain_sha256 = Get-FixtureSha256 $target
        line_count = $lineCount
        captured_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    }
    Write-Host "captured $caseId ($lineCount EXPLAIN rows)"
}

if ($CaseIds.Count -eq 0) {
    $manifest = [ordered]@{
        schema = 'sr-doctor.explain-corpus.v1'
        purpose = "official StarRocks 3.5.14 EXPLAIN COSTS for the runtime $CorpusName SQL, versus each runtime Profile Topology"
        server_version = $serverVersion
        image = $script:FixtureImage
        image_digest = $script:FixtureDigest
        source_kind = 'mysql-batch-raw-output-lines'
        runtime_corpus = "../../../runtime/3.5.14/$CorpusName/manifest.json"
        datasets = $datasets
        capture_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
        cases = $cases
    }
    [IO.File]::WriteAllText((Join-Path $explainRoot 'manifest.json'), ($manifest | ConvertTo-Json -Depth 6) + "`n", $utf8)
}
Remove-Item -LiteralPath $work -Recurse -Force
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'rm', '-rf', '/tmp/oracle') -DiscardOutput
[ordered]@{ status = 'PASS'; container = $ContainerName; server_version = $serverVersion; cases = $cases.Count } | ConvertTo-Json -Compress -Depth 4
