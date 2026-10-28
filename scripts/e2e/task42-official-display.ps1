<#
Task42 connected verification for the additive official-profile display projection.

This script intentionally does not persist source SQL, profile text, CLI output, or
credentials.  It observes the pre-existing fixed container; it never builds,
restarts, or stops Docker resources.  See the paired task README and
real-http-verification.md for the product/preparation boundary.
#>
[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)]
    [int]$ApiPort = 18049,
    [ValidateRange(1024, 65535)]
    [int]$MySqlPort = 39230,
    [string]$RunnerPath = (Join-Path $PSScriptRoot '..\..\target\debug\examples\loopback-v2-e2e.exe'),
    [string]$EvidenceRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\v02-development\official-display-real-20260906'),
    [switch]$ContractOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$containerName = 'sr-doctor-v0.1-e2e-3514-runtime'
$expectedImage = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
$expectedRunnerSha256 = '16d8d51ce08d4882aa22e74721e368b6acf8bc1e56dd5529be10e0ebf0e1091c'
$expectedVersion = '3.5.14-23a56ec'
$expectedNodeId = 27
$fixtureLabel = 'task31_complex_cte_join_aggregate_window'
$fixturePath = Join-Path $repoRoot 'docs\v0.2\tasks\31-analysis-product-boundaries\complex-query.sql'
$expectedFixtureSha256 = '517b9da3b06e8c7e41864b6187793d46267f110b1c4b5b21b12b555b8eeef024'
$normalization = 'ansi_sgr_strip_controls_escape_v1'
$runId = [Guid]::NewGuid().ToString('N')
$timestamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
$runDirectory = Join-Path $EvidenceRoot ("run-{0}-{1}" -f $timestamp, $runId)
$stdoutPath = Join-Path $runDirectory 'runner.stdout.tmp'
$stderrPath = Join-Path $runDirectory 'runner.stderr.tmp'
$mysqlStdoutPath = Join-Path $runDirectory 'mysql.stdout.tmp'
$mysqlStderrPath = Join-Path $runDirectory 'mysql.stderr.tmp'
$receiptPath = Join-Path $runDirectory 'receipt.json'

$runner = $null
$connectionId = $null
$csrfHeaders = $null
$status = 'FAIL'
$exitCode = 1
$failure = $null
$containerObserved = $null
$runnerStopped = $false
$sessionDeleted = $false
$sessionDeleteStatus = $null
$portFreed = $false
$environmentRestored = $false
$containerLeftRunning = $null
$stage = 'initialization'
$productCalls = [ordered]@{
    capabilities = 0
    connection_create = 0
    query_discovery = 0
    official_full = 0
    official_node_filtered = 0
    connection_delete = 0
    raw_profile_fetch = 0
    query_sql = 0
    explain = 0
    explain_analyze = 0
}
$selection = [ordered]@{
    selected_path = $null
    selected_query_id_sha256 = $null
    fixture_prepared = $false
    fixture_label = $fixtureLabel
    fixture_sql_sha256 = $null
    fixture_source_persisted = $false
    fixture_session_enable_profile_only = $true
    fixture_global_settings = $false
    profile_list_before_finished_count = 0
    profile_list_after_finished_count = 0
    profile_list_fresh_candidate_count = 0
    profile_list_fresh_query_id_bound = $false
    discovery_exact_match_count = 0
}
$checks = [ordered]@{
    image_exact = $false
    fixture_running = $false
    mysql_ready = $false
    loopback_mapping_exact = $false
    runner_sha256_exact = $false
    raw_profile_fetch_product_calls = 0
    product_query_execution_calls = 0
    full = $null
    node_filtered = $null
    node_filter_semantics = $null
    port_8000_unchanged = $false
}

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function New-Inconclusive([string]$Message) {
    $exception = [System.InvalidOperationException]::new("INCONCLUSIVE: $Message")
    throw $exception
}

function Get-TextSha256([AllowEmptyString()][string]$Text) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
        return ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Get-FileSha256([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-LoopbackListeners([int]$Port) {
    return @(
        Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
            ForEach-Object { "{0}|{1}" -f $_.LocalAddress, $_.OwningProcess } |
            Sort-Object -Unique
    )
}

function Assert-PortFree([int]$Port, [string]$Description) {
    $listeners = @(Get-LoopbackListeners $Port)
    Require ($listeners.Count -eq 0) "$Description port $Port is already listening: $($listeners -join ', ')"
}

function Get-ContainerInspection([string]$Name) {
    $raw = & docker inspect $Name 2>$null
    Require ($LASTEXITCODE -eq 0) "Docker container '$Name' cannot be inspected."
    return ($raw | ConvertFrom-Json)[0]
}

function Get-SafeHttpFailure($Response) {
    $body = $null
    try { $body = $Response.Content | ConvertFrom-Json } catch { }
    return [ordered]@{
        http_status = [int]$Response.StatusCode
        code = if ($null -ne $body -and $null -ne $body.code) { [string]$body.code } else { $null }
        kind = if ($null -ne $body -and $null -ne $body.kind) { [string]$body.kind } else { $null }
    }
}

function Invoke-Api([ValidateSet('GET', 'POST', 'DELETE')] [string]$Method, [string]$Path, $Body = $null, [hashtable]$Headers = @{}) {
    $uri = "http://127.0.0.1:$ApiPort/api/v1$Path"
    $parameters = @{
        Uri = $uri
        Method = $Method
        Headers = $Headers
        SkipHttpErrorCheck = $true
    }
    if ($Method -in @('POST', 'DELETE')) {
        $parameters.ContentType = 'application/json'
    }
    if ($null -ne $Body) {
        $parameters.Body = ($Body | ConvertTo-Json -Depth 16 -Compress)
    }
    return Invoke-WebRequest @parameters
}

function Wait-MySqlReady([int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        $null = & docker exec $containerName mysql --host=127.0.0.1 --port=9030 --user=root --batch --raw --skip-column-names --execute='SELECT @@version_comment' 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 1000
    } while ((Get-Date) -lt $deadline)
    return $false
}

function Wait-RunnerReady([int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        try {
            $response = Invoke-WebRequest -Uri "http://127.0.0.1:$ApiPort/api/v1/health" -Method GET -SkipHttpErrorCheck
            if ($response.StatusCode -eq 200) { return $true }
        }
        catch { }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    return $false
}

function Invoke-MySqlToTemporaryFile([string]$Statement, [string]$Purpose) {
    Remove-Item -LiteralPath $mysqlStdoutPath, $mysqlStderrPath -Force -ErrorAction SilentlyContinue
    $escaped = $Statement.Replace('"', '\"')
    $arguments = @(
        'exec', $containerName, 'mysql',
        '--host=127.0.0.1', '--port=9030', '--user=root',
        '--batch', '--raw', '--skip-column-names',
        ("--execute=`"{0}`"" -f $escaped)
    )
    $process = Start-Process -FilePath 'docker' -ArgumentList $arguments -NoNewWindow -PassThru -RedirectStandardOutput $mysqlStdoutPath -RedirectStandardError $mysqlStderrPath
    if (-not $process.WaitForExit(60000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        New-Inconclusive "The bounded MySQL $Purpose command exceeded 60 seconds."
    }
    if ($process.ExitCode -ne 0) {
        $stderr = if (Test-Path -LiteralPath $mysqlStderrPath) { [System.IO.File]::ReadAllText($mysqlStderrPath, [System.Text.UTF8Encoding]::new($false)) } else { '' }
        $errorCode = [regex]::Match($stderr, '(?i)\bERROR\s+(\d+)\b').Groups[1].Value
        $safeCategory = if ([string]::IsNullOrEmpty($errorCode)) { 'unclassified' } else { "mysql_error_$errorCode" }
        New-Inconclusive "The MySQL $Purpose command returned exit $($process.ExitCode) ($safeCategory, stderr_chars=$($stderr.Length), stderr_sha256=$(Get-TextSha256 $stderr))."
    }
    return [System.IO.File]::ReadAllText($mysqlStdoutPath, [System.Text.UTF8Encoding]::new($false))
}

function Normalize-MySqlRows([AllowEmptyString()][string]$Text) {
    if ($Text.EndsWith("`r`n", [System.StringComparison]::Ordinal)) { return $Text.Substring(0, $Text.Length - 2) }
    if ($Text.EndsWith("`n", [System.StringComparison]::Ordinal)) { return $Text.Substring(0, $Text.Length - 1) }
    return $Text
}

function Get-FinishedProfileListQueryIds([AllowEmptyString()][string]$ProfileList) {
    $ids = @()
    # Fixed 3.5.14 source shape: QueryId, User, Database, State, SQL.  The
    # mysql batch CLI may preserve a prior SQL field's newlines, so identify
    # only complete row headers and never inspect or persist the SQL display.
    $rows = [regex]::Matches($ProfileList, '(?im)^(?<id>[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\t[^\t\r\n]*\t[^\t\r\n]*\t(?<state>[^\t\r\n]*)\t')
    Require ($rows.Count -gt 0 -or [string]::IsNullOrWhiteSpace($ProfileList)) 'SHOW PROFILELIST did not return the fixed five-column 3.5.14 row-header shape.'
    foreach ($row in $rows) {
        if ($row.Groups['state'].Value.Equals('Finished', [System.StringComparison]::OrdinalIgnoreCase)) {
            $ids += $row.Groups['id'].Value.ToLowerInvariant()
        }
    }
    return @($ids | Sort-Object -Unique)
}

function Get-CurrentFinishedProfileListQueryIds([string]$Purpose) {
    $profileList = Invoke-MySqlToTemporaryFile -Statement 'SHOW PROFILELIST' -Purpose $Purpose
    return @(Get-FinishedProfileListQueryIds -ProfileList $profileList)
}

function Get-CompleteSgrLength([string]$Text, [int]$Index) {
    if (($Index + 1) -ge $Text.Length -or [int][char]$Text[$Index] -ne 27 -or [char]$Text[$Index + 1] -ne '[') { return 0 }
    $cursor = $Index + 2
    while ($cursor -lt $Text.Length) {
        $code = [int][char]$Text[$cursor]
        if (($code -ge 48 -and $code -le 57) -or $code -eq 58 -or $code -eq 59) {
            $cursor++
            continue
        }
        if ($code -eq 109) { return ($cursor - $Index + 1) }
        return 0
    }
    return 0
}

function Convert-ToOfficialDisplay([AllowEmptyString()][string]$Text) {
    $builder = [System.Text.StringBuilder]::new($Text.Length)
    $index = 0
    while ($index -lt $Text.Length) {
        $character = $Text[$index]
        $sgrLength = Get-CompleteSgrLength $Text $index
        if ($sgrLength -gt 0) {
            $index += $sgrLength
            continue
        }
        $code = [int][char]$character
        if ($character -eq "`n" -or $character -eq "`t") {
            [void]$builder.Append($character)
        }
        elseif (($code -le 31) -or ($code -ge 127 -and $code -le 159)) {
            [void]$builder.Append(('\u{{{0:X4}}}' -f $code))
        }
        else {
            [void]$builder.Append($character)
        }
        $index++
    }
    return $builder.ToString()
}

function Get-CompleteSgrCount([AllowEmptyString()][string]$Text) {
    $count = 0
    $index = 0
    while ($index -lt $Text.Length) {
        $length = Get-CompleteSgrLength $Text $index
        if ($length -gt 0) {
            $count++
            $index += $length
        }
        else { $index++ }
    }
    return $count
}

function Get-ExpectedNodeIdentityCount([AllowEmptyString()][string]$Text, [int]$NodeId) {
    # Fixed fixture evidence uses the official tree's `(id=N)` notation.  Work
    # against the same safe projection used for display so ANSI SGR color does
    # not split the identifier; this is not a product Profile parser.
    $display = Convert-ToOfficialDisplay $Text
    return [regex]::Matches($display, ("(?i)\(id\s*=\s*{0}\)" -f $NodeId)).Count
}

function Test-NodeIds($Actual, [int[]]$Expected) {
    $actualValues = @($Actual | ForEach-Object { [int]$_ })
    if ($actualValues.Count -ne $Expected.Count) { return $false }
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        if ($actualValues[$index] -ne $Expected[$index]) { return $false }
    }
    return $true
}

function Test-SameStringSet($Before, $After) {
    $beforeValues = @($Before | ForEach-Object { [string]$_ } | Sort-Object -Unique)
    $afterValues = @($After | ForEach-Object { [string]$_ } | Sort-Object -Unique)
    if ($beforeValues.Count -ne $afterValues.Count) { return $false }
    for ($index = 0; $index -lt $beforeValues.Count; $index++) {
        if ($beforeValues[$index] -cne $afterValues[$index]) { return $false }
    }
    return $true
}

function Assert-OfficialResponse($Body, [string]$QueryId, [long]$Generation, [int[]]$NodeIds, [string]$OracleText, [string]$Name) {
    Require ($null -ne $Body) "$Name official response was not JSON."
    Require ([string]$Body.kind -ceq 'analyze_profile_text') "$Name response kind changed."
    Require ($null -ne $Body.query) "$Name response omitted query identity."
    Require ([string]$Body.query.connection_id -ceq $connectionId) "$Name response connection identity changed."
    Require ([string]$Body.query.query_id -ceq $QueryId) "$Name response query identity changed."
    Require ([long]$Body.query.discovery_generation -eq $Generation) "$Name response generation changed."
    Require ($null -ne $Body.identity -and [string]$Body.identity.kind -ceq 'discovery') "$Name response omitted discovery authority."
    Require (@($Body.identity.PSObject.Properties).Count -eq 2 -and [long]$Body.identity.discovery_generation -eq $Generation) "$Name response authority is mixed or mismatched."
    Require (Test-NodeIds $Body.node_ids $NodeIds) "$Name response node identity changed."
    Require ([string]$Body.display_normalization -ceq $normalization) "$Name response normalization marker is wrong."
    Require (-not [string]::IsNullOrWhiteSpace([string]$Body.read_at_utc)) "$Name response omitted read_at_utc."
    Require ($null -ne $Body.detected_version -and -not [string]::IsNullOrWhiteSpace([string]$Body.detected_version.value)) "$Name response omitted detected_version evidence."
    Require (@($Body.source_refs) -contains 'analyze_profile:official_text') "$Name response omitted official provenance."
    Require ($null -ne $Body.limitations) "$Name response omitted limitations."
    Require ($null -eq $Body.profile_observed_at_utc) "$Name response fabricated profile observation time."

    $rawText = [string]$Body.text
    $rawHash = Get-TextSha256 $rawText
    $oracleHash = Get-TextSha256 $OracleText
    $expectedDisplay = Convert-ToOfficialDisplay $rawText
    $expectedDisplayHash = Get-TextSha256 $expectedDisplay
    $sgrCount = Get-CompleteSgrCount $rawText
    Require ([string]$Body.source_hash -ceq $rawHash) "$Name response source_hash does not hash product text."
    Require ($rawText -ceq $OracleText) "$Name response text differs from independent MySQL oracle."
    Require ($rawHash -ceq $oracleHash) "$Name response source hash differs from independent MySQL oracle hash."
    Require ([string]$Body.display_text -ceq $expectedDisplay) "$Name response display text violates frozen projection."
    Require ([string]$Body.display_hash -ceq $expectedDisplayHash) "$Name response display hash violates frozen projection."
    Require ($sgrCount -gt 0) "$Name source did not contain a complete ANSI SGR sequence."
    return [ordered]@{
        response_http_status = 200
        raw_length_chars = $rawText.Length
        raw_hash = $rawHash
        oracle_hash = $oracleHash
        oracle_text_equal = $true
        display_length_chars = ([string]$Body.display_text).Length
        display_hash = $expectedDisplayHash
        display_projection_equal = $true
        complete_sgr_sequence_count = $sgrCount
        normalization = [string]$Body.display_normalization
        query_id_sha256 = Get-TextSha256 $QueryId
        discovery_generation = $Generation
        node_ids = @($NodeIds)
        detected_version = [string]$Body.detected_version.value
    }
}

New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$contractReceiptPath = Join-Path $runDirectory 'contract-only-receipt.json'
if ($ContractOnly) {
    # No runner, HTTP endpoint, socket, Docker CLI, or environment value is used
    # in this branch. It freezes the exact successful DTO/payload shape before a
    # connected invocation is allowed to exercise the same assertions.
    $stage = 'contract_only_success_dto'
    $connectionId = 'static-contract-session'
    $queryId = '00000000-0000-0000-0000-000000000042'
    $generation = 42
    $sourceText = "`e[38;2;250;128;114mMERGE_EXCHANGE (id=27)`e[0m`r`n`t5µs"
    $displayText = Convert-ToOfficialDisplay $sourceText
    $baseResponse = [pscustomobject]@{
        kind = 'analyze_profile_text'
        identity = [pscustomobject]@{ kind = 'discovery'; discovery_generation = $generation }
        query = [pscustomobject]@{ connection_id = $connectionId; query_id = $queryId; discovery_generation = $generation }
        detected_version = [pscustomobject]@{ value = $expectedVersion; source_ref = 'connection:current_version' }
        node_ids = @()
        read_at_utc = '2026-09-06T00:00:00Z'
        profile_observed_at_utc = $null
        source_hash = Get-TextSha256 $sourceText
        text = $sourceText
        display_text = $displayText
        display_hash = Get-TextSha256 $displayText
        display_normalization = $normalization
        source_refs = @('analyze_profile:official_text')
        limitations = @('official_interpretation_not_raw_profile', 'profile_observation_time_unknown', 'query_status_from_discovery_not_fresh_probe', 'result_rows_joined_with_newline')
    }
    $searchPayload = ([ordered]@{ connection_id = $connectionId; limit = 500; status = 'finished'; digest = $null } | ConvertTo-Json -Compress | ConvertFrom-Json)
    Require ([string]$searchPayload.connection_id -ceq $connectionId -and [int]$searchPayload.limit -eq 500 -and [string]$searchPayload.status -ceq 'finished' -and $null -eq $searchPayload.digest) 'Static query-list payload differs from the traced DTO.'
    $staticBefore = @(Get-FinishedProfileListQueryIds "00000000-0000-0000-0000-000000000001`troot`tdb`tFinished`tSELECT 1")
    $staticAfter = @(Get-FinishedProfileListQueryIds ("00000000-0000-0000-0000-000000000001`troot`tdb`tFinished`tSELECT 1`n" + "00000000-0000-0000-0000-000000000042`troot`tdb`tFinished`tWITH fixed_fixture`n  continued SQL display"))
    $staticFresh = @($staticAfter | Where-Object { $staticBefore -notcontains $_ })
    Require ($staticFresh.Count -eq 1 -and $staticFresh[0] -ceq $queryId) 'Static ProfileList set-difference binding is not exact.'
    $fullPayload = ([ordered]@{ connection_id = $connectionId; query_id = $queryId; identity = [ordered]@{ kind = 'discovery'; discovery_generation = $generation }; node_ids = @() } | ConvertTo-Json -Compress | ConvertFrom-Json)
    Require ([string]$fullPayload.connection_id -ceq $connectionId -and [string]$fullPayload.query_id -ceq $queryId -and [int]$fullPayload.identity.discovery_generation -eq $generation -and (@($fullPayload.node_ids).Count -eq 0)) 'Static full official request differs from the traced DTO.'
    $fullCheck = Assert-OfficialResponse $baseResponse $queryId $generation @() $sourceText 'Static full'
    $nodeResponse = [pscustomobject]@{}
    foreach ($property in $baseResponse.PSObject.Properties) { $nodeResponse | Add-Member -NotePropertyName $property.Name -NotePropertyValue $property.Value }
    $nodeSourceText = "`e[38;2;250;128;114mMERGE_EXCHANGE (id=27)`e[0m`r`n`tselected detail"
    $nodeResponse.node_ids = @($expectedNodeId)
    $nodeResponse.text = $nodeSourceText
    $nodeResponse.source_hash = Get-TextSha256 $nodeSourceText
    $nodeResponse.display_text = Convert-ToOfficialDisplay $nodeSourceText
    $nodeResponse.display_hash = Get-TextSha256 $nodeResponse.display_text
    $nodePayload = ([ordered]@{ connection_id = $connectionId; query_id = $queryId; identity = [ordered]@{ kind = 'discovery'; discovery_generation = $generation }; node_ids = @($expectedNodeId) } | ConvertTo-Json -Compress | ConvertFrom-Json)
    Require ([int]$nodePayload.node_ids[0] -eq $expectedNodeId -and (@($nodePayload.node_ids).Count -eq 1)) 'Static node-filtered official request differs from the traced DTO.'
    $nodeCheck = Assert-OfficialResponse $nodeResponse $queryId $generation @($expectedNodeId) $nodeSourceText 'Static node-filtered'
    $rejectedAuthorities = 0
    foreach ($invalidIdentity in @(
        $null,
        [pscustomobject]@{ kind = 'discovery'; discovery_generation = ($generation + 1) },
        [pscustomobject]@{ kind = 'discovery'; discovery_generation = $generation; execution_attempt_id = 'mixed' },
        [pscustomobject]@{ kind = 'verified_standard_attempt'; execution_attempt_id = 'wrong-kind' }
    )) {
        $invalidResponse = $baseResponse | ConvertTo-Json -Depth 16 | ConvertFrom-Json
        $invalidResponse.identity = $invalidIdentity
        try { Assert-OfficialResponse $invalidResponse $queryId $generation @() $sourceText 'Static invalid authority' | Out-Null }
        catch { $rejectedAuthorities++; continue }
        throw 'Official oracle accepted an invalid authority.'
    }
    Require ($rejectedAuthorities -eq 4) 'Official authority negative checks were not all executed.'
    Require ((Get-ExpectedNodeIdentityCount $sourceText $expectedNodeId) -gt 0 -and (Get-ExpectedNodeIdentityCount $nodeSourceText $expectedNodeId) -gt 0 -and $sourceText -cne $nodeSourceText) 'Static node oracle semantics fixture is incomplete.'
    [ordered]@{
        task = 'task42_official_display_contract_only'
        result = 'PASS'
        timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')
        stage = $stage
        remote_operations = 0
        docker_operations = 0
        rejected_invalid_authorities = $rejectedAuthorities
        payloads = [ordered]@{ query_list_status = [string]$searchPayload.status; query_list_limit = [int]$searchPayload.limit; official_generation = [int]$fullPayload.identity.discovery_generation; filtered_node_id = [int]$nodePayload.node_ids[0] }
        full = $fullCheck
        node_filtered = $nodeCheck
        raw_text_persisted = $false
    } | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $contractReceiptPath -Encoding utf8
    Write-Host "Task42 official display contract-only verification: PASS (receipt: $contractReceiptPath)"
    exit 0
}
$previousDoctorFrontendDist = [Environment]::GetEnvironmentVariable('DOCTOR_FRONTEND_DIST', 'Process')
$port8000Before = Get-LoopbackListeners 8000

try {
    $stage = 'preflight_runner_and_fixture'
    Require (Test-Path -LiteralPath $RunnerPath -PathType Leaf) "Runner is missing: $RunnerPath"
    $runnerHash = Get-FileSha256 $RunnerPath
    Require ($runnerHash -ceq $expectedRunnerSha256) "Runner SHA-256 is not the Root-supplied build."
    $checks.runner_sha256_exact = $true
    Assert-PortFree $ApiPort 'Temporary Task42 API'

    $container = Get-ContainerInspection $containerName
    $fixturePortBindings = @($container.NetworkSettings.Ports.'9030/tcp' | ForEach-Object { "{0}:{1}" -f $_.HostIp, $_.HostPort })
    $containerObserved = [ordered]@{
        id = [string]$container.Id
        image = [string]$container.Config.Image
        running = [bool]$container.State.Running
        version_label = [string]$container.Config.Labels.'com.sr-doctor.e2e'
        port_binding = $fixturePortBindings
    }
    Require ($containerObserved.running) "Existing fixed fixture is not running; this script will not start it."
    Require ($containerObserved.image -ceq $expectedImage) 'Fixture image digest differs from the fixed 3.5.14 baseline.'
    Require ($containerObserved.version_label -ceq '3.5.14') 'Fixture version label differs from 3.5.14.'
    Require ($fixturePortBindings.Count -eq 1 -and $fixturePortBindings[0] -ceq "127.0.0.1:$MySqlPort") 'Fixture MySQL port is not the expected loopback binding.'
    $checks.fixture_running = $true
    $checks.image_exact = $true
    $checks.loopback_mapping_exact = $true
    Require (Wait-MySqlReady 120) 'Existing fixture did not become MySQL-ready within 120 seconds.'
    $checks.mysql_ready = $true
    $versionOutput = & docker exec $containerName mysql --host=127.0.0.1 --port=9030 --user=root --batch --raw --skip-column-names --execute='SELECT @@version_comment' 2>$null
    Require ($LASTEXITCODE -eq 0 -and [string]$versionOutput -match [regex]::Escape($expectedVersion)) "Fixture version did not contain $expectedVersion."
    Require (Test-Path -LiteralPath $fixturePath -PathType Leaf) "Existing fixture source is missing: $fixturePath"
    $fixtureSql = [System.IO.File]::ReadAllText($fixturePath, [System.Text.UTF8Encoding]::new($false))
    Require (-not [string]::IsNullOrWhiteSpace($fixtureSql)) 'Existing complex fixture source was empty.'
    $selection.fixture_sql_sha256 = Get-TextSha256 $fixtureSql
    Require ($selection.fixture_sql_sha256 -ceq $expectedFixtureSha256) 'Fixed complex fixture SHA-256 changed.'

    $stage = 'runner_and_connection'
    $runner = Start-Process -FilePath $RunnerPath -ArgumentList @([string]$ApiPort) -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    Require (Wait-RunnerReady 30) "Runner did not bind health endpoint on loopback port $ApiPort."
    $capabilitiesResponse = Invoke-Api GET '/capabilities'
    $productCalls.capabilities++
    Require ($capabilitiesResponse.StatusCode -eq 200) "Capabilities returned HTTP $($capabilitiesResponse.StatusCode)."
    $capabilities = $capabilitiesResponse.Content | ConvertFrom-Json
    $csrfHeaders = @{ 'X-Doctor-CSRF' = [string]$capabilities.csrf.nonce }
    Require (-not [string]::IsNullOrWhiteSpace($csrfHeaders['X-Doctor-CSRF'])) 'Capabilities response omitted CSRF nonce.'

    $createResponse = Invoke-Api POST '/connections' ([ordered]@{
            host = '127.0.0.1'
            port = $MySqlPort
            username = 'root'
            password = ''
            catalog = $null
            database = $null
            audit_table = $null
            tls_mode = 'docker_loopback_plaintext'
        }) $csrfHeaders
    $productCalls.connection_create++
    Require ($createResponse.StatusCode -eq 200) "Connection create returned HTTP $($createResponse.StatusCode)."
    $connection = $createResponse.Content | ConvertFrom-Json
    $connectionId = [string]$connection.connection_id
    Require (-not [string]::IsNullOrWhiteSpace($connectionId)) 'Connection create returned no identity.'

    $stage = 'isolated_fixture_preparation'
    # SHOW PROFILELIST truncates SQL text in this fixed server.  Never select an
    # existing Profile by that prefix: execute the fixed fixture once, then bind
    # its ID by the sole new Finished row in a before/after observation.
    $profileIdsBefore = @(Get-CurrentFinishedProfileListQueryIds 'preparation profile-list baseline')
    $selection.profile_list_before_finished_count = $profileIdsBefore.Count
    # Keep the direct fixture statement on one line: this fixed server exposes
    # multiline SQL in SHOW PROFILELIST as multiple CLI lines, which cannot be
    # distinguished from separate rows without persisting the display text.
    $fixtureStatement = "SET enable_profile = true; " + (($fixtureSql -replace "`r?`n", ' ').Trim())
    if (-not $fixtureStatement.EndsWith(';', [System.StringComparison]::Ordinal)) { $fixtureStatement += ';' }
    $null = Invoke-MySqlToTemporaryFile $fixtureStatement 'fixed complex fixture preparation'
    $selection.fixture_prepared = $true
    $selectedQueryId = $null
    for ($attempt = 1; $attempt -le 15; $attempt++) {
        Start-Sleep -Seconds 2
        $profileIdsAfter = @(Get-CurrentFinishedProfileListQueryIds 'post-preparation profile-list identity')
        $freshProfileIds = @($profileIdsAfter | Where-Object { $profileIdsBefore -notcontains $_ })
        $selection.profile_list_after_finished_count = $profileIdsAfter.Count
        $selection.profile_list_fresh_candidate_count = $freshProfileIds.Count
        if ($freshProfileIds.Count -gt 1) {
            New-Inconclusive 'More than one Finished Profile appeared during the single fixed fixture preparation.'
        }
        if ($freshProfileIds.Count -eq 1) {
            $selectedQueryId = [string]$freshProfileIds[0]
            $selection.profile_list_fresh_query_id_bound = $true
            break
        }
    }
    Require (-not [string]::IsNullOrWhiteSpace($selectedQueryId)) 'Fixed fixture preparation did not yield one newly observed Finished Profile ID within 30 seconds.'
    $selection.selected_query_id_sha256 = Get-TextSha256 $selectedQueryId
    $selection.selected_path = 'single_fixed_fixture_preparation_before_after_profilelist'

    $stage = 'query_discovery'
    $fixtureDiscovered = $false
    for ($attempt = 1; $attempt -le 15; $attempt++) {
        Start-Sleep -Seconds 2
        $searchResponse = Invoke-Api POST "/connections/$connectionId/queries/search" ([ordered]@{
                connection_id = $connectionId
                limit = 500
                status = 'finished'
                digest = $null
            }) $csrfHeaders
        $productCalls.query_discovery++
        Require ($searchResponse.StatusCode -eq 200) "Post-fixture query discovery returned HTTP $($searchResponse.StatusCode)."
        $discovery = $searchResponse.Content | ConvertFrom-Json
        $generation = [long]$discovery.discovery_generation
        Require ($generation -gt 0) 'Post-fixture discovery did not return a positive generation.'
        $exactMatches = @($discovery.queries | Where-Object { [string]$_.query_id -ceq $selectedQueryId })
        $selection.discovery_exact_match_count = $exactMatches.Count
        if ($exactMatches.Count -eq 1) {
            $fixtureDiscovered = $true
            break
        }
        if ($exactMatches.Count -gt 1) { New-Inconclusive 'Doctor discovery returned an ambiguous prepared Profile identity.' }
    }
    Require $fixtureDiscovered 'Fixed fixture Profile was not discoverable during the bounded 30-second AuditLoader wait.'
    $fullResponse = Invoke-Api POST "/connections/$connectionId/profiles/official-analysis" ([ordered]@{
            connection_id = $connectionId
            query_id = $selectedQueryId
            identity = [ordered]@{ kind = 'discovery'; discovery_generation = $generation }
            node_ids = @()
        }) $csrfHeaders
    $productCalls.official_full++
    Require ($fullResponse.StatusCode -eq 200) "Official full read after fixed fixture preparation returned HTTP $($fullResponse.StatusCode)."

    $stage = 'official_full_oracle'
    $selection.selected_query_id_sha256 = Get-TextSha256 $selectedQueryId
    $fullBody = $fullResponse.Content | ConvertFrom-Json
    $fullOracle = Normalize-MySqlRows (Invoke-MySqlToTemporaryFile ("ANALYZE PROFILE FROM '$selectedQueryId'") 'full oracle')
    $checks.full = Assert-OfficialResponse $fullBody $selectedQueryId $generation @() $fullOracle 'Full'

    $stage = 'official_node_oracle'
    $filteredResponse = Invoke-Api POST "/connections/$connectionId/profiles/official-analysis" ([ordered]@{
            connection_id = $connectionId
            query_id = $selectedQueryId
            identity = [ordered]@{ kind = 'discovery'; discovery_generation = $generation }
            node_ids = @($expectedNodeId)
        }) $csrfHeaders
    $productCalls.official_node_filtered++
    Require ($filteredResponse.StatusCode -eq 200) "Official node-filtered read returned HTTP $($filteredResponse.StatusCode)."
    $filteredBody = $filteredResponse.Content | ConvertFrom-Json
    $filteredOracle = Normalize-MySqlRows (Invoke-MySqlToTemporaryFile ("ANALYZE PROFILE FROM '$selectedQueryId',$expectedNodeId") 'node-filtered oracle')
    $checks.node_filtered = Assert-OfficialResponse $filteredBody $selectedQueryId $generation @($expectedNodeId) $filteredOracle 'Node-filtered'
    $fullNodeIdentityCount = Get-ExpectedNodeIdentityCount $fullOracle $expectedNodeId
    $filteredNodeIdentityCount = Get-ExpectedNodeIdentityCount $filteredOracle $expectedNodeId
    Require ($fullNodeIdentityCount -gt 0) "Independent full official output did not identify node $expectedNodeId in the fixed fixture syntax."
    Require ($filteredNodeIdentityCount -gt 0) "Independent node-filtered official output did not identify node $expectedNodeId in the fixed fixture syntax."
    Require ($fullOracle -cne $filteredOracle) 'Independent full and node-filtered official outputs were byte-identical; node filter effectiveness is not proven.'
    $checks.node_filter_semantics = [ordered]@{
        requested_node_id = $expectedNodeId
        same_query_id = $true
        full_oracle_node_identity_count = $fullNodeIdentityCount
        filtered_oracle_node_identity_count = $filteredNodeIdentityCount
        oracle_texts_differ = $true
        product_and_oracle_identity_bound = $true
    }

    $checks.raw_profile_fetch_product_calls = $productCalls.raw_profile_fetch
    $checks.product_query_execution_calls = $productCalls.query_sql
    $status = 'PASS'
    $exitCode = 0
}
catch {
    $failure = $_.Exception.Message
    if ($failure -like 'INCONCLUSIVE:*') {
        $status = 'INCONCLUSIVE'
        $exitCode = 2
    }
    else {
        $status = 'FAIL'
        $exitCode = 1
    }
}
finally {
    if ($null -ne $connectionId -and $null -ne $csrfHeaders) {
        try {
            $deleteResponse = Invoke-Api DELETE "/connections/$connectionId" $null $csrfHeaders
            $productCalls.connection_delete++
            $sessionDeleteStatus = [int]$deleteResponse.StatusCode
            $sessionDeleted = ($sessionDeleteStatus -in @(200, 204))
        }
        catch { $sessionDeleted = $false }
    }
    else { $sessionDeleted = $true }

    if ($null -ne $runner) {
        try {
            if (-not $runner.HasExited) { Stop-Process -Id $runner.Id -Force -ErrorAction Stop }
            $runner.WaitForExit(5000) | Out-Null
            $runnerStopped = $runner.HasExited
        }
        catch { $runnerStopped = $false }
    }
    else { $runnerStopped = $true }

    try {
        [Environment]::SetEnvironmentVariable('DOCTOR_FRONTEND_DIST', $previousDoctorFrontendDist, 'Process')
        $environmentRestored = $true
    }
    catch { $environmentRestored = $false }

    Remove-Item -LiteralPath $stdoutPath, $stderrPath, $mysqlStdoutPath, $mysqlStderrPath -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 250
    $portFreed = (@(Get-LoopbackListeners $ApiPort).Count -eq 0)
    $port8000After = Get-LoopbackListeners 8000
    $checks.port_8000_unchanged = Test-SameStringSet $port8000Before $port8000After
    try {
        $finalContainer = Get-ContainerInspection $containerName
        $containerLeftRunning = [bool]$finalContainer.State.Running
    }
    catch { $containerLeftRunning = $null }
    if (($status -eq 'PASS') -and (-not $sessionDeleted -or -not $runnerStopped -or -not $environmentRestored -or -not $portFreed -or -not $checks.port_8000_unchanged -or $containerLeftRunning -ne $true)) {
        $status = 'FAIL'
        $exitCode = 1
        $failure = 'Cleanup invariant failed (session, runner, environment, temporary port, port 8000 identity, or fixture running state).'
    }

    $runnerReceiptHash = $null
    if (Test-Path -LiteralPath $RunnerPath -PathType Leaf) {
        try { $runnerReceiptHash = Get-FileSha256 $RunnerPath } catch { }
    }
    $receipt = [ordered]@{
        task = 'task42_official_display_real_http'
        result = $status
        exit_code = $exitCode
        timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')
        stage = $stage
        runner = [ordered]@{ sha256 = $runnerReceiptHash; expected_sha256 = $expectedRunnerSha256; api_port = $ApiPort; loopback_only = $true }
        fixture = $containerObserved
        preparation = $selection
        product_http_operation_counts = $productCalls
        assertions = $checks
        cleanup = [ordered]@{
            temporary_connection_deleted = $sessionDeleted
            temporary_connection_delete_http_status = $sessionDeleteStatus
            runner_stopped = $runnerStopped
            temporary_port_free = $portFreed
            environment_restored = $environmentRestored
            docker_container_left_running = $containerLeftRunning
            port_8000_listener_unchanged = $checks.port_8000_unchanged
            raw_cli_output_removed = (-not (Test-Path -LiteralPath $mysqlStdoutPath) -and -not (Test-Path -LiteralPath $mysqlStderrPath))
        }
        failure = $failure
        evidence_policy = [ordered]@{
            credentials_persisted = $false
            fixture_source_persisted = $false
            fixture_output_persisted = $false
            official_raw_text_persisted = $false
            mock_counts_used = $false
        }
    }
    $receipt | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $receiptPath -Encoding utf8
}

Write-Host "Task42 official display verification: $status (receipt: $receiptPath)"
exit $exitCode
