// Test helpers intentionally mutate one budget field at a time to keep each boundary visible.
#![allow(clippy::field_reassign_with_default, clippy::result_large_err)]

use doctor_contracts::{
    AnalysisReceipt, AnalysisSnapshotReport, AnalysisState, GraphState, MergeState,
    MetricCellState, ProfileLevel, QueryState, RequestedSourceKind, SnapshotKind, SourceKind,
};
use doctor_core::{AdapterContext, AdapterLimits, InputEnvelope, adapt_input};

fn input(kind: RequestedSourceKind, content: &str) -> InputEnvelope {
    InputEnvelope::new(kind, content.as_bytes())
}

fn context() -> AdapterContext {
    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    AdapterContext::new(report.analysis_receipt).unwrap()
}

fn adapt(
    input: InputEnvelope,
    limits: &AdapterLimits,
) -> Result<doctor_core::AdapterOutput, doctor_core::AdapterError> {
    adapt_input(input, limits, &context())
}

#[test]
fn adapters_keep_selection_budget_and_partial_failures_distinct() {
    assert_eq!(
        AdapterContext::new(AnalysisReceipt { components: vec![] })
            .unwrap_err()
            .message_id,
        "analysis_receipt_invalid"
    );
    let mut malformed_receipt: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    malformed_receipt.analysis_receipt.components[0].manifest_hash = "not-a-hash".into();
    assert_eq!(
        AdapterContext::new(malformed_receipt.analysis_receipt)
            .unwrap_err()
            .message_id,
        "analysis_receipt_invalid"
    );
    let mismatch = adapt(
        input(RequestedSourceKind::JsonProfile, "StarRocks Profile 3.5.14"),
        &AdapterLimits::default(),
    )
    .unwrap_err();
    assert_eq!(mismatch.code, "kind_mismatch");

    let malformed = adapt(
        input(RequestedSourceKind::Auto, "{not-json"),
        &AdapterLimits::default(),
    )
    .unwrap_err();
    assert_eq!(malformed.code, "parse_failed");
    assert_eq!(malformed.adapter_evidence, ["auto:json:first_token"]);

    let mut limits = AdapterLimits::default();
    limits.max_bytes = 1;
    assert_eq!(
        adapt(input(RequestedSourceKind::Auto, "abc"), &limits)
            .unwrap_err()
            .code,
        "input_too_large"
    );
    let mut limits = AdapterLimits::default();
    limits.max_text_lines = 1;
    assert_eq!(
        adapt(input(RequestedSourceKind::TextProfile, "a\nb"), &limits)
            .unwrap_err()
            .code,
        "too_many_lines"
    );
    let mut limits = AdapterLimits::default();
    limits.max_json_depth = 1;
    assert_eq!(
        adapt(
            input(RequestedSourceKind::JsonProfile, r#"{"a":{"b":1}}"#),
            &limits
        )
        .unwrap_err()
        .code,
        "too_deep"
    );
    let mut limits = AdapterLimits::default();
    limits.max_json_nodes = 2;
    assert_eq!(
        adapt(input(RequestedSourceKind::JsonProfile, "[1,2,3]"), &limits)
            .unwrap_err()
            .code,
        "too_many_nodes"
    );
    let mut limits = AdapterLimits::default();
    limits.max_normalized_metrics = 0;
    assert_eq!(
        adapt(
            input(
                RequestedSourceKind::JsonProfile,
                r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#
            ),
            &limits
        )
        .unwrap_err()
        .code,
        "too_many_metrics"
    );

    let partial = adapt(
        input(
            RequestedSourceKind::TextProfile,
            "StarRocks Profile 3.5.14\nOperatorTotalTime: nope\n",
        ),
        &AdapterLimits::default(),
    )
    .unwrap();
    assert_eq!(partial.analysis_state, AnalysisState::Partial);
    assert_eq!(
        partial.facts.metric_cells[0].state,
        MetricCellState::Invalid
    );
    assert!(partial.facts.metric_cells[0].canonical_number.is_none());
    assert_eq!(partial.ir.input.detected_kind, SourceKind::TextProfile);
    assert!(!partial.ir.analysis_receipt.components.is_empty());
}

#[test]
fn json_adapter_preserves_typed_profile_context_and_provenance() {
    let output = adapt(
        input(
            RequestedSourceKind::JsonProfile,
            r#"{
                "profile_version":"3.5.14",
                "operator_total_time_ns":"500000",
                "pipeline_profile_level":1,
                "query_state":"Finished",
                "snapshot_kind":"final",
                "merge_state":"merged"
            }"#,
        ),
        &AdapterLimits::default(),
    )
    .unwrap();

    assert_eq!(
        output.ir.profile_context.profile_level,
        ProfileLevel::Level1
    );
    assert_eq!(output.ir.profile_context.query_state, QueryState::Finished);
    assert_eq!(output.ir.profile_context.snapshot_kind, SnapshotKind::Final);
    assert_eq!(output.ir.profile_context.merge_state, MergeState::Merged);
    assert_eq!(output.ir.profile_context.evidence_refs.len(), 4);
    assert!(
        output
            .ir
            .profile_context
            .evidence_refs
            .iter()
            .all(|source_ref| output
                .source_refs
                .iter()
                .any(|source| &source.ref_id == source_ref))
    );
}

#[test]
fn adapters_accept_only_explicit_topology_and_keep_source_order() {
    let json = adapt(input(RequestedSourceKind::JsonProfile, r#"{
        "profile_version":"3.5.14",
        "operator_total_time_ns":"500000",
        "execution_topology":{
            "complete":true,
            "rootNodeIds":["scan"],
            "nodes":[
                {"nodeId":"scan","planNodeType":"SCAN","operatorProfiles":["native-z","native-a"],"children":["aggregate"]},
                {"nodeId":"aggregate","planNodeType":"AGGREGATE","subordinateOperatorProfiles":["sub-a"],"children":[]}
            ]
        }
    }"#), &AdapterLimits::default()).unwrap();
    assert_eq!(json.ir.graph.state, GraphState::Complete);
    assert_eq!(json.ir.graph.root_node_ids, ["scan"]);
    assert_eq!(
        json.ir
            .graph
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        ["scan", "aggregate"]
    );
    assert_eq!(
        json.ir.graph.nodes[0]
            .native_operator_refs
            .iter()
            .map(|operator| operator.operator_ref.as_str())
            .collect::<Vec<_>>(),
        ["native-z", "native-a"]
    );
    assert_eq!(json.ir.graph.edges.len(), 1);
    assert_eq!(json.ir.graph.edges[0].from, "scan");
    assert_eq!(json.ir.graph.edges[0].to, "aggregate");
    assert_eq!(json.ir.graph.edges[0].source_ordinal, Some(0));
    assert!(
        !json
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "graph_unavailable")
    );

    let text = adapt(input(RequestedSourceKind::TextProfile, "StarRocks Profile 3.5.14\nOperatorTotalTime: 500us\nGraphTopologyComplete: true\nGraphRoot: scan\nGraphNode: node_id=scan;plan_node_type=SCAN;operator_profiles=native-z,native-a;children=aggregate\nGraphNode: node_id=aggregate;plan_node_type=AGGREGATE;subordinate_operator_profiles=sub-a\n"), &AdapterLimits::default()).unwrap();
    assert_eq!(text.ir.graph.state, GraphState::Complete);
    assert_eq!(
        text.ir
            .graph
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        ["scan", "aggregate"]
    );
    assert_eq!(
        text.ir.graph.nodes[0]
            .native_operator_refs
            .iter()
            .map(|operator| operator.source_ordinal)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(text.ir.graph.edges[0].source_ordinal, Some(0));

    let partial = adapt(
        input(
            RequestedSourceKind::JsonProfile,
            r#"{
        "profile_version":"3.5.14",
        "operator_total_time_ns":"500000",
        "execution_topology":{"rootNodeIds":["scan"],"nodes":[
            {"nodeId":"scan","children":["aggregate"]},
            {"nodeId":"aggregate"}
        ]}
    }"#,
        ),
        &AdapterLimits::default(),
    )
    .unwrap();
    assert_eq!(partial.ir.graph.state, GraphState::Partial);
    assert_eq!(partial.ir.graph.edges.len(), 1);
    assert!(
        partial
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "graph_partial")
    );

    let conflict = adapt(
        input(
            RequestedSourceKind::JsonProfile,
            r#"{
        "profile_version":"3.5.14",
        "operator_total_time_ns":"500000",
        "execution_topology":{"complete":true,"rootNodeIds":["missing"],"nodes":[{"nodeId":"scan"}]}
    }"#,
        ),
        &AdapterLimits::default(),
    )
    .unwrap();
    assert_eq!(conflict.ir.graph.state, GraphState::Unavailable);
    assert!(conflict.ir.graph.nodes.is_empty() && conflict.ir.graph.edges.is_empty());
    assert!(
        conflict
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "graph_evidence_conflict")
    );
}
