//! Ruleset 0.2.0 "why" signals over the execution summary's plan nodes.
//!
//! Every rule reads typed, merge-proven counters of the plan node's runtime
//! operators (official 3.5.14 `RuntimeProfile.mergeIsomorphicProfiles`
//! semantics: a time counter's merged base is the AVG over the merged drivers,
//! a count/bytes counter's base is the SUM, and `__MIN_OF_`/`__MAX_OF_` are the
//! per-driver extremes). Thresholds and floors are product decisions recorded
//! in `ruleset.json`; they are not official StarRocks rules.

use std::collections::BTreeSet;

use doctor_contracts::{
    ExecutionSummaryState, FindingMeasurement, GraphState, PlanNodeSummary, RuleEvaluationStatus,
    RuntimeInventoryFormatterShape,
};
use doctor_rules_v0_1::RuleDefinition;

use super::{
    Confidence, IntegerRange, RuleResult, evidence_for_observations, final_supported_release,
    is_join_type, known_evidence, not_applicable, not_applicable_with_inputs,
    official_stored_profile_source_stage_proof, ordered_unique, refs_valid, result_with_finding,
    stored_profile::OfficialStoredProfileSourceStageProof, summary_subject_group,
    typed_range_with_source_stage_proof,
};
use crate::{
    NormalizedIr, PhysicalOperatorMembership,
    execution_summary::{SummaryReading, group_thousands},
};

/// Product decision (ruleset 0.2.0): max / mean of one operator's
/// `OperatorTotalTime` across merged drivers strictly above 2 is skew.
pub(super) const SKEW_RATIO: i128 = 2;
/// Product decision: a skewed operator whose slowest driver ran under 100 ms
/// is suppressed (same floor as the time-share rules).
pub(super) const SKEW_FLOOR_NS: i128 = 100_000_000;
/// Product decision: probe output rows strictly above 10 × probe input rows.
pub(super) const FANOUT_RATIO: i128 = 10;
/// Product decision: fan-out producing fewer than 100,000 rows is suppressed.
pub(super) const FANOUT_FLOOR_ROWS: i128 = 100_000;
/// Product decision: a BROADCAST exchange sending strictly more than 100 MB (StarRocks' binary MB, 1,048,576 bytes).
pub(super) const BROADCAST_BYTES: i128 = 100 * 1_048_576;
/// Product decision: streaming pre-aggregation keeping at least 90% of rows.
pub(super) const PREAGG_KEPT_PERCENT: i128 = 90;
/// Product decision: pre-aggregation over fewer than 1,000,000 input rows is suppressed.
pub(super) const PREAGG_FLOOR_ROWS: i128 = 1_000_000;

/// Shared applicability context of one rule run.
pub(super) struct SignalContext<'a> {
    pub(super) ir: &'a NormalizedIr,
    pub(super) reading: &'a SummaryReading,
    pub(super) known_refs: &'a BTreeSet<String>,
    proof: Option<OfficialStoredProfileSourceStageProof>,
}

impl<'a> SignalContext<'a> {
    pub(super) fn new(
        ir: &'a NormalizedIr,
        reading: &'a SummaryReading,
        known_refs: &'a BTreeSet<String>,
    ) -> Self {
        let proof = official_stored_profile_source_stage_proof(
            ir,
            known_refs,
            matches!(
                ir.runtime_formatter_shape,
                RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514
            ),
        );
        Self {
            ir,
            reading,
            known_refs,
            proof,
        }
    }

    fn graph_type(&self, node: &PlanNodeSummary) -> Option<&'a str> {
        self.ir
            .graph
            .nodes
            .iter()
            .find(|graph_node| graph_node.node_id == node.node_id)?
            .plan_node_type
            .as_deref()
    }

    fn members(&self, node: &PlanNodeSummary) -> Vec<&'a PhysicalOperatorMembership> {
        let mut members = self
            .ir
            .physical_operator_memberships
            .iter()
            .filter(|member| member.plan_node_id.as_deref() == Some(node.plan_node_id.as_str()))
            .collect::<Vec<_>>();
        members.sort_by_key(|member| member.source_occurrence_ordinal);
        members
    }

    /// One merged counter of one physical operator: its base and, when the
    /// merge printed them, its MIN/MAX pair.
    fn counter(
        &self,
        member: &PhysicalOperatorMembership,
        metric_id: &str,
        unit: &str,
    ) -> Result<Option<Counter>, &'static str> {
        let candidates = self
            .ir
            .metric_observations
            .iter()
            .filter(|observation| {
                observation.runtime_occurrence_ref.as_deref()
                    == Some(member.physical_operator_ref.as_str())
                    && observation.metric_id.as_deref() == Some(metric_id)
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(None);
        }
        let role = |role: &str| {
            candidates
                .iter()
                .filter(|observation| observation.value_role == role)
                .copied()
                .collect::<Vec<_>>()
        };
        let typed = |observation: &crate::MetricObservation| {
            typed_range_with_source_stage_proof(
                observation,
                metric_id,
                unit,
                self.known_refs,
                self.proof.as_ref(),
            )
        };
        let [base] = role("reported_base")[..] else {
            return Err("operator_base_counter_missing_or_ambiguous");
        };
        let mut observation_ids = vec![base.observation_id.clone()];
        let extremes = match (&role("reported_min")[..], &role("reported_max")[..]) {
            ([], []) => None,
            ([minimum], [maximum]) => {
                observation_ids.push(minimum.observation_id.clone());
                observation_ids.push(maximum.observation_id.clone());
                Some((typed(minimum)?, typed(maximum)?))
            }
            _ => return Err("operator_min_max_pair_incomplete_or_ambiguous"),
        };
        Ok(Some(Counter {
            base: typed(base)?,
            extremes,
            observation_ids,
        }))
    }

    /// Σ of one count/bytes counter's base over the given operators.
    fn summed(
        &self,
        members: &[&PhysicalOperatorMembership],
        metric_id: &str,
        unit: &str,
    ) -> Result<(IntegerRange, Vec<String>), &'static str> {
        let mut total = IntegerRange {
            lower: 0,
            upper: 0,
            lower_inclusive: true,
            upper_inclusive: true,
            exact: true,
        };
        let mut ids = vec![];
        for member in members {
            let counter = self
                .counter(member, metric_id, unit)?
                .ok_or("operator_counter_missing")?;
            total = IntegerRange {
                lower: total
                    .lower
                    .checked_add(counter.base.lower)
                    .ok_or("metric_sum_overflow")?,
                upper: total
                    .upper
                    .checked_add(counter.base.upper)
                    .ok_or("metric_sum_overflow")?,
                lower_inclusive: total.lower_inclusive && counter.base.lower_inclusive,
                upper_inclusive: total.upper_inclusive && counter.base.upper_inclusive,
                exact: total.exact && counter.base.exact,
            };
            ids.push(counter.base_id().to_owned());
        }
        Ok((total, ids))
    }

    /// Common preconditions; `Err` is the finished not-applicable result.
    fn subject(
        &self,
        rule: &RuleDefinition,
        node: &PlanNodeSummary,
    ) -> Result<(Option<String>, Vec<String>), Box<RuleResult>> {
        let Some(group) = summary_subject_group(self.ir, node) else {
            return Err(Box::new(not_applicable(
                rule,
                None,
                "summary_subject_group_unresolved",
                known_evidence(node.total_time.source_refs.clone(), self.known_refs),
            )));
        };
        let subject = Some(group.operator_group_id.clone());
        let evidence = known_evidence(group.source_refs.clone(), self.known_refs);
        if !final_supported_release(self.ir) {
            return Err(Box::new(not_applicable(
                rule,
                subject,
                "version_or_profile_not_eligible",
                evidence,
            )));
        }
        if !refs_valid(&group.source_refs, self.known_refs) {
            return Err(Box::new(not_applicable(
                rule,
                subject,
                "subject_provenance_unverified",
                evidence,
            )));
        }
        Ok((subject, evidence))
    }

    fn evidence(&self, mut evidence: Vec<String>, inputs: &[String]) -> Vec<String> {
        if let Some(proof) = &self.proof {
            evidence.extend(proof.evidence_refs.iter().cloned());
        }
        evidence_for_observations(
            evidence,
            &self.ir.metric_observations,
            inputs,
            self.known_refs,
        )
    }

    /// Exact → exact; a text precision interval, a partial graph or a
    /// verified-derived source stage → bounded (same policy as time shares).
    fn confidence(&self, exact: bool) -> Confidence {
        if exact && self.ir.graph.state != GraphState::Partial && self.proof.is_none() {
            Confidence::Exact
        } else {
            Confidence::Bounded
        }
    }

    /// One duration observation in the same text its public metric cell shows.
    fn duration_display(&self, observation_id: &str) -> Option<String> {
        let observation = self
            .ir
            .metric_observations
            .iter()
            .find(|observation| observation.observation_id == observation_id)?;
        (observation.canonical_unit.as_deref() == Some("ns"))
            .then_some(observation.canonical_number.as_ref())
            .flatten()
            .and_then(crate::report::adaptive_duration_ns)
    }

    fn source_stage_check(&self) -> String {
        if self.proof.is_some() {
            "source_counter_stage:official_3514_stored_pipeline_counter_stage_complete"
        } else {
            "source_counter_stage:observation_merge_state_verified"
        }
        .into()
    }
}

pub(super) struct Counter {
    base: IntegerRange,
    extremes: Option<(IntegerRange, IntegerRange)>,
    observation_ids: Vec<String>,
}

impl Counter {
    fn base_id(&self) -> &str {
        &self.observation_ids[0]
    }
}

fn applicable_nodes<'a>(
    context: &SignalContext<'a>,
    applies: impl Fn(&PlanNodeSummary, &str, &[&PhysicalOperatorMembership]) -> bool,
) -> Option<Vec<&'a PlanNodeSummary>> {
    if context.reading.summary.state != ExecutionSummaryState::Available {
        return None;
    }
    Some(
        context
            .reading
            .summary
            .nodes
            .iter()
            .filter(|node| {
                context
                    .graph_type(node)
                    .is_some_and(|node_type| applies(node, node_type, &context.members(node)))
            })
            .collect(),
    )
}

fn none_applicable(rule: &RuleDefinition, reason: &str) -> Vec<RuleResult> {
    vec![not_applicable(rule, None, reason, vec![])]
}

fn evaluate_nodes(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    missing_reason: &str,
    applies: impl Fn(&PlanNodeSummary, &str, &[&PhysicalOperatorMembership]) -> bool,
    evaluate: impl Fn(&SignalContext<'_>, &RuleDefinition, &PlanNodeSummary) -> RuleResult,
) -> Vec<RuleResult> {
    let Some(nodes) = applicable_nodes(context, applies) else {
        return none_applicable(rule, "execution_summary_unavailable");
    };
    if nodes.is_empty() {
        return none_applicable(rule, missing_reason);
    }
    nodes
        .into_iter()
        .map(|node| evaluate(context, rule, node))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn signal_result(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    subject: Option<String>,
    mut checks: Vec<String>,
    status: RuleEvaluationStatus,
    inputs: Vec<String>,
    comparisons: Vec<String>,
    thresholds: Vec<String>,
    evidence: Vec<String>,
    confidence: Confidence,
    measurements: Vec<FindingMeasurement>,
) -> RuleResult {
    let mut applicability = vec![
        "version_profile:final_supported_3514".to_owned(),
        "subject:execution_summary_plan_node_producing_fragment_group".to_owned(),
        context.source_stage_check(),
    ];
    applicability.append(&mut checks);
    let inputs = ordered_unique(inputs);
    let evidence = context.evidence(evidence, &inputs);
    let confidence = match status {
        RuleEvaluationStatus::Suspected => Confidence::Suspected,
        _ => confidence,
    };
    let mut result = result_with_finding(
        rule,
        subject,
        applicability,
        status,
        inputs,
        comparisons,
        thresholds,
        vec![],
        evidence,
        None,
        Some(confidence),
    );
    if let Some(finding) = &mut result.finding {
        finding.measurements = measurements;
    }
    result
}

pub(super) fn measurement(
    measurement_id: &str,
    display_text: Option<String>,
    threshold_text: Option<String>,
    operator_type: Option<&str>,
    input_refs: &[String],
) -> Option<FindingMeasurement> {
    Some(FindingMeasurement {
        measurement_id: measurement_id.to_owned(),
        display_text: display_text?,
        threshold_text,
        operator_type: operator_type.map(str::to_owned),
        input_refs: input_refs.to_vec(),
    })
}

/// `numerator / denominator × scale` in tenths, as the widest interval the
/// two ranges allow (floor of the lowest, ceiling of the highest quotient);
/// one value when both ends agree. `None` without a positive denominator.
pub(super) fn ratio_text(
    numerator: IntegerRange,
    denominator: IntegerRange,
    scale: i128,
    suffix: &str,
) -> Option<String> {
    if denominator.lower <= 0 || numerator.lower < 0 {
        return None;
    }
    let tenths = |value: i128, divisor: i128, ceiling: bool| -> Option<i128> {
        let scaled = value.checked_mul(scale)?.checked_mul(10)?;
        Some(if ceiling {
            scaled.checked_add(divisor - 1)? / divisor
        } else {
            scaled / divisor
        })
    };
    let (lower, upper) = if numerator.lower == numerator.upper
        && denominator.lower == denominator.upper
    {
        // A point value rounds half up.
        let scaled = numerator.lower.checked_mul(scale)?.checked_mul(20)?;
        let rounded = scaled.checked_add(denominator.lower)? / denominator.lower.checked_mul(2)?;
        (rounded, rounded)
    } else {
        (
            tenths(numerator.lower, denominator.upper, false)?,
            tenths(numerator.upper, denominator.lower, true)?,
        )
    };
    let text = |value: i128| format!("{}.{}", group_thousands(value / 10), value % 10);
    Some(if lower == upper {
        format!("{}{suffix}", text(lower))
    } else {
        format!("{} – {}{suffix}", text(lower), text(upper))
    })
}

/// Exact rows grouped by thousands; an interval prints both ends.
pub(super) fn rows_text(rows: IntegerRange) -> String {
    crate::display::range(rows.lower, rows.upper, crate::display::count)
}

/// Bytes in the product's unit policy (`display::bytes`: StarRocks' binary KB / MB / GB).
pub(super) fn bytes_text(bytes: IntegerRange) -> String {
    crate::display::bytes_range(bytes.lower, bytes.upper)
}

/// `lower > k·other.upper` for every value pair (strict), or for some pair.
fn all_above(value: IntegerRange, factor: i128, other: IntegerRange) -> Option<bool> {
    Some(value.lower > factor.checked_mul(other.upper)?)
}

fn any_above(value: IntegerRange, factor: i128, other: IntegerRange) -> Option<bool> {
    Some(value.upper > factor.checked_mul(other.lower)?)
}

// ---------------------------------------------------------------------------
// SR3514.OPERATOR_TIME_SKEW
// ---------------------------------------------------------------------------

pub(super) fn skew_results(context: &SignalContext<'_>, rule: &RuleDefinition) -> Vec<RuleResult> {
    evaluate_nodes(
        context,
        rule,
        "runtime_operator_missing",
        |_, _, members| !members.is_empty(),
        skew_for_node,
    )
}

fn skew_for_node(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    node: &PlanNodeSummary,
) -> RuleResult {
    let (subject, evidence) = match context.subject(rule, node) {
        Ok(value) => value,
        Err(result) => return *result,
    };
    // (status rank, max, counter, operator type) of the most decisive operator.
    let mut decisive: Option<(u8, IntegerRange, Counter, String)> = None;
    let mut all_inputs = vec![];
    let mut exact = true;
    for member in context.members(node) {
        let counter = match context.counter(member, "OperatorTotalTime", "ns") {
            Ok(Some(counter)) => counter,
            Ok(None) => {
                return not_applicable_with_inputs(
                    rule,
                    subject,
                    "operator_total_time_missing",
                    all_inputs,
                    evidence,
                );
            }
            Err(reason) => {
                return not_applicable_with_inputs(rule, subject, reason, all_inputs, evidence);
            }
        };
        all_inputs.extend(counter.observation_ids.iter().cloned());
        exact &= counter.base.exact;
        // The official redundant-counter removal drops MIN/MAX only when
        // base == min == max: one even driver, no skew.
        let Some((minimum, maximum)) = counter.extremes else {
            continue;
        };
        exact &= minimum.exact && maximum.exact;
        // A time counter is AVG-merged: min ≤ mean ≤ max must hold.
        if minimum.lower > counter.base.upper || counter.base.lower > maximum.upper {
            return not_applicable_with_inputs(
                rule,
                subject,
                "time_counter_not_avg_merged",
                all_inputs,
                evidence,
            );
        }
        let (Some(all_skew), Some(any_skew)) = (
            all_above(maximum, SKEW_RATIO, counter.base),
            any_above(maximum, SKEW_RATIO, counter.base),
        ) else {
            return not_applicable_with_inputs(
                rule,
                subject,
                "skew_comparison_overflow",
                all_inputs,
                evidence,
            );
        };
        let material = maximum.lower >= SKEW_FLOOR_NS;
        let possibly_material = maximum.upper >= SKEW_FLOOR_NS;
        let rank = match (all_skew, any_skew, material, possibly_material) {
            (true, _, true, _) => 4,
            (_, true, _, true) => 3,
            (_, true, _, false) => 2,
            _ => 1,
        };
        if decisive
            .as_ref()
            .is_none_or(|(best, best_max, ..)| (rank, maximum.upper) > (*best, best_max.upper))
        {
            decisive = Some((rank, maximum, counter, member.canonical_type.clone()));
        }
    }
    let thresholds = vec![
        "sr3514.operator_time_skew@1".to_owned(),
        format!("strict_gt:max_over_mean:{SKEW_RATIO}"),
        format!(
            "absolute_floor:max_operator_time_ns<{SKEW_FLOOR_NS}ns:product_decision_ruleset_0.2.0"
        ),
    ];
    let checks = vec![
        "typed_metrics:operator_total_time_base_avg_min_max_verified".to_owned(),
        "merge_strategy:time_counter_avg_min_le_mean_le_max".to_owned(),
    ];
    let Some((rank, maximum, counter, operator_type)) = decisive.filter(|(rank, ..)| *rank > 1)
    else {
        return signal_result(
            context,
            rule,
            subject,
            checks,
            RuleEvaluationStatus::NotTriggered,
            all_inputs,
            vec!["max_over_mean_not_greater_than_2_for_every_operator".into()],
            thresholds,
            evidence,
            context.confidence(exact),
            vec![],
        );
    };
    let (status, comparison) = match rank {
        4 => (
            RuleEvaluationStatus::Triggered,
            "max_over_mean_strictly_gt_2_and_max_at_or_above_floor",
        ),
        3 => (
            RuleEvaluationStatus::Suspected,
            "max_over_mean_or_floor_intersects_boundary",
        ),
        _ => (
            RuleEvaluationStatus::Suppressed,
            "max_over_mean_gt_2_but_max_operator_time_below_floor",
        ),
    };
    // observation_ids: [base (driver mean), MIN, MAX] when extremes exist.
    let ids = &counter.observation_ids;
    let (mean_id, max_id) = (&ids[0], ids.last().unwrap_or(&ids[0]));
    let measurements = [
        measurement(
            "slowest_over_mean_driver_time",
            ratio_text(maximum, counter.base, 1, "×"),
            Some(format!("> {SKEW_RATIO}×")),
            Some(&operator_type),
            ids,
        ),
        measurement(
            "slowest_driver_time",
            context.duration_display(max_id),
            Some(format!("≥ {} ms", SKEW_FLOOR_NS / 1_000_000)),
            Some(&operator_type),
            std::slice::from_ref(max_id),
        ),
        measurement(
            "mean_driver_time",
            context.duration_display(mean_id),
            None,
            Some(&operator_type),
            std::slice::from_ref(mean_id),
        ),
        ids.get(1).filter(|_| ids.len() == 3).and_then(|min_id| {
            measurement(
                "fastest_driver_time",
                context.duration_display(min_id),
                None,
                Some(&operator_type),
                std::slice::from_ref(min_id),
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    signal_result(
        context,
        rule,
        subject,
        checks,
        status,
        counter.observation_ids.clone(),
        vec![
            comparison.into(),
            format!("skewed_operator_type:{operator_type}"),
        ],
        thresholds,
        evidence,
        context.confidence(exact),
        measurements,
    )
}

// ---------------------------------------------------------------------------
// SR3514.JOIN_PROBE_FANOUT
// ---------------------------------------------------------------------------

fn is_probe(member: &PhysicalOperatorMembership) -> bool {
    member.canonical_type.ends_with("_JOIN_PROBE")
}

pub(super) fn fanout_results(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    evaluate_nodes(
        context,
        rule,
        "join_probe_operator_missing",
        |_, node_type, members| is_join_type(node_type) && members.iter().any(|m| is_probe(m)),
        fanout_for_node,
    )
}

fn fanout_for_node(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    node: &PlanNodeSummary,
) -> RuleResult {
    let (subject, evidence) = match context.subject(rule, node) {
        Ok(value) => value,
        Err(result) => return *result,
    };
    let probes = context
        .members(node)
        .into_iter()
        .filter(|member| is_probe(member))
        .collect::<Vec<_>>();
    let rows = context
        .summed(&probes, "PushRowNum", "item")
        .and_then(|input| {
            context
                .summed(&probes, "PullRowNum", "item")
                .map(|output| (input, output))
        });
    let ((input, mut inputs), (output, output_ids)) = match rows {
        Ok(value) => value,
        Err(reason) => return not_applicable(rule, subject, reason, evidence),
    };
    let input_ids = inputs.clone();
    inputs.extend(output_ids.iter().cloned());
    if !input.exact || !output.exact {
        return not_applicable_with_inputs(rule, subject, "probe_rows_not_exact", inputs, evidence);
    }
    let measurements = [
        measurement(
            "probe_output_over_input_rows",
            ratio_text(output, input, 1, "×"),
            Some(format!("> {FANOUT_RATIO}×")),
            None,
            &inputs,
        ),
        measurement(
            "probe_input_rows",
            Some(rows_text(input)),
            None,
            None,
            &input_ids,
        ),
        measurement(
            "probe_output_rows",
            Some(rows_text(output)),
            Some(format!("≥ {}", group_thousands(FANOUT_FLOOR_ROWS))),
            None,
            &output_ids,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    if input.lower == 0 {
        return not_applicable_with_inputs(
            rule,
            subject,
            "probe_input_rows_zero",
            inputs,
            evidence,
        );
    }
    let Some(fanout) = all_above(output, FANOUT_RATIO, input) else {
        return not_applicable_with_inputs(
            rule,
            subject,
            "fanout_comparison_overflow",
            inputs,
            evidence,
        );
    };
    let (status, comparison) = match (fanout, output.lower >= FANOUT_FLOOR_ROWS) {
        (true, true) => (
            RuleEvaluationStatus::Triggered,
            "probe_output_over_input_strictly_gt_10",
        ),
        (true, false) => (
            RuleEvaluationStatus::Suppressed,
            "probe_output_over_input_gt_10_but_output_rows_below_floor",
        ),
        _ => (
            RuleEvaluationStatus::NotTriggered,
            "probe_output_over_input_not_greater_than_10",
        ),
    };
    signal_result(
        context,
        rule,
        subject,
        vec!["typed_metrics:exact_probe_push_pull_row_sums".into()],
        status,
        inputs,
        vec![comparison.into()],
        vec![
            "sr3514.join_probe_fanout@1".into(),
            format!("strict_gt:probe_pull_over_push:{FANOUT_RATIO}"),
            format!(
                "absolute_floor:probe_output_rows<{FANOUT_FLOOR_ROWS}:product_decision_ruleset_0.2.0"
            ),
        ],
        evidence,
        context.confidence(true),
        measurements,
    )
}

// ---------------------------------------------------------------------------
// SR3514.BROADCAST_EXCHANGE_BYTES
// ---------------------------------------------------------------------------

/// One identified broadcast exchange: its sending operators and the facts
/// and edge that identify it.
struct BroadcastExchange<'a> {
    sinks: Vec<&'a PhysicalOperatorMembership>,
    evidence: Vec<String>,
}

fn info_fact<'a>(
    context: &SignalContext<'a>,
    member: &PhysicalOperatorMembership,
    key: &str,
) -> Option<&'a crate::OperatorInfoFact> {
    context
        .ir
        .operator_info_facts
        .iter()
        .find(|fact| fact.physical_operator_ref == member.physical_operator_ref && fact.key == key)
}

/// Product derivation (3.5.14 prints no BROADCAST PartType: a broadcast
/// sender prints `PartType: UNPARTITIONED`, like a gather). The exchange is a
/// broadcast when its explicit Topology parent is a JOIN whose operators print
/// `DistributionMode: BROADCAST` and it is that join's only child exchange
/// whose senders print `UNPARTITIONED` (a broadcast join's build side is
/// always such an exchange; two candidates stay unidentified).
fn broadcast_exchange<'a>(
    context: &SignalContext<'a>,
    node: &PlanNodeSummary,
) -> Option<BroadcastExchange<'a>> {
    let unpartitioned_sinks = |summary: &PlanNodeSummary| {
        let sinks = context
            .members(summary)
            .into_iter()
            .filter(|member| member.canonical_type == "EXCHANGE_SINK")
            .collect::<Vec<_>>();
        let unpartitioned = !sinks.is_empty()
            && sinks.iter().all(|sink| {
                info_fact(context, sink, "PartType")
                    .is_some_and(|fact| fact.value.as_deref() == Some("UNPARTITIONED"))
            });
        unpartitioned.then_some(sinks)
    };
    let parent_edge = context.ir.graph.edges.iter().find(|edge| {
        edge.relation == doctor_contracts::GraphRelation::PlanChild
            && edge.to == node.node_id
            && super::trusted_row_edge(edge, context.known_refs)
    })?;
    let parent = context
        .reading
        .summary
        .nodes
        .iter()
        .find(|summary| summary.node_id == parent_edge.from)?;
    if !context.graph_type(parent).is_some_and(is_join_type) {
        return None;
    }
    let modes = context
        .members(parent)
        .into_iter()
        .filter_map(|member| info_fact(context, member, "DistributionMode"))
        .collect::<Vec<_>>();
    if modes.is_empty()
        || !modes
            .iter()
            .all(|fact| fact.value.as_deref() == Some("BROADCAST"))
    {
        return None;
    }
    let candidates = context
        .ir
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.relation == doctor_contracts::GraphRelation::PlanChild
                && edge.from == parent.node_id
                && super::trusted_row_edge(edge, context.known_refs)
        })
        .filter_map(|edge| {
            let child = context
                .reading
                .summary
                .nodes
                .iter()
                .find(|summary| summary.node_id == edge.to)?;
            context
                .graph_type(child)
                .is_some_and(|kind| kind.contains("EXCHANGE"))
                .then_some(child)?;
            unpartitioned_sinks(child).map(|sinks| (child, sinks))
        })
        .collect::<Vec<_>>();
    let [(child, sinks)] = candidates.as_slice() else {
        return None;
    };
    if child.node_id != node.node_id {
        return None;
    }
    let mut evidence = parent_edge.evidence_refs.clone();
    for fact in modes {
        evidence.extend(fact.source_refs.iter().cloned());
    }
    for sink in sinks {
        if let Some(fact) = info_fact(context, sink, "PartType") {
            evidence.extend(fact.source_refs.iter().cloned());
        }
    }
    Some(BroadcastExchange {
        sinks: sinks.clone(),
        evidence,
    })
}

pub(super) fn broadcast_results(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    evaluate_nodes(
        context,
        rule,
        "broadcast_exchange_missing",
        |node, node_type, _| {
            node_type.contains("EXCHANGE") && broadcast_exchange(context, node).is_some()
        },
        broadcast_for_node,
    )
}

fn broadcast_for_node(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    node: &PlanNodeSummary,
) -> RuleResult {
    let (subject, mut evidence) = match context.subject(rule, node) {
        Ok(value) => value,
        Err(result) => return *result,
    };
    let Some(broadcast) = broadcast_exchange(context, node) else {
        return not_applicable(rule, subject, "broadcast_exchange_unidentified", evidence);
    };
    evidence.extend(broadcast.evidence);
    let sinks = broadcast.sinks;
    let (bytes, inputs) = match context.summed(&sinks, "BytesSent", "byte") {
        Ok(value) => value,
        Err(reason) => return not_applicable(rule, subject, reason, evidence),
    };
    let (status, comparison) = if bytes.lower > BROADCAST_BYTES {
        (
            RuleEvaluationStatus::Triggered,
            "broadcast_bytes_sent_all_strictly_gt_100_mib",
        )
    } else if bytes.upper > BROADCAST_BYTES {
        (
            RuleEvaluationStatus::Suspected,
            "broadcast_bytes_sent_interval_intersects_100_mib",
        )
    } else {
        (
            RuleEvaluationStatus::NotTriggered,
            "broadcast_bytes_sent_not_greater_than_100_mib",
        )
    };
    let measurements = measurement(
        "broadcast_bytes_sent",
        Some(bytes_text(bytes)),
        Some(format!("> {}", crate::display::bytes(BROADCAST_BYTES))),
        Some("EXCHANGE_SINK"),
        &inputs,
    )
    .into_iter()
    .collect();
    signal_result(
        context,
        rule,
        subject,
        vec![
            "operator_info:join_distribution_mode_broadcast_and_single_unpartitioned_exchange_child".into(),
            "graph_edge:exact_explicit_plan_child_parent_to_child".into(),
            "typed_metrics:bytes_sent_sum_merged".into(),
        ],
        status,
        inputs,
        vec![comparison.into()],
        vec![
            "sr3514.broadcast_exchange_bytes@1".into(),
            format!("strict_gt:bytes_sent:{BROADCAST_BYTES}"),
        ],
        evidence,
        context.confidence(bytes.exact),
        measurements,
    )
}

// ---------------------------------------------------------------------------
// SR3514.STREAMING_PREAGG_INEFFECTIVE
// ---------------------------------------------------------------------------

pub(super) fn preagg_results(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
) -> Vec<RuleResult> {
    evaluate_nodes(
        context,
        rule,
        "streaming_preaggregation_missing",
        |_, _, members| {
            members
                .iter()
                .any(|member| member.canonical_type == "AGGREGATE_STREAMING_SINK")
                && members
                    .iter()
                    .any(|member| member.canonical_type == "AGGREGATE_STREAMING_SOURCE")
        },
        preagg_for_node,
    )
}

fn preagg_for_node(
    context: &SignalContext<'_>,
    rule: &RuleDefinition,
    node: &PlanNodeSummary,
) -> RuleResult {
    let (subject, evidence) = match context.subject(rule, node) {
        Ok(value) => value,
        Err(result) => return *result,
    };
    let members = context.members(node);
    let of_type = |kind: &str| {
        members
            .iter()
            .copied()
            .filter(|member| member.canonical_type == kind)
            .collect::<Vec<_>>()
    };
    let rows = context
        .summed(&of_type("AGGREGATE_STREAMING_SINK"), "PushRowNum", "item")
        .and_then(|input| {
            context
                .summed(&of_type("AGGREGATE_STREAMING_SOURCE"), "PullRowNum", "item")
                .map(|output| (input, output))
        });
    let ((input, mut inputs), (output, output_ids)) = match rows {
        Ok(value) => value,
        Err(reason) => return not_applicable(rule, subject, reason, evidence),
    };
    let input_ids = inputs.clone();
    inputs.extend(output_ids.iter().cloned());
    if !input.exact || !output.exact {
        return not_applicable_with_inputs(
            rule,
            subject,
            "preaggregation_rows_not_exact",
            inputs,
            evidence,
        );
    }
    let measurements = [
        measurement(
            "preaggregation_output_over_input_rows",
            ratio_text(output, input, 100, "%"),
            Some(format!("≥ {PREAGG_KEPT_PERCENT}%")),
            None,
            &inputs,
        ),
        measurement(
            "preaggregation_input_rows",
            Some(rows_text(input)),
            Some(format!("≥ {}", group_thousands(PREAGG_FLOOR_ROWS))),
            Some("AGGREGATE_STREAMING_SINK"),
            &input_ids,
        ),
        measurement(
            "preaggregation_output_rows",
            Some(rows_text(output)),
            None,
            Some("AGGREGATE_STREAMING_SOURCE"),
            &output_ids,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    if input.lower == 0 {
        return not_applicable_with_inputs(
            rule,
            subject,
            "preaggregation_input_rows_zero",
            inputs,
            evidence,
        );
    }
    let (Some(kept), Some(threshold)) = (
        output.lower.checked_mul(100),
        input.lower.checked_mul(PREAGG_KEPT_PERCENT),
    ) else {
        return not_applicable_with_inputs(
            rule,
            subject,
            "preaggregation_comparison_overflow",
            inputs,
            evidence,
        );
    };
    let (status, comparison) = match (kept >= threshold, input.lower >= PREAGG_FLOOR_ROWS) {
        (true, true) => (
            RuleEvaluationStatus::Triggered,
            "streaming_output_over_input_at_or_above_90_percent",
        ),
        (true, false) => (
            RuleEvaluationStatus::Suppressed,
            "streaming_output_over_input_at_or_above_90_percent_but_input_below_floor",
        ),
        _ => (
            RuleEvaluationStatus::NotTriggered,
            "streaming_output_over_input_below_90_percent",
        ),
    };
    signal_result(
        context,
        rule,
        subject,
        vec!["typed_metrics:exact_streaming_sink_push_source_pull_row_sums".into()],
        status,
        inputs,
        vec![comparison.into()],
        vec![
            "sr3514.streaming_preagg_ineffective@1".into(),
            format!("at_or_above:streaming_output_over_input_percent:{PREAGG_KEPT_PERCENT}"),
            format!(
                "absolute_floor:streaming_input_rows<{PREAGG_FLOOR_ROWS}:product_decision_ruleset_0.2.0"
            ),
        ],
        evidence,
        context.confidence(true),
        measurements,
    )
}

#[cfg(test)]
mod measurement_text_tests {
    use super::*;

    fn point(value: i128) -> IntegerRange {
        IntegerRange {
            lower: value,
            upper: value,
            lower_inclusive: true,
            upper_inclusive: true,
            exact: true,
        }
    }

    fn truncated(lower: i128, upper: i128) -> IntegerRange {
        IntegerRange {
            lower,
            upper,
            lower_inclusive: true,
            upper_inclusive: false,
            exact: false,
        }
    }

    #[test]
    fn point_ratios_round_half_up_to_one_decimal() {
        assert_eq!(
            ratio_text(point(20), point(1), 1, "×").as_deref(),
            Some("20.0×")
        );
        assert_eq!(
            ratio_text(point(1_960), point(100), 1, "×").as_deref(),
            Some("19.6×")
        );
        assert_eq!(
            ratio_text(point(9_995), point(10_000), 100, "%").as_deref(),
            Some("100.0%")
        );
        assert_eq!(
            ratio_text(point(25_000_000), point(1), 1, "×").as_deref(),
            Some("25,000,000.0×")
        );
    }

    #[test]
    fn bounded_ratios_print_the_widest_interval_and_never_divide_by_zero() {
        assert_eq!(
            ratio_text(
                truncated(499_854_000, 499_855_000),
                truncated(48_241_000, 48_242_000),
                1,
                "×"
            )
            .as_deref(),
            Some("10.3 – 10.4×")
        );
        assert_eq!(ratio_text(point(5), point(0), 1, "×"), None);
        assert_eq!(ratio_text(point(5), truncated(0, 1_000), 1, "×"), None);
    }

    #[test]
    fn bytes_and_rows_follow_the_display_policy() {
        assert_eq!(bytes_text(point(512)), "512 B");
        assert_eq!(bytes_text(point(100 << 20)), "100 MB");
        assert_eq!(bytes_text(point(3 << 30)), "3 GB");
        assert_eq!(bytes_text(truncated(1 << 20, (1 << 21) - 1)), "1 MB – 2 MB");
        assert_eq!(rows_text(point(1_020_000)), "1,020,000");
        assert_eq!(rows_text(truncated(10, 20)), "10 – 20");
    }
}
