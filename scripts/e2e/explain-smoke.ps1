param(
    [ValidateRange(1, 65535)][int]$ApiPort = 8000,
    [ValidateRange(1, 65535)][int]$MySqlPort = 39130,
    [switch]$SaveSnapshot
)

$ErrorActionPreference = 'Stop'
function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Sha256([string]$Value) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

# Deliberately bound to the owned synthetic fixture; no host/SQL/database input.
$container = 'sr-doctor-v0.1-e2e-3514-explain'
$image = 'starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f'
# Capture machine-readable JSON, not filtered terminal output.
$inspection = docker inspect $container
Require ($LASTEXITCODE -eq 0) 'Owned Explain fixture is absent.'
$fixture = @($inspection | ConvertFrom-Json)[0]
Require ($fixture.Config.Image -ceq $image) 'Explain fixture image mismatch.'
Require ($fixture.Config.Labels.'com.sr-doctor.task' -ceq 'explain-golden') 'Explain fixture owner mismatch.'
Require ($fixture.State.Running -eq $true) 'Explain fixture is not running.'
$bindings = @($fixture.NetworkSettings.Ports.'9030/tcp')
Require ($bindings.Count -eq 1) 'Unexpected fixture port bindings.'
Require ($bindings[0].HostIp -ceq '127.0.0.1' -and [int]$bindings[0].HostPort -eq $MySqlPort) 'Fixture is not bound to the selected loopback port.'

$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$sqlPath = Join-Path $repo 'fixtures/explain/starrocks-3.5.14/select-multijoin.sql'
$sql = [IO.File]::ReadAllText($sqlPath).TrimEnd()
Require ($sql.EndsWith(';')) 'Fixed CLI fixture terminator missing.'
$sql = $sql.Substring(0, $sql.Length - 1)
$sqlSha = [Security.Cryptography.SHA256]::Create()
try { $sqlHash = ([BitConverter]::ToString($sqlSha.ComputeHash([Text.Encoding]::UTF8.GetBytes($sql)))).Replace('-', '').ToLowerInvariant() }
finally { $sqlSha.Dispose() }
Require ($sqlHash -ceq 'fd8a02f73a1593277b3074d56788ffbada19949498c29ea51950384bdc5c9849') 'Fixed Explain SQL fixture content changed.'
# Independent CLI capture already verified row-for-row by the transport test.
# Do not derive expected byte lengths, node rows or proof roles from the API DTO.
$referencePath = Join-Path $repo 'fixtures/explain/starrocks-3.5.14/select-multijoin.rows4.costs.txt'
Require (Test-Path -LiteralPath $referencePath -PathType Leaf) 'Verified fresh CLI reference is required.'
Require ((Get-FileHash -LiteralPath $referencePath).Hash.ToLowerInvariant() -ceq '4c41880c000df1cec3b42641a619e5d240bdc7a2a5acc3697ba7d583e0772821') 'CLI reference changed; explicit recapture and review required.'
$referenceRows = [IO.File]::ReadAllText($referencePath).Split("`n")
$rowLengths = @($referenceRows | ForEach-Object { [Text.Encoding]::UTF8.GetByteCount($_) })
$expectedPlanHash = Sha256 (ConvertTo-Json -InputObject $referenceRows -Compress)
Require ($expectedPlanHash -ceq 'fc504917aca77f6c23d8179f7ed188ca2e6b491ce8cda7d24353fc67396f0699') 'Independent ordered-row hash mismatch.'
$expectedNodeRows = @{ '18'=10; '17'=24; '16'=33; '15'=41; '14'=52; '13'=61; '12'=70; '11'=87; '9'=91; '8'=104; '7'=119; '5'=126; '10'=137; '6'=154; '4'=174; '3'=185; '2'=199; '0'=203; '1'=224 }
# Each Exchange proof: sender Fragment header, sender root, outgoing ID, target.
$expectedExchangeRows = @{ '17>18'=@(18,24,22,10); '14>15'=@(46,52,50,41); '10>11'=@(131,137,135,87); '6>7'=@(148,154,152,119); '4>5'=@(168,174,172,126); '1>2'=@(218,224,222,199) }
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$headers = @{}
$connectionId = $null
$receipt = $null

function Api-Json([string]$Method, [string]$Path, $Payload = $null) {
    $parameters = @{ Uri = "$apiBase$Path"; Method = $Method; Headers = $headers; MaximumRedirection = 0 }
    if ($Method -in @('Post', 'Put', 'Patch', 'Delete')) { $parameters.ContentType = 'application/json' }
    if ($null -ne $Payload) {
        $parameters.ContentType = 'application/json'
        $parameters.Body = $Payload | ConvertTo-Json -Depth 100 -Compress
    }
    try { Invoke-RestMethod @parameters }
    catch {
        $status = if ($null -ne $_.Exception.Response) { [int]$_.Exception.Response.StatusCode } else { 'transport' }
        throw "Explain smoke HTTP $Method failed (status=$status); raw response omitted."
    }
}

try {
    $capabilities = Api-Json Get '/capabilities'
    Require (-not [string]::IsNullOrWhiteSpace($capabilities.csrf.nonce)) 'CSRF capability missing.'
    $headers['X-Doctor-CSRF'] = $capabilities.csrf.nonce
    $session = Api-Json Post '/connections' @{
        host = '127.0.0.1'; port = $MySqlPort; username = 'root'; password = ''
        catalog = $null; database = 'explain_fixture'; audit_table = $null
        tls_mode = 'docker_loopback_plaintext'
    }
    $connectionId = $session.connection_id
    Require (-not [string]::IsNullOrWhiteSpace($connectionId)) 'Connection identity missing.'
    Require ($session.detected_version.value -ceq '3.5.14-23a56ec') 'Unexpected fixture version.'
    $payload = @{ connection_id = $connectionId; candidate_sql = $sql }
    $response = Api-Json Post "/connections/$connectionId/explain" $payload
    $snapshot = $response.snapshot
    Require ($snapshot.explain_schema_version -ceq '0.2') 'Explain schema is not 0.2.'
    Require ($snapshot.explain_level -ceq 'costs') 'Explain level mismatch.'
    Require ($snapshot.runtime_state -ceq 'unavailable') 'Explain invented runtime evidence.'
    Require ($snapshot.graph_view.topology_source -ceq 'explain_costs') 'Plan source is not Explain.'
    Require ($snapshot.graph_view.state -ceq 'complete') 'Fixed supported plan is incomplete.'
    Require (@($snapshot.graph_view.nodes).Count -eq 19) 'Real plan node count changed.'
    Require (@($snapshot.graph_view.edges | Where-Object source -eq 'explicit').Count -eq 12) 'Explicit PlanChild count changed.'
    Require (@($snapshot.graph_view.edges | Where-Object source -eq 'verified_derived').Count -eq 6) 'Verified Exchange count changed.'
    Require (@($snapshot.graph_view.root_node_ids).Count -eq 1) 'Result root is not unique.'
    Require ($snapshot.plan_hash -match '^[0-9a-f]{64}$' -and $snapshot.snapshot_id -match '^[0-9a-f]{64}$') 'Snapshot hashes are missing.'
    Require ($snapshot.plan_hash -ceq $expectedPlanHash) 'Stable but incorrect ordered-row hash.'
    Require ($snapshot.version.detected_release -ceq '3.5.14' -and $snapshot.version.pack_match -ceq 'exact_pack') 'Version admission mismatch.'
    Require ($snapshot.version.selected_pack.upstream_commit -ceq '23a56ec517db763ea9bc7ee50b19357acdee0303') 'Official pack commit mismatch.'
    $expectedComponents = @{
        schema = @('doctor-explain-schema', '0.2', '955e385ef3720aa21d95b0b010ca455b0e9f96aaa33f2b6560cda63da95fbe75')
        adapter = @('sr3514-explain-costs-adapter', '0.1.0', '9e1cf3c55ac583f07c5cc5d114dcccf4e7ac5229e41dd6ae369af984193d5b52')
        exchange_recipe = @('sr3514-explain-costs-exchange-recipe', '1', '448480cadd709c14b66cd65328ed116fe7471da1f5ad0b53a08cc8f51ba78f67')
        version_pack = @('starrocks-3.5.14', '0.1.4', 'b84158917f9048293893e19c2555058776b5e7706077c1ac2713e6a61ef04b7a')
    }
    foreach ($name in @('schema', 'adapter', 'exchange_recipe', 'version_pack')) {
        $component = $snapshot.receipt.$name
        $expected = $expectedComponents[$name]
        Require ($component.id -ceq $expected[0] -and $component.version -ceq $expected[1] -and $component.manifest_hash -ceq $expected[2]) 'Registered receipt binding mismatch.'
    }
    Require ($snapshot.source_view.source_kind -ceq 'explain_costs') 'Source view kind mismatch.'
    Require ($snapshot.source_view.display_state -ceq 'safe_structure_only' -and $snapshot.source_view.truncation_state -ceq 'not_truncated') 'Source projection state mismatch.'
    $results = @($snapshot.source_locators | Where-Object kind -CEQ 'result')
    Require ($results.Count -eq 1 -and $results[0].row_count -eq 233 -and $results[0].byte_length -gt 0) 'Result source metadata mismatch.'
    $result = $results[0]
    Require ($result.byte_length -eq ($rowLengths | Measure-Object -Sum).Sum) 'Result bytes differ from independent CLI rows.'
    $refs = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $locatorByRef = @{}
    $metadataFields = @()
    $rowNumbers = [Collections.Generic.HashSet[int]]::new()
    foreach ($locator in $snapshot.source_locators) {
        Require (-not [string]::IsNullOrWhiteSpace($locator.source_ref)) 'Empty locator identity.'
        Require ($refs.Add($locator.source_ref)) 'Duplicate locator identity.'
        $locatorByRef[$locator.source_ref] = $locator
        switch -CaseSensitive ($locator.kind) {
            'result' { Require ($locator.source_ref -ceq 'explain:result') 'Result source identity mismatch.' }
            'connection_metadata' {
                Require ($locator.field -cin @('detected_version', 'default_database')) 'Unexpected metadata field.'
                Require ($locator.availability -ceq 'available' -and $locator.evidence_hash -match '^[0-9a-f]{64}$') 'Actual fixture metadata missing.'
                $metadataFields += $locator.field
            }
            'row_line' {
                Require ($locator.row_no -ge 1 -and $locator.row_no -le $result.row_count -and $locator.row_line_no -eq 1 -and $locator.global_line_no -eq $locator.row_no) 'Fixed linewise fixture locator identity mismatch.'
                Require ($locator.byte_start -eq 0 -and $null -ne $locator.byte_end -and $locator.byte_end -ge 0 -and $locator.byte_end -le $result.byte_length) 'Invalid row byte range.'
                Require ($locator.byte_end -eq $rowLengths[$locator.row_no - 1]) 'Row byte range differs from independent CLI row.'
                Require ($rowNumbers.Add([int]$locator.row_no)) 'Duplicate fixed fixture row locator.'
            }
            default { throw 'Unexpected Explain locator variant.' }
        }
    }
    Require ($rowNumbers.Count -eq 233 -and ((@($metadataFields | Sort-Object) -join ',') -ceq 'default_database,detected_version')) 'Incomplete result/metadata source coverage.'
    Require (@($snapshot.source_refs).Count -ge 3 -and @($snapshot.graph_view.source_refs).Count -gt 19 -and @($snapshot.version.source_refs).Count -eq 2) 'Empty root/graph/version source coverage.'
    $allRefs = @($snapshot.source_refs) + @($snapshot.graph_view.source_refs) + @($snapshot.version.source_refs)
    $nodeIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $nodeById = @{}
    $nodeFingerprint = @()
    foreach ($node in $snapshot.graph_view.nodes) {
        Require (-not [string]::IsNullOrWhiteSpace($node.node_id) -and @($node.source_refs).Count -gt 0) 'Empty node identity/evidence.'
        Require ($nodeIds.Add($node.node_id)) 'Duplicate graph node identity.'
        Require (@($node.source_refs).Count -eq 1 -and $node.source_refs[0] -ceq "explain:row:$($expectedNodeRows[$node.plan_node_id]):line:1") 'Node source points to the wrong input row.'
        Require ($node.entity_kind -ceq 'plan_node' -and $null -eq $node.execution_state) 'Node claims non-plan execution facts.'
        $nodeById[$node.node_id] = $node
        $nodeFingerprint += "$($node.plan_node_id)|$($node.plan_node_type)|$($node.fragment_id)"
        $allRefs += @($node.source_refs)
        Require (@($node.metric_observation_refs).Count -eq 0 -and @($node.native_operator_refs).Count -eq 0 -and @($node.subordinate_operator_refs).Count -eq 0 -and $null -eq $node.pipeline_id) 'Plan node includes runtime metric/operator references.'
    }
    foreach ($root in $snapshot.graph_view.root_node_ids) { Require ($nodeIds.Contains($root)) 'Unresolved result root.' }
    Require ($nodeById[$snapshot.graph_view.root_node_ids[0]].plan_node_id -ceq '18') 'Wrong result root.'
    $expectedNodes = @('0|OLAP_SCAN|F00','1|OLAP_SCAN|F01','2|EXCHANGE|F00','3|HASH_JOIN|F00','4|PROJECT|F00','5|EXCHANGE|F06','6|OLAP_SCAN|F04','7|EXCHANGE|F06','8|HASH_JOIN|F06','9|PROJECT|F06','10|OLAP_SCAN|F07','11|EXCHANGE|F06','12|HASH_JOIN|F06','13|PROJECT|F06','14|AGGREGATE|F06','15|EXCHANGE|F09','16|AGGREGATE|F09','17|TOP_N|F09','18|MERGING_EXCHANGE|F10')
    Require ((@($nodeFingerprint | Sort-Object) -join ',') -ceq (@($expectedNodes | Sort-Object) -join ',')) 'Actual node structure differs from independent golden.'
    $explicitFingerprint = @(); $exchangeFingerprint = @()
    Require (@($snapshot.graph_view.edges).Count -eq 18) 'Unexpected extra graph edges.'
    foreach ($edge in $snapshot.graph_view.edges) {
        Require ($nodeIds.Contains($edge.from) -and $nodeIds.Contains($edge.to)) 'Dangling graph edge.'
        Require (@($edge.evidence_refs).Count -gt 0) 'Edge lacks provenance.'
        $pair = "$($nodeById[$edge.from].plan_node_id)>$($nodeById[$edge.to].plan_node_id)"
        if ($edge.source -ceq 'explicit') {
            Require ($edge.relation -ceq 'plan_child' -and $edge.direction_semantics -ceq 'parent_to_child' -and $null -eq $edge.derivation_recipe) 'Explicit plan-child semantics changed.'
            $explicitFingerprint += $pair
            $expectedProof = @($expectedNodeRows[$nodeById[$edge.from].plan_node_id], $expectedNodeRows[$nodeById[$edge.to].plan_node_id]) | ForEach-Object { "explain:row:${_}:line:1" }
        } else {
            Require ($edge.relation -ceq 'exchange_link' -and $edge.direction_semantics -ceq 'upstream_to_downstream' -and $edge.derivation_recipe.id -ceq 'sr3514.explain_costs.unique_exchange_link' -and $edge.derivation_recipe.version -ceq '1') 'Derived Exchange proof changed.'
            Require (@($edge.evidence_refs).Count -ge 4) 'Exchange proof lacks four source spans.'
            $exchangeFingerprint += $pair
            $expectedProof = @($expectedExchangeRows[$pair]) | ForEach-Object { "explain:row:${_}:line:1" }
        }
        Require ((@($edge.evidence_refs) -join ',') -ceq ($expectedProof -join ',')) 'Edge source roles differ from independent CLI proof.'
        $allRefs += @($edge.evidence_refs)
    }
    foreach ($relation in $snapshot.graph_view.unresolved_relations) { $allRefs += @($relation.source_refs) }
    Require ((@($explicitFingerprint | Sort-Object) -join ',') -ceq (@('17>16','16>15','14>13','13>12','12>11','12>9','9>8','8>7','8>5','4>3','3>2','3>0' | Sort-Object) -join ',')) 'Plan-child structure fingerprint mismatch.'
    Require ((@($exchangeFingerprint | Sort-Object) -join ',') -ceq (@('17>18','14>15','10>11','6>7','4>5','1>2' | Sort-Object) -join ',')) 'Exchange structure fingerprint mismatch.'
    foreach ($diagnostic in $snapshot.diagnostics) { Require (@($diagnostic.source_refs).Count -gt 0) 'Diagnostic lacks source.'; $allRefs += @($diagnostic.source_refs) }
    Require (@($snapshot.source_view.lines).Count -eq 26) 'Safe source projection lacks fragment/node coverage.'
    foreach ($line in $snapshot.source_view.lines) {
        $allRefs += @($line.source_ref)
        Require ($locatorByRef[$line.source_ref].kind -ceq 'row_line' -and $locatorByRef[$line.source_ref].global_line_no -eq $line.line_no) 'Safe line does not resolve to its original row.'
        Require ($line.text -cmatch '^(PLAN FRAGMENT [0-9]+\(F[0-9]+\)|[0-9]+:(AGGREGATE|EXCHANGE|HASH_JOIN|MERGING_EXCHANGE|OLAP_SCAN|PROJECT|TOP_N))$') 'Unsafe source projection text.'
    }
    foreach ($ref in $allRefs) { Require ($refs.Contains($ref)) 'Unresolved snapshot source reference.' }
    $json = $response | ConvertTo-Json -Depth 100 -Compress
    foreach ($secret in @('explain_fixture', 'rows4', 'bucket', 'actualRows', 'column statistics', $sql)) {
        Require (-not $json.Contains($secret)) 'Explain response leaked an attribute or SQL/database value.'
    }
    $second = Api-Json Post "/connections/$connectionId/explain" $payload
    Require ($snapshot.plan_hash -ceq $second.snapshot.plan_hash -and $snapshot.snapshot_id -ceq $second.snapshot.snapshot_id) 'Stable fixture identity changed across identical requests.'
    $receipt = [ordered]@{
        status = 'PASS'; scope = 'real_fixture_database_connector_api_only'; image_digest = $image
        explain_schema_version = $snapshot.explain_schema_version; nodes = 19; explicit_edges = 12; derived_edges = 6
        source_locators = $refs.Count; plan_hash = $snapshot.plan_hash; snapshot_id = $snapshot.snapshot_id
        runtime_state = $snapshot.runtime_state; browser_and_visual = 'NOT_RUN'; audit = 'NOT_RUN'
    }
}
finally {
    if ($connectionId) { Api-Json Delete "/connections/$connectionId" | Out-Null }
    $sql = $null
    $payload = $null
}
# Emit PASS only after the connection has actually closed.
if ($SaveSnapshot) {
    # Only the checked, safe DTO is retained, never SQL, credentials or raw rows.
    $artifactRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e\explain-20260901')
    [IO.Directory]::CreateDirectory($artifactRoot) | Out-Null
    $artifactPath = Join-Path $artifactRoot 'explain-api-safe-snapshot.json'
    [IO.File]::WriteAllText($artifactPath, ($snapshot | ConvertTo-Json -Depth 100), [Text.UTF8Encoding]::new($false))
    $receipt.snapshot_file_hash = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
}
$receipt | ConvertTo-Json -Depth 10 -Compress
