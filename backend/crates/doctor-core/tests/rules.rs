use doctor_contracts::{
    AnalysisReceipt, BoundedInterval, CanonicalNumber, Compatibility, DirectionSemantics,
    EdgeSource, Fidelity, FindingConfidence, GraphEdge, GraphEntityKind, GraphNode, GraphRelation,
    GraphState, MergeState, NumberKind, PackMatch, ParseState, ProfileContext, ProfileLevel,
    QueryState, RecipeRef, RequestedSourceKind, RuntimeSourceRole, SelectedVersionPack,
    SnapshotKind, SourceQuality, SupportStatus, VersionContext, VersionSource,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, MetricObservation, OperatorGroup,
    OperatorInfoFact, PhysicalOperatorMembership, RuleEvaluationError, adapt_input, evaluate_rules,
    v01_analysis_receipt,
};
use doctor_rules_v0_1::PRODUCT_RULESET_VERSION;
use sha2::{Digest, Sha256};
use starrocks_version_pack_3_5_14::{PRODUCT_PACK_VERSION, SEMANTIC_MANIFEST_HASH};

/// Time-share rules read the execution summary, which reads the runtime
/// inventory (plan nodes and physical operator memberships). The synthetic
/// cases describe operators through their operator groups; this derives the
/// inventory from them once: one plan node per group plan node id, one
/// membership per native/subordinate operator ref (natives first, in group
/// order), and each operator observation bound by `runtime_occurrence_ref`.
fn synthesize_runtime_inventory(output: &mut doctor_core::AdapterOutput) {
    if !output.ir.physical_operator_memberships.is_empty() {
        return;
    }
    let mut ordinal = 0;
    let mut seen = std::collections::BTreeSet::new();
    for group in output.ir.operator_groups.clone() {
        let Some(plan_node_id) = group.plan_node_id.clone() else {
            continue;
        };
        if group.native_operator_refs.is_empty() && group.subordinate_operator_refs.is_empty() {
            continue;
        }
        if !output
            .ir
            .graph
            .nodes
            .iter()
            .any(|node| node.plan_node_id.as_deref() == Some(plan_node_id.as_str()))
        {
            output.ir.graph.nodes.push(GraphNode {
                sql_location: None,
                sql_facts: None,
                node_id: format!("plan:{plan_node_id}"),
                entity_kind: GraphEntityKind::PlanNode,
                plan_node_id: Some(plan_node_id.clone()),
                fragment_id: None,
                pipeline_id: None,
                plan_node_type: Some(group.canonical_type.clone()),
                display_name: None,
                source_ordinal: None,
                native_operator_refs: vec![],
                subordinate_operator_refs: vec![],
                metric_observation_refs: vec![],
                execution_state: None,
                source_quality: SourceQuality::Exact,
                source_refs: group.source_refs.clone(),
            });
        }
        let members = group
            .native_operator_refs
            .iter()
            .map(|(reference, _)| (reference, RuntimeSourceRole::Native))
            .chain(
                group
                    .subordinate_operator_refs
                    .iter()
                    .map(|(reference, _)| (reference, RuntimeSourceRole::Subordinate)),
            );
        for (reference, role) in members {
            if !seen.insert(reference.clone()) {
                continue;
            }
            output
                .ir
                .physical_operator_memberships
                .push(PhysicalOperatorMembership {
                    physical_operator_ref: reference.clone(),
                    source_occurrence_ordinal: ordinal,
                    fragment_ref: group.fragment_ref.clone(),
                    container_ref: None,
                    driver_occurrence_ref: None,
                    driver_occurrence_ordinal: None,
                    emission_ordinal: None,
                    plan_node_id: Some(plan_node_id.clone()),
                    canonical_type: group.canonical_type.clone(),
                    source_role: role,
                    source_refs: group.source_refs.clone(),
                    reasons: vec![],
                });
            ordinal += 1;
            for observation in &mut output.ir.metric_observations {
                if observation.subject_ref.as_deref() == Some(group.operator_group_id.as_str())
                    && observation.operator_ref.as_deref() == Some(reference.as_str())
                {
                    observation.runtime_occurrence_ref = Some(reference.clone());
                }
            }
        }
    }
}

fn evaluate(output: &mut doctor_core::AdapterOutput) -> Result<(), RuleEvaluationError> {
    synthesize_runtime_inventory(output);
    evaluate_rules(output)
}

const RULESET_HASH: &str = "84667b9b849fbda0a82933cf908b9884429cb85cee2c69cb7cd0c4fd91374784";

fn receipt() -> AnalysisReceipt {
    v01_analysis_receipt()
}

fn base_output() -> doctor_core::AdapterOutput {
    let context = AdapterContext::new(receipt()).expect("valid receipt");
    let mut output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            br#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#.to_vec(),
        ),
        &AdapterLimits::default(),
        &context,
    )
    .expect("adapter output");
    let source = output
        .source_refs
        .last()
        .expect("root source")
        .ref_id
        .clone();
    output.ir.version = VersionContext {
        reported_version: None,
        user_declared_version: None,
        effective_version: Some("3.5.14".into()),
        version_source: VersionSource::Detected,
        selected_pack: Some(SelectedVersionPack {
            pack_id: "starrocks-3.5.14".into(),
            product_pack_version: PRODUCT_PACK_VERSION.into(),
            upstream_release: "3.5.14".into(),
            upstream_commit: "23a56ec517db763ea9bc7ee50b19357acdee0303".into(),
            manifest_hash: SEMANTIC_MANIFEST_HASH.into(),
            release_deltas: None,
        }),
        pack_match: PackMatch::ExactPack,
        compatibility: Compatibility::Compatible,
        support_status: SupportStatus::Supported,
        detection_evidence: vec![],
        compatibility_checks: vec![],
        issues: vec![],
    };
    output.ir.profile_context = ProfileContext {
        profile_level: ProfileLevel::Level1,
        query_state: QueryState::Finished,
        snapshot_kind: SnapshotKind::Final,
        merge_state: MergeState::Merged,
        evidence_refs: vec![source.clone()],
        limitations: vec![],
    };
    output.ir.graph.state = GraphState::Partial;
    output.ir.graph.source_refs = vec![source];
    output.ir.metric_observations.clear();
    output.ir.operator_groups.clear();
    output.ir.findings.clear();
    output.ir.rule_evaluations.clear();
    output
}

fn integer(value: i128) -> CanonicalNumber {
    CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.to_string(),
        scale: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn observation(
    id: &str,
    metric_id: &str,
    subject_ref: Option<&str>,
    operator_ref: Option<&str>,
    ordinal: Option<u32>,
    scope: &str,
    role: &str,
    state: ParseState,
    value: Option<i128>,
    bounds: Option<(i128, i128)>,
    source: &str,
) -> MetricObservation {
    let (unit, fidelity, bounds) = match metric_id {
        "QuerySpillBytes" | "BytesSent" => (
            "byte",
            if bounds.is_some() {
                Fidelity::Interval
            } else {
                Fidelity::Exact
            },
            bounds.map(interval),
        ),
        "OutputRows" | "PushRowNum" | "PullRowNum" => ("item", Fidelity::Exact, None),
        _ => (
            "ns",
            if bounds.is_some() {
                Fidelity::Interval
            } else {
                Fidelity::Exact
            },
            bounds.map(interval),
        ),
    };
    MetricObservation {
        observation_id: id.into(),
        metric_id: Some(metric_id.into()),
        source_name: metric_id.into(),
        subject_ref: subject_ref.map(str::to_owned),
        operator_ref: operator_ref.map(str::to_owned),
        runtime_occurrence_ref: None,
        match_key: None,
        dimension: if matches!(metric_id, "QuerySpillBytes" | "BytesSent") {
            "bytes"
        } else if matches!(metric_id, "OutputRows" | "PushRowNum" | "PullRowNum") {
            "count"
        } else {
            "duration"
        }
        .into(),
        parse_state: state,
        canonical_number: value.map(integer),
        canonical_unit: value.map(|_| unit.into()),
        raw_text: None,
        source_refs: vec![source.into()],
        consumed_ranges: vec![],
        unconsumed_ranges: vec![],
        fidelity,
        bounds,
        semantic_state: "known".into(),
        definition_ref: Some(if metric_id == "OutputRows" {
            "sr3514.output_rows.general@1".into()
        } else {
            format!("sr3514-metric-registry:{metric_id}")
        }),
        scope: scope.into(),
        value_role: role.into(),
        strategy_origin: "registry".into(),
        counter_strategy: None,
        source_merge_state: "merged".into(),
        display_value: value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".into()),
        source_ordinal: ordinal,
        diagnostics: vec![],
    }
}

fn interval((lower, upper): (i128, i128)) -> BoundedInterval {
    BoundedInterval {
        lower: integer(lower),
        upper: integer(upper),
        lower_inclusive: true,
        upper_inclusive: true,
    }
}

fn group(
    kind: &str,
    native: Vec<(&str, u32)>,
    subordinate: Vec<(&str, u32)>,
    metric_refs: Vec<&str>,
    source: &str,
) -> OperatorGroup {
    OperatorGroup {
        operator_group_id: "operator-1".into(),
        plan_node_id: Some("1".into()),
        fragment_ref: None,
        pipeline_ref: None,
        native_operator_refs: native
            .into_iter()
            .map(|(reference, ordinal)| (reference.into(), ordinal))
            .collect(),
        subordinate_operator_refs: subordinate
            .into_iter()
            .map(|(reference, ordinal)| (reference.into(), ordinal))
            .collect(),
        canonical_type: kind.into(),
        raw_labels: vec![],
        metric_observation_refs: metric_refs.into_iter().map(str::to_owned).collect(),
        source_refs: vec![source.into()],
    }
}

fn named_group(
    id: &str,
    plan_node_id: &str,
    kind: &str,
    metric_refs: Vec<&str>,
    source: &str,
) -> OperatorGroup {
    OperatorGroup {
        operator_group_id: id.into(),
        plan_node_id: Some(plan_node_id.into()),
        fragment_ref: None,
        pipeline_ref: None,
        native_operator_refs: vec![],
        subordinate_operator_refs: vec![],
        canonical_type: kind.into(),
        raw_labels: vec![],
        metric_observation_refs: metric_refs.into_iter().map(str::to_owned).collect(),
        source_refs: vec![source.into()],
    }
}

fn time_output(
    numerator: Option<(i128, Option<(i128, i128)>)>,
    denominator: Option<(i128, Option<(i128, i128)>)>,
) -> doctor_core::AdapterOutput {
    time_output_for("AGGREGATE", numerator, denominator)
}

/// Share-semantics tests and the 36 frozen goldens express times in seconds so
/// their ratio and boundary cases stay above the ruleset 0.1.5 absolute floor.
/// The floor itself is covered by dedicated tests below.
const SHARE_UNIT_NS: i128 = 1_000_000_000;

fn seconds(value: Option<(i128, Option<(i128, i128)>)>) -> Option<(i128, Option<(i128, i128)>)> {
    value.map(|(value, bounds)| {
        (
            value * SHARE_UNIT_NS,
            bounds.map(|(lower, upper)| (lower * SHARE_UNIT_NS, upper * SHARE_UNIT_NS)),
        )
    })
}

fn share_output_for(
    kind: &str,
    numerator: Option<(i128, Option<(i128, i128)>)>,
    denominator: Option<(i128, Option<(i128, i128)>)>,
) -> doctor_core::AdapterOutput {
    time_output_for(kind, seconds(numerator), seconds(denominator))
}

fn share_output(
    numerator: Option<(i128, Option<(i128, i128)>)>,
    denominator: Option<(i128, Option<(i128, i128)>)>,
) -> doctor_core::AdapterOutput {
    share_output_for("AGGREGATE", numerator, denominator)
}

fn time_output_for(
    kind: &str,
    numerator: Option<(i128, Option<(i128, i128)>)>,
    denominator: Option<(i128, Option<(i128, i128)>)>,
) -> doctor_core::AdapterOutput {
    let mut output = base_output();
    let source = output.source_refs.last().unwrap().ref_id.clone();
    let mut refs = vec![];
    if let Some((value, bounds)) = numerator {
        output.ir.metric_observations.push(observation(
            "obs:op",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(value),
            bounds,
            &source,
        ));
        refs.push("obs:op");
        let special = match kind {
            "SCAN" => Some("ScanTime"),
            "EXCHANGE" => Some("NetworkTime"),
            _ => None,
        };
        if let Some(metric_id) = special {
            output.ir.metric_observations.push(observation(
                "obs:special",
                metric_id,
                Some("operator-1"),
                Some("native-1"),
                Some(0),
                "operator",
                "reported_base",
                ParseState::Exact,
                Some(0),
                None,
                &source,
            ));
            refs.push("obs:special");
        }
    }
    if let Some((value, bounds)) = denominator {
        output.ir.metric_observations.push(observation(
            "obs:query",
            "QueryCumulativeOperatorTime",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(value),
            bounds,
            &source,
        ));
    }
    output.ir.operator_groups.push(group(
        match kind {
            "JOIN" => "HASH_JOIN",
            "SCAN" => "OLAP_SCAN",
            other => other,
        },
        vec![("native-1", 0)],
        vec![],
        refs,
        &source,
    ));
    output
}

fn spill_output(
    state: ParseState,
    value: Option<i128>,
    bounds: Option<(i128, i128)>,
) -> doctor_core::AdapterOutput {
    let mut output = base_output();
    let source = output.source_refs.last().unwrap().ref_id.clone();
    output.ir.metric_observations.push(observation(
        "obs:spill",
        "QuerySpillBytes",
        None,
        None,
        None,
        "query",
        "reported_base",
        state,
        value,
        bounds,
        &source,
    ));
    output
}

fn row_output(
    parent: Option<i128>,
    child: Option<i128>,
    state: GraphState,
) -> doctor_core::AdapterOutput {
    let mut output = base_output();
    let source = output.source_refs.last().unwrap().ref_id.clone();
    output.ir.graph.state = state;
    output.ir.graph.nodes = vec![
        GraphNode {
            sql_location: None,
            sql_facts: None,
            node_id: "plan:1".into(),
            entity_kind: GraphEntityKind::PlanNode,
            plan_node_id: Some("1".into()),
            fragment_id: None,
            pipeline_id: None,
            plan_node_type: Some("HASH_JOIN".into()),
            display_name: None,
            source_ordinal: None,
            native_operator_refs: vec![],
            subordinate_operator_refs: vec![],
            metric_observation_refs: vec!["obs:parent".into()],
            execution_state: None,
            source_quality: SourceQuality::Exact,
            source_refs: vec![source.clone()],
        },
        GraphNode {
            sql_location: None,
            sql_facts: None,
            node_id: "plan:2".into(),
            entity_kind: GraphEntityKind::PlanNode,
            plan_node_id: Some("2".into()),
            fragment_id: None,
            pipeline_id: None,
            plan_node_type: Some("SCAN".into()),
            display_name: None,
            source_ordinal: None,
            native_operator_refs: vec![],
            subordinate_operator_refs: vec![],
            metric_observation_refs: vec!["obs:child".into()],
            execution_state: None,
            source_quality: SourceQuality::Exact,
            source_refs: vec![source.clone()],
        },
    ];
    output.ir.graph.edges = vec![GraphEdge {
        edge_id: "edge:1".into(),
        from: "plan:1".into(),
        to: "plan:2".into(),
        relation: GraphRelation::PlanChild,
        direction_semantics: DirectionSemantics::ParentToChild,
        source: EdgeSource::Explicit,
        quality: SourceQuality::Exact,
        source_ordinal: None,
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec![source.clone()],
    }];
    output.ir.operator_groups = vec![
        named_group("operator-1", "1", "HASH_JOIN", vec!["obs:parent"], &source),
        named_group("operator-2", "2", "SCAN", vec!["obs:child"], &source),
    ];
    // One runtime operator per plan node, so the execution summary reads the
    // nodes' OutputRows.
    output.ir.operator_groups[0].native_operator_refs = vec![("join-1".into(), 0)];
    output.ir.operator_groups[1].native_operator_refs = vec![("scan-1".into(), 1)];
    for (id, subject, value) in [
        ("obs:parent", "operator-1", parent),
        ("obs:child", "operator-2", child),
    ] {
        let mut metric = observation(
            id,
            "OutputRows",
            Some(subject),
            None,
            None,
            "operator",
            "derived",
            ParseState::Exact,
            value,
            None,
            &source,
        );
        metric.strategy_origin = "version_pack".into();
        output.ir.metric_observations.push(metric);
    }
    output
}

fn status_name(status: &doctor_contracts::RuleEvaluationStatus) -> &'static str {
    match status {
        doctor_contracts::RuleEvaluationStatus::Triggered => "triggered",
        doctor_contracts::RuleEvaluationStatus::Suspected => "suspected",
        doctor_contracts::RuleEvaluationStatus::NotTriggered => "not_triggered",
        doctor_contracts::RuleEvaluationStatus::NotApplicable => "not_applicable",
        doctor_contracts::RuleEvaluationStatus::Suppressed => "suppressed",
    }
}

fn evaluation<'a>(
    output: &'a doctor_core::AdapterOutput,
    rule_id: &str,
) -> &'a doctor_contracts::RuleEvaluation {
    output
        .ir
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == rule_id)
        .expect("rule evaluation")
}

#[test]
fn all_66_rule_goldens_are_evaluated_field_by_field() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_rule_contract_cases.json"
    ))
    .expect("golden cases");
    let mut blessed = vec![];
    for case in cases.as_array().expect("array") {
        let id = case["case_id"].as_str().expect("case id");
        let rule_id = case["rule_id"].as_str().expect("rule id");
        let mut output = match id {
            id if id.starts_with("scan_time_") => time_case_output("SCAN", id),
            id if id.starts_with("join_time_") => time_case_output("JOIN", id),
            id if id.starts_with("aggregate_time_") => time_case_output("AGGREGATE", id),
            id if id.starts_with("exchange_time_") => time_case_output("EXCHANGE", id),
            "spill_positive" => spill_output(ParseState::Exact, Some(1), None),
            "spill_negative" | "spill_boundary" => spill_output(
                ParseState::Exact,
                Some(0),
                if id == "spill_boundary" {
                    Some((0, 2))
                } else {
                    None
                },
            ),
            "spill_missing" => spill_output(ParseState::Missing, None, None),
            "spill_version" => unsupported(spill_output(ParseState::Exact, Some(1), None)),
            "spill_graph_degraded" => operator_spill_graph_degraded(),
            "row_flow_positive" => row_output(Some(101), Some(10), GraphState::Complete),
            "row_flow_negative" => row_output(Some(9), Some(10), GraphState::Complete),
            "row_flow_boundary" => row_output(Some(100), Some(10), GraphState::Complete),
            "row_flow_missing" => row_output(Some(101), None, GraphState::Complete),
            "row_flow_version" => {
                unsupported(row_output(Some(101), Some(10), GraphState::Complete))
            }
            "row_flow_graph_degraded" => row_output(Some(101), Some(10), GraphState::Partial),
            id if id.starts_with("node_time_") => time_case_output("PROJECT", id),
            id if id.starts_with("skew_") => signal_case(id, skew_output),
            id if id.starts_with("fanout_") => signal_case(id, fanout_output),
            id if id.starts_with("broadcast_") => signal_case(id, broadcast_output),
            id if id.starts_with("preagg_") => signal_case(id, preagg_output),
            _ => panic!("unexpected frozen case {id}"),
        };
        let before = output.facts_hash();
        evaluate(&mut output).expect("rule evaluation");
        let actual = evaluation(&output, rule_id);
        let finding = actual.finding_id.as_deref().and_then(|finding_id| {
            output
                .ir
                .findings
                .iter()
                .find(|finding| finding.finding_id == finding_id)
        });
        let outcome = serde_json::json!({
            "evaluation": actual,
            "finding": finding,
        });
        let outcome_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&outcome).expect("serialize rule outcome"))
        );
        if let Some(dir) = std::env::var_os("RULE_GOLDEN_DUMP") {
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{id}.json")),
                serde_json::to_string_pretty(&outcome).expect("outcome json"),
            )
            .expect("dump outcome");
        }
        if std::env::var_os("RULE_GOLDEN_BLESS").is_some() {
            blessed.push(serde_json::json!({
                "case_id": id,
                "rule_id": rule_id,
                "case_class": case["case_class"],
                "status": status_name(&actual.status),
                "evidence_completeness": format!("{:?}", actual.evidence_completeness).to_lowercase(),
                "outcome_sha256": outcome_sha256,
            }));
            continue;
        }
        assert_eq!(
            outcome_sha256,
            case["outcome_sha256"].as_str().expect("full outcome hash"),
            "{id}"
        );
        assert_eq!(
            status_name(&actual.status),
            case["status"].as_str().expect("status"),
            "{id}"
        );
        assert_eq!(
            format!("{:?}", actual.evidence_completeness).to_lowercase(),
            case["evidence_completeness"]
                .as_str()
                .expect("completeness"),
            "{id}"
        );
        assert_ne!(output.facts_hash(), before, "{id}");
        if matches!(
            actual.status,
            doctor_contracts::RuleEvaluationStatus::Triggered
                | doctor_contracts::RuleEvaluationStatus::Suspected
        ) {
            let finding = output
                .ir
                .findings
                .iter()
                .find(|finding| actual.finding_id.as_deref() == Some(finding.finding_id.as_str()))
                .expect("finding");
            assert_eq!(
                finding.evidence_completeness, actual.evidence_completeness,
                "{id}"
            );
            assert!(!finding.evidence.is_empty(), "{id}");
        } else {
            assert!(actual.finding_id.is_none(), "{id}");
        }
    }
    if std::env::var_os("RULE_GOLDEN_BLESS").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../fixtures/expected/3.5.14/sr3514_rule_contract_cases.json"
            ),
            serde_json::to_string_pretty(&blessed).expect("cases json") + "\n",
        )
        .expect("bless cases");
    }
}

fn time_case_output(kind: &str, id: &str) -> doctor_core::AdapterOutput {
    let complete = |mut output: doctor_core::AdapterOutput| {
        output.ir.graph.state = GraphState::Complete;
        output
    };
    match id.rsplit_once('_').map(|(_, suffix)| suffix) {
        Some("positive") => complete(share_output_for(
            kind,
            Some((151, None)),
            Some((1000, None)),
        )),
        Some("negative") => complete(share_output_for(
            kind,
            Some((149, None)),
            Some((1000, None)),
        )),
        Some("boundary") => complete(share_output_for(
            kind,
            Some((150, None)),
            Some((1000, None)),
        )),
        Some("missing") => complete(share_output_for(kind, None, Some((1000, None)))),
        Some("version") => unsupported(complete(share_output_for(
            kind,
            Some((151, None)),
            Some((1000, None)),
        ))),
        Some("degraded") => {
            let mut output = share_output_for(kind, Some((151, None)), Some((1000, None)));
            output.ir.graph.state = GraphState::Partial;
            output
        }
        _ => panic!("unknown time golden {id}"),
    }
}

fn unsupported(mut output: doctor_core::AdapterOutput) -> doctor_core::AdapterOutput {
    output.ir.version.pack_match = PackMatch::NoPack;
    output.ir.version.compatibility = Compatibility::Incompatible;
    output.ir.version.support_status = SupportStatus::Unsupported;
    output
}

fn operator_spill_graph_degraded() -> doctor_core::AdapterOutput {
    let mut output = base_output();
    let source = output.source_refs.last().unwrap().ref_id.clone();
    output.ir.graph.state = GraphState::Unavailable;
    output.ir.metric_observations.push(observation(
        "obs:spill",
        "QuerySpillBytes",
        Some("operator-1"),
        Some("native-1"),
        Some(0),
        "operator",
        "reported_base",
        ParseState::Exact,
        Some(1),
        None,
        &source,
    ));
    output.ir.operator_groups.push(group(
        "AGGREGATE",
        vec![("native-1", 0)],
        vec![],
        vec!["obs:spill"],
        &source,
    ));
    output
}
#[test]
fn time_share_rejects_wrong_roles_bindings_and_ordinals_without_guessing() {
    let mut unknown_role = share_output(Some((20, None)), Some((100, None)));
    synthesize_runtime_inventory(&mut unknown_role);
    unknown_role.ir.physical_operator_memberships[0].source_role = RuntimeSourceRole::Unknown;
    evaluate(&mut unknown_role).expect("evaluation");
    let actual = evaluation(&unknown_role, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_applicable");
    assert_eq!(actual.missing_prerequisites, ["operator_role_unknown"]);

    let mut duplicate_counter = share_output(Some((20, None)), Some((100, None)));
    let mut second = duplicate_counter.ir.metric_observations[0].clone();
    second.observation_id = "obs:op-duplicate".into();
    duplicate_counter.ir.metric_observations.push(second);
    evaluate(&mut duplicate_counter).expect("evaluation");
    let actual = evaluation(&duplicate_counter, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_applicable");
    assert_eq!(actual.missing_prerequisites, ["metric_ambiguous"]);

    let mut exchange = base_output();
    let source = exchange.source_refs.last().unwrap().ref_id.clone();
    exchange.ir.metric_observations = vec![
        observation(
            "obs:sub-network",
            "NetworkTime",
            Some("operator-1"),
            Some("sub-1"),
            Some(1),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(20),
            None,
            &source,
        ),
        observation(
            "obs:query",
            "QueryCumulativeOperatorTime",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(100),
            None,
            &source,
        ),
    ];
    exchange.ir.operator_groups = vec![group(
        "EXCHANGE",
        vec![("native-1", 0)],
        vec![("sub-1", 1)],
        vec!["obs:sub-network"],
        &source,
    )];
    evaluate(&mut exchange).expect("evaluation");
    let actual = evaluation(&exchange, "SR3514.EXCHANGE_NETWORK_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_applicable");
    assert_eq!(
        actual.missing_prerequisites,
        ["operator_total_time_missing"]
    );
}

#[test]
fn time_share_keeps_strict_boundaries_and_uses_reported_max_before_base() {
    let mut upper_open_15 = share_output(Some((10, Some((10, 15)))), Some((100, None)));
    upper_open_15.ir.metric_observations[0]
        .bounds
        .as_mut()
        .expect("bounds")
        .upper_inclusive = false;
    evaluate(&mut upper_open_15).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&upper_open_15, "SR3514.AGGREGATE_TIME_SHARE").status),
        "not_triggered"
    );

    let mut lower_open = share_output(Some((16, Some((15, 16)))), Some((100, None)));
    lower_open.ir.metric_observations[0]
        .bounds
        .as_mut()
        .expect("bounds")
        .lower_inclusive = false;
    evaluate(&mut lower_open).expect("evaluation");
    let finding = evaluation(&lower_open, "SR3514.AGGREGATE_TIME_SHARE")
        .finding_id
        .as_ref()
        .and_then(|id| {
            lower_open
                .ir
                .findings
                .iter()
                .find(|finding| &finding.finding_id == id)
        })
        .expect("strict lower boundary triggers");
    assert_eq!(finding.severity, "moderate");
    assert_eq!(finding.confidence, FindingConfidence::Medium);

    let mut open_15_to_30 = share_output(Some((16, Some((15, 30)))), Some((100, None)));
    let bounds = open_15_to_30.ir.metric_observations[0]
        .bounds
        .as_mut()
        .expect("bounds");
    bounds.lower_inclusive = false;
    bounds.upper_inclusive = false;
    evaluate(&mut open_15_to_30).expect("evaluation");
    let finding = evaluation(&open_15_to_30, "SR3514.AGGREGATE_TIME_SHARE")
        .finding_id
        .as_ref()
        .and_then(|id| {
            open_15_to_30
                .ir
                .findings
                .iter()
                .find(|finding| &finding.finding_id == id)
        })
        .expect("open upper 30 stays attention 15");
    assert_eq!(finding.severity, "moderate");
    assert_eq!(finding.confidence, FindingConfidence::Medium);

    let mut lower_open_30 = share_output(Some((31, Some((30, 31)))), Some((100, None)));
    lower_open_30.ir.metric_observations[0]
        .bounds
        .as_mut()
        .expect("bounds")
        .lower_inclusive = false;
    evaluate(&mut lower_open_30).expect("evaluation");
    let finding = evaluation(&lower_open_30, "SR3514.AGGREGATE_TIME_SHARE")
        .finding_id
        .as_ref()
        .and_then(|id| {
            lower_open_30
                .ir
                .findings
                .iter()
                .find(|finding| &finding.finding_id == id)
        })
        .expect("strict lower 30 triggers");
    assert_eq!(finding.severity, "high");
    assert_eq!(finding.confidence, FindingConfidence::Medium);

    let mut preferred_max = share_output(Some((10, None)), Some((100, None)));
    let source = preferred_max.source_refs.last().unwrap().ref_id.clone();
    preferred_max.ir.metric_observations.push(observation(
        "obs:op-max",
        "OperatorTotalTime",
        Some("operator-1"),
        Some("native-1"),
        Some(0),
        "operator",
        "reported_max",
        ParseState::Exact,
        Some(20 * SHARE_UNIT_NS),
        None,
        &source,
    ));
    preferred_max.ir.operator_groups[0]
        .metric_observation_refs
        .push("obs:op-max".into());
    let mut max_without_min = preferred_max.clone();
    evaluate(&mut max_without_min).expect("evaluation");
    let actual = evaluation(&max_without_min, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_applicable");
    assert_eq!(
        actual.missing_prerequisites,
        ["operator_min_max_pair_incomplete_or_ambiguous"]
    );
    preferred_max.ir.metric_observations.push(observation(
        "obs:op-min",
        "OperatorTotalTime",
        Some("operator-1"),
        Some("native-1"),
        Some(0),
        "operator",
        "reported_min",
        ParseState::Exact,
        Some(SHARE_UNIT_NS),
        None,
        &source,
    ));
    preferred_max.ir.operator_groups[0]
        .metric_observation_refs
        .push("obs:op-min".into());
    evaluate(&mut preferred_max).expect("evaluation");
    let actual = evaluation(&preferred_max, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "triggered");
    assert_eq!(actual.input_observation_refs, ["obs:op-max", "obs:query"]);
}

#[test]
fn special_operator_time_adds_cpu_once_and_lossy_zero_closure_stays_suspected() {
    let mut scan = base_output();
    let source = scan.source_refs.last().unwrap().ref_id.clone();
    scan.ir.metric_observations = vec![
        observation(
            "obs:scan-first",
            "ScanTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(6 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:cpu-first",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(5 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:scan-second",
            "ScanTime",
            Some("operator-1"),
            Some("native-2"),
            Some(1),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(15 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:cpu-second",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-2"),
            Some(1),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(5 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:query",
            "QueryCumulativeOperatorTime",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(100 * SHARE_UNIT_NS),
            None,
            &source,
        ),
    ];
    scan.ir.operator_groups = vec![group(
        "OLAP_SCAN",
        vec![("native-1", 0), ("native-2", 1)],
        vec![],
        vec![
            "obs:scan-first",
            "obs:cpu-first",
            "obs:scan-second",
            "obs:cpu-second",
        ],
        &source,
    )];
    evaluate(&mut scan).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&scan, "SR3514.SCAN_TIME_SHARE").status),
        "triggered"
    );
    assert_eq!(
        scan.ir
            .findings
            .iter()
            .find(|finding| finding.rule_id == "SR3514.SCAN_TIME_SHARE")
            .expect("finding")
            .severity,
        "moderate"
    );

    let mut exchange = base_output();
    let source = exchange.source_refs.last().unwrap().ref_id.clone();
    exchange.ir.metric_observations = vec![
        observation(
            "obs:network-first",
            "NetworkTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(6 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:cpu-first",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(5 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:network-second",
            "NetworkTime",
            Some("operator-1"),
            Some("native-2"),
            Some(1),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(15 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:cpu-second",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-2"),
            Some(1),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(5 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:query",
            "QueryCumulativeOperatorTime",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(100 * SHARE_UNIT_NS),
            None,
            &source,
        ),
    ];
    exchange.ir.operator_groups = vec![group(
        "EXCHANGE",
        vec![("native-1", 0), ("native-2", 1)],
        vec![],
        vec![
            "obs:network-first",
            "obs:cpu-first",
            "obs:network-second",
            "obs:cpu-second",
        ],
        &source,
    )];
    evaluate(&mut exchange).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&exchange, "SR3514.EXCHANGE_NETWORK_TIME_SHARE").status),
        "triggered"
    );
    assert_eq!(
        exchange
            .ir
            .findings
            .iter()
            .find(|finding| finding.rule_id == "SR3514.EXCHANGE_NETWORK_TIME_SHARE")
            .expect("finding")
            .severity,
        "moderate"
    );

    let mut spill = spill_output(ParseState::Exact, Some(0), Some((0, 2)));
    spill.ir.metric_observations[0]
        .bounds
        .as_mut()
        .expect("bounds")
        .lower_inclusive = false;
    evaluate(&mut spill).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&spill, "SR3514.SPILL_OBSERVED").status),
        "suspected"
    );
}

#[test]
fn time_share_preserves_native_subordinate_order_and_only_projects_selected_inputs() {
    let mut output = base_output();
    let source = output.source_refs.last().expect("source").ref_id.clone();
    output.ir.metric_observations = vec![
        observation(
            "obs:native",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(10 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:subordinate",
            "OperatorTotalTime",
            Some("operator-1"),
            Some("subordinate-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(10 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:query",
            "QueryCumulativeOperatorTime",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(100 * SHARE_UNIT_NS),
            None,
            &source,
        ),
        observation(
            "obs:unused",
            "QuerySpillBytes",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(SHARE_UNIT_NS),
            None,
            "source:missing",
        ),
    ];
    output.ir.operator_groups = vec![group(
        "AGGREGATE",
        vec![("native-1", 0)],
        vec![("subordinate-1", 0)],
        vec!["obs:native", "obs:subordinate", "obs:unused"],
        &source,
    )];
    evaluate(&mut output).expect("evaluation");
    let actual = evaluation(&output, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "triggered");
    assert_eq!(
        actual.input_observation_refs,
        vec!["obs:native", "obs:subordinate", "obs:query"]
    );
    assert!(
        actual
            .evidence_refs
            .iter()
            .all(|reference| reference == &source)
    );
    assert!(
        !actual
            .input_observation_refs
            .iter()
            .any(|reference| reference == "obs:unused")
    );

    let mut missing_denominator = time_output(Some((20, None)), None);
    let source = missing_denominator
        .source_refs
        .last()
        .expect("source")
        .ref_id
        .clone();
    evaluate(&mut missing_denominator).expect("evaluation");
    let actual = evaluation(&missing_denominator, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_applicable");
    assert_eq!(actual.input_observation_refs, vec!["obs:op"]);
    assert_eq!(actual.evidence_refs, vec![source]);

    let mut dangling_group = time_output(Some((20, None)), Some((100, None)));
    dangling_group.ir.operator_groups[0].source_refs = vec!["source:missing".into()];
    evaluate(&mut dangling_group).expect("evaluation");
    assert!(
        evaluation(&dangling_group, "SR3514.AGGREGATE_TIME_SHARE")
            .evidence_refs
            .is_empty()
    );
}

#[test]
fn typed_metric_integrity_requires_dimension_and_observation_merge_provenance() {
    let mut wrong_dimension = spill_output(ParseState::Exact, Some(1), None);
    wrong_dimension.ir.metric_observations[0].dimension = "duration".into();
    evaluate(&mut wrong_dimension).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&wrong_dimension, "SR3514.SPILL_OBSERVED").status),
        "not_applicable"
    );

    let mut unmerged = time_output(Some((20, None)), Some((100, None)));
    unmerged.ir.metric_observations[0].source_merge_state = "unmerged".into();
    evaluate(&mut unmerged).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&unmerged, "SR3514.AGGREGATE_TIME_SHARE").status),
        "not_applicable"
    );

    let mut unknown_merge = spill_output(ParseState::Exact, Some(1), None);
    unknown_merge.ir.metric_observations[0].source_merge_state = "unknown".into();
    evaluate(&mut unknown_merge).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&unknown_merge, "SR3514.SPILL_OBSERVED").status),
        "not_applicable"
    );
}

#[test]
fn duplicate_rule_subjects_are_single_not_applicable_results() {
    let mut duplicate_spill = spill_output(ParseState::Exact, Some(1), None);
    let source = duplicate_spill
        .source_refs
        .last()
        .expect("source")
        .ref_id
        .clone();
    duplicate_spill.ir.metric_observations.push(observation(
        "obs:spill-second",
        "QuerySpillBytes",
        None,
        None,
        None,
        "query",
        "reported_base",
        ParseState::Exact,
        Some(2),
        None,
        &source,
    ));
    evaluate(&mut duplicate_spill).expect("evaluation");
    let spills = duplicate_spill
        .ir
        .rule_evaluations
        .iter()
        .filter(|evaluation| evaluation.rule_id == "SR3514.SPILL_OBSERVED")
        .collect::<Vec<_>>();
    assert_eq!(spills.len(), 1);
    assert_eq!(status_name(&spills[0].status), "not_applicable");
    assert!(spills[0].finding_id.is_none());
    assert!(
        spills[0]
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "spill_observation_ambiguous")
    );

    let mut duplicate_group = time_output(Some((20, None)), Some((100, None)));
    duplicate_group
        .ir
        .operator_groups
        .push(duplicate_group.ir.operator_groups[0].clone());
    evaluate(&mut duplicate_group).expect("evaluation");
    let times = duplicate_group
        .ir
        .rule_evaluations
        .iter()
        .filter(|evaluation| evaluation.rule_id == "SR3514.AGGREGATE_TIME_SHARE")
        .collect::<Vec<_>>();
    assert_eq!(times.len(), 1);
    assert_eq!(status_name(&times[0].status), "not_applicable");
    assert!(times[0].finding_id.is_none());
    assert!(
        times[0]
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "summary_subject_group_unresolved")
    );

    let mut operator_spill = base_output();
    let source = operator_spill
        .source_refs
        .last()
        .expect("source")
        .ref_id
        .clone();
    operator_spill.ir.metric_observations.push(observation(
        "obs:operator-spill",
        "QuerySpillBytes",
        Some("operator-1"),
        Some("native-1"),
        Some(0),
        "operator",
        "reported_base",
        ParseState::Exact,
        Some(1),
        None,
        &source,
    ));
    operator_spill.ir.operator_groups.push(group(
        "AGGREGATE",
        vec![("native-1", 0)],
        vec![],
        vec!["obs:operator-spill"],
        &source,
    ));
    operator_spill
        .ir
        .operator_groups
        .push(operator_spill.ir.operator_groups[0].clone());
    evaluate(&mut operator_spill).expect("evaluation");
    let spills = operator_spill
        .ir
        .rule_evaluations
        .iter()
        .filter(|evaluation| evaluation.rule_id == "SR3514.SPILL_OBSERVED")
        .collect::<Vec<_>>();
    assert_eq!(spills.len(), 1);
    assert_eq!(status_name(&spills[0].status), "not_applicable");
    assert!(spills[0].finding_id.is_none());
    assert!(
        spills[0]
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "operator_subject_ambiguous")
    );
}

#[test]
fn successful_evaluations_record_replayable_checks_and_receipt_chain() {
    let mut output = share_output(Some((20, None)), Some((100, None)));
    evaluate(&mut output).expect("evaluation");
    let actual = evaluation(&output, "SR3514.AGGREGATE_TIME_SHARE");
    for expected in [
        "version_profile:final_supported_3514",
        "subject:execution_summary_plan_node_producing_fragment_group",
        "node_time:execution_summary_official_node_info_recipe",
        "typed_metrics:duration_ns_dimension_scope_provenance_fidelity_verified",
        "stored_profile_merge:level1_final_merged_verified",
        "query_denominator:unique_positive_nonzero",
        "share_class:shared_with_execution_summary_hotspot",
    ] {
        assert!(
            actual
                .applicability_checks
                .iter()
                .any(|check| check == expected)
        );
    }
    assert!(actual.threshold_receipts.iter().any(|receipt| receipt
        == &format!("ruleset:doctor-v0.1@{PRODUCT_RULESET_VERSION}#{RULESET_HASH}")));
    assert!(
        actual
            .threshold_receipts
            .iter()
            .any(|receipt| receipt == "evaluator:doctor-core-rules@0.1.0")
    );
    assert!(
        actual
            .threshold_receipts
            .iter()
            .any(|receipt| receipt == "strict_gt:15%")
    );
    let finding = actual
        .finding_id
        .as_ref()
        .and_then(|id| {
            output
                .ir
                .findings
                .iter()
                .find(|finding| &finding.finding_id == id)
        })
        .expect("finding");
    assert_eq!(
        finding.recommendation.risk,
        "aggregate time share needs manual investigation; it does not prove an aggregation root cause"
    );
    assert_eq!(
        finding.recommendation.expected_direction,
        "reduce selected aggregate time share after one controlled manual change"
    );
    assert_eq!(
        finding.recommendation.validation_steps,
        vec![
            "inspect grouping, stages, input/output rows, memory and spill context",
            "re-profile after one controlled manual change"
        ]
    );
}

#[test]
fn query_scope_cannot_pollute_operator_subjects_and_operator_spill_keeps_binding_provenance() {
    let mut polluted_denominator = time_output(Some((20, None)), Some((100, None)));
    polluted_denominator.ir.metric_observations[1].subject_ref = Some("operator-1".into());
    evaluate(&mut polluted_denominator).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&polluted_denominator, "SR3514.AGGREGATE_TIME_SHARE").status),
        "not_applicable"
    );

    let mut output = base_output();
    let metric_source = output
        .source_refs
        .last()
        .expect("metric source")
        .ref_id
        .clone();
    let mut binding_source = output.source_refs.last().expect("source template").clone();
    binding_source.ref_id = "source:operator-binding".into();
    output.source_refs.push(binding_source.clone());
    output.ir.metric_observations = vec![
        observation(
            "obs:operator-spill",
            "QuerySpillBytes",
            Some("operator-1"),
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(1),
            None,
            &metric_source,
        ),
        observation(
            "obs:polluted-query-spill",
            "QuerySpillBytes",
            Some("operator-1"),
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(1),
            None,
            &metric_source,
        ),
    ];
    output.ir.operator_groups = vec![group(
        "AGGREGATE",
        vec![("native-1", 0)],
        vec![],
        vec!["obs:operator-spill"],
        &binding_source.ref_id,
    )];
    evaluate(&mut output).expect("evaluation");
    let spills = output
        .ir
        .rule_evaluations
        .iter()
        .filter(|evaluation| evaluation.rule_id == "SR3514.SPILL_OBSERVED")
        .collect::<Vec<_>>();
    assert_eq!(spills.len(), 2);
    assert_eq!(
        output
            .ir
            .findings
            .iter()
            .filter(|finding| finding.rule_id == "SR3514.SPILL_OBSERVED")
            .count(),
        1
    );
    let operator_spill = spills
        .iter()
        .find(|evaluation| evaluation.status == doctor_contracts::RuleEvaluationStatus::Triggered)
        .expect("operator spill finding");
    assert_eq!(operator_spill.subject_ref.as_deref(), Some("operator-1"));
    assert!(
        operator_spill
            .evidence_refs
            .iter()
            .any(|reference| reference == &metric_source)
    );
    assert!(
        operator_spill
            .evidence_refs
            .iter()
            .any(|reference| reference == &binding_source.ref_id)
    );
    let polluted = spills
        .iter()
        .find(|evaluation| {
            evaluation.status == doctor_contracts::RuleEvaluationStatus::NotApplicable
        })
        .expect("polluted query rejected");
    assert!(polluted.subject_ref.is_none());
    assert!(polluted.finding_id.is_none());
    assert!(
        polluted
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "query_spill_subject_or_operator_binding_forbidden")
    );
    let identities = output
        .ir
        .rule_evaluations
        .iter()
        .map(|evaluation| (evaluation.rule_id.clone(), evaluation.subject_ref.clone()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identities.len(), output.ir.rule_evaluations.len());

    let mut invalids = base_output();
    let source = invalids.source_refs.last().expect("source").ref_id.clone();
    invalids.ir.metric_observations = vec![
        observation(
            "obs:query",
            "QuerySpillBytes",
            None,
            None,
            None,
            "query",
            "reported_base",
            ParseState::Exact,
            Some(1),
            None,
            &source,
        ),
        observation(
            "obs:invalid-scope",
            "QuerySpillBytes",
            Some("not-a-subject"),
            None,
            None,
            "invalid",
            "reported_base",
            ParseState::Exact,
            Some(1),
            None,
            &source,
        ),
        observation(
            "obs:missing-subject",
            "QuerySpillBytes",
            None,
            Some("native-1"),
            Some(0),
            "operator",
            "reported_base",
            ParseState::Exact,
            Some(1),
            None,
            &source,
        ),
    ];
    evaluate(&mut invalids).expect("evaluation");
    let spills = invalids
        .ir
        .rule_evaluations
        .iter()
        .filter(|evaluation| evaluation.rule_id == "SR3514.SPILL_OBSERVED")
        .collect::<Vec<_>>();
    assert_eq!(spills.len(), 1);
    assert!(spills[0].subject_ref.is_none());
    assert_eq!(status_name(&spills[0].status), "not_applicable");
    assert!(spills[0].finding_id.is_none());
}

#[test]
fn query_spill_is_not_blocked_by_an_unrelated_unavailable_graph_and_receipt_is_exact() {
    let mut output = spill_output(ParseState::Exact, Some(1), None);
    output.ir.graph.state = GraphState::Unavailable;
    evaluate(&mut output).expect("query spill remains applicable");
    assert_eq!(
        status_name(&evaluation(&output, "SR3514.SPILL_OBSERVED").status),
        "triggered"
    );

    let mut bad = spill_output(ParseState::Exact, Some(1), None);
    bad.ir
        .analysis_receipt
        .components
        .iter_mut()
        .find(|component| component.id == "doctor-v0.1")
        .expect("ruleset component")
        .version = "0.1.1".into();
    assert_eq!(
        evaluate(&mut bad),
        Err(RuleEvaluationError::ReceiptComponentInvalid)
    );
}

#[test]
fn ruleset_and_fixture_link_every_case_once() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../rule-sets/doctor_v0_1/ruleset.json"))
            .expect("ruleset");
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_rule_contract_cases.json"
    ))
    .expect("cases");
    let expected = cases
        .as_array()
        .expect("array")
        .iter()
        .map(|case| case["case_id"].as_str().expect("case id"))
        .collect::<std::collections::BTreeSet<_>>();
    let linked = manifest["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .flat_map(|rule| rule["golden_case_ids"].as_array().expect("goldens"))
        .map(|case| case.as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(linked.len(), 66);
    assert_eq!(
        linked
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
}

#[test]
fn evidence_completeness_is_hashed_and_not_applicable_keeps_rule_limitations() {
    let mut complete = time_case_output("AGGREGATE", "aggregate_time_positive");
    let mut partial = time_case_output("AGGREGATE", "aggregate_time_graph_degraded");
    evaluate(&mut complete).expect("complete evaluation");
    evaluate(&mut partial).expect("partial evaluation");
    let complete_eval = evaluation(&complete, "SR3514.AGGREGATE_TIME_SHARE");
    let partial_eval = evaluation(&partial, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(
        format!("{:?}", complete_eval.evidence_completeness).to_lowercase(),
        "complete"
    );
    assert_eq!(
        format!("{:?}", partial_eval.evidence_completeness).to_lowercase(),
        "partial"
    );
    assert_ne!(complete.facts_hash(), partial.facts_hash());

    let mut unavailable = time_case_output("AGGREGATE", "aggregate_time_missing");
    evaluate(&mut unavailable).expect("unavailable evaluation");
    let unavailable = evaluation(&unavailable, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(
        format!("{:?}", unavailable.evidence_completeness).to_lowercase(),
        "unavailable"
    );
    assert!(!unavailable.limitations.is_empty());
    assert!(!unavailable.evidence_completeness_reasons.is_empty());
}

#[test]
fn row_flow_rejects_unregistered_output_rows_recipe() {
    let mut output = row_output(Some(101), Some(10), GraphState::Complete);
    output.ir.metric_observations[0].definition_ref = Some("untrusted-output-rows@1".into());
    evaluate(&mut output).expect("rule evaluation");
    let evaluation = evaluation(&output, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(status_name(&evaluation.status), "not_applicable");
    assert!(
        evaluation
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "output_rows_unavailable")
    );
}

#[test]
fn row_flow_requires_the_complete_exact_child_closure() {
    let mut output = row_output(Some(101), Some(10), GraphState::Complete);
    let source = output.source_refs.last().unwrap().ref_id.clone();
    output.ir.graph.nodes.push(GraphNode {
        sql_location: None,
        sql_facts: None,
        node_id: "plan:3".into(),
        entity_kind: GraphEntityKind::PlanNode,
        plan_node_id: Some("3".into()),
        fragment_id: None,
        pipeline_id: None,
        plan_node_type: Some("SCAN".into()),
        display_name: None,
        source_ordinal: None,
        native_operator_refs: vec![],
        subordinate_operator_refs: vec![],
        metric_observation_refs: vec!["obs:child-extra".into()],
        execution_state: None,
        source_quality: SourceQuality::Partial,
        source_refs: vec![source.clone()],
    });
    output.ir.graph.edges.push(GraphEdge {
        edge_id: "edge:partial-child".into(),
        from: "plan:1".into(),
        to: "plan:3".into(),
        relation: GraphRelation::PlanChild,
        direction_semantics: DirectionSemantics::ParentToChild,
        source: EdgeSource::Explicit,
        quality: SourceQuality::Partial,
        source_ordinal: None,
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec![source],
    });
    evaluate(&mut output).expect("rule evaluation");
    let partial_edge_evaluation = evaluation(&output, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(
        status_name(&partial_edge_evaluation.status),
        "not_applicable"
    );
    assert!(
        partial_edge_evaluation
            .missing_prerequisites
            .contains(&"graph_direct_child_closure_unverified".into())
    );

    let mut partial_child = row_output(Some(101), Some(10), GraphState::Complete);
    partial_child.ir.graph.nodes[1].source_quality = SourceQuality::Partial;
    evaluate(&mut partial_child).expect("partial child evaluation");
    let child_evaluation = evaluation(&partial_child, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(status_name(&child_evaluation.status), "not_applicable");
    assert!(
        child_evaluation
            .missing_prerequisites
            .contains(&"direct_child_provenance_unverified".into())
    );

    let mut dangling_child = row_output(Some(101), Some(10), GraphState::Complete);
    synthesize_runtime_inventory(&mut dangling_child);
    dangling_child.ir.graph.nodes.pop();
    evaluate(&mut dangling_child).expect("dangling child evaluation");
    let dangling_evaluation = evaluation(&dangling_child, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(status_name(&dangling_evaluation.status), "not_applicable");
    assert!(
        dangling_evaluation
            .missing_prerequisites
            .contains(&"graph_child_node_missing".into())
    );

    let mut duplicate_endpoint = row_output(Some(101), Some(10), GraphState::Complete);
    let mut duplicate_edge = duplicate_endpoint.ir.graph.edges[0].clone();
    duplicate_edge.edge_id = "edge:duplicate-child-endpoint".into();
    duplicate_endpoint.ir.graph.edges.push(duplicate_edge);
    evaluate(&mut duplicate_endpoint).expect("duplicate endpoint evaluation");
    let duplicate_evaluation = evaluation(&duplicate_endpoint, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(status_name(&duplicate_evaluation.status), "not_applicable");
    assert!(
        duplicate_evaluation
            .missing_prerequisites
            .contains(&"graph_direct_child_duplicate_endpoint".into())
    );
}

#[test]
fn row_flow_rejects_verified_data_flow_until_a_production_recipe_emits_it() {
    let verified = |recipe_id: &str| {
        let mut output = row_output(Some(101), Some(10), GraphState::Complete);
        let source = output.source_refs.last().unwrap().ref_id.clone();
        output.ir.graph.edges = vec![GraphEdge {
            edge_id: "edge:verified-input".into(),
            from: "plan:2".into(),
            to: "plan:1".into(),
            relation: GraphRelation::VerifiedDataFlow,
            direction_semantics: DirectionSemantics::UpstreamToDownstream,
            source: EdgeSource::VerifiedDerived,
            quality: SourceQuality::Exact,
            source_ordinal: None,
            input_role: None,
            derivation_recipe: Some(RecipeRef {
                id: recipe_id.into(),
                version: "1".into(),
            }),
            restates_edge_id: None,
            evidence_refs: vec![source],
        }];
        output
    };

    let mut unknown = verified("sr3514.unknown-structural-recipe");
    evaluate(&mut unknown).expect("unknown rule evaluation");
    let unknown = evaluation(&unknown, "SR3514.ROW_FLOW_AMPLIFICATION");
    assert_eq!(status_name(&unknown.status), "not_applicable");
    assert!(
        unknown
            .missing_prerequisites
            .contains(&"graph_direct_child_relation_unregistered".into())
    );
}

#[test]
fn negative_time_share_preserves_bounded_and_partial_completeness() {
    let mut bounded = time_output_for(
        "AGGREGATE",
        Some((100, Some((90, 110)))),
        Some((1000, None)),
    );
    bounded.ir.graph.state = GraphState::Complete;
    evaluate(&mut bounded).expect("bounded negative evaluation");
    let bounded = evaluation(&bounded, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&bounded.status), "not_triggered");
    assert_eq!(
        format!("{:?}", bounded.evidence_completeness).to_lowercase(),
        "partial"
    );
    assert_eq!(
        bounded.evidence_completeness_reasons,
        ["bounded_typed_inputs"]
    );

    let mut partial = time_output_for("AGGREGATE", Some((100, None)), Some((1000, None)));
    partial.ir.graph.state = GraphState::Partial;
    evaluate(&mut partial).expect("partial negative evaluation");
    let partial = evaluation(&partial, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&partial.status), "not_triggered");
    assert_eq!(
        format!("{:?}", partial.evidence_completeness).to_lowercase(),
        "partial"
    );
    assert_eq!(
        partial.evidence_completeness_reasons,
        ["bounded_typed_inputs"]
    );
}

#[test]
fn partial_graph_does_not_upgrade_a_threshold_intersection() {
    let mut output = share_output_for(
        "AGGREGATE",
        Some((150, Some((149, 151)))),
        Some((1000, None)),
    );
    output.ir.graph.state = GraphState::Partial;
    evaluate(&mut output).expect("rule evaluation");
    let evaluation = evaluation(&output, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&evaluation.status), "suspected");
    let finding = output
        .ir
        .findings
        .iter()
        .find(|finding| evaluation.finding_id.as_deref() == Some(finding.finding_id.as_str()))
        .expect("suspected finding");
    assert_eq!(finding.confidence, FindingConfidence::Low);
    assert_eq!(
        finding.evidence_completeness_reasons,
        ["threshold_boundary_intersection"]
    );
}

#[test]
fn row_finding_identity_is_not_labeled_as_spill() {
    let mut output = row_output(Some(101), Some(10), GraphState::Complete);
    evaluate(&mut output).expect("rule evaluation");
    let evaluation = evaluation(&output, "SR3514.ROW_FLOW_AMPLIFICATION");
    let finding = output
        .ir
        .findings
        .iter()
        .find(|finding| evaluation.finding_id.as_deref() == Some(finding.finding_id.as_str()))
        .expect("row finding");
    assert!(
        finding
            .finding_id
            .contains("row_flow_amplification:complete")
    );
    assert!(!finding.finding_id.contains("spill_observed"));
}

#[test]
fn time_rules_fail_closed_for_zero_denominator_overflow_and_wrong_unit() {
    let mut zero_denominator = time_output(Some((20, None)), Some((0, None)));
    evaluate(&mut zero_denominator).expect("zero denominator is a typed NA result");
    let zero = evaluation(&zero_denominator, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&zero.status), "not_applicable");
    assert!(
        zero.missing_prerequisites
            .contains(&"invalid_denominator".into())
    );

    let mut overflow = time_output(Some((i128::MAX, None)), Some((1, None)));
    evaluate(&mut overflow).expect("comparison overflow is a typed NA result");
    let overflow = evaluation(&overflow, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&overflow.status), "not_applicable");
    assert!(
        overflow
            .missing_prerequisites
            .contains(&"metric_value_out_of_range".into())
    );

    let mut wrong_unit = time_output(Some((20, None)), Some((100, None)));
    wrong_unit.ir.metric_observations[0].canonical_unit = Some("byte".into());
    evaluate(&mut wrong_unit).expect("wrong unit is a typed NA result");
    let wrong_unit = evaluation(&wrong_unit, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&wrong_unit.status), "not_applicable");
    assert!(
        wrong_unit
            .missing_prerequisites
            .contains(&"metric_unit_unexpected".into())
    );
}

#[test]
fn deterministic_rule_evaluation_is_idempotent() {
    let mut output = time_case_output("AGGREGATE", "aggregate_time_positive");
    evaluate(&mut output).expect("first evaluation");
    let first = (
        output.ir.rule_evaluations.clone(),
        output.ir.findings.clone(),
        output.facts_hash(),
    );

    evaluate(&mut output).expect("second evaluation");
    assert_eq!(
        (
            output.ir.rule_evaluations.clone(),
            output.ir.findings.clone(),
            output.facts_hash(),
        ),
        first
    );
}

#[test]
fn time_share_below_absolute_floor_is_suppressed_without_a_finding() {
    // 60 ms is 60% of the query, which alone would be a high finding.
    let mut output = time_output(Some((60_000_000, None)), Some((100_000_000, None)));
    evaluate(&mut output).expect("evaluation");
    let actual = evaluation(&output, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "suppressed");
    assert!(actual.finding_id.is_none());
    assert!(
        output
            .ir
            .findings
            .iter()
            .all(|finding| finding.rule_id != "SR3514.AGGREGATE_TIME_SHARE")
    );
    assert!(
        actual
            .comparisons
            .iter()
            .any(|comparison| comparison == "ratio_all_strictly_gt_30_percent")
    );
    assert!(
        actual
            .comparisons
            .iter()
            .any(|comparison| comparison == "subject_time_all_below_absolute_floor_100000000ns")
    );
    assert!(actual.threshold_receipts.iter().any(|receipt| receipt
        == "absolute_floor:subject_operator_time_ns<100000000ns:product_decision_ruleset_0.1.5"));
}

#[test]
fn time_share_at_or_straddling_absolute_floor_keeps_its_finding() {
    // Exactly 100 ms is not below the floor.
    let mut at_floor = time_output(Some((100_000_000, None)), Some((200_000_000, None)));
    evaluate(&mut at_floor).expect("evaluation");
    let actual = evaluation(&at_floor, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "triggered");
    assert!(actual.finding_id.is_some());

    // A range that reaches the floor is not entirely below it.
    let mut straddling = time_output(
        Some((90_000_000, Some((80_000_000, 100_000_000)))),
        Some((200_000_000, None)),
    );
    evaluate(&mut straddling).expect("evaluation");
    let actual = evaluation(&straddling, "SR3514.AGGREGATE_TIME_SHARE");
    assert_ne!(status_name(&actual.status), "suppressed");
    assert!(actual.finding_id.is_some());
}

#[test]
fn absolute_floor_never_turns_a_quiet_share_into_a_finding() {
    let mut output = time_output(Some((10_000_000, None)), Some((100_000_000, None)));
    evaluate(&mut output).expect("evaluation");
    let actual = evaluation(&output, "SR3514.AGGREGATE_TIME_SHARE");
    assert_eq!(status_name(&actual.status), "not_triggered");
    assert!(actual.finding_id.is_none());
}

// ---------------------------------------------------------------------------
// Ruleset 0.2.0 signal-rule goldens: one plan node with explicit runtime
// operators (canonical type per operator) and merged counters on them.
// ---------------------------------------------------------------------------

const MIB: i128 = 1_048_576;

/// The signal golden classes: positive / negative / boundary / missing build
/// their own inputs; version and graph_degraded reuse the positive input.
fn signal_case(
    id: &str,
    build: fn(&str) -> doctor_core::AdapterOutput,
) -> doctor_core::AdapterOutput {
    match id.rsplit_once('_').map(|(_, suffix)| suffix) {
        Some("version") => unsupported(build("positive")),
        Some("degraded") => {
            let mut output = build("positive");
            output.ir.graph.state = GraphState::Partial;
            output
        }
        Some(class) => build(class),
        None => panic!("unknown signal golden {id}"),
    }
}

/// Adds plan node `plan_node_id` with one operator group and a native
/// membership per (operator ref, operator type).
fn add_plan_node(
    output: &mut doctor_core::AdapterOutput,
    plan_node_id: &str,
    node_type: &str,
    operators: &[(&str, &str)],
) {
    let source = output.source_refs.last().unwrap().ref_id.clone();
    let ordinal = output.ir.physical_operator_memberships.len() as u32;
    let mut group = group(node_type, vec![], vec![], vec![], &source);
    group.operator_group_id = format!("operator-{plan_node_id}");
    group.plan_node_id = Some(plan_node_id.into());
    output.ir.operator_groups.push(group);
    output.ir.graph.nodes.push(GraphNode {
        sql_location: None,
        sql_facts: None,
        node_id: format!("plan:{plan_node_id}"),
        entity_kind: GraphEntityKind::PlanNode,
        plan_node_id: Some(plan_node_id.into()),
        fragment_id: None,
        pipeline_id: None,
        plan_node_type: Some(node_type.into()),
        display_name: None,
        source_ordinal: None,
        native_operator_refs: vec![],
        subordinate_operator_refs: vec![],
        metric_observation_refs: vec![],
        execution_state: None,
        source_quality: SourceQuality::Exact,
        source_refs: vec![source.clone()],
    });
    for (index, (reference, canonical_type)) in operators.iter().enumerate() {
        output
            .ir
            .physical_operator_memberships
            .push(PhysicalOperatorMembership {
                physical_operator_ref: (*reference).into(),
                source_occurrence_ordinal: ordinal + index as u32,
                fragment_ref: None,
                container_ref: None,
                driver_occurrence_ref: None,
                driver_occurrence_ordinal: None,
                emission_ordinal: None,
                plan_node_id: Some(plan_node_id.into()),
                canonical_type: (*canonical_type).into(),
                source_role: RuntimeSourceRole::Native,
                source_refs: vec![source.clone()],
                reasons: vec![],
            });
    }
}

/// One merged counter of operator `reference` on plan node `plan_node_id`.
fn add_counter(
    output: &mut doctor_core::AdapterOutput,
    plan_node_id: &str,
    reference: &str,
    metric_id: &str,
    role: &str,
    value: i128,
) {
    let source = output.source_refs.last().unwrap().ref_id.clone();
    let subject = format!("operator-{plan_node_id}");
    let id = format!("obs:{reference}:{metric_id}:{role}");
    let mut metric = observation(
        &id,
        metric_id,
        Some(&subject),
        Some(reference),
        Some(0),
        "operator",
        role,
        ParseState::Exact,
        Some(value),
        None,
        &source,
    );
    metric.runtime_occurrence_ref = Some(reference.into());
    metric.source_name = match role {
        "reported_min" => format!("__MIN_OF_{metric_id}"),
        "reported_max" => format!("__MAX_OF_{metric_id}"),
        _ => metric_id.into(),
    };
    output.ir.metric_observations.push(metric);
    output
        .ir
        .operator_groups
        .iter_mut()
        .find(|group| group.operator_group_id == subject)
        .expect("plan node group")
        .metric_observation_refs
        .push(id);
}

fn signal_base(complete: bool) -> doctor_core::AdapterOutput {
    let mut output = base_output();
    if complete {
        output.ir.graph.state = GraphState::Complete;
    }
    output
}

/// HASH_JOIN probe: mean 1 s, min 0.1 s, max per class (ratio 3 / 1.5 / exactly 2).
fn skew_output(class: &str) -> doctor_core::AdapterOutput {
    let mut output = signal_base(true);
    add_plan_node(
        &mut output,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    let max = match class {
        "positive" | "missing" => 3,
        "negative" => 1,
        "boundary" => 2,
        _ => panic!("skew class {class}"),
    } * SHARE_UNIT_NS
        + if class == "negative" {
            SHARE_UNIT_NS / 2
        } else {
            0
        };
    add_counter(
        &mut output,
        "1",
        "probe-1",
        "OperatorTotalTime",
        "reported_base",
        SHARE_UNIT_NS,
    );
    add_counter(
        &mut output,
        "1",
        "probe-1",
        "OperatorTotalTime",
        "reported_max",
        max,
    );
    if class != "missing" {
        add_counter(
            &mut output,
            "1",
            "probe-1",
            "OperatorTotalTime",
            "reported_min",
            SHARE_UNIT_NS / 10,
        );
    }
    output
}

/// HASH_JOIN probe: 100,000 input rows; output 11× / 5× / exactly 10× / missing.
fn fanout_output(class: &str) -> doctor_core::AdapterOutput {
    let mut output = signal_base(true);
    add_plan_node(
        &mut output,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    add_counter(
        &mut output,
        "1",
        "probe-1",
        "PushRowNum",
        "reported_base",
        100_000,
    );
    let pulled = match class {
        "positive" => Some(1_100_000),
        "negative" => Some(500_000),
        "boundary" => Some(1_000_000),
        "missing" => None,
        _ => panic!("fanout class {class}"),
    };
    if let Some(pulled) = pulled {
        add_counter(
            &mut output,
            "1",
            "probe-1",
            "PullRowNum",
            "reported_base",
            pulled,
        );
    }
    output
}

/// A BROADCAST hash join (node 1) whose only UNPARTITIONED child exchange
/// (node 2) sends 200 MiB / 10 MiB / exactly 100 MiB / nothing typed.
fn broadcast_output(class: &str) -> doctor_core::AdapterOutput {
    let mut output = signal_base(true);
    let source = output.source_refs.last().unwrap().ref_id.clone();
    add_plan_node(
        &mut output,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    add_plan_node(&mut output, "2", "EXCHANGE", &[("sink-2", "EXCHANGE_SINK")]);
    output.ir.graph.edges.push(GraphEdge {
        edge_id: "edge:join-build".into(),
        from: "plan:1".into(),
        to: "plan:2".into(),
        relation: GraphRelation::PlanChild,
        direction_semantics: DirectionSemantics::ParentToChild,
        source: EdgeSource::Explicit,
        quality: SourceQuality::Exact,
        source_ordinal: None,
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec![source.clone()],
    });
    for (reference, key, value) in [
        ("probe-1", "DistributionMode", "BROADCAST"),
        ("sink-2", "PartType", "UNPARTITIONED"),
    ] {
        output.ir.operator_info_facts.push(OperatorInfoFact {
            physical_operator_ref: reference.into(),
            key: key.into(),
            value: Some(value.into()),
            source_refs: vec![source.clone()],
        });
    }
    let bytes = match class {
        "positive" => Some(200 * MIB),
        "negative" => Some(10 * MIB),
        "boundary" => Some(100 * MIB),
        "missing" => None,
        _ => panic!("broadcast class {class}"),
    };
    if let Some(bytes) = bytes {
        add_counter(
            &mut output,
            "2",
            "sink-2",
            "BytesSent",
            "reported_base",
            bytes,
        );
    }
    output
}

/// Streaming pre-aggregation over 2,000,000 rows keeping 97.5% / 10% /
/// exactly 90% / an untyped output.
fn preagg_output(class: &str) -> doctor_core::AdapterOutput {
    let mut output = signal_base(true);
    add_plan_node(
        &mut output,
        "1",
        "AGGREGATION",
        &[
            ("streaming-sink-1", "AGGREGATE_STREAMING_SINK"),
            ("streaming-source-1", "AGGREGATE_STREAMING_SOURCE"),
        ],
    );
    add_counter(
        &mut output,
        "1",
        "streaming-sink-1",
        "PushRowNum",
        "reported_base",
        2_000_000,
    );
    let kept = match class {
        "positive" => Some(1_950_000),
        "negative" => Some(200_000),
        "boundary" => Some(1_800_000),
        "missing" => None,
        _ => panic!("preagg class {class}"),
    };
    if let Some(kept) = kept {
        add_counter(
            &mut output,
            "1",
            "streaming-source-1",
            "PullRowNum",
            "reported_base",
            kept,
        );
    }
    output
}

#[test]
fn signal_rules_fail_closed_and_respect_floors() {
    // Skew whose slowest driver is under 100 ms is visible but suppressed.
    let mut small = signal_base(true);
    add_plan_node(
        &mut small,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    add_counter(
        &mut small,
        "1",
        "probe-1",
        "OperatorTotalTime",
        "reported_base",
        10_000_000,
    );
    add_counter(
        &mut small,
        "1",
        "probe-1",
        "OperatorTotalTime",
        "reported_max",
        40_000_000,
    );
    add_counter(
        &mut small,
        "1",
        "probe-1",
        "OperatorTotalTime",
        "reported_min",
        1_000_000,
    );
    evaluate(&mut small).expect("evaluation");
    let actual = evaluation(&small, "SR3514.OPERATOR_TIME_SKEW");
    assert_eq!(status_name(&actual.status), "suppressed");
    assert!(actual.finding_id.is_none());

    // A mean outside [min, max] is not an AVG-merged time counter.
    let mut not_avg = skew_output("positive");
    not_avg
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| observation.value_role == "reported_base")
        .expect("base")
        .canonical_number = Some(integer(5 * SHARE_UNIT_NS));
    evaluate(&mut not_avg).expect("evaluation");
    let actual = evaluation(&not_avg, "SR3514.OPERATOR_TIME_SKEW");
    assert_eq!(
        actual.missing_prerequisites,
        ["time_counter_not_avg_merged"]
    );

    // Fan-out below 100,000 output rows is suppressed; zero probe input is NA.
    let mut tiny = signal_base(true);
    add_plan_node(
        &mut tiny,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    add_counter(
        &mut tiny,
        "1",
        "probe-1",
        "PushRowNum",
        "reported_base",
        100,
    );
    add_counter(
        &mut tiny,
        "1",
        "probe-1",
        "PullRowNum",
        "reported_base",
        5_000,
    );
    evaluate(&mut tiny).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&tiny, "SR3514.JOIN_PROBE_FANOUT").status),
        "suppressed"
    );
    let mut zero = signal_base(true);
    add_plan_node(
        &mut zero,
        "1",
        "HASH_JOIN",
        &[("probe-1", "HASH_JOIN_PROBE")],
    );
    add_counter(&mut zero, "1", "probe-1", "PushRowNum", "reported_base", 0);
    add_counter(&mut zero, "1", "probe-1", "PullRowNum", "reported_base", 0);
    evaluate(&mut zero).expect("evaluation");
    assert_eq!(
        evaluation(&zero, "SR3514.JOIN_PROBE_FANOUT").missing_prerequisites,
        ["probe_input_rows_zero"]
    );

    // A shuffle join (no BROADCAST distribution) has no broadcast exchange.
    let mut shuffle = broadcast_output("positive");
    shuffle.ir.operator_info_facts[0].value = Some("PARTITIONED".into());
    evaluate(&mut shuffle).expect("evaluation");
    assert_eq!(
        evaluation(&shuffle, "SR3514.BROADCAST_EXCHANGE_BYTES").missing_prerequisites,
        ["broadcast_exchange_missing"]
    );

    // A rounded BytesSent interval across 100 MiB is suspected, not triggered.
    let mut rounded = broadcast_output("boundary");
    let bytes = rounded
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| observation.metric_id.as_deref() == Some("BytesSent"))
        .expect("bytes");
    bytes.fidelity = Fidelity::Rounded;
    bytes.bounds = Some(interval((100 * MIB - 512, 100 * MIB + 512)));
    evaluate(&mut rounded).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&rounded, "SR3514.BROADCAST_EXCHANGE_BYTES").status),
        "suspected"
    );

    // An ineffective pre-aggregation under 1,000,000 input rows is suppressed.
    let mut few = signal_base(true);
    add_plan_node(
        &mut few,
        "1",
        "AGGREGATION",
        &[
            ("streaming-sink-1", "AGGREGATE_STREAMING_SINK"),
            ("streaming-source-1", "AGGREGATE_STREAMING_SOURCE"),
        ],
    );
    add_counter(
        &mut few,
        "1",
        "streaming-sink-1",
        "PushRowNum",
        "reported_base",
        1_000,
    );
    add_counter(
        &mut few,
        "1",
        "streaming-source-1",
        "PullRowNum",
        "reported_base",
        1_000,
    );
    evaluate(&mut few).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&few, "SR3514.STREAMING_PREAGG_INEFFECTIVE").status),
        "suppressed"
    );
}

#[test]
fn node_time_share_covers_only_types_without_a_specific_rule() {
    let mut project = share_output_for("PROJECT", Some((40, None)), Some((100, None)));
    project.ir.graph.state = GraphState::Complete;
    evaluate(&mut project).expect("evaluation");
    assert_eq!(
        status_name(&evaluation(&project, "SR3514.NODE_TIME_SHARE").status),
        "triggered"
    );
    let mut aggregate = share_output_for("AGGREGATE", Some((40, None)), Some((100, None)));
    evaluate(&mut aggregate).expect("evaluation");
    assert_eq!(
        evaluation(&aggregate, "SR3514.NODE_TIME_SHARE").missing_prerequisites,
        ["matching_plan_node_missing"]
    );
}

// Ruleset 0.3.0: a recommendation step cites only exact observations of the
// finding's own subject; a metric the subject did not print is left out.
#[test]
fn recommendation_steps_cite_only_exact_metrics_of_their_subject() {
    let mut output = time_case_output("SCAN", "scan_time_positive");
    evaluate(&mut output).expect("rule evaluation");
    let finding = output
        .ir
        .findings
        .iter()
        .find(|finding| finding.rule_id == "SR3514.SCAN_TIME_SHARE")
        .expect("scan finding");
    let recommendation = &finding.recommendation;
    assert_eq!(recommendation.recommendation_id, "rec.scan.reduce_read@v1");
    assert_eq!(
        recommendation.kind,
        doctor_contracts::RecommendationKind::OptimizationTarget
    );
    assert_eq!(recommendation.steps.len(), 2);
    let subject = finding.subject_ref.as_deref().expect("operator subject");
    for reference in recommendation
        .steps
        .iter()
        .flat_map(|step| step.evidence_refs.iter())
    {
        let observation = output
            .ir
            .metric_observations
            .iter()
            .find(|observation| &observation.observation_id == reference)
            .expect("cited observation exists");
        assert_eq!(observation.subject_ref.as_deref(), Some(subject));
        assert_eq!(observation.parse_state, ParseState::Exact);
    }
    // Step 1 reads ScanTime (printed); step 2 reads OutputRows, which this
    // synthetic subject does not print, so it cites nothing.
    assert_eq!(recommendation.steps[0].evidence_refs.len(), 1);
    assert!(recommendation.steps[1].evidence_refs.is_empty());
}
