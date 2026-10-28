use doctor_core::{
    EXPLAIN_EXCHANGE_RECIPE_ID, EXPLAIN_EXCHANGE_RECIPE_VERSION, ExplainLimits, ExplainNodeKind,
    ExplainSinkKind, ExplainStructureState, normalize_explain_costs,
};

const ACTUAL_3514_MULTI_JOIN_COSTS: &str =
    include_str!("../../../../fixtures/explain/starrocks-3.5.14/select-multijoin.costs.txt");
const ACTUAL_3514_CONSTANT_COSTS: &str =
    include_str!("../../../../fixtures/explain/starrocks-3.5.14/select-constant.costs.txt");
const SANITIZED_3514_CAPTURE_047: &str =
    include_str!("../../../../fixtures/explain/starrocks-3.5.14/capture-047.sanitized.costs.txt");

fn contains_limitation(facts: &doctor_core::ExplainPlanFacts, code: &str) -> bool {
    facts
        .limitations
        .iter()
        .any(|limitation| limitation.code == code)
}

fn two_child_tree() -> &'static str {
    concat!(
        "PLAN FRAGMENT 0(F0)\n",
        "  RESULT SINK\n",
        "\n",
        "  3:Project\n",
        "  |  \n",
        "  |----2:EXCHANGE\n",
        "  |    \n",
        "  1:OlapScanNode\n",
    )
}

fn three_deep_child_zero_tree() -> &'static str {
    concat!(
        "PLAN FRAGMENT 0(F0)\n",
        "  RESULT SINK\n",
        "\n",
        "  3:Project\n",
        "  |  \n",
        "  2:Project\n",
        "  |  \n",
        "  1:OlapScanNode\n",
    )
}

#[test]
fn normalizes_captured_3514_multijoin_costs_to_safe_facts() {
    let facts = normalize_explain_costs(ACTUAL_3514_MULTI_JOIN_COSTS, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert_eq!(facts.fragments.len(), 7);
    assert_eq!(facts.nodes.len(), 19);
    assert_eq!(facts.plan_children.len(), 12);
    assert_eq!(facts.exchange_candidates.len(), 6);
    assert_eq!(facts.verified_exchange_links.len(), 6);
    assert!(
        facts
            .fragments
            .iter()
            .any(|fragment| fragment.fragment_ref == "F09")
    );
    assert_eq!(
        facts.result_root_node_refs,
        vec!["explain:F10:plan:18".to_owned()]
    );
    assert!(facts.nodes.iter().any(|node| {
        node.node_ref == "explain:F10:plan:18" && node.kind == ExplainNodeKind::MergingExchange
    }));
    assert!(facts.nodes.iter().all(|node| node.source.line_start > 0));
    assert_eq!(
        facts
            .exchange_candidates
            .iter()
            .map(|candidate| candidate.exchange_identity.as_str())
            .collect::<Vec<_>>(),
        vec!["18", "15", "11", "7", "5", "2"]
    );
    assert_eq!(
        facts
            .verified_exchange_links
            .iter()
            .map(|link| link.exchange_identity.as_str())
            .collect::<Vec<_>>(),
        vec!["18", "15", "11", "7", "5", "2"]
    );
    assert!(facts.verified_exchange_links.iter().all(|link| {
        link.recipe_id == EXPLAIN_EXCHANGE_RECIPE_ID
            && link.recipe_version == EXPLAIN_EXCHANGE_RECIPE_VERSION
            && link.evidence.len() == 4
    }));

    let safe_debug = format!("{:?}", facts.safe_structure);
    assert!(!safe_debug.contains("bucket"));
    assert!(!safe_debug.contains("cardinality"));
    assert!(!safe_debug.contains("column statistics"));
}

#[test]
fn parses_sanitized_45_node_capture_with_only_allowlisted_plan_facts() {
    let facts = normalize_explain_costs(SANITIZED_3514_CAPTURE_047, &ExplainLimits::default());
    assert_eq!(facts.fragments.len(), 10);
    assert_eq!(facts.nodes.len(), 45);
    assert_eq!(
        facts.plan_children.len() + facts.verified_exchange_links.len(),
        44
    );
    assert_eq!(facts.plan_node_facts.len(), 45);
    assert_eq!(
        facts
            .plan_node_facts
            .iter()
            .flat_map(|set| &set.facts)
            .filter(|fact| {
                matches!(
                    fact,
                    doctor_core::ExplainPlanNodeFact::Available {
                        kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate
                            | doctor_core::ExplainPlanFactKind::Distribution
                            | doctor_core::ExplainPlanFactKind::JoinStrategy,
                        ..
                    }
                )
            })
            .count(),
        58,
    );
    assert_eq!(
        facts
            .plan_node_facts
            .iter()
            .flat_map(|set| &set.facts)
            .filter(|fact| matches!(
                fact,
                doctor_core::ExplainPlanNodeFact::Available {
                    kind: doctor_core::ExplainPlanFactKind::AggregatePhase,
                    ..
                }
            ))
            .count(),
        4
    );
    assert!(
        facts
            .plan_node_facts
            .iter()
            .all(|set| set.facts.iter().all(|fact| {
                matches!(
                    fact,
                    doctor_core::ExplainPlanNodeFact::Available { .. }
                        | doctor_core::ExplainPlanNodeFact::Unavailable { .. }
                )
            }))
    );
    assert!(contains_limitation(&facts, "result_root_unproven"));
}

#[test]
fn all_sixteen_frozen_3514_node_labels_are_recognized() {
    let facts = normalize_explain_costs(
        include_str!("../../../../fixtures/explain/starrocks-3.5.14/all-node-labels.costs.txt"),
        &ExplainLimits::default(),
    );
    assert_eq!(facts.nodes.len(), 17);
    assert_eq!(
        facts
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            ExplainNodeKind::Aggregate,
            ExplainNodeKind::Analytic,
            ExplainNodeKind::AssertNumberOfRows,
            ExplainNodeKind::Decode,
            ExplainNodeKind::Exchange,
            ExplainNodeKind::HashJoin,
            ExplainNodeKind::MergingExchange,
            ExplainNodeKind::OlapScan,
            ExplainNodeKind::PartitionTopN,
            ExplainNodeKind::Project,
            ExplainNodeKind::ScanMysql,
            ExplainNodeKind::Select,
            ExplainNodeKind::Sort,
            ExplainNodeKind::TableValueFunction,
            ExplainNodeKind::TopN,
            ExplainNodeKind::Union,
            ExplainNodeKind::Aggregate,
        ]
    );
}

#[test]
fn sanitized_goldens_cover_every_frozen_scalar_fact_variant() {
    let fixture = normalize_explain_costs(
        include_str!("../../../../fixtures/explain/starrocks-3.5.14/all-node-labels.costs.txt"),
        &ExplainLimits::default(),
    );
    let capture = normalize_explain_costs(SANITIZED_3514_CAPTURE_047, &ExplainLimits::default());
    let mut values = fixture
        .plan_node_facts
        .iter()
        .chain(&capture.plan_node_facts)
        .flat_map(|set| &set.facts)
        .filter_map(|fact| match fact {
            doctor_core::ExplainPlanNodeFact::Available { value, .. } => Some(format!("{value:?}")),
            doctor_core::ExplainPlanNodeFact::Unavailable { .. } => None,
        })
        .collect::<Vec<_>>();
    values.sort();
    for expected in [
        "Broadcast",
        "Gather",
        "Shuffle",
        "InnerBroadcast",
        "InnerPartitioned",
        "RightOuterBucketShuffle",
        "RightOuterPartitioned",
        "UpdateSerialize",
        "UpdateFinalize",
        "MergeFinalize",
    ] {
        assert!(
            values.iter().any(|value| value.contains(expected)),
            "missing {expected}"
        );
    }
}

#[test]
fn unknown_labels_values_and_wrong_indentation_never_become_typed_facts() {
    for input in [
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  0:MAGIC NODE\n",
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  0:Project\n  | cardinality: 7\n",
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  0:Project\n  |  cardinality: secret-value\n",
    ] {
        let facts = normalize_explain_costs(input, &ExplainLimits::default());
        assert!(
            facts
                .plan_node_facts
                .iter()
                .flat_map(|set| &set.facts)
                .all(|fact| !matches!(
                    fact,
                    doctor_core::ExplainPlanNodeFact::Available {
                        kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                        ..
                    }
                ))
        );
    }
}

#[test]
fn unknown_node_label_is_partial_with_stable_code_and_does_not_store_raw_label() {
    let facts = normalize_explain_costs(
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  0:MAGIC NODE\n",
        &ExplainLimits::default(),
    );
    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(contains_limitation(&facts, "unregistered_plan_node"));
    assert!(!format!("{facts:?}").contains("MAGIC NODE"));
}

#[test]
fn invalid_cardinality_lexemes_are_parser_level_unavailable_facts() {
    for (value, expected_reason) in [
        ("1.5", "invalid_cardinality_estimate"),
        ("1e3", "invalid_cardinality_estimate"),
        ("-1", "invalid_cardinality_estimate"),
        (
            "1234567890123456789012345678901234567890",
            "fact_digit_budget_exceeded",
        ),
        ("(7, 8)", "invalid_cardinality_estimate"),
        ("\"7\"", "invalid_cardinality_estimate"),
    ] {
        let input = format!(
            "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  0:Project\n  |  cardinality: {value}\n"
        );
        let facts = normalize_explain_costs(&input, &ExplainLimits::default());
        let cardinality = facts.plan_node_facts[0]
            .facts
            .iter()
            .find(|fact| {
                matches!(
                    fact,
                    doctor_core::ExplainPlanNodeFact::Available {
                        kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                        ..
                    } | doctor_core::ExplainPlanNodeFact::Unavailable {
                        kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                        ..
                    }
                )
            })
            .expect("cardinality fact exists");
        let doctor_core::ExplainPlanNodeFact::Unavailable { reason_codes, .. } = cardinality else {
            panic!("invalid cardinality became available: {value}");
        };
        assert_eq!(reason_codes, &[expected_reason], "value {value}");
    }
}

#[test]
fn fact_candidate_budget_stops_before_accepting_the_next_registered_fact() {
    let limits = ExplainLimits {
        max_fact_candidates: 1,
        ..ExplainLimits::default()
    };
    let facts = normalize_explain_costs(
        concat!(
            "PLAN FRAGMENT 0(F0)\n",
            "  RESULT SINK\n",
            "\n",
            "  0:Project\n",
            "  |  cardinality: 7\n",
            "  |  cardinality: 8\n",
        ),
        &limits,
    );
    assert!(contains_limitation(&facts, "fact_candidate_limit_exceeded"));
    let cardinality = facts.plan_node_facts[0]
        .facts
        .iter()
        .find(|fact| {
            matches!(
                fact,
                doctor_core::ExplainPlanNodeFact::Available {
                    kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                    ..
                } | doctor_core::ExplainPlanNodeFact::Unavailable {
                    kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                    ..
                }
            )
        })
        .expect("first candidate remains represented");
    assert!(matches!(
        cardinality,
        doctor_core::ExplainPlanNodeFact::Available { .. }
    ));
}

#[test]
fn repeated_identical_fact_is_unavailable_without_claiming_a_value_conflict() {
    let facts = normalize_explain_costs(
        concat!(
            "PLAN FRAGMENT 0(F0)\n",
            "  RESULT SINK\n",
            "\n",
            "  0:Project\n",
            "  |  cardinality: 0007\n",
            "  |  cardinality: 7\n",
        ),
        &ExplainLimits::default(),
    );
    let cardinality = facts.plan_node_facts[0]
        .facts
        .iter()
        .find(|fact| {
            matches!(
                fact,
                doctor_core::ExplainPlanNodeFact::Unavailable {
                    kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                    ..
                }
            )
        })
        .expect("duplicate cardinality fact");
    let doctor_core::ExplainPlanNodeFact::Unavailable { reason_codes, .. } = cardinality else {
        unreachable!();
    };
    assert_eq!(reason_codes, &["duplicate_fact"]);
}

#[test]
fn repeated_different_fact_is_unavailable_and_records_the_conflict() {
    let facts = normalize_explain_costs(
        concat!(
            "PLAN FRAGMENT 0(F0)\n",
            "  RESULT SINK\n",
            "\n",
            "  0:Project\n",
            "  |  cardinality: 7\n",
            "  |  cardinality: 8\n",
        ),
        &ExplainLimits::default(),
    );
    let cardinality = facts.plan_node_facts[0]
        .facts
        .iter()
        .find(|fact| {
            matches!(
                fact,
                doctor_core::ExplainPlanNodeFact::Unavailable {
                    kind: doctor_core::ExplainPlanFactKind::CardinalityEstimate,
                    ..
                }
            )
        })
        .expect("conflicting cardinality fact");
    let doctor_core::ExplainPlanNodeFact::Unavailable { reason_codes, .. } = cardinality else {
        unreachable!();
    };
    assert_eq!(reason_codes, &["duplicate_fact", "conflicting_fact"]);
}

#[test]
fn normalizes_captured_3514_constant_union_without_downgrading_the_graph() {
    let facts = normalize_explain_costs(ACTUAL_3514_CONSTANT_COSTS, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert_eq!(facts.fragments.len(), 1);
    assert_eq!(facts.nodes.len(), 2);
    assert_eq!(facts.plan_children.len(), 1);
    assert_eq!(facts.result_root_node_refs, vec!["explain:F00:plan:1"]);
    assert!(facts.nodes.iter().any(|node| {
        node.node_ref == "explain:F00:plan:0" && node.kind == ExplainNodeKind::Union
    }));
    assert!(facts.limitations.is_empty());
}

fn child_relations(facts: &doctor_core::ExplainPlanFacts) -> Vec<(&str, &str, &str)> {
    facts
        .plan_children
        .iter()
        .map(|relation| {
            (
                relation.parent_node_ref.as_str(),
                relation.child_node_ref.as_str(),
                relation.input_role.as_str(),
            )
        })
        .collect()
}

/// `PlanNode.getCostExplain` prints `children[1..]` as `|----` branches before
/// `children[0]`; relations follow the child index, as the runtime Topology does.
#[test]
fn orders_plan_children_by_official_child_index_without_inferred_roles() {
    let facts = normalize_explain_costs(two_child_tree(), &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert_eq!(
        child_relations(&facts),
        vec![
            ("explain:F0:plan:3", "explain:F0:plan:1", "child"),
            ("explain:F0:plan:3", "explain:F0:plan:2", "child"),
        ]
    );
}

#[test]
fn three_input_parent_keeps_branch_order_after_child_zero_and_other_slots() {
    let facts = normalize_explain_costs(
        concat!(
            "PLAN FRAGMENT 0(F0)\n",
            "  RESULT SINK\n",
            "\n",
            "  0:UNION\n",
            "  |  \n",
            "  |----2:Project\n",
            "  |    |  \n",
            "  |    1:OlapScanNode\n",
            "  |    \n",
            "  |----4:EXCHANGE\n",
            "  |    \n",
            "  6:EXCHANGE\n",
        ),
        &ExplainLimits::default(),
    );

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert_eq!(
        child_relations(&facts),
        vec![
            ("explain:F0:plan:0", "explain:F0:plan:6", "child"),
            ("explain:F0:plan:2", "explain:F0:plan:1", "child"),
            ("explain:F0:plan:0", "explain:F0:plan:2", "child"),
            ("explain:F0:plan:0", "explain:F0:plan:4", "child"),
        ]
    );
}

#[test]
fn preserves_crlf_line_and_byte_locations_without_dedent() {
    let input = two_child_tree().replace('\n', "\r\n");
    let facts = normalize_explain_costs(&input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Complete);
    let child_zero = facts
        .nodes
        .iter()
        .find(|node| node.node_ref == "explain:F0:plan:1")
        .expect("synthetic child0 node exists");
    assert_eq!(child_zero.source.line_start, 8);
    assert_eq!(
        child_zero.source.byte_start,
        input.find("  1:OlapScanNode").expect("source line exists")
    );
}

#[test]
fn ignores_structure_shaped_text_inside_registered_attribute_lines() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  1:Project
     predicate: PLAN FRAGMENT 9(F9)
     output: 2:EXCHANGE
     note: OutPut Exchange Id: 2
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert_eq!(facts.fragments.len(), 1);
    assert_eq!(facts.nodes.len(), 1);
    assert!(facts.exchange_candidates.is_empty());
    assert!(facts.verified_exchange_links.is_empty());
}

#[test]
fn keeps_fragment_display_ordinals_and_fragment_ids_separately_unique() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  1:Project

PLAN FRAGMENT 1(F0)
  RESULT SINK

  2:Project

PLAN FRAGMENT 1(F1)
  RESULT SINK

  3:Project
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.fragments.len(), 1);
    assert_eq!(facts.nodes.len(), 1);
    assert_eq!(
        facts
            .limitations
            .iter()
            .filter(|limitation| limitation.code == "duplicate_plan_fragment")
            .count(),
        2
    );
}

#[test]
fn duplicate_fragment_exchange_identity_blocks_links_without_trusting_the_duplicate() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 2(F2)
  OutPut Exchange Id: 9

  1:OlapScanNode
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(
        &facts,
        "exchange_identity_conflicted_by_duplicate_fragment"
    ));
}

#[test]
fn duplicate_exchange_targets_remain_visible_to_uniqueness_resolution() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F1)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 2(F2)
  OutPut Exchange Id: 9

  1:OlapScanNode
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|node| node.plan_node_id == 9)
            .count(),
        2
    );
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(&facts, "duplicate_plan_node_id"));
    assert!(contains_limitation(&facts, "exchange_identity_not_unique"));
}

#[test]
fn advertised_children_without_child_zero_are_partial() {
    let input = concat!(
        "PLAN FRAGMENT 0(F0)\n",
        "  RESULT SINK\n",
        "\n",
        "  3:Project\n",
        "  |  \n",
    );
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(contains_limitation(&facts, "plan_child_zero_missing"));
}

#[test]
fn multiline_quoted_attribute_never_opens_a_synthetic_fragment() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  1:Project
     output: 'literal
PLAN FRAGMENT 1(F1)
  RESULT SINK

  99:Project
end'
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(facts.nodes.iter().all(|node| node.plan_node_id != 99));
    assert!(contains_limitation(
        &facts,
        "multiline_attribute_unsupported"
    ));
}

#[test]
fn branch_after_child_zero_is_not_accepted_as_another_child() {
    let input = concat!(
        "PLAN FRAGMENT 0(F0)\n",
        "  RESULT SINK\n",
        "\n",
        "  3:Project\n",
        "  |  \n",
        "  1:OlapScanNode\n",
        "  \n",
        "  |----2:EXCHANGE\n",
    );
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.plan_children.len(), 1);
    assert!(contains_limitation(&facts, "plan_child_parent_unproven"));
}

#[test]
fn duplicate_plan_identity_is_unresolved_even_when_only_one_node_is_exchange() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F1)
  RESULT SINK

  9:Project

PLAN FRAGMENT 2(F2)
  OutPut Exchange Id: 9

  1:OlapScanNode
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(&facts, "duplicate_plan_node_id"));
    assert!(contains_limitation(&facts, "exchange_identity_not_unique"));
}

#[test]
fn result_or_multicast_exchange_identity_blocks_a_later_unique_candidate() {
    // RESULT SINK cannot name an exchange; a multicast destination is a real
    // second source of identity 9, so neither fragment is a unique sender.
    for (sink, code) in [
        (
            "RESULT SINK",
            "exchange_identity_conflicted_by_duplicate_fragment",
        ),
        ("MultiCastDataSinks:", "exchange_identity_not_unique"),
    ] {
        let input = format!(
            "PLAN FRAGMENT 0(F0)\n\
               \x20 RESULT SINK\n\
               \n\
               \x20 9:EXCHANGE\n\
               \n\
               PLAN FRAGMENT 1(F1)\n\
               \x20 {sink}\n\
               \x20 OutPut Exchange Id: 9\n\
               \n\
               \x20 1:OlapScanNode\n\
               \n\
               PLAN FRAGMENT 2(F2)\n\
               \x20 OutPut Exchange Id: 9\n\
               \n\
               \x20 2:OlapScanNode\n"
        );
        let facts = normalize_explain_costs(&input, &ExplainLimits::default());

        assert_eq!(facts.state, ExplainStructureState::Partial, "{sink}");
        assert!(facts.verified_exchange_links.is_empty(), "{sink}");
        assert!(contains_limitation(&facts, code), "{sink}");
    }
}

#[test]
fn duplicate_fragment_without_an_exchange_still_blocks_its_prior_candidate() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F1)
  OutPut Exchange Id: 9

  1:OlapScanNode

PLAN FRAGMENT 1(F1)
  RESULT SINK

  2:Project
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.exchange_candidates.len(), 1);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(
        &facts,
        "exchange_source_fragment_conflicted"
    ));
}

#[test]
fn line_budget_tail_disables_all_unique_exchange_derivations() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F1)
  OutPut Exchange Id: 9

  1:OlapScanNode

PLAN FRAGMENT 2(F2)
  OutPut Exchange Id: 9

  2:OlapScanNode
";
    let limits = ExplainLimits {
        max_lines: 9,
        ..ExplainLimits::default()
    };
    let facts = normalize_explain_costs(input, &limits);

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.exchange_candidates.len(), 1);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(
        &facts,
        "exchange_identity_observation_incomplete"
    ));
}

#[test]
fn duplicate_exchange_sources_remain_unresolved() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  9:EXCHANGE

PLAN FRAGMENT 1(F1)
  OutPut Exchange Id: 9

  1:OlapScanNode

PLAN FRAGMENT 2(F2)
  OutPut Exchange Id: 9

  2:OlapScanNode
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.exchange_candidates.len(), 2);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(&facts, "exchange_identity_not_unique"));
}

#[test]
fn missing_exchange_target_remains_an_unresolved_candidate() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  4:Project

PLAN FRAGMENT 1(F1)
  OutPut Exchange Id: 9

  1:OlapScanNode
";
    let facts = normalize_explain_costs(input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.exchange_candidates.len(), 1);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(&facts, "exchange_identity_not_unique"));
}

/// `MultiCastDataSink.getVerboseExplain`: `MultiCastDataSinks:`, then one
/// `OutPut Partition` / `OutPut Exchange Id` pair per destination (the CTE
/// producer of c08_cte_multicast feeds two exchanges).
fn multicast_plan(destinations: &str) -> String {
    format!(
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  5:HASH JOIN\n  |  \n  |----4:EXCHANGE\n  |    \n  \
         2:EXCHANGE\n\nPLAN FRAGMENT 1(F1)\n  MultiCastDataSinks:\n{destinations}\n  1:OlapScanNode\n"
    )
}

#[test]
fn multicast_sink_links_its_root_to_every_destination_exchange() {
    let facts = normalize_explain_costs(
        &multicast_plan(
            "  OutPut Partition: RANDOM\n  OutPut Exchange Id: 02\n  OutPut Partition: RANDOM\n  OutPut Exchange Id: 04\n",
        ),
        &ExplainLimits::default(),
    );

    assert_eq!(facts.state, ExplainStructureState::Complete);
    assert!(facts.limitations.is_empty());
    assert_eq!(facts.fragments[1].sink_kind, ExplainSinkKind::Multicast);
    assert_eq!(
        facts
            .verified_exchange_links
            .iter()
            .map(|link| (link.source_node_ref.as_str(), link.target_node_ref.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("explain:F1:plan:1", "explain:F0:plan:2"),
            ("explain:F1:plan:1", "explain:F0:plan:4"),
        ]
    );
}

#[test]
fn multicast_without_distinct_destinations_is_partial_and_links_nothing() {
    for destinations in [
        "",
        "  OutPut Partition: RANDOM\n  OutPut Exchange Id: 02\n  OutPut Partition: RANDOM\n  OutPut Exchange Id: 02\n",
    ] {
        let facts =
            normalize_explain_costs(&multicast_plan(destinations), &ExplainLimits::default());

        assert_eq!(facts.state, ExplainStructureState::Partial);
        assert!(facts.exchange_candidates.is_empty());
        assert!(facts.verified_exchange_links.is_empty());
        assert!(contains_limitation(
            &facts,
            "multicast_destinations_not_proven"
        ));
    }
}

#[test]
fn multicast_after_a_data_stream_sink_conflicts() {
    let facts = normalize_explain_costs(
        "PLAN FRAGMENT 0(F0)\n  RESULT SINK\n\n  2:EXCHANGE\n\nPLAN FRAGMENT 1(F1)\n  \
         OutPut Exchange Id: 02\n  MultiCastDataSinks:\n\n  1:OlapScanNode\n",
        &ExplainLimits::default(),
    );

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(facts.verified_exchange_links.is_empty());
    assert!(contains_limitation(&facts, "conflicting_fragment_sink"));
}

#[test]
fn malformed_prefix_blocks_only_the_affected_fragment_structure() {
    let input = r"PLAN FRAGMENT 0(F0)
  RESULT SINK

  2:Project
";
    let input = format!("{input}  |  \n  |---1:OlapScanNode\n  |    \n  0:OlapScanNode\n");
    let facts = normalize_explain_costs(&input, &ExplainLimits::default());

    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert_eq!(facts.nodes.len(), 1);
    assert!(facts.plan_children.is_empty());
    assert!(contains_limitation(&facts, "malformed_tree_prefix"));
    assert!(contains_limitation(
        &facts,
        "node_after_untrusted_structure"
    ));
}

#[test]
fn every_frozen_budget_boundary_degrades_deterministically() {
    let defaults = ExplainLimits::default();
    assert_eq!(defaults.max_input_bytes, 16 * 1024 * 1024);
    assert_eq!(defaults.max_lines, 250_000);
    assert_eq!(defaults.max_line_bytes, 256 * 1024);
    assert_eq!(defaults.max_nodes, 20_000);
    assert_eq!(defaults.max_depth, 128);
    assert_eq!(defaults.max_relations, 40_000);
    assert_eq!(defaults.max_fact_candidates, 250_000);
    assert_eq!(defaults.max_fact_digits, 39);

    let tree = two_child_tree();
    let line_count = tree.lines().count();
    let max_line_bytes = tree.lines().map(str::len).max().unwrap();
    let mut limits = ExplainLimits {
        max_input_bytes: tree.len(),
        max_lines: line_count,
        max_line_bytes,
        max_nodes: 3,
        max_depth: 1,
        max_relations: 2,
        max_fact_candidates: 250_000,
        max_fact_digits: 39,
    };

    assert_eq!(
        normalize_explain_costs(tree, &limits).state,
        ExplainStructureState::Complete
    );

    limits.max_input_bytes -= 1;
    let facts = normalize_explain_costs(tree, &limits);
    assert_eq!(facts.state, ExplainStructureState::Unavailable);
    assert!(contains_limitation(&facts, "input_bytes_exceeded"));

    limits.max_input_bytes = tree.len();
    limits.max_lines -= 1;
    let facts = normalize_explain_costs(tree, &limits);
    assert_ne!(facts.state, ExplainStructureState::Complete);
    assert!(contains_limitation(&facts, "input_lines_exceeded"));

    limits.max_lines = line_count;
    limits.max_line_bytes -= 1;
    let facts = normalize_explain_costs(tree, &limits);
    assert_ne!(facts.state, ExplainStructureState::Complete);
    assert!(contains_limitation(&facts, "input_line_bytes_exceeded"));

    limits.max_line_bytes = max_line_bytes;
    limits.max_nodes = 2;
    let facts = normalize_explain_costs(tree, &limits);
    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(contains_limitation(&facts, "node_limit_exceeded"));

    limits.max_nodes = 3;
    let deep_tree = three_deep_child_zero_tree();
    limits.max_depth = 2;
    let facts = normalize_explain_costs(deep_tree, &limits);
    assert_eq!(facts.state, ExplainStructureState::Complete);

    limits.max_depth = 1;
    let facts = normalize_explain_costs(deep_tree, &limits);
    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(contains_limitation(&facts, "depth_limit_exceeded"));

    limits.max_depth = 1;
    limits.max_relations = 1;
    let facts = normalize_explain_costs(tree, &limits);
    assert_eq!(facts.state, ExplainStructureState::Partial);
    assert!(contains_limitation(&facts, "relation_limit_exceeded"));
}
