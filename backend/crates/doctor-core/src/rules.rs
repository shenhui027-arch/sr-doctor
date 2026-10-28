use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use doctor_contracts::{
    CanonicalNumber, DirectionSemantics, EdgeSource, EvidenceCompleteness, ExecutionSummaryState,
    Fidelity, Finding, GraphRelation, GraphState, NumberKind, PlanNodeSummary, Recommendation,
    RecommendationStep, RuleEvaluation, RuleEvaluationStatus, RuntimeInventoryFormatterShape,
    RuntimeSourceRole, SourceQuality, SummaryValueState,
};
use doctor_rules_v0_1::{
    AGGREGATE_TIME_SHARE_RULE_ID, BROADCAST_EXCHANGE_BYTES_RULE_ID,
    EXCHANGE_NETWORK_TIME_SHARE_RULE_ID, JOIN_PROBE_FANOUT_RULE_ID, JOIN_TIME_SHARE_RULE_ID,
    NODE_TIME_SHARE_RULE_ID, OPERATOR_TIME_SKEW_RULE_ID, PRODUCT_RULESET_VERSION,
    ROW_FLOW_AMPLIFICATION_RULE_ID, RULESET_ID, RecommendationTemplate, RuleDefinition,
    RuleSetError, SCAN_TIME_SHARE_RULE_ID, SEMANTIC_MANIFEST_HASH, SPILL_OBSERVED_RULE_ID,
    STREAMING_PREAGG_INEFFECTIVE_RULE_ID, load,
};

use crate::{
    AdapterOutput, CancelSignal, MetricObservation, NormalizedIr, OperatorGroup,
    PortableFindingFact, PortableRuleEvaluationFact,
    execution_summary::{
        PICO_PER_NS, ShareClass, Span, SummaryReading, classify_share, read_execution_summary,
    },
};

#[path = "stored_profile.rs"]
pub(crate) mod stored_profile;

#[path = "rule_signals.rs"]
mod signals;

use stored_profile::{
    OfficialStoredProfileSourceStageProof, final_supported_release,
    official_stored_profile_metric_shape, official_stored_profile_source_stage_proof,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleEvaluationError {
    RuleSet(RuleSetError),
    ReceiptComponentInvalid,
}

#[derive(Clone, Debug)]
struct RuleResult {
    evaluation: RuleEvaluation,
    finding: Option<Finding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Bucket {
    Attention15,
    Attention30,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Confidence {
    Exact,
    Bounded,
    Suspected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IntegerRange {
    lower: i128,
    upper: i128,
    lower_inclusive: bool,
    upper_inclusive: bool,
    exact: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedRange {
    range: IntegerRange,
    observation_ids: Vec<String>,
}

/// Adds only the frozen deterministic v0.1 rules after adapter, version and graph resolution.
/// It never parses raw profile text and leaves the source-bound analysis key unchanged.
pub fn evaluate_rules(output: &mut AdapterOutput) -> Result<(), RuleEvaluationError> {
    let ruleset = load().map_err(RuleEvaluationError::RuleSet)?;
    let scan_rule = required_rule(&ruleset, SCAN_TIME_SHARE_RULE_ID)?;
    let join_rule = required_rule(&ruleset, JOIN_TIME_SHARE_RULE_ID)?;
    let aggregate_rule = required_rule(&ruleset, AGGREGATE_TIME_SHARE_RULE_ID)?;
    let exchange_rule = required_rule(&ruleset, EXCHANGE_NETWORK_TIME_SHARE_RULE_ID)?;
    let spill_rule = required_rule(&ruleset, SPILL_OBSERVED_RULE_ID)?;
    let row_rule = required_rule(&ruleset, ROW_FLOW_AMPLIFICATION_RULE_ID)?;
    let node_rule = required_rule(&ruleset, NODE_TIME_SHARE_RULE_ID)?;
    let skew_rule = required_rule(&ruleset, OPERATOR_TIME_SKEW_RULE_ID)?;
    let fanout_rule = required_rule(&ruleset, JOIN_PROBE_FANOUT_RULE_ID)?;
    let broadcast_rule = required_rule(&ruleset, BROADCAST_EXCHANGE_BYTES_RULE_ID)?;
    let preagg_rule = required_rule(&ruleset, STREAMING_PREAGG_INEFFECTIVE_RULE_ID)?;
    validate_ruleset_receipt(&output.ir.analysis_receipt.components)?;
    let known_refs = output
        .source_refs
        .iter()
        .map(|source| source.ref_id.clone())
        .collect::<BTreeSet<_>>();
    // Rules run between stage boundaries and are not polled (v0.6-08).
    let reading = crate::never_cancelled(read_execution_summary(output, &CancelSignal::never()));
    let mut results = time_results(&output.ir, &reading, &known_refs, scan_rule);
    results.extend(time_results(&output.ir, &reading, &known_refs, join_rule));
    results.extend(time_results(
        &output.ir,
        &reading,
        &known_refs,
        aggregate_rule,
    ));
    results.extend(time_results(
        &output.ir,
        &reading,
        &known_refs,
        exchange_rule,
    ));
    results.extend(spill_results(&output.ir, &known_refs, spill_rule));
    results.extend(row_results(&output.ir, &reading, &known_refs, row_rule));
    results.extend(time_results(&output.ir, &reading, &known_refs, node_rule));
    let context = signals::SignalContext::new(&output.ir, &reading, &known_refs);
    results.extend(signals::skew_results(&context, skew_rule));
    results.extend(signals::fanout_results(&context, fanout_rule));
    results.extend(signals::broadcast_results(&context, broadcast_rule));
    results.extend(signals::preagg_results(&context, preagg_rule));
    for result in &mut results {
        filter_result_evidence(result, &known_refs);
    }
    results.sort_by(|left, right| {
        left.evaluation
            .rule_id
            .cmp(&right.evaluation.rule_id)
            .then(
                left.evaluation
                    .subject_ref
                    .cmp(&right.evaluation.subject_ref),
            )
            .then(left.evaluation.finding_id.cmp(&right.evaluation.finding_id))
    });

    output
        .ir
        .findings
        .retain(|finding| !is_v01_rule(&finding.rule_id));
    output
        .ir
        .rule_evaluations
        .retain(|evaluation| !is_v01_rule(&evaluation.rule_id));
    for result in results {
        if let Some(mut finding) = result.finding {
            if let Some(rule) = ruleset.rule(&finding.rule_id) {
                bind_recommendation_evidence(
                    &mut finding,
                    &rule.recommendation_template,
                    &output.ir.metric_observations,
                );
            }
            output.ir.findings.push(finding);
        }
        output.ir.rule_evaluations.push(result.evaluation);
    }
    refresh_facts(output);
    Ok(())
}

fn required_rule<'a>(
    ruleset: &'a doctor_rules_v0_1::DoctorRuleSet,
    rule_id: &'static str,
) -> Result<&'a RuleDefinition, RuleEvaluationError> {
    ruleset.rule(rule_id).ok_or(RuleEvaluationError::RuleSet(
        RuleSetError::RuleContractMismatch("active_rule_missing"),
    ))
}

fn validate_ruleset_receipt(
    components: &[doctor_contracts::ReceiptComponent],
) -> Result<(), RuleEvaluationError> {
    if components.iter().any(|component| {
        component.id == RULESET_ID
            && component.version == PRODUCT_RULESET_VERSION
            && component.manifest_hash == SEMANTIC_MANIFEST_HASH
    }) {
        Ok(())
    } else {
        Err(RuleEvaluationError::ReceiptComponentInvalid)
    }
}

fn time_results(
    ir: &NormalizedIr,
    reading: &SummaryReading,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    if reading.summary.state != ExecutionSummaryState::Available {
        return vec![not_applicable(
            rule,
            None,
            "execution_summary_unavailable",
            vec![],
        )];
    }
    let nodes = reading
        .summary
        .nodes
        .iter()
        .filter_map(|node| {
            let node_type = ir
                .graph
                .nodes
                .iter()
                .find(|graph_node| graph_node.node_id == node.node_id)?
                .plan_node_type
                .as_deref()?;
            time_rule_applies(rule, node_type).then_some(node)
        })
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return vec![not_applicable(
            rule,
            None,
            "matching_plan_node_missing",
            vec![],
        )];
    }
    nodes
        .into_iter()
        .map(|node| time_for_node(ir, reading, node, known_refs, rule))
        .collect()
}

/// Plan-node types per time-share rule. SCAN and EXCHANGE follow the same
/// type tests as the official NodeInfo special metrics (`ScanTime` for
/// `*_SCAN`, `NetworkTime` for `*EXCHANGE*`).
fn time_rule_applies(rule: &RuleDefinition, node_type: &str) -> bool {
    match rule.rule_id.as_str() {
        SCAN_TIME_SHARE_RULE_ID => node_type.ends_with("_SCAN"),
        JOIN_TIME_SHARE_RULE_ID => is_join_type(node_type),
        AGGREGATE_TIME_SHARE_RULE_ID => matches!(
            node_type,
            "AGGREGATE" | "AGGREGATION" | "GLOBAL_AGGREGATE" | "LOCAL_AGGREGATE"
        ),
        EXCHANGE_NETWORK_TIME_SHARE_RULE_ID => node_type.contains("EXCHANGE"),
        // Every plan node the four type-specific rules do not cover.
        NODE_TIME_SHARE_RULE_ID => [
            SCAN_TIME_SHARE_RULE_ID,
            JOIN_TIME_SHARE_RULE_ID,
            AGGREGATE_TIME_SHARE_RULE_ID,
            EXCHANGE_NETWORK_TIME_SHARE_RULE_ID,
        ]
        .into_iter()
        .all(|specific| !time_rule_applies_to(specific, node_type)),
        _ => false,
    }
}

fn time_rule_applies_to(rule_id: &str, node_type: &str) -> bool {
    match rule_id {
        SCAN_TIME_SHARE_RULE_ID => node_type.ends_with("_SCAN"),
        JOIN_TIME_SHARE_RULE_ID => is_join_type(node_type),
        AGGREGATE_TIME_SHARE_RULE_ID => matches!(
            node_type,
            "AGGREGATE" | "AGGREGATION" | "GLOBAL_AGGREGATE" | "LOCAL_AGGREGATE"
        ),
        EXCHANGE_NETWORK_TIME_SHARE_RULE_ID => node_type.contains("EXCHANGE"),
        _ => false,
    }
}

fn is_join_type(node_type: &str) -> bool {
    matches!(
        node_type,
        "HASH_JOIN" | "MERGE_JOIN" | "NEST_LOOP_JOIN" | "NESTLOOP_JOIN" | "NESTED_LOOP_JOIN"
    )
}

/// The operator group a plan-node finding is located by: the node's producing
/// fragment, i.e. the `operator_rows` entry the browser joins to the graph.
fn summary_subject_group<'a>(
    ir: &'a NormalizedIr,
    node: &PlanNodeSummary,
) -> Option<&'a OperatorGroup> {
    let groups = ir
        .operator_groups
        .iter()
        .filter(|group| group.plan_node_id.as_deref() == Some(node.plan_node_id.as_str()))
        .collect::<Vec<_>>();
    let in_fragment = groups
        .iter()
        .copied()
        .filter(|group| group.fragment_ref == node.fragment_ref)
        .collect::<Vec<_>>();
    match (in_fragment.as_slice(), groups.as_slice()) {
        ([group], _) => Some(*group),
        ([], [group]) if node.fragment_ref.is_none() => Some(*group),
        _ => None,
    }
}

fn time_for_node(
    ir: &NormalizedIr,
    reading: &SummaryReading,
    node: &PlanNodeSummary,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
) -> RuleResult {
    let Some(group) = summary_subject_group(ir, node) else {
        return not_applicable(
            rule,
            None,
            "summary_subject_group_unresolved",
            known_evidence(node.total_time.source_refs.clone(), known_refs),
        );
    };
    let subject = Some(group.operator_group_id.clone());
    let group_evidence = known_evidence(group.source_refs.clone(), known_refs);
    if !final_supported_release(ir) {
        return not_applicable(
            rule,
            subject,
            "version_or_profile_not_eligible",
            group_evidence,
        );
    }
    if !refs_valid(&group.source_refs, known_refs) {
        return not_applicable(
            rule,
            subject,
            "subject_provenance_unverified",
            group_evidence,
        );
    }
    let official_source_stage_proof = official_stored_profile_source_stage_proof(
        ir,
        known_refs,
        matches!(
            ir.runtime_formatter_shape,
            RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514
        ),
    );
    let Some(inputs) = reading.node_time_inputs.get(&node.node_id) else {
        let reason = node
            .total_time
            .reasons
            .first()
            .map_or("node_total_time_unavailable", String::as_str);
        return not_applicable(rule, subject, reason, group_evidence);
    };
    // The summary selected the counters (official NodeInfo recipe); every one
    // must still be a typed, merge-proven, provenance-bound observation. Under
    // the verified 3.5.14 pretty-print grammar a native counter carries no
    // operator_ref and a subordinate one names its own occurrence.
    let roles = ir
        .physical_operator_memberships
        .iter()
        .map(|membership| {
            (
                membership.physical_operator_ref.as_str(),
                &membership.source_role,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for id in &inputs.observation_ids {
        let checked = ir
            .metric_observations
            .iter()
            .find(|observation| observation.observation_id == *id)
            .ok_or("typed_metric_unavailable")
            .and_then(|observation| {
                let metric_id = observation
                    .metric_id
                    .as_deref()
                    .ok_or("typed_metric_unavailable")?;
                if official_source_stage_proof.is_some() {
                    let occurrence = observation.runtime_occurrence_ref.as_deref();
                    let grammar = match occurrence.and_then(|occurrence| roles.get(occurrence)) {
                        Some(RuntimeSourceRole::Native) => observation.operator_ref.is_none(),
                        Some(RuntimeSourceRole::Subordinate) => {
                            observation.operator_ref.as_deref() == occurrence
                        }
                        _ => false,
                    };
                    if !grammar {
                        return Err("operator_binding_unproven");
                    }
                }
                typed_range_with_source_stage_proof(
                    observation,
                    metric_id,
                    "ns",
                    known_refs,
                    official_source_stage_proof.as_ref(),
                )
            });
        if let Err(reason) = checked {
            return not_applicable(rule, subject, reason, group_evidence);
        }
    }
    let denominator = match query_denominator(
        &ir.metric_observations,
        known_refs,
        official_source_stage_proof.as_ref(),
    ) {
        Ok(value) => value,
        Err(reason) => {
            let evidence = evidence_for_observations(
                group_evidence,
                &ir.metric_observations,
                &inputs.observation_ids,
                known_refs,
            );
            return not_applicable_with_inputs(
                rule,
                subject,
                reason,
                inputs.observation_ids.clone(),
                evidence,
            );
        }
    };
    let subject_time = inputs.span;
    let mut input_refs = inputs.observation_ids.clone();
    input_refs.extend(denominator.observation_ids);
    let input_refs = ordered_unique(input_refs);
    let mut base_evidence = group_evidence;
    base_evidence.extend(node.total_time.source_refs.iter().cloned());
    if let Some(proof) = &official_source_stage_proof {
        base_evidence.extend(proof.evidence_refs.iter().cloned());
    }
    let evidence = evidence_for_observations(
        base_evidence,
        &ir.metric_observations,
        &input_refs,
        known_refs,
    );
    let Some(denominator_span) = pico_span(denominator.range) else {
        return not_applicable_with_inputs(
            rule,
            subject,
            "ratio_comparison_overflow",
            input_refs,
            evidence,
        );
    };
    let Some(class) = classify_share(subject_time, denominator_span) else {
        return not_applicable_with_inputs(
            rule,
            subject,
            "invalid_denominator",
            input_refs,
            evidence,
        );
    };
    let range_confidence = if subject_time.is_point() && denominator_span.is_point() {
        Confidence::Exact
    } else {
        Confidence::Bounded
    };
    let (status, bucket, comparison, mut confidence) = match class {
        ShareClass::Above30 => (
            RuleEvaluationStatus::Triggered,
            Some(Bucket::Attention30),
            "ratio_all_strictly_gt_30_percent",
            Some(range_confidence),
        ),
        ShareClass::Boundary30 => (
            RuleEvaluationStatus::Suspected,
            Some(Bucket::Attention30),
            "ratio_intersects_30_percent_boundary",
            Some(Confidence::Suspected),
        ),
        ShareClass::Above15 => (
            RuleEvaluationStatus::Triggered,
            Some(Bucket::Attention15),
            "ratio_all_strictly_gt_15_percent",
            Some(range_confidence),
        ),
        ShareClass::Boundary15 => (
            RuleEvaluationStatus::Suspected,
            Some(Bucket::Attention15),
            "ratio_intersects_15_percent_boundary",
            Some(Confidence::Suspected),
        ),
        ShareClass::NotAbove15 => (
            RuleEvaluationStatus::NotTriggered,
            None,
            "ratio_all_not_greater_than_15_percent",
            Some(range_confidence),
        ),
    };
    if ir.graph.state == GraphState::Partial && confidence == Some(Confidence::Exact) {
        confidence = Some(Confidence::Bounded);
    }
    if official_source_stage_proof.is_some() && confidence == Some(Confidence::Exact) {
        confidence = Some(Confidence::Bounded);
    }
    let mut thresholds = vec![
        "sr3514.operator_time_share@1".into(),
        "strict_gt:15%".into(),
        "strict_gt:30%".into(),
    ];
    // Product decision (ruleset 0.1.5): a share finding on a subject whose whole
    // time range is below the absolute floor is kept as a visible, suppressed
    // evaluation instead of a finding. A range touching the floor keeps the finding.
    let mut comparisons = vec![comparison.to_owned()];
    let (status, bucket) = match &rule.absolute_floor {
        Some(floor) => {
            thresholds.push(format!(
                "absolute_floor:{}<{}ns:{}",
                floor.input, floor.suppress_below_ns, floor.source
            ));
            if matches!(
                status,
                RuleEvaluationStatus::Triggered | RuleEvaluationStatus::Suspected
            ) && floor
                .suppress_below_ns
                .checked_mul(PICO_PER_NS)
                .is_some_and(|floor_pico| entirely_below(subject_time, floor_pico))
            {
                comparisons.push(format!(
                    "subject_time_all_below_absolute_floor_{}ns",
                    floor.suppress_below_ns
                ));
                (RuleEvaluationStatus::Suppressed, None)
            } else {
                (status, bucket)
            }
        }
        None => (status, bucket),
    };
    // The finding's share and time are the execution summary's own display of
    // the same interval the rule classified.
    let measurements = [
        signals::measurement(
            "node_time_share",
            node.time_share_display.clone(),
            bucket.map(|bucket| match bucket {
                Bucket::Attention30 => "> 30%".to_owned(),
                Bucket::Attention15 => "> 15%".to_owned(),
            }),
            None,
            &input_refs,
        ),
        signals::measurement(
            "node_time",
            node.total_time.display_text.clone(),
            None,
            None,
            &inputs.observation_ids,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let mut result = result_with_finding(
        rule,
        subject,
        vec![
            "version_profile:final_supported_3514".into(),
            "subject:execution_summary_plan_node_producing_fragment_group".into(),
            "node_time:execution_summary_official_node_info_recipe".into(),
            "typed_metrics:duration_ns_dimension_scope_provenance_fidelity_verified".into(),
            "stored_profile_merge:level1_final_merged_verified".into(),
            if official_source_stage_proof.is_some() {
                "source_counter_stage:official_3514_stored_pipeline_counter_stage_complete"
            } else {
                "source_counter_stage:observation_merge_state_verified"
            }
            .into(),
            "query_denominator:unique_positive_nonzero".into(),
            "share_class:shared_with_execution_summary_hotspot".into(),
        ],
        status,
        input_refs,
        comparisons,
        thresholds,
        vec![],
        evidence,
        bucket,
        confidence,
    );
    if let Some(finding) = &mut result.finding {
        finding.measurements = measurements;
    }
    if official_source_stage_proof.is_some() {
        let limitation = "source_merge_state_unknown_not_promoted; source counter stage and MAX-counter selection are verified-derived from the official 3.5.14 stored pipeline Profile build recipe; a missing __MAX_OF_ counter is consumed only under the official redundant base=min=max removal contract".to_owned();
        result.evaluation.limitations.push(limitation.clone());
        if let Some(finding) = &mut result.finding {
            finding.limitations.push(limitation);
        }
    }
    result
}

/// A nanosecond integer range as a picosecond span.
fn pico_span(range: IntegerRange) -> Option<Span> {
    Some(Span {
        lower: range.lower.checked_mul(PICO_PER_NS)?,
        upper: range.upper.checked_mul(PICO_PER_NS)?,
        lower_inclusive: range.lower_inclusive,
        upper_inclusive: range.upper_inclusive,
    })
}

fn spill_results(
    ir: &NormalizedIr,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    let candidates = ir
        .metric_observations
        .iter()
        .filter(|observation| observation.metric_id.as_deref() == Some("QuerySpillBytes"))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return vec![not_applicable(
            rule,
            None,
            "spill_observation_missing",
            vec![],
        )];
    }
    let mut grouped = BTreeMap::<String, Vec<&MetricObservation>>::new();
    for observation in candidates {
        grouped
            .entry(spill_subject_key(observation))
            .or_default()
            .push(observation);
    }
    grouped
        .into_values()
        .map(|observations| {
            if observations.len() != 1 {
                let observation = observations[0];
                not_applicable(
                    rule,
                    spill_subject(observation),
                    "spill_observation_ambiguous",
                    observation.source_refs.clone(),
                )
            } else {
                spill_for_observation(ir, observations[0], known_refs, rule)
            }
        })
        .collect()
}

fn spill_subject_key(observation: &MetricObservation) -> String {
    spill_subject(observation)
        .map(|subject| format!("operator:{subject}"))
        .unwrap_or_else(|| "query_or_invalid".into())
}

fn spill_subject(observation: &MetricObservation) -> Option<String> {
    (observation.scope == "operator"
        && observation
            .subject_ref
            .as_deref()
            .is_some_and(|subject| !subject.trim().is_empty())
        && observation
            .operator_ref
            .as_deref()
            .is_some_and(|operator| !operator.trim().is_empty()))
    .then(|| observation.subject_ref.clone())
    .flatten()
}

fn spill_for_observation(
    ir: &NormalizedIr,
    observation: &MetricObservation,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
) -> RuleResult {
    let subject = spill_subject(observation);
    let mut evidence = observation.source_refs.clone();
    let mut applicability_checks = vec!["version_profile:final_supported_3514".into()];
    if !final_supported_release(ir) {
        return not_applicable(
            rule,
            subject,
            "version_or_profile_not_eligible",
            observation.source_refs.clone(),
        );
    }
    let operator_scoped = observation.scope == "operator";
    if observation.scope != "query" && !operator_scoped {
        return not_applicable(
            rule,
            subject,
            "spill_scope_not_registered",
            observation.source_refs.clone(),
        );
    }
    if operator_scoped {
        if observation.subject_ref.as_deref().is_none()
            || observation.operator_ref.as_deref().is_none()
        {
            return not_applicable(
                rule,
                subject,
                "operator_binding_unverified",
                observation.source_refs.clone(),
            );
        }
        if ir.graph.state == GraphState::Unavailable {
            return not_applicable(
                rule,
                subject,
                "operator_subject_unavailable",
                observation.source_refs.clone(),
            );
        }
        let matching_groups = observation
            .subject_ref
            .as_ref()
            .map(|reference| {
                ir.operator_groups
                    .iter()
                    .filter(|group| group.operator_group_id == *reference)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let group = match matching_groups.as_slice() {
            [group] => *group,
            [] => {
                return not_applicable(
                    rule,
                    subject,
                    "operator_subject_unavailable",
                    observation.source_refs.clone(),
                );
            }
            _ => {
                return not_applicable(
                    rule,
                    subject,
                    "operator_subject_ambiguous",
                    observation.source_refs.clone(),
                );
            }
        };
        if operator_binding(group, observation).is_err()
            || !refs_valid(&group.source_refs, known_refs)
        {
            return not_applicable(
                rule,
                subject,
                "operator_binding_unverified",
                observation.source_refs.clone(),
            );
        }
        evidence.extend(group.source_refs.clone());
        applicability_checks.push("graph_subject:operator_available".into());
        applicability_checks.push("operator_binding:verified".into());
    } else if observation
        .subject_ref
        .as_deref()
        .is_some_and(|value| value != "query")
        || observation.operator_ref.is_some()
    {
        // Query metrics carry subject_ref "query" (as query_denominator accepts); only an
        // operator subject or binding is forbidden for the query-scope spill counter.
        return not_applicable(
            rule,
            subject,
            "query_spill_subject_or_operator_binding_forbidden",
            observation.source_refs.clone(),
        );
    } else {
        applicability_checks.push("graph_subject:query_spill_graph_not_required".into());
        applicability_checks.push("operator_binding:query_operator_ref_absent".into());
    }
    let source_stage_proof = (!operator_scoped)
        .then(|| {
            official_stored_profile_source_stage_proof(
                ir,
                known_refs,
                matches!(
                    ir.runtime_formatter_shape,
                    RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514
                ),
            )
        })
        .flatten();
    let range = match typed_range_with_source_stage_proof(
        observation,
        "QuerySpillBytes",
        "byte",
        known_refs,
        source_stage_proof.as_ref(),
    ) {
        Ok(range) => range,
        Err(reason) => {
            return not_applicable(rule, subject, reason, observation.source_refs.clone());
        }
    };
    let (status, comparison, confidence) = if range.exact && range.lower == 0 {
        (
            RuleEvaluationStatus::NotTriggered,
            "canonical_exact_zero".into(),
            None,
        )
    } else if range.lower > 0 {
        let confidence = if range.exact {
            Confidence::Exact
        } else {
            Confidence::Bounded
        };
        (
            RuleEvaluationStatus::Triggered,
            "canonical_or_lower_bound_gt_zero".into(),
            Some(confidence),
        )
    } else if contains_zero_by_closure(range) {
        (
            RuleEvaluationStatus::Suspected,
            "interval_closure_contains_zero".into(),
            Some(Confidence::Suspected),
        )
    } else {
        return not_applicable(
            rule,
            subject,
            "spill_range_not_nonnegative",
            observation.source_refs.clone(),
        );
    };
    result_with_finding(
        rule,
        subject,
        {
            applicability_checks.push(
                "typed_metric:bytes_dimension_scope_merge_provenance_fidelity_verified".into(),
            );
            applicability_checks
        },
        status,
        vec![observation.observation_id.clone()],
        vec![comparison],
        vec![
            "sr3514.spill_observed@1".into(),
            "exact_gt_zero_or_lower_bound_gt_zero".into(),
        ],
        vec![],
        evidence,
        None,
        confidence,
    )
}

fn row_results(
    ir: &NormalizedIr,
    reading: &SummaryReading,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    if reading.summary.state != ExecutionSummaryState::Available {
        return vec![not_applicable(
            rule,
            None,
            "execution_summary_unavailable",
            vec![],
        )];
    }
    let joins = reading
        .summary
        .nodes
        .iter()
        .filter(|node| {
            graph_node(ir, &node.node_id)
                .and_then(|graph_node| graph_node.plan_node_type.as_deref())
                .is_some_and(is_join_type)
        })
        .collect::<Vec<_>>();
    if joins.is_empty() {
        return vec![not_applicable(rule, None, "join_operator_missing", vec![])];
    }
    joins
        .into_iter()
        .map(|node| row_for_join_node(ir, reading, known_refs, rule, node))
        .collect()
}

fn graph_node<'a>(ir: &'a NormalizedIr, node_id: &str) -> Option<&'a doctor_contracts::GraphNode> {
    ir.graph.nodes.iter().find(|node| node.node_id == node_id)
}

/// Parent rows over the sum of the direct children's rows, all read from the
/// execution summary (official NodeInfo `OutputRows`). The children are the
/// complete set of explicit official Topology `PlanChild` edges of the node.
fn row_for_join_node(
    ir: &NormalizedIr,
    reading: &SummaryReading,
    known_refs: &BTreeSet<String>,
    rule: &RuleDefinition,
    node: &PlanNodeSummary,
) -> RuleResult {
    let Some(group) = summary_subject_group(ir, node) else {
        return not_applicable(
            rule,
            None,
            "summary_subject_group_unresolved",
            known_evidence(node.output_rows.source_refs.clone(), known_refs),
        );
    };
    let subject = Some(group.operator_group_id.clone());
    let parent_evidence = known_evidence(group.source_refs.clone(), known_refs);
    if !final_supported_release(ir) {
        return not_applicable(
            rule,
            subject,
            "version_or_profile_not_eligible",
            parent_evidence,
        );
    }
    if ir.graph.state != GraphState::Complete {
        return not_applicable(
            rule,
            subject,
            "graph_direct_child_closure_incomplete",
            parent_evidence,
        );
    }
    let Some(parent_node) = graph_node(ir, &node.node_id).filter(|parent| {
        parent.source_quality == SourceQuality::Exact && refs_valid(&parent.source_refs, known_refs)
    }) else {
        return not_applicable(
            rule,
            subject,
            "graph_subject_provenance_unverified",
            parent_evidence,
        );
    };
    if !refs_valid(&group.source_refs, known_refs) {
        return not_applicable(
            rule,
            subject,
            "subject_provenance_unverified",
            parent_evidence,
        );
    }
    if ir.graph.edges.iter().any(|edge| {
        edge.relation == GraphRelation::VerifiedDataFlow
            && (edge.from == parent_node.node_id || edge.to == parent_node.node_id)
    }) {
        return not_applicable(
            rule,
            subject,
            "graph_direct_child_relation_unregistered",
            parent_evidence,
        );
    }
    let structural_edges = ir
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.relation == GraphRelation::PlanChild && edge.from == parent_node.node_id
        })
        .collect::<Vec<_>>();
    if structural_edges.is_empty() {
        return not_applicable(rule, subject, "direct_child_missing", parent_evidence);
    }
    if structural_edges
        .iter()
        .any(|edge| !trusted_row_edge(edge, known_refs))
    {
        return not_applicable(
            rule,
            subject,
            "graph_direct_child_closure_unverified",
            parent_evidence,
        );
    }
    let direct_children = structural_edges
        .iter()
        .map(|edge| edge.to.as_str())
        .collect::<Vec<_>>();
    if direct_children
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .len()
        != direct_children.len()
    {
        return not_applicable(
            rule,
            subject,
            "graph_direct_child_duplicate_endpoint",
            parent_evidence,
        );
    }
    let mut evidence = parent_evidence;
    evidence.extend(
        structural_edges
            .iter()
            .flat_map(|edge| edge.evidence_refs.iter().cloned()),
    );
    let parent_rows = match verified_summary_rows(ir, reading, node, known_refs) {
        Ok(value) => value,
        Err(reason) => return not_applicable(rule, subject, reason, evidence),
    };
    let mut input_refs = parent_rows.observation_ids.clone();
    let mut child_sum = 0_i128;
    for node_id in direct_children {
        let Some(child_node) = graph_node(ir, node_id) else {
            return not_applicable_with_inputs(
                rule,
                subject,
                "graph_child_node_missing",
                input_refs,
                evidence,
            );
        };
        if child_node.source_quality != SourceQuality::Exact
            || !refs_valid(&child_node.source_refs, known_refs)
        {
            return not_applicable_with_inputs(
                rule,
                subject,
                "direct_child_provenance_unverified",
                input_refs,
                evidence,
            );
        }
        let Some(child) = reading
            .summary
            .nodes
            .iter()
            .find(|summary| summary.node_id == child_node.node_id)
        else {
            return not_applicable_with_inputs(
                rule,
                subject,
                "direct_child_summary_missing",
                input_refs,
                evidence,
            );
        };
        let rows = match verified_summary_rows(ir, reading, child, known_refs) {
            Ok(value) => value,
            Err(reason) => {
                return not_applicable_with_inputs(rule, subject, reason, input_refs, evidence);
            }
        };
        child_sum = match child_sum.checked_add(rows.range.lower) {
            Some(value) => value,
            None => {
                return not_applicable_with_inputs(
                    rule,
                    subject,
                    "child_output_rows_sum_overflow",
                    input_refs,
                    evidence,
                );
            }
        };
        input_refs.extend(rows.observation_ids);
    }
    let input_refs = ordered_unique(input_refs);
    let evidence =
        evidence_for_observations(evidence, &ir.metric_observations, &input_refs, known_refs);
    if child_sum <= 0 {
        return not_applicable_with_inputs(
            rule,
            subject,
            "child_output_rows_nonpositive",
            input_refs,
            evidence,
        );
    }
    let status = match compare_fraction(parent_rows.range.lower, child_sum, 10, 1) {
        Some(Ordering::Greater) => RuleEvaluationStatus::Triggered,
        Some(_) => RuleEvaluationStatus::NotTriggered,
        None => {
            return not_applicable_with_inputs(
                rule,
                subject,
                "row_amplification_comparison_overflow",
                input_refs,
                evidence,
            );
        }
    };
    let child_rows = IntegerRange {
        lower: child_sum,
        upper: child_sum,
        lower_inclusive: true,
        upper_inclusive: true,
        exact: true,
    };
    let measurements = [
        signals::measurement(
            "output_over_child_output_rows",
            signals::ratio_text(parent_rows.range, child_rows, 1, "×"),
            Some("> 10×".into()),
            None,
            &input_refs,
        ),
        signals::measurement(
            "node_output_rows",
            Some(signals::rows_text(parent_rows.range)),
            None,
            None,
            &parent_rows.observation_ids,
        ),
        signals::measurement(
            "child_output_rows",
            Some(signals::rows_text(child_rows)),
            None,
            None,
            &input_refs
                .iter()
                .filter(|id| !parent_rows.observation_ids.contains(id))
                .cloned()
                .collect::<Vec<_>>(),
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let mut result = result_with_finding(
        rule,
        subject,
        vec![
            "version_profile:final_supported_3514".into(),
            "graph:complete_direct_child_closure".into(),
            "graph_edge:exact_explicit_plan_child_parent_to_child".into(),
            "subject:execution_summary_plan_node_producing_fragment_group".into(),
            "typed_metrics:exact_output_rows_recipe_provenance_verified".into(),
        ],
        status,
        input_refs,
        vec!["parent_output_rows_over_direct_child_sum_strict_gt_10".into()],
        vec![
            "sr3514.row_flow_amplification@1".into(),
            "strict_gt:10".into(),
        ],
        vec![],
        evidence,
        None,
        Some(Confidence::Exact),
    );
    if let Some(finding) = &mut result.finding {
        finding.measurements = measurements;
    }
    result
}

fn trusted_row_edge(edge: &doctor_contracts::GraphEdge, known_refs: &BTreeSet<String>) -> bool {
    edge.quality == SourceQuality::Exact
        && refs_valid(&edge.evidence_refs, known_refs)
        && edge.relation == GraphRelation::PlanChild
        && edge.direction_semantics == DirectionSemantics::ParentToChild
        && edge.source == EdgeSource::Explicit
        && edge.derivation_recipe.is_none()
}

/// The exact summary `OutputRows` of a node, backed by one registered
/// version-pack recipe observation.
fn verified_summary_rows(
    ir: &NormalizedIr,
    reading: &SummaryReading,
    node: &PlanNodeSummary,
    known_refs: &BTreeSet<String>,
) -> Result<SelectedRange, &'static str> {
    if node.output_rows.state != SummaryValueState::Exact {
        return Err("output_rows_unavailable");
    }
    if node.output_rows.bounds.is_some() {
        return Err("output_rows_not_exact");
    }
    let observation = reading
        .node_output_rows_inputs
        .get(&node.node_id)
        .and_then(|id| {
            ir.metric_observations
                .iter()
                .find(|observation| observation.observation_id == *id)
        })
        .filter(|observation| {
            observation.metric_id.as_deref() == Some("OutputRows")
                && observation.operator_ref.is_none()
                && observation.strategy_origin == "version_pack"
                && observation.value_role == "derived"
                && is_registered_output_rows_recipe(observation.definition_ref.as_deref())
                && ir.operator_groups.iter().any(|group| {
                    observation.subject_ref.as_deref() == Some(group.operator_group_id.as_str())
                        && group
                            .metric_observation_refs
                            .contains(&observation.observation_id)
                })
        })
        .ok_or("output_rows_unavailable")?;
    let range = typed_range(observation, "OutputRows", "item", known_refs)?;
    range
        .exact
        .then_some(SelectedRange {
            range,
            observation_ids: vec![observation.observation_id.clone()],
        })
        .ok_or("output_rows_not_exact")
}

fn is_registered_output_rows_recipe(definition_ref: Option<&str>) -> bool {
    matches!(
        definition_ref,
        Some(
            "sr3514.output_rows.sink@1"
                | "sr3514.output_rows.union@1"
                | "sr3514.output_rows.general@1"
                | "sr3514.output_rows.broadcast_exchange@1"
        )
    )
}

fn operator_binding(
    group: &OperatorGroup,
    observation: &MetricObservation,
) -> Result<(), &'static str> {
    let Some(operator_ref) = observation.operator_ref.as_deref() else {
        return Err("operator_ref_missing");
    };
    let Some(ordinal) = observation.source_ordinal else {
        return Err("operator_ordinal_missing");
    };
    if !group
        .metric_observation_refs
        .iter()
        .any(|reference| reference == &observation.observation_id)
    {
        return Err("operator_metric_binding_unverified");
    }
    let native = group
        .native_operator_refs
        .iter()
        .filter(|(reference, _)| reference == operator_ref)
        .collect::<Vec<_>>();
    let subordinate = group
        .subordinate_operator_refs
        .iter()
        .filter(|(reference, _)| reference == operator_ref)
        .collect::<Vec<_>>();
    if native.len() + subordinate.len() != 1
        || native
            .iter()
            .chain(subordinate.iter())
            .any(|(_, member_ordinal)| *member_ordinal != ordinal)
    {
        return Err("operator_binding_ambiguous");
    }
    Ok(())
}

fn query_denominator(
    observations: &[MetricObservation],
    known_refs: &BTreeSet<String>,
    source_stage_proof: Option<&OfficialStoredProfileSourceStageProof>,
) -> Result<SelectedRange, &'static str> {
    let candidates = observations
        .iter()
        .filter(|observation| {
            observation.metric_id.as_deref() == Some("QueryCumulativeOperatorTime")
                && observation.scope == "query"
                && observation.value_role == "reported_base"
                && observation.strategy_origin == "registry"
                && observation.counter_strategy.is_none()
                && observation
                    .subject_ref
                    .as_deref()
                    .is_none_or(|subject| subject == "query")
                && observation.operator_ref.is_none()
                && observation.runtime_occurrence_ref.is_none()
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err("query_denominator_unavailable");
    }
    Ok(SelectedRange {
        range: typed_range_with_source_stage_proof(
            candidates[0],
            "QueryCumulativeOperatorTime",
            "ns",
            known_refs,
            source_stage_proof,
        )?,
        observation_ids: vec![candidates[0].observation_id.clone()],
    })
}

fn typed_range(
    observation: &MetricObservation,
    metric_id: &str,
    unit: &str,
    known_refs: &BTreeSet<String>,
) -> Result<IntegerRange, &'static str> {
    typed_range_with_source_stage_proof(observation, metric_id, unit, known_refs, None)
}

fn typed_range_with_source_stage_proof(
    observation: &MetricObservation,
    metric_id: &str,
    unit: &str,
    known_refs: &BTreeSet<String>,
    source_stage_proof: Option<&OfficialStoredProfileSourceStageProof>,
) -> Result<IntegerRange, &'static str> {
    let merge_is_proven = observation.source_merge_state == "merged"
        || (source_stage_proof.is_some()
            && official_stored_profile_metric_shape(observation, metric_id));
    if observation.metric_id.as_deref() != Some(metric_id)
        || observation.parse_state != doctor_contracts::ParseState::Exact
        || observation.semantic_state != "known"
        || observation.canonical_unit.as_deref() != Some(unit)
        || !merge_is_proven
        || !metric_dimension_and_scope_valid(observation, metric_id, unit)
        || (metric_id == "OutputRows"
            && !is_registered_output_rows_recipe(observation.definition_ref.as_deref()))
        || (metric_id != "OutputRows"
            && observation.definition_ref.as_deref()
                != Some(&format!("sr3514-metric-registry:{metric_id}")))
        || !refs_valid(&observation.source_refs, known_refs)
    {
        return Err("typed_metric_unavailable");
    }
    let range = match (&observation.fidelity, &observation.bounds) {
        (Fidelity::Exact, None) => {
            let value = observation
                .canonical_number
                .as_ref()
                .and_then(integer_value)
                .ok_or("typed_metric_invalid")?;
            IntegerRange {
                lower: value,
                upper: value,
                lower_inclusive: true,
                upper_inclusive: true,
                exact: true,
            }
        }
        // Rounded display values (e.g. "43.512 MB") have decimal canonical bounds; widen them
        // outward to integers so the interval still contains the true value.
        (Fidelity::Rounded | Fidelity::Truncated | Fidelity::Interval, Some(bounds)) => {
            let (lower, lower_integral) =
                decimal_floor(&bounds.lower).ok_or("typed_metric_invalid")?;
            let (upper, upper_integral) =
                decimal_ceil(&bounds.upper).ok_or("typed_metric_invalid")?;
            IntegerRange {
                lower,
                upper,
                lower_inclusive: bounds.lower_inclusive || !lower_integral,
                upper_inclusive: bounds.upper_inclusive || !upper_integral,
                exact: false,
            }
        }
        _ => return Err("typed_metric_fidelity_unavailable"),
    };
    if range.lower < 0
        || range.upper < range.lower
        || (range.upper == range.lower && (!range.lower_inclusive || !range.upper_inclusive))
    {
        return Err("typed_metric_range_invalid");
    }
    Ok(range)
}

/// Floor of a canonical decimal, and whether it was already integral.
fn decimal_floor(number: &CanonicalNumber) -> Option<(i128, bool)> {
    decimal_parts(number).map(|(whole, fraction_nonzero, negative)| {
        if fraction_nonzero && negative {
            (whole - 1, false)
        } else {
            (whole, !fraction_nonzero)
        }
    })
}

/// Ceiling of a canonical decimal, and whether it was already integral.
fn decimal_ceil(number: &CanonicalNumber) -> Option<(i128, bool)> {
    decimal_parts(number).map(|(whole, fraction_nonzero, negative)| {
        if fraction_nonzero && !negative {
            (whole + 1, false)
        } else {
            (whole, !fraction_nonzero)
        }
    })
}

/// (truncated integer part, any non-zero fraction digit, negative) of an integer or decimal.
fn decimal_parts(number: &CanonicalNumber) -> Option<(i128, bool, bool)> {
    if let Some(value) = integer_value(number) {
        return Some((value, false, value < 0));
    }
    if number.kind != NumberKind::Decimal {
        return None;
    }
    let (whole, fraction) = number.value.split_once('.')?;
    let digits = whole.strip_prefix('-').unwrap_or(whole);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_empty()
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let negative = whole.starts_with('-');
    let magnitude = digits.parse::<i128>().ok()?;
    Some((
        if negative { -magnitude } else { magnitude },
        fraction.bytes().any(|byte| byte != b'0'),
        negative,
    ))
}

fn integer_value(number: &CanonicalNumber) -> Option<i128> {
    (number.kind == NumberKind::Integer
        && number.scale.is_none()
        && !number.value.is_empty()
        && number
            .value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
        && number.value.matches('-').count() <= 1
        && !number.value.starts_with('+'))
    .then(|| number.value.parse().ok())
    .flatten()
}

fn metric_dimension_and_scope_valid(
    observation: &MetricObservation,
    metric_id: &str,
    unit: &str,
) -> bool {
    let (dimension, scope) = match metric_id {
        "QueryCumulativeOperatorTime" => ("duration", observation.scope == "query"),
        "QuerySpillBytes" => (
            "bytes",
            observation.scope == "query" || observation.scope == "operator",
        ),
        "OperatorTotalTime" | "NetworkTime" | "ScanTime" => (
            "duration",
            observation.scope == "operator" || observation.scope == "driver",
        ),
        "OutputRows" | "PushRowNum" | "PullRowNum" => ("count", observation.scope == "operator"),
        "BytesSent" => ("bytes", observation.scope == "operator"),
        _ => return false,
    };
    observation.dimension == dimension
        && matches!(
            (unit, dimension),
            ("ns", "duration") | ("byte", "bytes") | ("item", "count")
        )
        && scope
}

// True when every value of `range` is strictly below `floor`.
fn entirely_below(range: Span, floor: i128) -> bool {
    range.upper < floor || (range.upper == floor && !range.upper_inclusive)
}

fn compare_fraction(
    left_numerator: i128,
    left_denominator: i128,
    right_numerator: i128,
    right_denominator: i128,
) -> Option<Ordering> {
    left_numerator
        .checked_mul(right_denominator)?
        .cmp(&right_numerator.checked_mul(left_denominator)?)
        .into()
}

fn contains_zero_by_closure(range: IntegerRange) -> bool {
    range.lower <= 0 && range.upper >= 0
}

fn refs_valid(refs: &[String], known_refs: &BTreeSet<String>) -> bool {
    !refs.is_empty()
        && refs
            .iter()
            .all(|reference| !reference.trim().is_empty() && known_refs.contains(reference))
}

fn evidence_for_observations(
    mut evidence: Vec<String>,
    observations: &[MetricObservation],
    ids: &[String],
    known_refs: &BTreeSet<String>,
) -> Vec<String> {
    for observation in observations
        .iter()
        .filter(|observation| ids.iter().any(|id| id == &observation.observation_id))
    {
        evidence.extend(observation.source_refs.clone());
    }
    known_evidence(evidence, known_refs)
}

fn known_evidence(values: Vec<String>, known_refs: &BTreeSet<String>) -> Vec<String> {
    ordered_unique(
        values
            .into_iter()
            .filter(|value| known_refs.contains(value))
            .collect(),
    )
}

fn filter_result_evidence(result: &mut RuleResult, known_refs: &BTreeSet<String>) {
    result.evaluation.evidence_refs = known_evidence(
        std::mem::take(&mut result.evaluation.evidence_refs),
        known_refs,
    );
    if let Some(finding) = result.finding.as_mut() {
        finding.evidence = known_evidence(std::mem::take(&mut finding.evidence), known_refs);
    }
}

fn ordered_unique(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn rule_threshold_receipts(mut receipts: Vec<String>) -> Vec<String> {
    let mut chain = vec![
        format!("ruleset:{RULESET_ID}@{PRODUCT_RULESET_VERSION}#{SEMANTIC_MANIFEST_HASH}"),
        "evaluator:doctor-core-rules@0.1.0".into(),
    ];
    chain.append(&mut receipts);
    ordered_unique(chain)
}

fn not_applicable(
    rule: &RuleDefinition,
    subject_ref: Option<String>,
    reason: &str,
    evidence_refs: Vec<String>,
) -> RuleResult {
    not_applicable_with_inputs(rule, subject_ref, reason, vec![], evidence_refs)
}

fn not_applicable_with_inputs(
    rule: &RuleDefinition,
    subject_ref: Option<String>,
    reason: &str,
    input_observation_refs: Vec<String>,
    evidence_refs: Vec<String>,
) -> RuleResult {
    RuleResult {
        evaluation: RuleEvaluation {
            rule_id: rule.rule_id.clone(),
            rule_version: rule.version.clone(),
            status: RuleEvaluationStatus::NotApplicable,
            evidence_completeness: EvidenceCompleteness::Unavailable,
            evidence_completeness_reasons: vec![reason.into()],
            subject_ref,
            applicability_checks: vec![format!("applicability_failed:{reason}")],
            input_observation_refs,
            comparisons: vec![],
            threshold_receipts: rule_threshold_receipts(vec![]),
            missing_prerequisites: vec![reason.into()],
            evidence_refs: ordered_unique(evidence_refs),
            limitations: rule.limitations.clone(),
            finding_id: None,
        },
        finding: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn result_with_finding(
    rule: &RuleDefinition,
    subject_ref: Option<String>,
    applicability_checks: Vec<String>,
    status: RuleEvaluationStatus,
    input_observation_refs: Vec<String>,
    comparisons: Vec<String>,
    threshold_receipts: Vec<String>,
    missing_prerequisites: Vec<String>,
    evidence_refs: Vec<String>,
    bucket: Option<Bucket>,
    confidence: Option<Confidence>,
) -> RuleResult {
    let completeness = evidence_completeness(&status, confidence);
    let completeness_reasons = evidence_completeness_reasons(&status, confidence);
    let finding = matches!(
        status,
        RuleEvaluationStatus::Triggered | RuleEvaluationStatus::Suspected
    )
    .then(|| {
        finding(
            rule,
            subject_ref.clone(),
            bucket,
            confidence,
            evidence_refs.clone(),
        )
    });
    RuleResult {
        evaluation: RuleEvaluation {
            rule_id: rule.rule_id.clone(),
            rule_version: rule.version.clone(),
            status,
            evidence_completeness: completeness,
            evidence_completeness_reasons: completeness_reasons,
            subject_ref,
            applicability_checks,
            input_observation_refs,
            comparisons,
            threshold_receipts: rule_threshold_receipts(threshold_receipts),
            missing_prerequisites,
            evidence_refs: ordered_unique(evidence_refs),
            limitations: rule.limitations.clone(),
            finding_id: finding.as_ref().map(|finding| finding.finding_id.clone()),
        },
        finding,
    }
}

fn finding(
    rule: &RuleDefinition,
    subject_ref: Option<String>,
    bucket: Option<Bucket>,
    confidence: Option<Confidence>,
    evidence: Vec<String>,
) -> Finding {
    let completeness_confidence = confidence.unwrap_or(Confidence::Suspected);
    let subject = subject_ref.as_deref().unwrap_or("query");
    let bucket_name = match bucket {
        Some(Bucket::Attention15) => "attention_15",
        Some(Bucket::Attention30) => "attention_30",
        None if rule.rule_id == SPILL_OBSERVED_RULE_ID => "spill_observed",
        None if rule.rule_id == ROW_FLOW_AMPLIFICATION_RULE_ID => "row_flow_amplification",
        None if rule.rule_id == OPERATOR_TIME_SKEW_RULE_ID => "operator_time_skew",
        None if rule.rule_id == JOIN_PROBE_FANOUT_RULE_ID => "join_probe_fanout",
        None if rule.rule_id == BROADCAST_EXCHANGE_BYTES_RULE_ID => "broadcast_exchange_bytes",
        None if rule.rule_id == STREAMING_PREAGG_INEFFECTIVE_RULE_ID => {
            "streaming_preagg_ineffective"
        }
        None => "unbucketed",
    };
    let severity = match bucket {
        Some(Bucket::Attention15) => rule
            .finding_policy
            .bucket_severity
            .as_ref()
            .map(|policy| policy.attention_15.as_str()),
        Some(Bucket::Attention30) => rule
            .finding_policy
            .bucket_severity
            .as_ref()
            .map(|policy| policy.attention_30.as_str()),
        None => rule.finding_policy.severity.as_deref(),
    }
    .unwrap_or("invalid_loaded_policy");
    let (confidence, confidence_reason) = match completeness_confidence {
        Confidence::Exact => (
            rule.finding_policy.confidence.exact_definite.clone(),
            "exact_typed_inputs",
        ),
        Confidence::Bounded => (
            rule.finding_policy.confidence.bounded_definite.clone(),
            "bounded_typed_inputs",
        ),
        Confidence::Suspected => (
            rule.finding_policy.confidence.suspected.clone(),
            "threshold_or_zero_boundary_intersection",
        ),
    };
    let evidence_completeness = evidence_completeness_for_confidence(completeness_confidence);
    let completeness_name = match evidence_completeness {
        EvidenceCompleteness::Complete => "complete",
        EvidenceCompleteness::Partial => "partial",
        EvidenceCompleteness::Unavailable => "unavailable",
    };
    let trace = format!(
        "rule-trace:{}@{}:{subject}:{bucket_name}:{completeness_name}",
        rule.rule_id, rule.version
    );
    Finding {
        finding_id: format!(
            "finding:{}@{}:{subject}:{bucket_name}:{completeness_name}",
            rule.rule_id, rule.version
        ),
        rule_id: rule.rule_id.clone(),
        rule_version: rule.version.clone(),
        category: rule.category.clone(),
        severity: severity.into(),
        confidence,
        confidence_reasons: vec![confidence_reason.into()],
        evidence_completeness,
        evidence_completeness_reasons: evidence_completeness_reasons_for_confidence(
            completeness_confidence,
        ),
        subject_ref,
        evidence: ordered_unique(evidence),
        limitations: rule.limitations.clone(),
        recommendation: recommendation(&rule.recommendation_template),
        related_finding_refs: vec![],
        rule_trace_ref: trace,
        measurements: vec![],
    }
}

/// The rule's recommendation text; step evidence is bound later by
/// `bind_recommendation_evidence`, once the subject's observations are known.
fn recommendation(template: &RecommendationTemplate) -> Recommendation {
    Recommendation {
        risk: template.risk.clone(),
        expected_direction: template.expected_direction.clone(),
        validation_steps: template.validation_steps.clone(),
        recommendation_id: template.recommendation_id.clone(),
        kind: template.kind,
        title: template.title.clone(),
        next_step: template.next_step.clone(),
        risk_level: template.risk_level,
        result_effect: template.result_effect,
        steps: template
            .steps
            .iter()
            .map(|step| RecommendationStep {
                title: step.title.clone(),
                detail: step.detail.clone(),
                evidence_refs: vec![],
            })
            .collect(),
        expected_changes: template.expected_changes.clone(),
        limitations: template.limitations.clone(),
    }
}

/// Each step cites a metric of the finding's own subject only when exactly one
/// exact observation of it exists; an absent or ambiguous counter is left out,
/// never substituted.
fn bind_recommendation_evidence(
    finding: &mut Finding,
    template: &RecommendationTemplate,
    observations: &[MetricObservation],
) {
    // Only operator-group subjects carry per-subject observations; a query-scoped
    // finding (spill) cites none here.
    let Some(subject) = finding.subject_ref.as_deref() else {
        return;
    };
    for (step, step_template) in finding.recommendation.steps.iter_mut().zip(&template.steps) {
        step.evidence_refs = step_template
            .evidence_metric_ids
            .iter()
            .filter_map(|metric_id| {
                let mut matches = observations.iter().filter(|observation| {
                    observation.subject_ref.as_deref() == Some(subject)
                        && observation.metric_id.as_deref() == Some(metric_id.as_str())
                        && observation.parse_state == doctor_contracts::ParseState::Exact
                });
                match (matches.next(), matches.next()) {
                    (Some(observation), None) => Some(observation.observation_id.clone()),
                    _ => None,
                }
            })
            .collect();
    }
}

fn evidence_completeness(
    status: &RuleEvaluationStatus,
    confidence: Option<Confidence>,
) -> EvidenceCompleteness {
    if matches!(status, RuleEvaluationStatus::NotApplicable) {
        EvidenceCompleteness::Unavailable
    } else {
        evidence_completeness_for_confidence(confidence.unwrap_or(Confidence::Exact))
    }
}

fn evidence_completeness_reasons(
    status: &RuleEvaluationStatus,
    confidence: Option<Confidence>,
) -> Vec<String> {
    if matches!(status, RuleEvaluationStatus::NotApplicable) {
        vec!["required_evidence_unavailable".into()]
    } else {
        evidence_completeness_reasons_for_confidence(confidence.unwrap_or(Confidence::Exact))
    }
}

fn evidence_completeness_for_confidence(confidence: Confidence) -> EvidenceCompleteness {
    match confidence {
        Confidence::Exact => EvidenceCompleteness::Complete,
        Confidence::Bounded | Confidence::Suspected => EvidenceCompleteness::Partial,
    }
}

fn evidence_completeness_reasons_for_confidence(confidence: Confidence) -> Vec<String> {
    match confidence {
        Confidence::Exact => vec!["exact_typed_inputs".into()],
        Confidence::Bounded => vec!["bounded_typed_inputs".into()],
        Confidence::Suspected => vec!["threshold_boundary_intersection".into()],
    }
}

fn refresh_facts(output: &mut AdapterOutput) {
    let mut findings = output
        .ir
        .findings
        .iter()
        .map(|finding| PortableFindingFact {
            finding_id: finding.finding_id.clone(),
            rule_id: finding.rule_id.clone(),
            rule_version: finding.rule_version.clone(),
            category: finding.category.clone(),
            severity: finding.severity.clone(),
            confidence: finding.confidence.clone(),
            evidence_completeness: finding.evidence_completeness.clone(),
            evidence_completeness_reasons: finding.evidence_completeness_reasons.clone(),
            subject_ref: finding.subject_ref.clone(),
        })
        .collect::<Vec<_>>();
    findings.sort_by(|left, right| left.finding_id.cmp(&right.finding_id));
    let mut evaluations = output
        .ir
        .rule_evaluations
        .iter()
        .map(|evaluation| PortableRuleEvaluationFact {
            rule_id: evaluation.rule_id.clone(),
            rule_version: evaluation.rule_version.clone(),
            status: evaluation.status.clone(),
            evidence_completeness: evaluation.evidence_completeness.clone(),
            evidence_completeness_reasons: evaluation.evidence_completeness_reasons.clone(),
            subject_ref: evaluation.subject_ref.clone(),
            finding_id: evaluation.finding_id.clone(),
        })
        .collect::<Vec<_>>();
    evaluations.sort_by(|left, right| {
        left.rule_id
            .cmp(&right.rule_id)
            .then(left.subject_ref.cmp(&right.subject_ref))
    });
    output.facts.findings = findings;
    output.facts.rule_evaluations = evaluations;
}

fn is_v01_rule(rule_id: &str) -> bool {
    matches!(
        rule_id,
        SCAN_TIME_SHARE_RULE_ID
            | JOIN_TIME_SHARE_RULE_ID
            | AGGREGATE_TIME_SHARE_RULE_ID
            | EXCHANGE_NETWORK_TIME_SHARE_RULE_ID
            | SPILL_OBSERVED_RULE_ID
            | ROW_FLOW_AMPLIFICATION_RULE_ID
            | NODE_TIME_SHARE_RULE_ID
            | OPERATOR_TIME_SKEW_RULE_ID
            | JOIN_PROBE_FANOUT_RULE_ID
            | BROADCAST_EXCHANGE_BYTES_RULE_ID
            | STREAMING_PREAGG_INEFFECTIVE_RULE_ID
    )
}
