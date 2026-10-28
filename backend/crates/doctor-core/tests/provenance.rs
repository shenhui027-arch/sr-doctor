use doctor_contracts::{AnalysisSnapshotReport, GraphState, RequestedSourceKind, SourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, RedactionState, SourceLocator, adapt_input,
};

fn adapt(kind: RequestedSourceKind, content: &str) -> doctor_core::AdapterOutput {
    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    let context = AdapterContext::new(report.analysis_receipt).unwrap();
    adapt_input(
        InputEnvelope::new(kind, content.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap()
}

#[test]
fn equivalent_json_and_text_keep_facts_but_not_source_identity() {
    let json = adapt(
        RequestedSourceKind::Auto,
        include_str!("../../../../fixtures/synthetic/3.5.14/sr3514_minimal.json"),
    );
    let text = adapt(
        RequestedSourceKind::Auto,
        include_str!("../../../../fixtures/synthetic/3.5.14/sr3514_minimal.txt"),
    );
    assert_eq!(json.facts_hash(), text.facts_hash());
    assert_ne!(json.ir.input.source_hash, text.ir.input.source_hash);
    assert_ne!(json.ir.analysis_key, text.ir.analysis_key);
    assert_eq!(json.ir.input.detected_kind, SourceKind::JsonProfile);
    assert_eq!(text.ir.input.detected_kind, SourceKind::TextProfile);
    assert_eq!(
        json.ir.metric_observations[0].source_refs,
        ["json:/operator_total_time_ns"]
    );
    assert_eq!(text.ir.metric_observations[0].source_refs, ["text:3"]);
    assert!(text.ir.diagnostics.iter().any(|diagnostic| diagnostic.code
        == "fragment_structure_unresolved"
        && diagnostic.source_refs == ["text:2"]));
}

#[test]
fn provenance_is_located_and_diagnostics_never_echo_input() {
    let secret = "SELECT secret FROM C:\\\\private\\\\profile";
    let output = adapt(
        RequestedSourceKind::TextProfile,
        &format!("StarRocks Profile 3.5.14\nOperatorTotalTime: 500us {secret}\n"),
    );
    assert!(matches!(
        output
            .source_refs
            .iter()
            .find(|source| source.ref_id == "text:2")
            .unwrap()
            .locator,
        SourceLocator::Text {
            line_start: 2,
            line_end: 2,
            ..
        }
    ));
    assert_eq!(output.ir.graph.state, GraphState::Unavailable);
    assert!(
        output.ir.graph.nodes.is_empty()
            && output.ir.graph.edges.is_empty()
            && output.ir.graph.root_node_ids.is_empty()
            && output.ir.graph.unresolved_relations.is_empty()
    );
    let visible = format!("{:?}", output);
    assert!(!visible.contains(secret));
    assert!(!visible.contains("C:\\\\private"));
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.source_refs == ["text:2"])
    );
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .flat_map(|diagnostic| &diagnostic.source_refs)
            .all(|reference| output
                .source_refs
                .iter()
                .any(|source| source.ref_id == *reference))
    );

    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    let context = AdapterContext::new(report.analysis_receipt).unwrap();
    let error = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::Auto,
            format!("{{{secret}").into_bytes(),
        ),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap_err();
    assert_eq!(error.code, "parse_failed");
    assert!(!format!("{:?}", error).contains(secret));

    let unknown_key = "SELECT secret FROM profile";
    let unknown = adapt(
        RequestedSourceKind::JsonProfile,
        &format!(
            r#"{{"profile_version":"3.5.14","operator_total_time_ns":"1","{unknown_key}":true}}"#
        ),
    );
    let diagnostic = unknown
        .ir
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "unknown_json_field")
        .unwrap();
    assert_eq!(diagnostic.source_refs, ["json:/unknown_field/0"]);
    let source = unknown
        .source_refs
        .iter()
        .find(|source| source.ref_id == "json:/unknown_field/0")
        .unwrap();
    assert_eq!(source.redaction_state, RedactionState::Redacted);
    assert!(!format!("{:?}", unknown).contains(unknown_key));
    assert!(
        matches!(&unknown.source_refs.iter().find(|source| source.ref_id == "json:").unwrap().locator,
        SourceLocator::Json { json_pointer, .. } if json_pointer.is_empty())
    );
    assert!(
        unknown
            .ir
            .diagnostics
            .iter()
            .flat_map(|diagnostic| &diagnostic.source_refs)
            .all(|reference| unknown
                .source_refs
                .iter()
                .any(|source| source.ref_id == *reference))
    );
}

#[test]
fn explicit_graph_evidence_keeps_located_json_and_text_provenance() {
    let json = adapt(
        RequestedSourceKind::JsonProfile,
        r#"{
        "profile_version":"3.5.14",
        "operator_total_time_ns":"500000",
        "execution_topology":{"complete":true,"rootNodeIds":["scan"],"nodes":[
            {"nodeId":"scan","children":["aggregate"]},
            {"nodeId":"aggregate"}
        ]}
    }"#,
    );
    assert_eq!(json.ir.graph.state, GraphState::Complete);
    assert_eq!(json.ir.graph.source_refs, ["json:/execution_topology"]);
    let json_source = json
        .source_refs
        .iter()
        .find(|source| source.ref_id == "json:/execution_topology")
        .unwrap();
    assert!(
        matches!(&json_source.locator, SourceLocator::Json { json_pointer, byte_start, byte_end }
        if json_pointer == "/execution_topology" && byte_start < byte_end)
    );
    assert!(
        json.ir
            .graph
            .nodes
            .iter()
            .all(|node| node.source_refs == ["json:/execution_topology"])
    );
    assert!(
        json.ir
            .graph
            .edges
            .iter()
            .all(|edge| edge.evidence_refs == ["json:/execution_topology"])
    );

    let text = adapt(
        RequestedSourceKind::TextProfile,
        "StarRocks Profile 3.5.14\nOperatorTotalTime: 500us\nGraphTopologyComplete: true\nGraphRoot: scan\nGraphNode: node_id=scan;children=aggregate\nGraphNode: node_id=aggregate\n",
    );
    assert_eq!(text.ir.graph.state, GraphState::Complete);
    assert_eq!(text.ir.graph.nodes[0].source_refs, ["text:5"]);
    assert_eq!(text.ir.graph.edges[0].evidence_refs, ["text:5"]);
    assert!(text.ir.graph.source_refs.iter().all(|reference| {
        text.source_refs
            .iter()
            .any(|source| source.ref_id == *reference)
    }));
    assert!(
        text.ir
            .graph
            .nodes
            .iter()
            .flat_map(|node| &node.source_refs)
            .chain(
                text.ir
                    .graph
                    .edges
                    .iter()
                    .flat_map(|edge| &edge.evidence_refs)
            )
            .all(|reference| text
                .source_refs
                .iter()
                .any(|source| source.ref_id == *reference))
    );
}
