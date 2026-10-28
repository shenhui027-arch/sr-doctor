use doctor_core::{ExplainLimits, ExplainStructureState, normalize_explain_costs};

#[test]
fn duplicate_target_must_not_be_deleted_to_make_exchange_unique() {
    let input = "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  9:EXCHANGE\n\nPLAN FRAGMENT 1(F1)\n  RESULT SINK\n\n  9:EXCHANGE\n\nPLAN FRAGMENT 2(F2)\n  OutPut Exchange Id: 9\n\n  1:OlapScanNode\n";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());
    assert!(facts.verified_exchange_links.is_empty());
}

#[test]
fn advertised_child_list_without_child_zero_is_not_complete() {
    let input = "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  3:Project\n  |  \n";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());
    assert_ne!(facts.state, ExplainStructureState::Complete);
}

#[test]
fn exchange_output_keeps_evidence_source_order_not_identity_sort() {
    let input =
        include_str!("../../../../fixtures/explain/starrocks-3.5.14/select-multijoin.costs.txt");
    let facts = normalize_explain_costs(input, &ExplainLimits::default());
    assert_eq!(
        facts
            .verified_exchange_links
            .iter()
            .map(|link| &link.exchange_identity)
            .collect::<Vec<_>>(),
        facts
            .exchange_candidates
            .iter()
            .map(|candidate| &candidate.exchange_identity)
            .collect::<Vec<_>>(),
    );
}

#[test]
fn multiline_attribute_cannot_introduce_a_structural_fragment() {
    let input = "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  1:Project\n     output: 'literal\nPLAN FRAGMENT 1(F1)\n  RESULT SINK\n\n  99:Project\nend'\n";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());
    assert!(facts.nodes.iter().all(|node| node.plan_node_id != 99));
}

#[test]
fn conflicting_sink_is_not_discarded_before_global_uniqueness_check() {
    let input = "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  9:EXCHANGE\n\nPLAN FRAGMENT 1(F1)\n  OutPut Exchange Id: 9\n  OutPut Exchange Id: 10\n\n  1:OlapScanNode\n\nPLAN FRAGMENT 2(F2)\n  OutPut Exchange Id: 9\n\n  2:OlapScanNode\n";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());
    assert!(facts.verified_exchange_links.is_empty());
}

#[test]
fn node_budget_cannot_hide_a_later_duplicate_target() {
    let input = "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  9:EXCHANGE\n\nPLAN FRAGMENT 1(F1)\n  OutPut Exchange Id: 9\n\n  1:OlapScanNode\n\nPLAN FRAGMENT 2(F2)\n  RESULT SINK\n\n  9:EXCHANGE\n";
    let limits = ExplainLimits {
        max_nodes: 2,
        ..ExplainLimits::default()
    };
    let facts = normalize_explain_costs(input, &limits);
    assert!(facts.verified_exchange_links.is_empty());
}
