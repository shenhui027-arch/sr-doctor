use doctor_contracts::{
    AnalysisAttempt, AnalysisErrorAttempt, BoundedFidelity, BoundedInterval,
    COMPARISON_SCHEMA_VERSION, CanonicalNumber, Comparison, ComparisonReceipt, ComparisonState,
    Compatibility, DirectionSemantics, EdgeSource, ErrorSummary, FindingDelta, GraphEdge,
    GraphEntityKind, GraphNode, GraphRelation, GraphState, LosslessFidelity, MergeState,
    MetricCell, MetricCellState, MetricCellValue, NumberKind, ObservedMetricFidelity, PackMatch,
    PercentageState, ProfileContext, ProfileLevel, QueryState, ReceiptComponent, RecipeRef,
    SnapshotKind, SourceQuality, StructureDelta, SupportStatus, TopologySource, VersionContext,
    VersionSource,
};
use doctor_core::{
    analysis_key, comparison_key, facts_hash, graph_view, interval_contains_zero_by_closure,
    percentage_state_for_baseline, portable_facts, report_facts,
};
use std::collections::BTreeMap;

fn integer(value: &str) -> CanonicalNumber {
    CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.into(),
        scale: None,
    }
}

fn metric(
    state: MetricCellState,
    value: Option<&str>,
    bounds: Option<BoundedInterval>,
    display: &str,
    source: &str,
) -> MetricCell {
    let metric_value = match state {
        MetricCellState::Exact => MetricCellValue::Exact {
            canonical_number: integer(value.expect("exact metric value")),
            canonical_unit: "byte".into(),
            observation: match bounds {
                Some(bounds) => ObservedMetricFidelity::Bounded {
                    fidelity: BoundedFidelity::Interval,
                    bounds,
                },
                None => ObservedMetricFidelity::Lossless {
                    fidelity: LosslessFidelity::Exact,
                    bounds: (),
                },
            },
        },
        MetricCellState::Missing => MetricCellValue::Missing {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Invalid => MetricCellValue::Invalid {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Partial => MetricCellValue::Partial {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Overflow => MetricCellValue::Overflow {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Unavailable => MetricCellValue::Unavailable {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Incompatible => MetricCellValue::Incompatible {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        MetricCellState::Unknown => MetricCellValue::Unknown {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
    };
    MetricCell {
        metric_cell_ref: "metric-1".into(),
        metric_id: "QuerySpillBytes".into(),
        subject_ref: "operator-1".into(),
        display_text: display.into(),
        order_rank: Some(1),
        source_refs: vec![source.into()],
        reasons: vec![],
        match_key: None,
        value: metric_value,
    }
}

fn profile_context() -> ProfileContext {
    ProfileContext {
        profile_level: ProfileLevel::Level1,
        query_state: QueryState::Finished,
        snapshot_kind: SnapshotKind::Final,
        merge_state: MergeState::Merged,
        evidence_refs: vec!["profile-context".into()],
        limitations: vec![],
    }
}

fn version_context() -> VersionContext {
    VersionContext {
        reported_version: None,
        user_declared_version: None,
        effective_version: Some("3.5.14".into()),
        version_source: VersionSource::Detected,
        selected_pack: None,
        pack_match: PackMatch::ExactPack,
        compatibility: Compatibility::Partial,
        support_status: SupportStatus::ParseOnly,
        detection_evidence: vec![],
        compatibility_checks: vec![],
        issues: vec![],
    }
}

fn graph(display: &str, source: &str) -> doctor_contracts::GraphView {
    graph_view(
        GraphState::Partial,
        TopologySource::EmbeddedPlan,
        vec!["root".into()],
        vec![GraphNode {
            sql_location: None,
            sql_facts: None,
            node_id: "root".into(),
            entity_kind: GraphEntityKind::PlanNode,
            plan_node_id: Some("0".into()),
            fragment_id: None,
            pipeline_id: None,
            plan_node_type: Some("SCAN".into()),
            display_name: Some(display.into()),
            source_ordinal: Some(0),
            native_operator_refs: vec![],
            subordinate_operator_refs: vec![],
            metric_observation_refs: vec![],
            execution_state: None,
            source_quality: SourceQuality::Partial,
            source_refs: vec![source.into()],
        }],
        vec![GraphEdge {
            edge_id: "edge-0".into(),
            from: "root".into(),
            to: "sink".into(),
            relation: GraphRelation::PlanChild,
            direction_semantics: DirectionSemantics::ParentToChild,
            source: EdgeSource::Explicit,
            quality: SourceQuality::Exact,
            source_ordinal: Some(0),
            input_role: None,
            derivation_recipe: None,
            restates_edge_id: None,
            evidence_refs: vec![source.into()],
        }],
        vec![],
    )
    .unwrap()
}

#[test]
fn json_and_text_portable_facts_hashes_ignore_display_and_provenance() {
    let receipt = vec![ReceiptComponent {
        id: "pack".into(),
        version: "0.1.0".into(),
        manifest_hash: "manifest".into(),
    }];
    let mut json_context = profile_context();
    json_context.evidence_refs = vec!["json:/profile/context".into()];
    json_context.limitations = vec!["json-source-note".into()];
    let mut text_context = profile_context();
    text_context.evidence_refs = vec!["text:1".into()];
    text_context.limitations = vec!["text-source-note".into()];
    let json = portable_facts(
        &version_context(),
        json_context,
        receipt.clone(),
        &[metric(
            MetricCellState::Exact,
            Some("1024"),
            None,
            "1KB",
            "json:/metric",
        )],
        &graph("JSON scan", "json:/graph"),
        &[],
        &[],
    );
    let text = portable_facts(
        &version_context(),
        text_context,
        receipt,
        &[metric(
            MetricCellState::Exact,
            Some("1024"),
            None,
            "1.000 KB",
            "text:2",
        )],
        &graph("Text scan", "text:3"),
        &[],
        &[],
    );
    assert_eq!(facts_hash(&json), facts_hash(&text));
}

#[test]
fn portable_facts_include_graph_semantics_but_not_graph_labels() {
    let base = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &graph("scan label A", "json:/graph"),
        &[],
        &[],
    );
    let relabeled = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &graph("scan label B", "text:3"),
        &[],
        &[],
    );
    assert_eq!(facts_hash(&base), facts_hash(&relabeled));

    let mut changed_graph = graph("scan label A", "json:/graph");
    changed_graph.nodes[0].plan_node_type = Some("HASH_JOIN".into());
    let changed = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &changed_graph,
        &[],
        &[],
    );
    assert_ne!(facts_hash(&base), facts_hash(&changed));
}

#[test]
fn portable_facts_keep_metric_subject_ownership() {
    let mut first = metric(MetricCellState::Exact, Some("1"), None, "1B", "json:/a");
    first.metric_id = "A".into();
    first.subject_ref = "operator-left".into();
    let mut second = metric(MetricCellState::Exact, Some("2"), None, "2B", "json:/b");
    second.metric_id = "B".into();
    second.subject_ref = "operator-right".into();
    let receipt = vec![];
    let original = portable_facts(
        &version_context(),
        profile_context(),
        receipt.clone(),
        &[first.clone(), second.clone()],
        &graph("scan", "json:/graph"),
        &[],
        &[],
    );
    first.subject_ref = "operator-right".into();
    second.subject_ref = "operator-left".into();
    let swapped = portable_facts(
        &version_context(),
        profile_context(),
        receipt,
        &[first, second],
        &graph("scan", "text:3"),
        &[],
        &[],
    );
    assert_ne!(facts_hash(&original), facts_hash(&swapped));
}

#[test]
fn portable_facts_include_version_compatibility_state() {
    let compatible = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &graph("scan", "json:/graph"),
        &[],
        &[],
    );
    let mut incompatible_version = version_context();
    incompatible_version.compatibility = Compatibility::Incompatible;
    incompatible_version.support_status = SupportStatus::Unsupported;
    let incompatible = portable_facts(
        &incompatible_version,
        profile_context(),
        vec![],
        &[],
        &graph("scan", "text:3"),
        &[],
        &[],
    );
    assert_ne!(facts_hash(&compatible), facts_hash(&incompatible));
}

#[test]
fn source_specific_analysis_keys_do_not_collapse_like_portable_facts() {
    let receipt = doctor_contracts::AnalysisReceipt { components: vec![] };
    let options = BTreeMap::from([("version_hint".into(), "auto".into())]);
    assert_ne!(
        analysis_key("json-source-hash", &receipt, &options),
        analysis_key("text-source-hash", &receipt, &options)
    );
}

#[test]
fn checked_in_json_and_text_reports_have_distinct_replay_keys_and_equal_facts() {
    let json: doctor_contracts::AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    let text: doctor_contracts::AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal_text.report.json"
    ))
    .unwrap();
    let options = BTreeMap::from([
        ("requested_kind".into(), "auto".into()),
        ("version_hint".into(), "auto".into()),
    ]);
    assert_ne!(json.input.source_hash, text.input.source_hash);
    assert_ne!(json.analysis_key, text.analysis_key);
    assert_eq!(
        json.analysis_key,
        analysis_key(&json.input.source_hash, &json.analysis_receipt, &options)
    );
    assert_eq!(
        text.analysis_key,
        analysis_key(&text.input.source_hash, &text.analysis_receipt, &options)
    );
    let json_facts = facts_hash(&report_facts(&json));
    let text_facts = facts_hash(&report_facts(&text));
    assert_eq!(json_facts, text_facts);
    assert_eq!(json.facts_hash, json_facts);
    assert_eq!(text.facts_hash, text_facts);
}

#[test]
fn graph_keeps_source_order_and_rejects_unavailable_edges() {
    let edges = vec![
        GraphEdge {
            edge_id: "second-id".into(),
            from: "a".into(),
            to: "b".into(),
            relation: GraphRelation::PlanChild,
            direction_semantics: DirectionSemantics::ParentToChild,
            source: EdgeSource::Explicit,
            quality: SourceQuality::Exact,
            source_ordinal: Some(0),
            input_role: None,
            derivation_recipe: None,
            restates_edge_id: None,
            evidence_refs: vec![],
        },
        GraphEdge {
            edge_id: "first-id".into(),
            from: "b".into(),
            to: "c".into(),
            relation: GraphRelation::PlanChild,
            direction_semantics: DirectionSemantics::ParentToChild,
            source: EdgeSource::Explicit,
            quality: SourceQuality::Exact,
            source_ordinal: Some(1),
            input_role: None,
            derivation_recipe: None,
            restates_edge_id: None,
            evidence_refs: vec![],
        },
    ];
    let graph = graph_view(
        GraphState::Partial,
        TopologySource::EmbeddedPlan,
        vec![],
        vec![],
        edges.clone(),
        vec![],
    )
    .unwrap();
    assert_eq!(
        graph
            .edges
            .iter()
            .map(|edge| edge.edge_id.as_str())
            .collect::<Vec<_>>(),
        ["second-id", "first-id"]
    );
    assert!(
        graph_view(
            GraphState::Unavailable,
            TopologySource::None,
            vec![],
            vec![],
            edges,
            vec![]
        )
        .is_err()
    );

    let derived = GraphEdge {
        edge_id: "derived".into(),
        from: "exchange".into(),
        to: "fragment".into(),
        relation: GraphRelation::ExchangeLink,
        direction_semantics: DirectionSemantics::UpstreamToDownstream,
        source: EdgeSource::VerifiedDerived,
        quality: SourceQuality::Exact,
        source_ordinal: None,
        input_role: None,
        derivation_recipe: Some(RecipeRef {
            id: "test.structural-recipe".into(),
            version: "1".into(),
        }),
        restates_edge_id: None,
        evidence_refs: vec!["json:/fragments".into()],
    };
    let derived_graph = graph_view(
        GraphState::Partial,
        TopologySource::VerifiedProfileRelation,
        vec![],
        vec![],
        vec![derived],
        vec![],
    )
    .unwrap();
    assert_eq!(derived_graph.edges[0].source, EdgeSource::VerifiedDerived);
    assert_eq!(
        derived_graph.edges[0]
            .derivation_recipe
            .as_ref()
            .unwrap()
            .version,
        "1"
    );

    let missing_recipe = GraphEdge {
        edge_id: "derived-without-recipe".into(),
        from: "exchange".into(),
        to: "fragment".into(),
        relation: GraphRelation::ExchangeLink,
        direction_semantics: DirectionSemantics::UpstreamToDownstream,
        source: EdgeSource::VerifiedDerived,
        quality: SourceQuality::Exact,
        source_ordinal: None,
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec!["json:/fragments".into()],
    };
    assert!(
        graph_view(
            GraphState::Partial,
            TopologySource::VerifiedProfileRelation,
            vec![],
            vec![],
            vec![missing_recipe],
            vec![]
        )
        .is_err()
    );
}

#[test]
fn percentage_states_distinguish_exact_zero_closure_zero_and_exact_nonzero() {
    assert_eq!(
        percentage_state_for_baseline(&metric(MetricCellState::Exact, Some("0"), None, "0B", "s")),
        PercentageState::UndefinedBaselineZero
    );
    let closure_zero = BoundedInterval {
        lower: integer("0"),
        upper: integer("5"),
        lower_inclusive: false,
        upper_inclusive: true,
    };
    assert!(interval_contains_zero_by_closure(&closure_zero));
    assert_eq!(
        percentage_state_for_baseline(&metric(
            MetricCellState::Exact,
            Some("1"),
            Some(closure_zero),
            "1B",
            "s"
        )),
        PercentageState::UndefinedBaselineIntervalContainsZero
    );
    assert_eq!(
        percentage_state_for_baseline(&metric(MetricCellState::Exact, Some("1"), None, "1B", "s")),
        PercentageState::Computed
    );
    let strictly_positive = BoundedInterval {
        lower: integer("1"),
        upper: integer("5"),
        lower_inclusive: true,
        upper_inclusive: false,
    };
    assert_eq!(
        percentage_state_for_baseline(&metric(
            MetricCellState::Exact,
            Some("1"),
            Some(strictly_positive),
            "1B",
            "s"
        )),
        PercentageState::Approximate
    );
    assert_eq!(
        percentage_state_for_baseline(&metric(MetricCellState::Unavailable, None, None, "—", "s")),
        PercentageState::Missing
    );
    assert_eq!(
        percentage_state_for_baseline(&metric(MetricCellState::Incompatible, None, None, "—", "s")),
        PercentageState::SemanticMismatch
    );
    let malformed = BoundedInterval {
        lower: integer("not-a-number"),
        upper: integer("5"),
        lower_inclusive: true,
        upper_inclusive: true,
    };
    assert!(interval_contains_zero_by_closure(&malformed));
}

fn comparison(key: &str) -> Comparison {
    let failed = AnalysisAttempt::Error(AnalysisErrorAttempt {
        error: ErrorSummary {
            code: "parse_failed".into(),
            stage: "parse".into(),
            message_id: "parse_failed".into(),
            recoverability: "change_input".into(),
            affected_side: None,
        },
    });
    Comparison {
        schema_version: COMPARISON_SCHEMA_VERSION.into(),
        comparison_receipt: ComparisonReceipt { components: vec![] },
        comparison_key: key.into(),
        baseline: failed.clone(),
        candidate: failed,
        state: ComparisonState::NotComparable,
        reasons: vec!["failed".into()],
        compatibility_delta: vec![],
        structure_delta: StructureDelta {
            state: "not_comparable".into(),
            matches: vec![],
            added: vec![],
            removed: vec![],
            changed: vec![],
            edge_delta: vec![],
            unresolved: vec![],
        },
        metric_deltas: vec![],
        finding_delta: FindingDelta {
            added: vec![],
            resolved: vec![],
            persisting: vec![],
            changed: vec![],
            incomparable: vec![],
            algorithm_changed: vec![],
            partial_evidence: vec![],
            measurement_changes: Vec::new(),
        },
        diagnostics: vec![],
        limitations: vec![],
    }
}

#[test]
fn comparison_key_excludes_its_own_field() {
    assert_eq!(
        comparison_key(&comparison("old-key")),
        comparison_key(&comparison("new-key"))
    );
}

#[test]
fn metric_fixture_freezes_strict_units_and_overflow_strings() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_metric_contract_cases.json"
    ))
    .unwrap();
    let cases = cases.as_array().unwrap();
    for id in [
        "duration_micro_greek_rejected",
        "duration_micro_sign_rejected",
        "bytes_1024",
        "bytes_iec_rejected",
        "duration_overflow",
        "intermediate_overflow",
        "exact_zero",
    ] {
        assert!(
            cases.iter().any(|case| case["case_id"] == id),
            "missing {id}"
        );
    }
    assert_eq!(
        cases
            .iter()
            .find(|case| case["case_id"] == "bytes_1024")
            .unwrap()["canonical_number"],
        "1024"
    );
    assert_eq!(
        cases
            .iter()
            .find(|case| case["case_id"] == "duration_overflow")
            .unwrap()["error_code"],
        "numeric_overflow"
    );
}

#[test]
fn lossy_text_interval_contains_the_exact_json_counterpart() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_metric_contract_cases.json"
    ))
    .unwrap();
    let cases = cases.as_array().unwrap();
    let json = cases
        .iter()
        .find(|case| case["case_id"] == "lossy_pair_json_exact")
        .unwrap();
    let text = cases
        .iter()
        .find(|case| case["case_id"] == "lossy_pair_text_interval")
        .unwrap();
    assert_eq!(json["pair_id"], text["pair_id"]);
    let exact: i128 = json["canonical_number"].as_str().unwrap().parse().unwrap();
    let lower: i128 = text["bounds"]["lower"].as_str().unwrap().parse().unwrap();
    let upper: i128 = text["bounds"]["upper"].as_str().unwrap().parse().unwrap();
    assert!(exact >= lower && exact < upper);
}

#[test]
fn task01_case_tables_freeze_rules_compare_versions_and_ordering() {
    let rules: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_rule_contract_cases.json"
    ))
    .unwrap();
    let rules = rules.as_array().unwrap();
    for rule_id in [
        "SR3514.SCAN_TIME_SHARE",
        "SR3514.JOIN_TIME_SHARE",
        "SR3514.AGGREGATE_TIME_SHARE",
        "SR3514.EXCHANGE_NETWORK_TIME_SHARE",
        "SR3514.SPILL_OBSERVED",
        "SR3514.ROW_FLOW_AMPLIFICATION",
        "SR3514.NODE_TIME_SHARE",
        "SR3514.OPERATOR_TIME_SKEW",
        "SR3514.JOIN_PROBE_FANOUT",
        "SR3514.BROADCAST_EXCHANGE_BYTES",
        "SR3514.STREAMING_PREAGG_INEFFECTIVE",
    ] {
        let classes = rules
            .iter()
            .filter(|case| case["rule_id"] == rule_id)
            .map(|case| case["case_class"].as_str().unwrap())
            .collect::<Vec<_>>();
        for class in [
            "positive",
            "negative",
            "boundary",
            "missing",
            "version",
            "graph_degraded",
        ] {
            assert!(classes.contains(&class), "{rule_id} lacks {class}");
        }
        assert!(
            rules
                .iter()
                .filter(|case| case["rule_id"] == rule_id)
                .all(|case| matches!(
                    case["evidence_completeness"].as_str(),
                    Some("complete" | "partial" | "unavailable")
                ))
        );
    }
    assert_eq!(rules.len(), 66);
    assert_eq!(
        rules
            .iter()
            .find(|case| case["case_id"] == "scan_time_boundary")
            .unwrap()["status"],
        "not_triggered"
    );
    assert_eq!(
        rules
            .iter()
            .find(|case| case["case_id"] == "spill_boundary")
            .unwrap()["status"],
        "suspected"
    );
    assert_eq!(
        rules
            .iter()
            .find(|case| case["case_id"] == "row_flow_boundary")
            .unwrap()["status"],
        "not_triggered"
    );

    let comparisons: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_compare_contract_cases.json"
    ))
    .unwrap();
    let comparisons = comparisons.as_array().unwrap();
    assert_eq!(
        comparisons
            .iter()
            .find(|case| case["case_id"] == "compare_zero_baseline")
            .unwrap()["expected"]["metric_deltas"][0]["percentage_state"],
        "undefined_baseline_zero"
    );
    assert_eq!(
        comparisons
            .iter()
            .find(|case| case["case_id"] == "compare_interval_baseline_contains_zero")
            .unwrap()["expected"]["metric_deltas"][0]["percentage_state"],
        "undefined_baseline_interval_contains_zero"
    );

    let versions: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_version_profile_contract_cases.json"
    ))
    .unwrap();
    let versions = versions.as_array().unwrap();
    assert_eq!(
        versions
            .iter()
            .find(|case| case["case_id"] == "version_user_declared_3514")
            .unwrap()["support_status"],
        "parse_only"
    );
    assert_eq!(
        versions
            .iter()
            .find(|case| case["case_id"] == "version_unknown")
            .unwrap()["pack_match"],
        "no_pack"
    );

    let order: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_order_recipe_contract_cases.json"
    ))
    .unwrap();
    let join = order
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["case_id"] == "ordered_join_children")
        .unwrap();
    assert_eq!(join["children"][0]["ref"], "child-z");
    assert_eq!(join["children"][1]["ref"], "child-a");
}
