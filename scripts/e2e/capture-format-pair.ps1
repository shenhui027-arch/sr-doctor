[CmdletBinding()]
param(
    # Test infrastructure only: for each oracle-corpus query, captures the same
    # statement's Profile once in the FE default text format and once in the
    # JsonProfileFormatter format, from an isolated, already bootstrapped
    # fixed-digest fixture. It never touches any container other than this one.
    [ValidatePattern('^sr-doctor-v0\.1-e2e-3514(?:[-_.][a-z0-9][a-z0-9_.-]*)?$')]
    [string]$ContainerName = 'sr-doctor-v0.1-e2e-3514-pair',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [string[]]$CaseIds = @('c03_shuffle_join_agg', 'c04_broadcast_multi_join', 'c07_union_all_agg', 'c08_cte_multicast'),
    [switch]$SkipDataset,
    [ValidateRange(5, 120)][int]$ProfilePollSeconds = 60
)

# Official switch (3.5.14 ProfileManager.generateProfileString, lines 153-160):
# the mutable FE config `profile_info_format` selects `default` (prettyPrint
# text) or `json` (RuntimeProfile.JsonProfileFormatter) when a Profile is
# stored. The script restores `default` in `finally`.

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $repoRoot 'dev\starrocks-3.5.14\common.ps1')
$corpusRoot = Join-Path $repoRoot 'fixtures\runtime\3.5.14\oracle-corpus'
$pairRoot = Join-Path $repoRoot 'fixtures\runtime\3.5.14\format-pair'
$utf8 = [Text.UTF8Encoding]::new($false)

$docker = Get-FixtureDockerExecutable $DockerExecutable
Assert-FixtureContainerName $ContainerName
$identity = Get-FixtureContainerIdentity -DockerExecutable $docker -ContainerName $ContainerName
if ($null -eq $identity -or $identity.Image -cne $script:FixtureImage -or $identity.Label -cne $script:FixtureLabelValue) {
    throw 'The named container is not the dedicated fixed-digest labelled fixture.'
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('format-pair-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
New-Item -ItemType Directory -Force -Path $pairRoot | Out-Null
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'mkdir', '-p', '/tmp/pair') -DiscardOutput

# Same transport as capture-oracle-corpus.ps1: output is redirected inside the
# container and copied out, so captured bytes never pass a host console encoding.
function Invoke-ContainerBatch {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Sql, [string]$Database = '')
    $hostSql = Join-Path $work "$Name.sql"
    [IO.File]::WriteAllText($hostSql, $Sql.Replace("`r`n", "`n"), $utf8)
    Invoke-FixtureDocker $docker @('cp', $hostSql, "$ContainerName`:/tmp/pair/$Name.sql") -DiscardOutput
    $dbArgument = if ($Database) { " --database=$Database" } else { '' }
    $command = "mysql --batch --skip-column-names --raw --host=127.0.0.1 --port=9030 --user=root$dbArgument < /tmp/pair/$Name.sql > /tmp/pair/$Name.out 2> /tmp/pair/$Name.err"
    & $docker exec $ContainerName sh -c $command
    $exitCode = $LASTEXITCODE
    $hostOut = Join-Path $work "$Name.out"
    Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/pair/$Name.out", $hostOut) -DiscardOutput
    if ($exitCode -ne 0) {
        $hostErr = Join-Path $work "$Name.err"
        Invoke-FixtureDocker $docker @('cp', "$ContainerName`:/tmp/pair/$Name.err", $hostErr) -DiscardOutput
        $diagnostic = Get-FixtureMysqlDiagnostic -OutputLines @([IO.File]::ReadAllLines($hostErr, $utf8)) -ExitCode $exitCode
        throw "Batch $Name failed (exit $exitCode; code=$($diagnostic.error_code); line=$($diagnostic.line_number))."
    }
    $hostOut
}

function Set-ProfileFormat([ValidateSet('default', 'json')][string]$Format) {
    Invoke-ContainerBatch "format-$Format" "ADMIN SET FRONTEND CONFIG (`"profile_info_format`" = `"$Format`");" | Out-Null
}

function Get-CaseProfile([string]$CaseId, [string]$CaseSql, [string]$Format) {
    $batch = "SET enable_profile=true;`nSET pipeline_profile_level=1;`n$($CaseSql.TrimEnd())`nSELECT last_query_id();`n"
    $runOut = Invoke-ContainerBatch "$CaseId.$Format.run" $batch 'oracle_corpus'
    $queryId = ([IO.File]::ReadAllLines($runOut, $utf8) | Select-Object -Last 1).Trim().ToLowerInvariant()
    if ($queryId -notmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') { throw "Case $CaseId ($Format) returned no query id." }
    $deadline = [DateTime]::UtcNow.AddSeconds($ProfilePollSeconds)
    $published = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        $lengthOut = Invoke-ContainerBatch "$CaseId.$Format.len" "SELECT LENGTH(get_query_profile('$queryId'));"
        if (([IO.File]::ReadAllText($lengthOut, $utf8)).Trim() -match '^[1-9][0-9]*$') { $published = $true; break }
        Start-Sleep -Seconds 1
    }
    if (-not $published) { throw "Case $CaseId ($Format) Profile was not published within $ProfilePollSeconds s." }
    $profileOut = Invoke-ContainerBatch "$CaseId.$Format.profile" "SELECT get_query_profile('$queryId');"
    [ordered]@{ query_id = $queryId; path = $profileOut }
}

$serverVersion = (([IO.File]::ReadAllText((Invoke-ContainerBatch 'version' 'SELECT current_version();'), $utf8)).Trim())
if ($serverVersion -notmatch '^3\.5\.14-23a56ec') { throw "Unexpected server version $serverVersion." }

$datasetPath = Join-Path $corpusRoot 'dataset.sql'
if (-not $SkipDataset) {
    Invoke-ContainerBatch 'dataset' ([IO.File]::ReadAllText($datasetPath, $utf8)) | Out-Null
}

$pairs = @()
try {
    foreach ($caseId in $CaseIds) {
        $caseFile = Join-Path $corpusRoot "$caseId.sql"
        $caseSql = [IO.File]::ReadAllText($caseFile, $utf8)
        Set-ProfileFormat 'default'
        $text = Get-CaseProfile $caseId $caseSql 'text'
        Set-ProfileFormat 'json'
        $json = Get-CaseProfile $caseId $caseSql 'json'
        Set-ProfileFormat 'default'
        $textTarget = Join-Path $pairRoot "$caseId.profile.txt"
        $jsonTarget = Join-Path $pairRoot "$caseId.profile.json"
        Copy-Item -LiteralPath $text.path -Destination $textTarget -Force
        Copy-Item -LiteralPath $json.path -Destination $jsonTarget -Force
        $pairs += [ordered]@{
            case_id = $caseId
            sql_file = "../oracle-corpus/$caseId.sql"; sql_sha256 = Get-FixtureSha256 $caseFile
            text_profile_file = "$caseId.profile.txt"; text_profile_sha256 = Get-FixtureSha256 $textTarget; text_query_id = $text.query_id
            json_profile_file = "$caseId.profile.json"; json_profile_sha256 = Get-FixtureSha256 $jsonTarget; json_query_id = $json.query_id
            captured_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
        }
        Write-Host "captured $caseId (text $($text.query_id), json $($json.query_id))"
    }
} finally {
    Set-ProfileFormat 'default'
}

$manifest = [ordered]@{
    schema = 'sr-doctor.format-pair.v1'
    purpose = 'one oracle-corpus statement per case, run twice on the official 3.5.14 FE: Profile stored as default text, then as JsonProfileFormatter JSON'
    server_version = $serverVersion
    image = $script:FixtureImage
    image_digest = $script:FixtureDigest
    session = @('SET enable_profile=true', 'SET pipeline_profile_level=1')
    format_switch = 'ADMIN SET FRONTEND CONFIG ("profile_info_format" = "default" | "json")'
    dataset_file = '../oracle-corpus/dataset.sql'; dataset_sha256 = Get-FixtureSha256 $datasetPath
    capture_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    cases = $pairs
}
[IO.File]::WriteAllText((Join-Path $pairRoot 'manifest.json'), ($manifest | ConvertTo-Json -Depth 6) + "`n", $utf8)
Remove-Item -LiteralPath $work -Recurse -Force
Invoke-FixtureDocker $docker @('exec', $ContainerName, 'rm', '-rf', '/tmp/pair') -DiscardOutput
[ordered]@{ status = 'PASS'; container = $ContainerName; server_version = $serverVersion; cases = $pairs.Count } | ConvertTo-Json -Compress -Depth 4
