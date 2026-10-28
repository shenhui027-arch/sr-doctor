param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$')]
    [string]$QueryId,
    [ValidatePattern('^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$')]
    [string]$CandidateQueryId,
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string]$ProfilePath,
    [ValidateNotNullOrEmpty()]
    [string]$Username = $env:STARROCKS_USER,
    [ValidatePattern('^[A-Za-z_][A-Za-z0-9_]*$')]
    [string]$PasswordEnvironmentVariable = 'STARROCKS_PASSWORD',
    [int]$MySqlPort = 39030,
    [int]$ApiPort = 8000
)

$ErrorActionPreference = 'Stop'
if ($MySqlPort -lt 1 -or $MySqlPort -gt 65535 -or $ApiPort -lt 1 -or $ApiPort -gt 65535) {
    throw 'Invalid local port.'
}
$password = [Environment]::GetEnvironmentVariable($PasswordEnvironmentVariable)
if ([string]::IsNullOrWhiteSpace($Username) -or [string]::IsNullOrEmpty($password)) {
    throw "Set STARROCKS_USER and $PasswordEnvironmentVariable before running the smoke test."
}
$resolvedProfilePath = (Resolve-Path -LiteralPath $ProfilePath -ErrorAction Stop).Path
$profileText = [IO.File]::ReadAllText($resolvedProfilePath)
if ([string]::IsNullOrWhiteSpace($profileText)) {
    throw 'ProfilePath must contain the raw non-empty get_query_profile result.'
}
$explainAdapterManifestPath = Join-Path $PSScriptRoot '..\..\contracts\manifests\sr3514-explain-costs-adapter.json'
$explainAdapterManifest = Get-Content -LiteralPath $explainAdapterManifestPath -Raw | ConvertFrom-Json
if ($explainAdapterManifest.id -ne 'sr3514-explain-costs-adapter' `
    -or [string]::IsNullOrWhiteSpace($explainAdapterManifest.version)) {
    throw 'The registered EXPLAIN adapter manifest is invalid.'
}
if (-not [string]::IsNullOrWhiteSpace($CandidateQueryId) -and $CandidateQueryId -eq $QueryId) {
    throw 'CandidateQueryId must differ from QueryId for connected Profile comparison.'
}

# This harness does not provision tables or execute the profiled query. The query
# must already have been run in the isolated Docker fixture, outside the product.
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$capabilities = Invoke-RestMethod "$apiBase/capabilities"
$headers = @{ 'X-Doctor-CSRF' = $capabilities.csrf.nonce }
$connectionId = $null
$connectedCompareCandidateQueryId = $null
$connectedCompareKey = $null

function Post-Json([string]$Path, $Payload) {
    Invoke-RestMethod "$apiBase$Path" -Method Post -Headers $headers -ContentType 'application/json' -Body ($Payload | ConvertTo-Json -Depth 100 -Compress)
}

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

# This fixture permits only its exact table reference in the two typed graph
# location slots. Check the remaining response unchanged for source disclosure.
function Assert-RuntimeSqlDisclosure($Response, [string]$Sql, [string]$Secret) {
    $serialized = $Response | ConvertTo-Json -Depth 100 -Compress
    Require (-not $serialized.Contains($Sql)) 'Runtime Top response leaked the complete submitted SQL.'
    Require (-not $serialized.Contains('p11_runtime_count')) 'Runtime Top response leaked the submitted SQL alias.'
    $encodedSecret = ConvertTo-Json -InputObject $Secret -Compress
    Require (-not $serialized.Contains($encodedSecret.Substring(1, $encodedSecret.Length - 2))) 'Runtime Top response leaked a credential.'
    $copy = $serialized | ConvertFrom-Json
    foreach ($graph in @($copy.outcome.report.graph_view)) {
        if ($null -eq $graph) { continue }
        foreach ($node in @($graph.nodes)) {
            $property = $node.PSObject.Properties['sql_location']
            if ($null -eq $property -or $null -eq $property.Value) { continue }
            $location = $property.Value
            if ($location.state -eq 'located') {
                Require ($location.kind -eq 'table' -and $location.highlight -ceq 'information_schema.columns' `
                    -and $location.before -ceq '' -and $location.after -ceq '') 'Runtime Top SQL location exceeded the exact fixture excerpt.'
                $location.highlight = ''
            }
            else {
                Require ($location.state -in @('ambiguous', 'unavailable', 'unsupported')) 'Runtime Top SQL location state is invalid.'
            }
        }
    }
    $remaining = $copy | ConvertTo-Json -Depth 100 -Compress
    Require (-not $remaining.Contains('information_schema.columns')) 'Runtime Top response leaked SQL outside a bounded location.'
}

try {
    $session = Post-Json '/connections' @{
        host = '127.0.0.1'; port = $MySqlPort; username = $Username
        password = $password
        catalog = $null; database = $null; audit_table = $null
        tls_mode = 'docker_loopback_plaintext'
    }
    $connectionId = $session.connection_id
    Require ($session.detected_version.value -match '^3\.5\.14(?:-|$)') 'Wrong StarRocks version.'
    Require ($session.capabilities.time_zone -eq 'available') 'Session time-zone probe is unavailable.'
    Require (-not [string]::IsNullOrWhiteSpace($session.time_zone.effective_time_zone)) 'Effective database time zone is missing.'
    $queries = Post-Json "/connections/$connectionId/queries/search" @{
        connection_id = $connectionId; limit = 100; status = $null; digest = $null
    }
    Require (@($queries.queries).Count -gt 0) 'The fixture Audit/Profile discovery is empty.'
    $discoveryGeneration = [int]$queries.discovery_generation
    Require ($discoveryGeneration -gt 0) 'Query discovery did not return a positive generation.'
    $baselineRows = @($queries.queries | Where-Object { $_.query_id -eq $QueryId })
    Require ($baselineRows.Count -eq 1) 'The requested Profile is absent from AuditLoader discovery.'
    Require ($null -ne $baselineRows[0].observed_at_utc) 'A valid Audit timestamp was not interpreted as UTC by the reader session.'
    Require ($baselineRows[0].timestamp_state -eq 'reader_session_interpreted') 'The Audit timestamp reader-session evidence is missing.'
    Require ($queries.limitations -contains 'audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch') 'The Audit timestamp interpretation limitation is missing.'
    $unavailableRows = @($queries.queries | Where-Object {
        $_.profile_fetch.state -eq 'unavailable' -and $_.profile_fetch.reason -eq 'query_not_finished'
    })
    Require ($unavailableRows.Count -gt 0) 'The controlled non-Profile Audit row was not exposed as typed unavailable.'
    Require (@($unavailableRows | Where-Object { $_.plan_check.state -eq 'unavailable' }).Count -gt 0) 'The unavailable row exposed an executable plan action.'

    $fetched = Post-Json "/connections/$connectionId/profiles/fetch" @{
        connection_id = $connectionId; query_id = $QueryId
        discovery_generation = $discoveryGeneration; kind = 'baseline'
    }
    $report = $fetched.outcome.report
    Require ($fetched.state -eq 'fetched' -and $null -ne $report) 'Fetch did not return an analysis outcome.'
    Require ($report.input.source_hash -eq $fetched.source_hash) 'Fetch source hash drift.'
    Require ($report.analysis_key -eq $fetched.analysis_key) 'Fetch analysis key drift.'
    Require ($report.input.detected_kind -eq $fetched.source_kind) 'Fetch source kind drift.'
    Require (($report.analysis_receipt | ConvertTo-Json -Depth 100 -Compress) -ceq ($fetched.analysis_receipt | ConvertTo-Json -Depth 100 -Compress)) 'Fetch receipt drift.'
    Require ($report.query_summary.query_id -eq $QueryId) 'The real query identity was not parsed.'
    $operatorRows = @($report.operator_rows)
    $operatorMetricCells = @($operatorRows | ForEach-Object { @($_.metric_cells) })
    Require ($operatorRows.Count -gt 0) 'The real Profile did not yield trusted operator rows.'
    Require (@($operatorMetricCells | Where-Object { $_.metric_id -eq 'OperatorTotalTime' -and $_.state -eq 'exact' -and @($_.source_refs).Count -gt 0 }).Count -gt 0) 'The real Profile did not retain exact OperatorTotalTime evidence.'
    Require (@($operatorRows | Where-Object { @($_.source_refs).Count -gt 0 }).Count -eq $operatorRows.Count) 'An operator row lacks Profile provenance.'
    Require ($report.profile_context.profile_level -eq 'level_1') 'The official default stored Profile was not identified as Level 1.'
    Require ($report.profile_context.query_state -eq 'finished' -and $report.profile_context.snapshot_kind -eq 'final' -and $report.profile_context.merge_state -eq 'merged') 'The stored Profile final/merged context was not proven.'
    Require ($report.graph_view.state -in @('complete', 'partial')) 'The real stored Profile did not produce a trusted plan graph state.'
    Require (@($report.graph_view.nodes).Count -gt 0 -and @($report.graph_view.edges).Count -gt 0) 'The real stored Profile did not dynamically produce a non-empty plan graph.'
    Require (@($report.graph_view.source_refs).Count -gt 0) 'The real plan graph lacks typed provenance evidence.'
    # sr3514.explain_analyzer.node_info.v1: one exact summary per plan node of a merged Level 1 Profile.
    $summary = $report.execution_summary
    $summaryNodes = @($summary.nodes)
    $graphNodeIds = @($report.graph_view.nodes | ForEach-Object { $_.node_id })
    Require ($summary.state -eq 'available' -and $summary.recipe_id -eq 'sr3514.explain_analyzer.node_info.v1') 'The merged real Profile did not produce an available ExplainAnalyzer execution summary.'
    Require ($summary.query_total_time.state -eq 'exact' -and $summary.cumulative_operator_time.state -eq 'exact') 'The execution summary lost its exact query/operator time basis.'
    Require ($summaryNodes.Count -gt 0 -and @($summaryNodes | Where-Object { $graphNodeIds -notcontains $_.node_id }).Count -eq 0) 'Execution summary nodes are not plan graph nodes.'
    Require (@($summaryNodes | Where-Object { $_.total_time.state -eq 'exact' -and @($_.operators).Count -gt 0 }).Count -gt 0) 'No plan node carries an exact time with its runtime operators.'
    # Black-box oracle: the 3.5.14 FE's own ANALYZE PROFILE text for the same
    # Profile lists "N. NAME (id=K) ...: time (P%)" for its top nodes. Every
    # listed share must equal our ExplainAnalyzer recipe within 0.01 points, and
    # the official >30/>15 classes must equal our hotspot.
    $official = Post-Json "/connections/$connectionId/profiles/official-analysis" @{
        connection_id = $connectionId; query_id = $QueryId; node_ids = @()
        identity = @{ kind = 'discovery'; discovery_generation = $discoveryGeneration }
    }
    $officialShares = @([regex]::Matches([string]$official.display_text, '(?m)^\s*\d+\.\s.*?\(id=(\d+)\).*?:\s.*\((\d+\.\d{2})%\)\s*$') | ForEach-Object {
        [pscustomobject]@{ plan_node_id = $_.Groups[1].Value; percent = [decimal]$_.Groups[2].Value }
    })
    Require ($officialShares.Count -gt 0) 'The official ANALYZE PROFILE text listed no time-consuming nodes.'
    foreach ($share in $officialShares) {
        $ours = @($summaryNodes | Where-Object { $_.plan_node_id -eq $share.plan_node_id })
        Require ($ours.Count -eq 1 -and $null -ne $ours[0].time_share_basis_points) "Official node $($share.plan_node_id) has no exact share in execution_summary."
        $oursPercent = [decimal]$ours[0].time_share_basis_points / 100
        Require ([math]::Abs($oursPercent - $share.percent) -le 0.01) "Node $($share.plan_node_id): official $($share.percent)% but execution_summary $oursPercent%."
        # The FE classifies the unrounded value; within 0.01 of a boundary the
        # two-decimal text cannot decide the class, so only the share is checked.
        if ([math]::Abs($share.percent - 30) -gt 0.01 -and [math]::Abs($share.percent - 15) -gt 0.01) {
            $officialClass = if ($share.percent -gt 30) { 'most_consuming' } elseif ($share.percent -gt 15) { 'second_most_consuming' } else { 'none' }
            Require ($ours[0].hotspot -eq $officialClass) "Node $($share.plan_node_id): official class $officialClass but execution_summary $($ours[0].hotspot)."
        }
    }
    $findings = @($report.findings)
    # Ruleset 0.1.5 suppresses share findings on sub-floor subjects. The small
    # controlled workload must still yield a deterministic rule outcome: either a
    # finding, or a suppressed evaluation that records the floor comparison.
    $floorSuppressed = @($report.rule_evaluations | Where-Object {
        $_.status -eq 'suppressed' -and $null -eq $_.finding_id -and
        @($_.comparisons | Where-Object { $_ -match '^subject_time_all_below_absolute_floor_\d+ns$' }).Count -eq 1 -and
        @($_.threshold_receipts | Where-Object { $_ -like 'absolute_floor:*:product_decision_ruleset_0.1.5' }).Count -eq 1
    })
    Require ($findings.Count + $floorSuppressed.Count -gt 0) 'The controlled real Profile did not yield a deterministic finding or floor-suppressed evaluation.'
    Require (@($findings | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_.rule_id) -and
        @($_.evidence).Count -gt 0 -and
        -not [string]::IsNullOrWhiteSpace($_.recommendation.risk) -and
        -not [string]::IsNullOrWhiteSpace($_.recommendation.expected_direction) -and
        @($_.recommendation.validation_steps).Count -gt 0 -and
        $_.recommendation.recommendation_id -match '^rec\.[a-z_]+\.[a-z_]+@v\d+$' -and
        -not [string]::IsNullOrWhiteSpace($_.recommendation.title) -and
        @($_.recommendation.steps).Count -gt 0 -and
        @($_.recommendation.expected_changes).Count -gt 0 -and
        @($_.recommendation.limitations).Count -gt 0
    }).Count -eq $findings.Count) 'A deterministic finding lacks rule, evidence, risk, direction, validation steps or its structured recommendation (ruleset 0.3.0).'
    Require ($fetched.source_refs -contains 'profile:query_id') 'Connected fetch lacks query Profile provenance.'
    Require ($fetched.query.source_refs -contains 'profile:query_id') 'Connected query reference lacks Profile provenance.'
    Require ($fetched.query.discovery_source -eq $baselineRows[0].discovery_source) 'Connected fetch lost the discovered source.'
    Require ($fetched.query.discovery_generation -eq $discoveryGeneration) 'Connected fetch lost the discovery generation.'
    Require ($fetched.query.observed_at_utc -eq $baselineRows[0].observed_at_utc `
        -and $fetched.query.timestamp_state -eq $baselineRows[0].timestamp_state) 'Connected fetch lost the discovered timestamp evidence.'
    $serialized = $fetched | ConvertTo-Json -Depth 100 -Compress
    Require (-not $serialized.Contains('SELECT COUNT(*) AS column_count')) 'Raw SQL leaked into the connected response.'
    Require (-not $serialized.Contains('"profile_text"')) 'Raw Profile response field is forbidden.'

    $offline = Post-Json '/analyses' @{
        source_kind = 'text_profile'; content = $profileText
        filename = $null; version_hint = $null
    }
    Require ($offline.outcome.report.input.source_hash -eq $report.input.source_hash) 'ProfilePath is not byte-equivalent to the fetched Profile.'
    # Acquisition shape is canonical evidence (`imported` versus
    # `get_query_profile`), so whole-report facts_hash is allowed to differ.
    # The actual parsed workload facts must remain identical for the same bytes.
    Require (($offline.outcome.report.query_summary | ConvertTo-Json -Depth 100 -Compress) -ceq ($report.query_summary | ConvertTo-Json -Depth 100 -Compress)) 'Offline and connected query facts differ.'
    Require (($offline.outcome.report.operator_rows | ConvertTo-Json -Depth 100 -Compress) -ceq ($report.operator_rows | ConvertTo-Json -Depth 100 -Compress)) 'Offline and connected operator facts differ.'
    Require (($offline.outcome.report.graph_view | ConvertTo-Json -Depth 100 -Compress) -ceq ($report.graph_view | ConvertTo-Json -Depth 100 -Compress)) 'Offline and connected graph facts differ.'
    Require (($offline.outcome.report.findings | ConvertTo-Json -Depth 100 -Compress) -ceq ($report.findings | ConvertTo-Json -Depth 100 -Compress)) 'Offline and connected findings differ.'

    $candidateFetchQueryId = if ([string]::IsNullOrWhiteSpace($CandidateQueryId)) { $QueryId } else { $CandidateQueryId }
    $candidate = Post-Json "/connections/$connectionId/profiles/fetch" @{
        connection_id = $connectionId; query_id = $candidateFetchQueryId
        discovery_generation = $discoveryGeneration; kind = 'candidate_user_external_rerun'
    }
    Require ($candidate.input_origin -eq 'candidate_profile_user_external_rerun') 'Candidate responsibility was lost.'
    $connectedComparison = $null
    if (-not [string]::IsNullOrWhiteSpace($CandidateQueryId)) {
        # Both query IDs identify profiles that were created outside the product.
        # The product only reads them through the same short-lived connection.
        $connectedComparison = Post-Json "/connections/$connectionId/profiles/compare" @{
            connection_id = $connectionId
            baseline_query_id = $QueryId
            candidate_query_id = $CandidateQueryId
            discovery_generation = $discoveryGeneration
        }
        Require ($connectedComparison.baseline.query.query_id -eq $QueryId) 'Comparison baseline query ID drift.'
        Require ($connectedComparison.candidate.query.query_id -eq $CandidateQueryId) 'Comparison candidate query ID drift.'
        Require ($connectedComparison.baseline.input_origin -eq 'connected_profile_fetch') 'Comparison baseline origin drift.'
        Require ($connectedComparison.candidate.input_origin -eq 'candidate_profile_user_external_rerun') 'Comparison candidate responsibility was lost.'
        Require ($connectedComparison.comparison.comparison_key -match '^[0-9a-f]{64}$') 'Comparison receipt/key is missing.'
        Require ($null -ne $connectedComparison.comparison.baseline.outcome.report) 'Comparison baseline report is missing.'
        Require ($null -ne $connectedComparison.comparison.candidate.outcome.report) 'Comparison candidate report is missing.'
        Require ($connectedComparison.validation_context.interpretation_state -eq 'observed_only' `
            -and $connectedComparison.validation_context.same_connection_session `
            -and $connectedComparison.validation_context.same_discovery_generation `
            -and $connectedComparison.validation_context.candidate_attestation -eq 'user_external_controlled_rerun') 'Connected comparison validation context is incomplete.'
        $findingDeltaCount = @($connectedComparison.comparison.finding_delta.added).Count `
            + @($connectedComparison.comparison.finding_delta.resolved).Count `
            + @($connectedComparison.comparison.finding_delta.persisting).Count `
            + @($connectedComparison.comparison.finding_delta.changed).Count `
            + @($connectedComparison.comparison.finding_delta.incomparable).Count `
            + @($connectedComparison.comparison.finding_delta.algorithm_changed).Count
        $structureDeltaCount = @($connectedComparison.comparison.structure_delta.added).Count `
            + @($connectedComparison.comparison.structure_delta.removed).Count `
            + @($connectedComparison.comparison.structure_delta.changed).Count `
            + @($connectedComparison.comparison.structure_delta.edge_delta).Count `
            + @($connectedComparison.comparison.structure_delta.unresolved).Count
        Require (@($connectedComparison.comparison.metric_deltas).Count -gt 0 -or $findingDeltaCount -gt 0 -or $structureDeltaCount -gt 0) 'Connected comparison contains no observable delta evidence.'
        $comparisonJson = $connectedComparison | ConvertTo-Json -Depth 100 -Compress
        Require (-not $comparisonJson.Contains('SELECT COUNT(*) AS column_count')) 'Raw SQL leaked into the connected comparison response.'
        Require (-not $comparisonJson.Contains('"profile_text"')) 'Raw Profile response field is forbidden in connected comparison.'
        $connectedCompareCandidateQueryId = $connectedComparison.candidate.query.query_id
        $connectedCompareKey = $connectedComparison.comparison.comparison_key
    }
    $advisor = Invoke-RestMethod "$apiBase/connections/$connectionId/advisor-guides" -Headers $headers
    Require ($advisor.capability -eq 'available') 'Plan Advisor capability is not available in the isolated fixture.'
    Require (@($advisor.recommendations).Count -eq 0) 'v0.1 unexpectedly normalized Advisor rows.'
    Require ($advisor.limitations -contains 'advisor_rows_not_normalized_in_v0_1') 'Advisor limitation is missing.'
    $plan = Post-Json "/connections/$connectionId/explain" @{
        input_kind = 'user_sql'; connection_id = $connectionId; candidate_sql = 'SELECT 1'
    }
    Require ($plan.snapshot.runtime_state -eq 'unavailable') 'EXPLAIN invented runtime evidence.'
    # EXPLAIN COSTS is plan evidence, so its normalized plan graph is expected.
    # Runtime metrics remain unavailable on this topology-only path. Even a
    # constant SELECT can contain a Project -> UNION
    # structure in 3.5.14; every accepted node must retain source evidence.
    Require ($plan.snapshot.graph_view.state -eq 'complete') 'EXPLAIN did not produce the expected complete plan graph.'
    Require ($plan.snapshot.graph_view.topology_source -eq 'explain_costs') 'EXPLAIN graph source is not typed as explain_costs.'
    Require (@($plan.snapshot.graph_view.nodes).Count -gt 0) 'EXPLAIN plan graph contains no nodes.'
    Require (@($plan.snapshot.graph_view.root_node_ids).Count -gt 0) 'EXPLAIN plan graph contains no root.'
    Require (@($plan.snapshot.graph_view.nodes | Where-Object { @($_.source_refs).Count -gt 0 }).Count -eq @($plan.snapshot.graph_view.nodes).Count) 'An EXPLAIN plan node lacks source evidence.'
    Require ($plan.snapshot.receipt.adapter.id -eq $explainAdapterManifest.id `
        -and $plan.snapshot.receipt.adapter.version -eq $explainAdapterManifest.version `
        -and $plan.snapshot.receipt.adapter.manifest_hash -match '^[0-9a-f]{64}$') 'EXPLAIN adapter receipt does not match the registered manifest.'
    Require ($plan.snapshot.plan_hash -match '^[0-9a-f]{64}$') 'EXPLAIN plan hash is missing.'
    Require ($plan.snapshot.source_refs -contains 'explain:result') 'EXPLAIN provenance is missing.'

    $runtimeSql = 'SELECT COUNT(*) AS p11_runtime_count FROM information_schema.columns'
    $runtime = Post-Json "/connections/$connectionId/runtime-profile" @{
        connection_id = $connectionId; candidate_sql = $runtimeSql
    }
    $runtimeReport = $runtime.outcome.report
    Require ($runtime.input_origin -eq 'connected_sql_runtime') 'Runtime Top input origin drifted.'
    Require ($runtime.state -eq 'fetched' -and $null -ne $runtimeReport) 'Runtime Top did not return an analyzed Profile.'
    Require ($runtime.query.query_id -match '^[0-9a-fA-F-]{36}$' `
        -and $runtimeReport.query_summary.query_id -eq $runtime.query.query_id) 'Runtime Top lost the exact query identity.'
    Require ($runtimeReport.profile_context.profile_level -eq 'level_1') 'Runtime Top did not enable the required Level 1 Profile.'
    Require ($runtimeReport.analysis_state -in @('complete', 'partial')) 'Runtime Top did not produce a usable typed analysis state.'
    Require (@($runtimeReport.graph_view.nodes).Count -gt 0 -and @($runtimeReport.graph_view.edges).Count -gt 0) 'Runtime Top produced no plan topology.'
    Require (@($runtimeReport.operator_rows).Count -gt 0) 'Runtime Top produced no operator rows.'
    Require ($runtimeReport.execution_summary.state -eq 'available' -and @($runtimeReport.execution_summary.nodes).Count -gt 0) 'Runtime Top produced no ExplainAnalyzer execution summary.'
    Require ($runtime.source_refs -contains 'runtime:last_query_id' `
        -and $runtime.source_refs -contains 'runtime:get_query_profile') 'Runtime Top provenance is incomplete.'
    Require ($runtime.limitations -contains 'runtime_sql_only_bounded_location_excerpt_returned_not_persisted' `
        -and $runtime.limitations -contains 'runtime_query_timeout_10_seconds' `
        -and $runtime.limitations -contains 'runtime_result_rows_are_discarded') 'Runtime Top execution boundaries are incomplete.'
    Assert-RuntimeSqlDisclosure $runtime $runtimeSql $password

    $rejectedStatus = $null
    try {
        $rejected = Invoke-WebRequest "$apiBase/connections/$connectionId/explain" -Method Post -Headers $headers -ContentType 'application/json' -Body (@{
            input_kind = 'user_sql'; connection_id = $connectionId; candidate_sql = 'DELETE FROM forbidden_fixture'
        } | ConvertTo-Json -Compress)
        $rejectedStatus = [int]$rejected.StatusCode
    }
    catch {
        if ($null -eq $_.Exception.Response) { throw }
        $rejectedStatus = [int]$_.Exception.Response.StatusCode
    }
    Require ($rejectedStatus -eq 422) 'A forbidden statement was accepted.'
    $runtimeRejectedStatus = $null
    try {
        $runtimeRejected = Invoke-WebRequest "$apiBase/connections/$connectionId/runtime-profile" -Method Post -Headers $headers -ContentType 'application/json' -Body (@{
            connection_id = $connectionId; candidate_sql = 'DELETE FROM forbidden_fixture'
        } | ConvertTo-Json -Compress)
        $runtimeRejectedStatus = [int]$runtimeRejected.StatusCode
    }
    catch {
        if ($null -eq $_.Exception.Response) { throw }
        $runtimeRejectedStatus = [int]$_.Exception.Response.StatusCode
    }
    Require ($runtimeRejectedStatus -eq 422) 'Runtime Top accepted a forbidden statement.'

    [pscustomobject]@{
        status = 'PASS'; evidence = 'real_docker_http_not_production'
        detected_version = $session.detected_version.value
        effective_time_zone = $session.time_zone.effective_time_zone
        current_utc_offset_seconds = $session.time_zone.current_utc_offset_seconds
        discovered_queries = @($queries.queries).Count
        unavailable_profile_rows = $unavailableRows.Count
        operator_rows = $operatorRows.Count
        exact_operator_total_time_cells = @($operatorMetricCells | Where-Object { $_.metric_id -eq 'OperatorTotalTime' -and $_.state -eq 'exact' }).Count
        query_metrics = @($report.derived_metric_cells).Count
        graph_state = $report.graph_view.state; graph_nodes = @($report.graph_view.nodes).Count; graph_edges = @($report.graph_view.edges).Count
        summary_state = $summary.state; summary_nodes = $summaryNodes.Count; official_share_nodes_matched = $officialShares.Count
        summary_hotspot_nodes = @($summaryNodes | Where-Object { $_.hotspot -in @('most_consuming', 'second_most_consuming') }).Count
        profile_level = $report.profile_context.profile_level; snapshot_kind = $report.profile_context.snapshot_kind; merge_state = $report.profile_context.merge_state
        findings = $findings.Count; floor_suppressed_evaluations = $floorSuppressed.Count; finding_rule_ids = @($findings | ForEach-Object { $_.rule_id } | Sort-Object -Unique)
        analysis_state = $report.analysis_state; source_hash = $report.input.source_hash
        facts_hash = $report.facts_hash; candidate_origin = $candidate.input_origin
        profile_provenance = 'profile:query_id'
        connected_compare_candidate_query_id = $connectedCompareCandidateQueryId
        connected_compare_key = $connectedCompareKey
        advisor_capability = $advisor.capability
        explain_runtime_state = $plan.snapshot.runtime_state
        explain_graph_state = $plan.snapshot.graph_view.state
        explain_graph_nodes = @($plan.snapshot.graph_view.nodes).Count
        explain_graph_edges = @($plan.snapshot.graph_view.edges).Count
        explain_adapter_version = $plan.snapshot.receipt.adapter.version
        forbidden_statement_status = $rejectedStatus
        runtime_top_query_id = $runtime.query.query_id
        runtime_top_analysis_state = $runtimeReport.analysis_state
        runtime_top_graph_nodes = @($runtimeReport.graph_view.nodes).Count
        runtime_top_graph_edges = @($runtimeReport.graph_view.edges).Count
        runtime_top_operator_rows = @($runtimeReport.operator_rows).Count
        runtime_top_summary_nodes = @($runtimeReport.execution_summary.nodes).Count
        runtime_top_findings = @($runtimeReport.findings).Count
        runtime_top_forbidden_statement_status = $runtimeRejectedStatus
    } | ConvertTo-Json -Compress
}
finally {
    if ($connectionId) {
        Invoke-RestMethod "$apiBase/connections/$connectionId" -Method Delete -Headers $headers -ContentType 'application/json' | Out-Null
    }
}
