//! The runtime graph is `complete` only when the FE `Topology` satisfies the
//! invariants StarRocks 3.5.14 guarantees for it (ProfilingExecPlan.Topology,
//! ExchangeNode, DataStreamSink `dest_node_id`). Each violation is reported as
//! a typed unresolved relation and keeps the graph partial; a same-id exchange
//! source/sink pair in two fragments is one exchange node, never "unproven".

use doctor_contracts::{AnalysisSnapshotReport, GraphState, InputRole, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    resolve_version_and_graph, v01_analysis_receipt,
};

fn report(topology: &str, fragments: &str) -> AnalysisSnapshotReport {
    let text = format!(
        "Query:\n  Summary:\n    - Query Type: Query\n    - Query State: Finished\n    - StarRocks Version: 3.5.14-23a56ec\n  Execution:\n    - Topology: {topology}\n{fragments}"
    );
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, text.into_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    assemble_public_report(output).unwrap()
}

fn operator(name: &str, plan_node_id: i32) -> String {
    format!(
        "        {name} (plan_node_id={plan_node_id}):\n          CommonMetrics:\n            - OperatorTotalTime: 1ms\n"
    )
}

fn fragment(id: u32, operators: &[(&str, i32)]) -> String {
    let body = operators
        .iter()
        .map(|(name, plan_node_id)| operator(name, *plan_node_id))
        .collect::<String>();
    format!("    Fragment {id}:\n      Pipeline (id={id}):\n{body}")
}

/// AGGREGATION 4 <- EXCHANGE 3 <- [Fragment 1 root] PROJECT 2 <- OLAP_SCAN 1.
const TOPOLOGY: &str = r#"{"rootId":4,"nodes":[{"id":4,"name":"AGGREGATION","properties":{"sinkIds":[]},"children":[3]},{"id":3,"name":"EXCHANGE","children":[2]},{"id":2,"name":"PROJECT","properties":{"sinkIds":[3]},"children":[1]},{"id":1,"name":"OLAP_SCAN","children":[]}]}"#;

fn two_fragments() -> String {
    fragment(0, &[("AGGREGATION", 4), ("EXCHANGE_SOURCE", 3)])
        + &fragment(1, &[("EXCHANGE_SINK", 3), ("PROJECT", 2), ("OLAP_SCAN", 1)])
}

fn reasons(report: &AnalysisSnapshotReport) -> Vec<String> {
    report
        .graph_view
        .unresolved_relations
        .iter()
        .map(|relation| relation.reason.clone())
        .collect()
}

fn assert_partial_with(report: &AnalysisSnapshotReport, reason: &str) {
    assert_eq!(
        report.graph_view.state,
        GraphState::Partial,
        "{:?}",
        reasons(report)
    );
    assert!(
        reasons(report).iter().any(|value| value == reason),
        "{reason} not in {:?}",
        reasons(report)
    );
}

#[test]
fn an_official_two_fragment_topology_is_complete_without_unproven_exchange_links() {
    let report = report(TOPOLOGY, &two_fragments());
    assert_eq!(report.graph_view.state, GraphState::Complete);
    assert!(report.graph_view.unresolved_relations.is_empty());
    assert!(report.graph_view.unconnected_node_ids.is_empty());
    // The exchange node lists both halves (Fragment 0 source, Fragment 1 sink) as native operators.
    let exchange = report
        .graph_view
        .nodes
        .iter()
        .find(|node| node.node_id == "3")
        .unwrap();
    assert_eq!(exchange.native_operator_refs.len(), 2);
}

#[test]
fn join_children_carry_the_official_probe_and_build_roles() {
    let topology = r#"{"rootId":3,"nodes":[{"id":3,"name":"HASH_JOIN","properties":{"sinkIds":[]},"children":[1,2]},{"id":1,"name":"OLAP_SCAN","children":[]},{"id":2,"name":"OLAP_SCAN","children":[]}]}"#;
    let report = report(
        topology,
        &fragment(
            0,
            &[
                ("HASH_JOIN_PROBE", 3),
                ("OLAP_SCAN", 1),
                ("HASH_JOIN_BUILD", 3),
                ("OLAP_SCAN", 2),
            ],
        ),
    );
    let role = |to: &str| {
        report
            .graph_view
            .edges
            .iter()
            .find(|edge| edge.from == "3" && edge.to == to)
            .and_then(|edge| edge.input_role.clone())
    };
    assert_eq!(role("1"), Some(InputRole::Probe));
    assert_eq!(role("2"), Some(InputRole::Build));
}

#[test]
fn a_sender_outside_its_root_fragment_contradicts_the_topology() {
    let fragments = fragment(
        0,
        &[
            ("AGGREGATION", 4),
            ("EXCHANGE_SOURCE", 3),
            ("EXCHANGE_SINK", 3),
        ],
    ) + &fragment(1, &[("PROJECT", 2), ("OLAP_SCAN", 1)]);
    let report = report(TOPOLOGY, &fragments);
    assert_partial_with(&report, "exchange_sink_fragment_mismatch");
    assert_partial_with(&report, "exchange_sender_receiver_same_fragment");
}

#[test]
fn sink_ids_that_do_not_name_the_parent_exchange_contradict_the_topology() {
    let topology = TOPOLOGY.replace(r#""sinkIds":[3]"#, r#""sinkIds":[4]"#);
    assert_partial_with(
        &report(&topology, &two_fragments()),
        "topology_sink_target_mismatch",
    );
}

#[test]
fn a_scan_with_children_contradicts_the_topology() {
    let topology = r#"{"rootId":2,"nodes":[{"id":2,"name":"PROJECT","properties":{"sinkIds":[]},"children":[1]},{"id":1,"name":"OLAP_SCAN","children":[0]},{"id":0,"name":"OLAP_SCAN","children":[]}]}"#;
    let fragments = fragment(0, &[("PROJECT", 2), ("OLAP_SCAN", 1), ("OLAP_SCAN", 0)]);
    assert_partial_with(&report(topology, &fragments), "topology_scan_has_children");
}

#[test]
fn an_exchange_with_two_children_contradicts_the_topology() {
    let topology = r#"{"rootId":3,"nodes":[{"id":3,"name":"EXCHANGE","properties":{"sinkIds":[]},"children":[1,2]},{"id":1,"name":"OLAP_SCAN","children":[]},{"id":2,"name":"OLAP_SCAN","children":[]}]}"#;
    let fragments = fragment(
        0,
        &[("EXCHANGE_SOURCE", 3), ("OLAP_SCAN", 1), ("OLAP_SCAN", 2)],
    );
    assert_partial_with(
        &report(topology, &fragments),
        "topology_exchange_child_count",
    );
}

#[test]
fn an_unreachable_node_contradicts_the_topology() {
    let topology = r#"{"rootId":2,"nodes":[{"id":2,"name":"PROJECT","properties":{"sinkIds":[]},"children":[1]},{"id":1,"name":"OLAP_SCAN","children":[]},{"id":0,"name":"OLAP_SCAN","children":[]}]}"#;
    let fragments = fragment(0, &[("PROJECT", 2), ("OLAP_SCAN", 1), ("OLAP_SCAN", 0)]);
    assert_partial_with(&report(topology, &fragments), "topology_node_unreachable");
}

#[test]
fn a_cycle_contradicts_the_topology() {
    let topology = r#"{"rootId":2,"nodes":[{"id":2,"name":"PROJECT","properties":{"sinkIds":[]},"children":[1]},{"id":1,"name":"PROJECT","children":[2]}]}"#;
    let fragments = fragment(0, &[("PROJECT", 2), ("PROJECT", 1)]);
    assert_partial_with(&report(topology, &fragments), "topology_cycle");
}

#[test]
fn only_a_multicast_producer_may_have_several_parents() {
    let shared = r#"{"rootId":3,"nodes":[{"id":3,"name":"UNION","properties":{"sinkIds":[]},"children":[2,1]},{"id":2,"name":"PROJECT","children":[0]},{"id":1,"name":"PROJECT","children":[0]},{"id":0,"name":"OLAP_SCAN","children":[]}]}"#;
    let fragments = fragment(
        0,
        &[
            ("UNION", 3),
            ("PROJECT", 2),
            ("PROJECT", 1),
            ("OLAP_SCAN", 0),
        ],
    );
    assert_partial_with(&report(shared, &fragments), "topology_multiple_parents");

    // A CTE producer root sinks to each consuming exchange: a legitimate DAG.
    let multicast = r#"{"rootId":5,"nodes":[{"id":5,"name":"HASH_JOIN","properties":{"sinkIds":[]},"children":[3,4]},{"id":3,"name":"EXCHANGE","children":[1]},{"id":4,"name":"EXCHANGE","children":[1]},{"id":1,"name":"PROJECT","properties":{"sinkIds":[3,4]},"children":[0]},{"id":0,"name":"OLAP_SCAN","children":[]}]}"#;
    let fragments = fragment(
        0,
        &[
            ("HASH_JOIN_PROBE", 5),
            ("EXCHANGE_SOURCE", 3),
            ("HASH_JOIN_BUILD", 5),
            ("EXCHANGE_SOURCE", 4),
        ],
    ) + &fragment(
        1,
        &[
            ("EXCHANGE_SINK", 3),
            ("EXCHANGE_SINK", 4),
            ("PROJECT", 1),
            ("OLAP_SCAN", 0),
        ],
    );
    let report = report(multicast, &fragments);
    assert_eq!(
        report.graph_view.state,
        GraphState::Complete,
        "{:?}",
        reasons(&report)
    );
}

fn fragment_of(report: &AnalysisSnapshotReport, node_id: &str) -> Option<String> {
    report
        .graph_view
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .and_then(|node| node.fragment_id.clone())
}

#[test]
fn each_plan_node_is_bound_to_the_fragment_that_executes_it() {
    let report = report(TOPOLOGY, &two_fragments());
    // The receiving EXCHANGE stays with its consumer (Fragment 0), as ExplainAnalyzer prints it.
    for (node, fragment) in [("4", "0"), ("3", "0"), ("2", "1"), ("1", "1")] {
        assert_eq!(
            fragment_of(&report, node).as_deref(),
            Some(fragment),
            "node {node}"
        );
    }
}

#[test]
fn a_segment_without_operator_evidence_stays_unbound_without_a_contradiction() {
    let report = report(
        TOPOLOGY,
        &fragment(0, &[("AGGREGATION", 4), ("EXCHANGE_SOURCE", 3)]),
    );
    assert_eq!(report.graph_view.state, GraphState::Complete);
    assert_eq!(fragment_of(&report, "3").as_deref(), Some("0"));
    assert_eq!(fragment_of(&report, "2"), None);
    assert_eq!(fragment_of(&report, "1"), None);
}

#[test]
fn a_plan_node_executed_in_two_fragments_is_a_fragment_conflict() {
    let fragments = fragment(
        0,
        &[("AGGREGATION", 4), ("EXCHANGE_SOURCE", 3), ("PROJECT", 2)],
    ) + &fragment(1, &[("EXCHANGE_SINK", 3), ("PROJECT", 2), ("OLAP_SCAN", 1)]);
    let report = report(TOPOLOGY, &fragments);
    assert_partial_with(&report, "fragment_attribution_conflict");
    assert_eq!(fragment_of(&report, "4").as_deref(), Some("0"));
    assert_eq!(fragment_of(&report, "2"), None);
    assert_eq!(fragment_of(&report, "1"), None);
}

#[test]
fn two_segments_claiming_one_fragment_are_both_unbound() {
    let fragments = fragment(
        0,
        &[
            ("AGGREGATION", 4),
            ("EXCHANGE_SOURCE", 3),
            ("EXCHANGE_SINK", 3),
            ("PROJECT", 2),
            ("OLAP_SCAN", 1),
        ],
    );
    let report = report(TOPOLOGY, &fragments);
    assert_partial_with(&report, "fragment_attribution_conflict");
    assert!(
        report
            .graph_view
            .nodes
            .iter()
            .all(|node| node.fragment_id.is_none())
    );
}

#[test]
fn a_repeated_topology_line_is_a_conflict_not_a_silent_overwrite() {
    let report = report(
        &format!("{TOPOLOGY}\n    - Topology: {TOPOLOGY}"),
        &two_fragments(),
    );
    assert_eq!(report.graph_view.state, GraphState::Unavailable);
    assert!(report.graph_view.edges.is_empty());
    let conflict = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "runtime_topology_conflict")
        .expect("conflict diagnostic");
    assert_eq!(conflict.source_refs, ["text:7", "text:8"]);
}

#[test]
fn the_fragment_sink_edge_is_marked_as_restating_the_exchange_plan_child_edge() {
    let report = report(TOPOLOGY, &two_fragments());
    let edges = &report.graph_view.edges;
    let plan_child = edges
        .iter()
        .find(|edge| edge.from == "3" && edge.to == "2")
        .expect("exchange -> sender root plan_child edge");
    let sink = edges
        .iter()
        .find(|edge| edge.from == "2" && edge.to == "3")
        .expect("sender root -> exchange fragment_sink edge");
    assert_eq!(
        sink.restates_edge_id.as_deref(),
        Some(plan_child.edge_id.as_str())
    );
    assert!(
        edges
            .iter()
            .filter(|edge| edge.edge_id != sink.edge_id)
            .all(|edge| edge.restates_edge_id.is_none())
    );
}

#[test]
fn contradictions_and_their_refs_are_ordered_numerically() {
    // Unreachable 11 and 4: "4" sorts before "11".
    let topology = r#"{"rootId":2,"nodes":[{"id":2,"name":"PROJECT","properties":{"sinkIds":[]},"children":[1]},{"id":1,"name":"OLAP_SCAN","children":[]},{"id":11,"name":"OLAP_SCAN","children":[]},{"id":4,"name":"OLAP_SCAN","children":[]}]}"#;
    let unreachable = report(topology, &fragment(0, &[("PROJECT", 2), ("OLAP_SCAN", 1)]));
    let refs = unreachable
        .graph_view
        .unresolved_relations
        .iter()
        .map(|relation| relation.relation_ref.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        refs,
        [
            "official_topology:topology_node_unreachable:4",
            "official_topology:topology_node_unreachable:11",
        ]
    );

    // Evidence refs from lines 6 and 12 keep numeric order (lexically "text:12" < "text:6").
    let fragments = fragment(
        0,
        &[
            ("AGGREGATION", 4),
            ("EXCHANGE_SOURCE", 3),
            ("EXCHANGE_SINK", 3),
        ],
    ) + &fragment(1, &[("PROJECT", 2), ("OLAP_SCAN", 1)]);
    let report = report(TOPOLOGY, &fragments);
    for relation in &report.graph_view.unresolved_relations {
        let lines = relation
            .source_refs
            .iter()
            .map(|reference| {
                reference
                    .strip_prefix("text:")
                    .and_then(|line| line.parse::<u32>().ok())
                    .expect("text line ref")
            })
            .collect::<Vec<_>>();
        assert!(lines.windows(2).all(|pair| pair[0] < pair[1]), "{lines:?}");
    }
}
