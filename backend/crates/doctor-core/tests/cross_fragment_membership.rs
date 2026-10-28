use doctor_contracts::{OrderedOperatorRef, RequestedSourceKind, SourceRole};
use doctor_core::{
    AdapterContext, AdapterLimits, AdapterOutput, InputEnvelope, adapt_input,
    assemble_public_report, evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const PROFILE: &str = include_str!(
    "../../../../fixtures/synthetic/3.5.14/sr3514_cross_fragment_exchange.profile.txt"
);

fn resolved() -> AdapterOutput {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, PROFILE.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, graph_evidence).unwrap();
    // This suite validates presentation membership invariants, not the text
    // formatter's runtime-role inference. Inject explicit native-role evidence
    // so the test cannot turn an absent `IsSubordinate` marker into native.
    for group in &mut output.ir.operator_groups {
        if group.plan_node_id.is_none() {
            continue;
        }
        group.native_operator_refs = output
            .ir
            .physical_operator_memberships
            .iter()
            .filter(|membership| {
                membership.plan_node_id == group.plan_node_id
                    && membership.fragment_ref == group.fragment_ref
            })
            .enumerate()
            .map(|(ordinal, membership)| {
                (
                    membership.physical_operator_ref.clone(),
                    u32::try_from(ordinal).unwrap(),
                )
            })
            .collect();
    }
    for observation in &mut output.ir.metric_observations {
        observation.operator_ref = observation.runtime_occurrence_ref.clone();
    }
    for node in &mut output.ir.graph.nodes {
        let mut ordinal = 0u32;
        node.native_operator_refs = output
            .ir
            .operator_groups
            .iter()
            .filter(|group| group.plan_node_id == node.plan_node_id)
            .flat_map(|group| group.native_operator_refs.iter())
            .map(|(operator_ref, _)| {
                let reference = OrderedOperatorRef {
                    operator_ref: operator_ref.clone(),
                    source_ordinal: ordinal,
                    source_role: SourceRole::Native,
                    source_refs: node.source_refs.clone(),
                };
                ordinal += 1;
                reference
            })
            .collect();
    }
    evaluate_rules(&mut output).unwrap();
    output
}

#[test]
fn graph_and_group_ordinals_are_independent_across_fragments() {
    let output = resolved();
    let node = &output.ir.graph.nodes[0];
    assert_eq!(
        node.native_operator_refs
            .iter()
            .map(|r| r.source_ordinal)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    let groups = output
        .ir
        .operator_groups
        .iter()
        .filter(|g| !g.native_operator_refs.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|g| g.native_operator_refs[0].1 == 0));

    let report = assemble_public_report(output).unwrap();
    assert_eq!(report.graph_view.nodes.len(), 1);
    assert_eq!(report.operator_rows.len(), 2);
    // No merge evidence in this synthetic capture: no per-node summary.
    assert!(report.execution_summary.nodes.is_empty());
}
