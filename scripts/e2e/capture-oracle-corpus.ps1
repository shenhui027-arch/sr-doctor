[CmdletBinding()]
param(
    # Test infrastructure only: captures official 3.5.14 FE answers
    # (`get_query_profile` + `ANALYZE PROFILE`) from an isolated, already
    # bootstrapped fixture. It never touches any container other than this one.
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514-oracle',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [string[]]$CaseIds = @(),
    [switch]$SkipDataset,
    [ValidateRange(5, 120)][int]$ProfilePollSeconds = 60,
    # `multi-be`: the 1 FE + 3 BE fixture of dev/starrocks-3.5.14-multi-be
    # (the container is always its FE; root authenticates with the password
    # derived from the process-only STARROCKS_E2E_SECRET).
    [ValidateSet('allin1', 'multi-be')][string]$Topology = 'allin1',
    [ValidatePattern('^oracle-corpus(?:-[a-z0-9]+)*$')][string]$CorpusName = 'oracle-corpus'
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if ($Topology -eq 'multi-be') {
    . (Join-Path $repoRoot 'dev\starrocks-3.5.14-multi-be\common.ps1')
    $ContainerName = $script:MultiBeFeName
    $expectedImage = $script:MultiBeFeImage
    $expectedLabel = $script:MultiBeLabelValue
    $rootPassword = Get-MultiBeRootPassword
} else {
    . (Join-Path $repoRoot 'dev\starrocks-3.5.14\common.ps1')
    $expectedImage = $script:FixtureImage
    $expectedLabel = $script:FixtureLabelValue
    $rootPassword = ''
}
$corpusRoot = Join-Path $repoRoot "fixtures\runtime\3.5.14\$CorpusName"
if (-not (Test-Path -LiteralPath $corpusRoot -PathType Container)) { throw "Corpus directory $CorpusName does not exist." }
$utf8 = [Text.UTF8Encoding]::new($false)

$docker = Get-FixtureDockerExecutable $DockerExecutable
Assert-FixtureContainerName $ContainerName
$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $ContainerName
if ($null -eq $identity -or $identity.Image -cne $expectedImage -or $identity.Label -cne $expectedLabel) {
    throw 'The named container is not the dedicated fixed-digest labelled fixture.'
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('oracle-corpus-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'mkdir', '-p', '/tmp/oracle') -DiscardOutput

# Run one SQL batch as fixture root inside the container. Output is redirected
# inside the container and copied out with `docker cp`, so the captured bytes
# never pass through a host console encoding.
function Invoke-ContainerBatch {
    # -ExpectFailure returns the FE error text and fails if the batch succeeds.
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Sql, [string]$Database = '', [switch]$ExpectFailure)
    $hostSql = Join-Path $work "$Name.sql"
    [IO.File]::WriteAllText($hostSql, $Sql.Replace("`r`n", "`n"), $utf8)
    Invoke-FixtureDocker $docker @('cp', $hostSql, "$ContainerName`:/tmp/oracle/$Name.sql") -DiscardOutput
    $dbArgument = if ($Database) { " --database=$Database" } else { '' }
    $command = "mysql --batch --skip-column-names --raw --host=127.0.0.1 --port=9030 --user=root$dbArgument < /tmp/oracle/$Name.sql > /tmp/oracle/$Name.out 2> /tmp/oracle/$Name.err"
    if ($rootPassword) {
        # Inherited by `docker exec -e MYSQL_PWD`; never on a command line.
        $previousMysqlPassword = [Environment]::GetEnvironmentVariable('MYSQL_PWD', 'Process')
        [Environment]::SetEnvironmentVariable('MYSQL_PWD', $rootPassword, 'Process')
        try { & $docker exec -e MYSQL_PWD $ContainerName sh -c $command }
        finally { [Environment]::SetEnvironmentVariable('MYSQL_PWD', $previousMysqlPassword, 'Process') }
    } else {
        & $docker exec $ContainerName sh -c $command
    }
    $exitCode = $LASTEXITCODE
    $hostOut = Join-Path $work "$Name.out"
    Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/oracle/$Name.out", $hostOut) -DiscardOutput
    if ($ExpectFailure) {
        if ($exitCode -eq 0) { throw "Batch $Name was expected to fail but succeeded." }
        $hostErr = Join-Path $work "$Name.err"
        Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/oracle/$Name.err", $hostErr) -DiscardOutput
        return ([IO.File]::ReadAllText($hostErr, $utf8)).Trim()
    }
    if ($exitCode -ne 0) {
        $hostErr = Join-Path $work "$Name.err"
        Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/oracle/$Name.err", $hostErr) -DiscardOutput
        $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @([IO.File]::ReadAllLines($hostErr, $utf8)) -ExitCode $exitCode
        throw "Batch $Name failed (exit $exitCode; code=$($diagnostic.error_code); line=$($diagnostic.line_number))."
    }
    $hostOut
}

$serverVersion = (([IO.File]::ReadAllText((Invoke-ContainerBatch 'version' 'SELECT current_version();'), $utf8)).Trim())
if ($serverVersion -notmatch '^3\.5\.14-23a56ec') { throw "Unexpected server version $serverVersion." }

$datasetPath = Join-Path $corpusRoot 'dataset.sql'
function Read-TableCounts([string]$CountsOut) {
    $counts = [ordered]@{}
    foreach ($line in [IO.File]::ReadAllLines($CountsOut, $utf8)) {
        $cells = $line -split "`t"
        if ($cells.Count -eq 2 -and $cells[1] -match '^[0-9]+$') { $counts[$cells[0]] = [long]$cells[1] }
    }
    $counts
}
if (-not $SkipDataset) {
    $countsOut = Invoke-ContainerBatch 'dataset' ([IO.File]::ReadAllText($datasetPath, $utf8))
    $tableCounts = Read-TableCounts $countsOut
} else {
    $tableCounts = [ordered]@{}
    $countsOut = Invoke-ContainerBatch 'counts' 'SELECT ''customers'', COUNT(*) FROM customers UNION ALL SELECT ''products'', COUNT(*) FROM products UNION ALL SELECT ''sales'', COUNT(*) FROM sales UNION ALL SELECT ''returns'', COUNT(*) FROM returns UNION ALL SELECT ''events'', COUNT(*) FROM events;' 'oracle_corpus'
    foreach ($line in [IO.File]::ReadAllLines($countsOut, $utf8)) {
        $cells = $line -split "`t"
        if ($cells.Count -eq 2) { $tableCounts[$cells[0]] = [long]$cells[1] }
    }
}
# Supplementary datasets (`dataset-*.sql`) only add new tables for later cases;
# `dataset.sql` and its tables stay byte-identical, so earlier cases keep their
# provenance. Each supplementary file ends with its own row-count SELECT, which
# is all that runs under -SkipDataset.
$supplementary = @()
foreach ($file in @(Get-ChildItem -LiteralPath $corpusRoot -Filter 'dataset-*.sql' | Sort-Object Name)) {
    $batchName = [IO.Path]::GetFileNameWithoutExtension($file.Name)
    $countsOut = if ($SkipDataset) {
        $countSql = @([IO.File]::ReadAllLines($file.FullName, $utf8) | Where-Object { $_.Trim() }) | Select-Object -Last 1
        Invoke-ContainerBatch "$batchName.counts" $countSql 'oracle_corpus'
    } else {
        Invoke-ContainerBatch $batchName ([IO.File]::ReadAllText($file.FullName, $utf8))
    }
    $supplementary += [ordered]@{
        dataset_file = $file.Name; dataset_sha256 = Get-FixtureSha256 $file.FullName
        table_rows = Read-TableCounts $countsOut
    }
}

$topologyEvidence = $null
if ($Topology -eq 'multi-be') {
    # Evidence that the answers really span several BEs: alive backends and the
    # tablet placement of every corpus table (replication_num=1, so each tablet
    # lives on exactly one BE).
    $backendsOut = Invoke-ContainerBatch 'backends' 'SHOW BACKENDS;'
    $alive = @([IO.File]::ReadAllLines($backendsOut, $utf8) | Where-Object { ($_ -split "`t")[8] -eq 'true' }).Count
    if ($alive -ne $script:MultiBeBackendCount) { throw "Expected $script:MultiBeBackendCount alive BEs, found $alive." }
    $placementOut = Invoke-ContainerBatch 'placement' "SELECT t.TABLE_NAME, b.BE_ID, COUNT(*) FROM information_schema.be_tablets b JOIN information_schema.tables_config t ON b.TABLE_ID = t.TABLE_ID WHERE t.TABLE_SCHEMA = 'oracle_corpus' GROUP BY t.TABLE_NAME, b.BE_ID ORDER BY t.TABLE_NAME, b.BE_ID;"
    $placement = [ordered]@{}
    $backendIds = [Collections.Generic.SortedSet[long]]::new()
    foreach ($line in [IO.File]::ReadAllLines($placementOut, $utf8)) {
        $cells = $line -split "`t"
        if ($cells.Count -ne 3) { continue }
        [void]$backendIds.Add([long]$cells[1])
        if (-not $placement.Contains($cells[0])) { $placement[$cells[0]] = [ordered]@{} }
        $placement[$cells[0]]["be_$($cells[1])"] = [long]$cells[2]
    }
    if ($backendIds.Count -ne $script:MultiBeBackendCount) { throw 'Corpus tablets do not span every BE.' }
    $topologyEvidence = [ordered]@{
        kind = 'split_fe_be'; frontends = 1; backends_alive = $alive
        fe_image = $script:MultiBeFeImage; be_image = $script:MultiBeBeImage
        tablets_per_backend = $placement
    }
}

$caseFiles = @(Get-ChildItem -LiteralPath $corpusRoot -Filter 'c*.sql' | Sort-Object Name)
if ($CaseIds.Count -gt 0) { $caseFiles = @($caseFiles | Where-Object { $CaseIds -contains $_.BaseName }) }
$cases = @()
foreach ($caseFile in $caseFiles) {
    $caseId = $caseFile.BaseName
    $caseSql = [IO.File]::ReadAllText($caseFile.FullName, $utf8)
    # Standard Runtime Top session shape: Level 1 merged Profile in the same session.
    $batch = "SET enable_profile=true;`nSET pipeline_profile_level=1;`n$($caseSql.TrimEnd())`nSELECT last_query_id();`n"
    $runOut = Invoke-ContainerBatch "$caseId.run" $batch 'oracle_corpus'
    $queryId = ([IO.File]::ReadAllLines($runOut, $utf8) | Select-Object -Last 1).Trim().ToLowerInvariant()
    if ($queryId -notmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') { throw "Case $caseId returned no query id." }
    $deadline = [DateTime]::UtcNow.AddSeconds($ProfilePollSeconds)
    $profileLength = 0
    while ([DateTime]::UtcNow -lt $deadline) {
        $lengthOut = Invoke-ContainerBatch "$caseId.len" "SELECT LENGTH(get_query_profile('$queryId'));"
        $text = ([IO.File]::ReadAllText($lengthOut, $utf8)).Trim()
        if ($text -match '^[1-9][0-9]*$') { $profileLength = [long]$text; break }
        Start-Sleep -Seconds 1
    }
    if ($profileLength -le 0) { throw "Case $caseId Profile was not published within $ProfilePollSeconds s." }
    $profileOut = Invoke-ContainerBatch "$caseId.profile" "SELECT get_query_profile('$queryId');"
    $profileTarget = Join-Path $corpusRoot "$caseId.profile.txt"
    $analyzeTarget = Join-Path $corpusRoot "$caseId.analyze.txt"
    Copy-Item -LiteralPath $profileOut -Destination $profileTarget -Force
    $case = [ordered]@{ case_id = $caseId }
    if ($caseSql -match '(?im)^EXPLAIN ANALYZE ') {
        # ANALYZE PROFILE re-parses the stored Profile text
        # (StmtExecutor.java:1780-1782), and RuntimeProfileParser.java:242-244
        # cannot read a DOUBLE_VALUE counter such as spill's
        # `SkewMemTableSkewRatio`. EXPLAIN ANALYZE runs the same ExplainAnalyzer
        # on the in-memory Profile of this run (StmtExecutor.java:837-847), whose
        # text is the Profile captured above. Its rows precede the query id.
        $runText = [IO.File]::ReadAllText($runOut, $utf8)
        $analyzeText = $runText.Substring(0, $runText.TrimEnd("`n").LastIndexOf("`n") + 1)
        if ($analyzeText -notmatch "QueryId: $queryId") { throw "Case $caseId EXPLAIN ANALYZE output is not this query." }
        [IO.File]::WriteAllText($analyzeTarget, $analyzeText, $utf8)
        $case['oracle_statement'] = 'EXPLAIN ANALYZE'
        $case['analyze_profile_refusal'] = Invoke-ContainerBatch "$caseId.analyze" "ANALYZE PROFILE FROM '$queryId';" -ExpectFailure
    } else {
        $analyzeOut = Invoke-ContainerBatch "$caseId.analyze" "ANALYZE PROFILE FROM '$queryId';"
        Copy-Item -LiteralPath $analyzeOut -Destination $analyzeTarget -Force
    }
    foreach ($entry in ([ordered]@{
        sql_file = "$caseId.sql"; sql_sha256 = Get-FixtureSha256 $caseFile.FullName
        profile_file = "$caseId.profile.txt"; profile_sha256 = Get-FixtureSha256 $profileTarget
        analyze_file = "$caseId.analyze.txt"; analyze_sha256 = Get-FixtureSha256 $analyzeTarget
        query_id = $queryId
        captured_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    }).GetEnumerator()) { $case[$entry.Key] = $entry.Value }
    $cases += $case
    Write-Host "captured $caseId ($queryId, profile $profileLength chars)"
}

$manifestPath = Join-Path $corpusRoot 'manifest.json'
if ($CaseIds.Count -gt 0 -and (Test-Path -LiteralPath $manifestPath)) {
    # Selective capture merges into the existing manifest: entries of cases not
    # named by -CaseIds are kept verbatim (their files are not touched either).
    $existing = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -DateKind String
    if ($existing.dataset_sha256 -cne (Get-FixtureSha256 $datasetPath)) {
        throw 'dataset.sql no longer matches the manifest; earlier cases would lose their provenance.'
    }
    foreach ($table in $existing.table_rows.PSObject.Properties) {
        if ($tableCounts[$table.Name] -ne $table.Value) { throw "Base table $($table.Name) row count differs from the manifest." }
    }
    $merged = [ordered]@{}
    foreach ($case in @($existing.cases)) { $merged[$case.case_id] = $case }
    foreach ($case in $cases) { $merged[$case.case_id] = $case }
    $manifest = [ordered]@{}
    foreach ($property in $existing.PSObject.Properties) { $manifest[$property.Name] = $property.Value }
    $manifest.Remove('supplementary_datasets'); $manifest.Remove('cases')
    if ($supplementary.Count -gt 0) { $manifest['supplementary_datasets'] = $supplementary }
    $manifest['cases'] = @($merged.Keys | Sort-Object | ForEach-Object { $merged[$_] })
    [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 6) + "`n", $utf8)
} elseif ($CaseIds.Count -eq 0) {
    $manifest = [ordered]@{
        schema = 'sr-doctor.oracle-corpus.v1'
        purpose = 'official StarRocks 3.5.14 FE black-box answers (get_query_profile + ANALYZE PROFILE) for synthetic data'
        server_version = $serverVersion
    }
    if ($null -eq $topologyEvidence) {
        $manifest['image'] = $script:FixtureImage
        $manifest['image_digest'] = $script:FixtureDigest
    } else {
        $manifest['topology'] = $topologyEvidence
    }
    foreach ($entry in ([ordered]@{
        session = @('SET enable_profile=true', 'SET pipeline_profile_level=1')
        dataset_file = 'dataset.sql'; dataset_sha256 = Get-FixtureSha256 $datasetPath
        table_rows = $tableCounts
        capture_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    }).GetEnumerator()) { $manifest[$entry.Key] = $entry.Value }
    if ($supplementary.Count -gt 0) { $manifest['supplementary_datasets'] = $supplementary }
    $manifest['cases'] = $cases
    [IO.File]::WriteAllText((Join-Path $corpusRoot 'manifest.json'), ($manifest | ConvertTo-Json -Depth 6) + "`n", $utf8)
}
Remove-Item -LiteralPath $work -Recurse -Force
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'rm', '-rf', '/tmp/oracle') -DiscardOutput
[ordered]@{ status = 'PASS'; container = $ContainerName; server_version = $serverVersion; cases = $cases.Count; table_rows = $tableCounts } | ConvertTo-Json -Compress -Depth 4
