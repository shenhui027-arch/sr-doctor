//! Golden regression for the plan-node execution summary on a real StarRocks
//! 3.5.14 Docker capture, checked against the official ExplainAnalyzer recipe.

use doctor_contracts::{
    AnalysisSnapshotReport, ExecutionSummaryState, HotspotTier, PlanNodeSummary,
    RequestedSourceKind, SummaryValueState,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const CORPUS: &str =
    include_str!("../../../../fixtures/runtime/3.5.14/docker_runtime_corpus_complex.profile.txt");
const MINIMAL_JSON: &str =
    include_str!("../../../../fixtures/synthetic/3.5.14/sr3514_minimal.json");

fn report(kind: RequestedSourceKind, input: &str) -> AnalysisSnapshotReport {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(kind, input.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    evaluate_rules(&mut output).unwrap();
    assemble_public_report(output).unwrap()
}

fn node<'a>(report: &'a AnalysisSnapshotReport, plan_node_id: &str) -> &'a PlanNodeSummary {
    report
        .execution_summary
        .nodes
        .iter()
        .find(|node| node.plan_node_id == plan_node_id)
        .unwrap()
}

fn display(value: &doctor_contracts::SummaryValue) -> &str {
    value.display_text.as_deref().unwrap_or("")
}

#[test]
fn real_corpus_has_one_exact_summary_per_plan_node() {
    let report = report(RequestedSourceKind::TextProfile, CORPUS);
    let summary = &report.execution_summary;
    assert_eq!(summary.state, ExecutionSummaryState::Available);
    assert_eq!(display(&summary.query_total_time), "474 ms");
    assert_eq!(display(&summary.cumulative_operator_time), "153.505 ms");
    assert_eq!(display(&summary.result_rows), "24");
    assert_eq!(summary.nodes.len(), report.graph_view.nodes.len());
    for node in &summary.nodes {
        assert_eq!(
            node.total_time.state,
            SummaryValueState::Exact,
            "{}",
            node.plan_node_id
        );
        assert_eq!(
            node.output_rows.state,
            SummaryValueState::Exact,
            "{}",
            node.plan_node_id
        );
        assert!(node.reasons.is_empty(), "{}", node.plan_node_id);
        assert!(
            node.operators
                .iter()
                .all(|operator| operator.operator_type != "UNKNOWN"),
            "{}",
            node.plan_node_id
        );
    }
}

#[test]
fn real_corpus_follows_explain_analyzer_time_and_rows() {
    let report = report(RequestedSourceKind::TextProfile, CORPUS);
    // Scans: Σ max(OperatorTotalTime) + ScanTime; both above the official 15%.
    let sales = node(&report, "0");
    assert_eq!(sales.hotspot, HotspotTier::SecondMostConsuming);
    assert_eq!(sales.time_share_display.as_deref(), Some("17.28%"));
    assert_eq!(display(&sales.output_rows), "50,000");
    let returns = node(&report, "1");
    assert_eq!(returns.hotspot, HotspotTier::SecondMostConsuming);
    assert_eq!(returns.time_share_display.as_deref(), Some("16.69%"));
    // Merge exchange: source side rows, both fragments' operators plus NetworkTime.
    let merge = node(&report, "27");
    assert_eq!(merge.fragment_ref.as_deref(), Some("0"));
    assert_eq!(display(&merge.output_rows), "24");
    assert_eq!(display(&merge.total_time), "4.415 ms");
    assert_eq!(merge.operators.len(), 5);
    // Hash-partitioned exchange reads PullRowNum without broadcast division.
    assert_eq!(display(&node(&report, "11").output_rows), "20,000");
    // Join: probe side rows, build and probe time both counted.
    let join = node(&report, "16");
    assert_eq!(display(&join.output_rows), "50,000");
    assert_eq!(display(&join.total_time), "8.059 ms");
    let tiers = report
        .execution_summary
        .nodes
        .iter()
        .filter(|node| node.hotspot == HotspotTier::MostConsuming)
        .count();
    assert_eq!(tiers, 0);
}

#[test]
fn summary_is_explicitly_unavailable_without_runtime_operators() {
    let report = report(RequestedSourceKind::JsonProfile, MINIMAL_JSON);
    let summary = &report.execution_summary;
    assert_eq!(summary.state, ExecutionSummaryState::Unavailable);
    assert!(summary.nodes.is_empty());
    assert!(!summary.reasons.is_empty());
}

/// v0.6 F3: on the real complex capture every plan node is bound to a fragment
/// from the Topology, and that fragment is the one its producing operators run
/// in (the execution summary's independent `fragment_ref`).
#[test]
fn real_corpus_graph_fragments_match_the_operator_fragments() {
    let report = report(RequestedSourceKind::TextProfile, CORPUS);
    let mut fragments = std::collections::BTreeSet::new();
    for graph_node in &report.graph_view.nodes {
        let fragment = graph_node
            .fragment_id
            .as_deref()
            .unwrap_or_else(|| panic!("plan node {} is unbound", graph_node.node_id));
        let plan_node_id = graph_node.plan_node_id.as_deref().unwrap();
        assert_eq!(
            node(&report, plan_node_id).fragment_ref.as_deref(),
            Some(fragment),
            "plan node {plan_node_id}"
        );
        fragments.insert(fragment.to_owned());
    }
    assert!(fragments.len() > 1, "{fragments:?}");
}
