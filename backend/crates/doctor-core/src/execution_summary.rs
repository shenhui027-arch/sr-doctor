//! Plan-node execution summary following official StarRocks 3.5.14
//! `ExplainAnalyzer.NodeInfo`:
//!
//! * time = Σ max(OperatorTotalTime) over native + subordinate operators of the
//!   plan node, plus the first native `NetworkTime` (exchange) or `ScanTime`
//!   (scan), where max is `__MAX_OF_` when reported, else the base counter;
//! * share = time / QueryCumulativeOperatorTime; > 30% most consuming, > 15%
//!   second most consuming;
//! * output rows = the adapter's version-pack `OutputRows` recipe result.
//!
//! Every value is either exact or explicitly unavailable with a reason.
//! "Exact" is the official `ANALYZE PROFILE` reading of the same text (its
//! `RuntimeProfileParser` reads the printed counters too); a counter printed
//! with less precision than the runtime value adds a `bounds` interval.
//!
//! The time-share rules read this summary (`read_execution_summary`) instead
//! of a second NodeInfo implementation, and classify shares with the same
//! `classify_share`, so a hotspot tier and a rule status never disagree.

use std::{cmp::Ordering, collections::BTreeMap};

use doctor_contracts::{
    BoundedInterval, CanonicalNumber, EXECUTION_SUMMARY_RECIPE_ID, ExecutionSummary,
    ExecutionSummaryState, Fidelity, HotspotTier, MergeState, MetricCellValue, NumberKind,
    PlanNodeOperatorSummary, PlanNodeSummary, QueryState, RuntimeSourceRole, SnapshotKind,
    SummaryOperatorRole, SummaryValue, SummaryValueState,
};

use crate::{
    AdapterOutput, CancelSignal, Cancelled, MetricObservation, ParseState,
    PhysicalOperatorMembership,
};

/// Fixed-point nanoseconds with three fractional digits (picoseconds).
pub(crate) type Pico = i128;
pub(crate) const PICO_PER_NS: i128 = 1_000;

/// An interval of one quantity (picoseconds for durations). A lossless value
/// is the closed point interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) lower: i128,
    pub(crate) upper: i128,
    pub(crate) lower_inclusive: bool,
    pub(crate) upper_inclusive: bool,
}

impl Span {
    pub(crate) fn point(value: i128) -> Self {
        Self {
            lower: value,
            upper: value,
            lower_inclusive: true,
            upper_inclusive: true,
        }
    }

    pub(crate) fn is_point(&self) -> bool {
        self.lower == self.upper
    }

    fn add(self, other: Self) -> Option<Self> {
        Some(Self {
            lower: self.lower.checked_add(other.lower)?,
            upper: self.upper.checked_add(other.upper)?,
            lower_inclusive: self.lower_inclusive && other.lower_inclusive,
            upper_inclusive: self.upper_inclusive && other.upper_inclusive,
        })
    }
}

/// Where a share sits relative to the official > 30% / > 15% thresholds over
/// the whole interval `numerator / denominator`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShareClass {
    Above30,
    Boundary30,
    Above15,
    Boundary15,
    NotAbove15,
}

/// The one share classifier for hotspot tiers and time-share rules. `None`:
/// the denominator interval is not strictly positive, the numerator is
/// negative, or the arithmetic overflows.
pub(crate) fn classify_share(numerator: Span, denominator: Span) -> Option<ShareClass> {
    if denominator.lower <= 0 || numerator.lower < 0 {
        return None;
    }
    // Lowest share: smallest numerator over largest denominator; highest: the reverse.
    let all_above = |percent: i128| -> Option<bool> {
        let ordering = numerator
            .lower
            .checked_mul(100)?
            .cmp(&percent.checked_mul(denominator.upper)?);
        Some(
            ordering == Ordering::Greater
                || (ordering == Ordering::Equal
                    && !(numerator.lower_inclusive && denominator.upper_inclusive)),
        )
    };
    let any_above = |percent: i128| -> Option<bool> {
        Some(numerator.upper.checked_mul(100)? > percent.checked_mul(denominator.lower)?)
    };
    Some(if all_above(30)? {
        ShareClass::Above30
    } else if any_above(30)? {
        ShareClass::Boundary30
    } else if all_above(15)? {
        ShareClass::Above15
    } else if any_above(15)? {
        ShareClass::Boundary15
    } else {
        ShareClass::NotAbove15
    })
}

/// The typed node time a rule consumes: the summary interval plus the
/// observations the official recipe selected for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NodeTimeInputs {
    pub(crate) span: Span,
    pub(crate) observation_ids: Vec<String>,
}

pub(crate) struct SummaryReading {
    pub(crate) summary: ExecutionSummary,
    /// Keyed by `PlanNodeSummary::node_id`; present only for an exact total.
    pub(crate) node_time_inputs: BTreeMap<String, NodeTimeInputs>,
    /// Keyed by `PlanNodeSummary::node_id`: the `OutputRows` observation
    /// behind an exact `output_rows`.
    pub(crate) node_output_rows_inputs: BTreeMap<String, String>,
}

pub(crate) fn execution_summary(
    output: &AdapterOutput,
    cancel: &CancelSignal<'_>,
) -> Result<ExecutionSummary, Cancelled> {
    Ok(read_execution_summary(output, cancel)?.summary)
}

/// Polls `cancel` per observation, membership and plan node.
pub(crate) fn read_execution_summary(
    output: &AdapterOutput,
    cancel: &CancelSignal<'_>,
) -> Result<SummaryReading, Cancelled> {
    let ir = &output.ir;
    let mut node_time_inputs = BTreeMap::new();
    let mut summary = ExecutionSummary {
        state: ExecutionSummaryState::Available,
        recipe_id: EXECUTION_SUMMARY_RECIPE_ID.to_owned(),
        query_total_time: query_duration(ir, "QueryTotalTime"),
        cumulative_operator_time: query_duration(ir, "QueryCumulativeOperatorTime"),
        result_rows: SummaryValue::unavailable("result_sink_rows_unavailable"),
        nodes: vec![],
        reasons: vec![],
    };
    if ir.physical_operator_memberships.is_empty() {
        summary.state = ExecutionSummaryState::Unavailable;
        summary
            .reasons
            .push("runtime_operators_unavailable".to_owned());
        return Ok(SummaryReading {
            summary,
            node_time_inputs,
            node_output_rows_inputs: BTreeMap::new(),
        });
    }
    if ir.profile_context.merge_state != MergeState::Merged {
        // Unmerged profiles repeat one operator per driver; summing them would
        // not be the official per-node reading.
        summary.state = ExecutionSummaryState::Unavailable;
        summary.reasons.push("profile_not_merged".to_owned());
        return Ok(SummaryReading {
            summary,
            node_time_inputs,
            node_output_rows_inputs: BTreeMap::new(),
        });
    }
    if let Some(reason) = provisional_profile_reason(ir) {
        summary.state = ExecutionSummaryState::Unavailable;
        summary.reasons.push(reason.to_owned());
        return Ok(SummaryReading {
            summary,
            node_time_inputs,
            node_output_rows_inputs: BTreeMap::new(),
        });
    }

    let observations = observations_by_operator(&ir.metric_observations, cancel)?;
    let cumulative = summary_span(&summary.cumulative_operator_time);
    summary.result_rows = group_output_rows(ir, "-1").0;
    let mut node_output_rows_inputs = BTreeMap::new();

    let mut memberships: BTreeMap<&str, Vec<&PhysicalOperatorMembership>> = BTreeMap::new();
    for membership in &ir.physical_operator_memberships {
        cancel.tick()?;
        if let Some(plan_node_id) = membership.plan_node_id.as_deref() {
            memberships
                .entry(plan_node_id)
                .or_default()
                .push(membership);
        }
    }
    for members in memberships.values_mut() {
        members.sort_by_key(|member| member.source_occurrence_ordinal);
    }

    for node in &ir.graph.nodes {
        cancel.tick()?;
        let Some(plan_node_id) = node.plan_node_id.as_deref() else {
            continue;
        };
        let members = memberships.get(plan_node_id).cloned().unwrap_or_default();
        let node_type = node.plan_node_type.as_deref().unwrap_or("");
        let (output_rows, rows_input) = group_output_rows(ir, plan_node_id);
        if let Some(rows_input) = rows_input {
            node_output_rows_inputs.insert(node.node_id.clone(), rows_input);
        }
        let (node_summary, inputs) = node_summary(
            &node.node_id,
            plan_node_id,
            &members,
            &observations,
            cumulative,
            node_type,
            output_rows,
        );
        if let Some(inputs) = inputs {
            node_time_inputs.insert(node.node_id.clone(), inputs);
        }
        summary.nodes.push(node_summary);
    }
    Ok(SummaryReading {
        summary,
        node_time_inputs,
        node_output_rows_inputs,
    })
}

/// A merged Profile whose counters are not the final official reading: a
/// query still running (FE runtime report, `QueryRuntimeProfile.java:333-348`),
/// a failed query, or an official incomplete marker (`NotIdentical`,
/// `RuntimeProfile.java:889`; `MissingInstanceIds` and operator `IsRunning`,
/// `QueryRuntimeProfile.java:477-478, 698-718`). Summing such operators would
/// print a precise-looking value that is provisional or covers only part of
/// the instances.
fn provisional_profile_reason(ir: &crate::NormalizedIr) -> Option<&'static str> {
    let context = &ir.profile_context;
    if context.query_state == QueryState::Running {
        return Some("profile_running_snapshot");
    }
    if context.snapshot_kind == SnapshotKind::Runtime {
        return Some("profile_runtime_snapshot");
    }
    if context.query_state == QueryState::Error {
        return Some("profile_error");
    }
    [
        "runtime_operator_running",
        "runtime_profile_missing_instances",
        "runtime_profile_not_identical",
        "runtime_profile_incomplete_marker_malformed",
    ]
    .into_iter()
    .find(|code| {
        context
            .limitations
            .iter()
            .any(|limitation| limitation == code)
    })
}

fn node_summary(
    node_id: &str,
    plan_node_id: &str,
    members: &[&PhysicalOperatorMembership],
    observations: &BTreeMap<&str, Vec<&MetricObservation>>,
    cumulative: Option<(Pico, Span)>,
    node_type: &str,
    output_rows: SummaryValue,
) -> (PlanNodeSummary, Option<NodeTimeInputs>) {
    let mut reasons = vec![];
    let operators = members
        .iter()
        .map(|member| operator_summary(member, observations))
        .collect::<Vec<_>>();
    let natives = members
        .iter()
        .filter(|member| member.source_role == RuntimeSourceRole::Native)
        .copied()
        .collect::<Vec<_>>();
    if members
        .iter()
        .any(|member| member.source_role == RuntimeSourceRole::Unknown)
    {
        reasons.push("operator_role_unknown".to_owned());
    }

    let (total_time, inputs) = if members.is_empty() {
        (
            SummaryValue::unavailable("no_runtime_operator_for_plan_node"),
            None,
        )
    } else if !reasons.is_empty() {
        (SummaryValue::unavailable("operator_role_unknown"), None)
    } else {
        match node_total_time(node_type, members, &natives, observations) {
            Ok((value, inputs)) => (value, Some(inputs)),
            Err(reason) => (SummaryValue::unavailable(reason), None),
        }
    };
    let (time_share_basis_points, time_share_display, hotspot) =
        match (summary_span(&total_time), cumulative) {
            (Some((total, total_span)), Some((cumulative, cumulative_span))) if cumulative > 0 => {
                let basis_points = (total * 10_000 + cumulative / 2) / cumulative;
                let tier = match classify_share(total_span, cumulative_span) {
                    Some(ShareClass::Above30) => HotspotTier::MostConsuming,
                    Some(ShareClass::Above15) => HotspotTier::SecondMostConsuming,
                    Some(ShareClass::Boundary30 | ShareClass::Boundary15) => HotspotTier::Boundary,
                    Some(ShareClass::NotAbove15) => HotspotTier::None,
                    None => HotspotTier::Unknown,
                };
                (
                    u32::try_from(basis_points).ok(),
                    Some(format!("{}.{:02}%", basis_points / 100, basis_points % 100)),
                    tier,
                )
            }
            _ => (None, None, HotspotTier::Unknown),
        };
    let fragment_ref = node_fragment(&natives);
    let summary = PlanNodeSummary {
        node_id: node_id.to_owned(),
        plan_node_id: plan_node_id.to_owned(),
        fragment_ref,
        total_time,
        time_share_basis_points,
        time_share_display,
        hotspot,
        output_rows,
        operators,
        reasons,
    };
    (summary, inputs)
}

fn node_total_time(
    node_type: &str,
    members: &[&PhysicalOperatorMembership],
    natives: &[&PhysicalOperatorMembership],
    observations: &BTreeMap<&str, Vec<&MetricObservation>>,
) -> Result<(SummaryValue, NodeTimeInputs), &'static str> {
    let mut total: Pico = 0;
    let mut span = Span::point(0);
    let mut source_refs = vec![];
    let mut observation_ids = vec![];
    let mut add = |selected: SelectedDuration| -> Result<(), &'static str> {
        total = total
            .checked_add(selected.value)
            .ok_or("metric_sum_overflow")?;
        span = span.add(selected.span).ok_or("metric_sum_overflow")?;
        source_refs.extend(selected.source_refs);
        observation_ids.push(selected.observation_id);
        Ok(())
    };
    for member in members {
        match max_duration(
            observations,
            &member.physical_operator_ref,
            "OperatorTotalTime",
        )? {
            Some(selected) => add(selected)?,
            // ExplainAnalyzer's sumUpMetric would skip it, but a merged 3.5.14 Profile always
            // carries the counter (QueryRuntimeProfile checkNotNull on OperatorTotalTime), so
            // an absent one means a truncated Profile: never report the partial sum as exact.
            None => return Err("operator_total_time_missing"),
        }
    }
    let extra_metric = if node_type.contains("EXCHANGE") {
        Some("NetworkTime")
    } else if node_type.ends_with("_SCAN") {
        Some("ScanTime")
    } else {
        None
    };
    if let Some(metric_id) = extra_metric {
        // Official searchMetric: the first native operator carrying it.
        for native in natives {
            if let Some(selected) =
                max_duration(observations, &native.physical_operator_ref, metric_id)?
            {
                add(selected)?;
                break;
            }
        }
    }
    Ok((
        duration_value(total, span, source_refs),
        NodeTimeInputs {
            span,
            observation_ids,
        },
    ))
}

fn operator_summary(
    member: &PhysicalOperatorMembership,
    observations: &BTreeMap<&str, Vec<&MetricObservation>>,
) -> PlanNodeOperatorSummary {
    let operator_time = match max_duration(
        observations,
        &member.physical_operator_ref,
        "OperatorTotalTime",
    ) {
        Ok(Some(selected)) => duration_value(selected.value, selected.span, selected.source_refs),
        Ok(None) => SummaryValue::unavailable("metric_missing"),
        Err(reason) => SummaryValue::unavailable(reason),
    };
    PlanNodeOperatorSummary {
        operator_type: member.canonical_type.clone(),
        role: if member.source_role == RuntimeSourceRole::Subordinate {
            SummaryOperatorRole::Subordinate
        } else {
            SummaryOperatorRole::Native
        },
        fragment_ref: member.fragment_ref.clone(),
        operator_time,
        pull_rows: base_count(observations, &member.physical_operator_ref, "PullRowNum"),
        push_rows: base_count(observations, &member.physical_operator_ref, "PushRowNum"),
        source_refs: member.source_refs.clone(),
    }
}

/// Official ExplainAnalyzer merges every operator group with one plan node id.
/// The adapter computes `OutputRows` per fragment group with the version-pack
/// recipe; the official pick is the group whose first native operator is not a
/// `_SINK` / `_BUILD`, i.e. the producing side of an exchange.
/// The summary value and, when exact, the observation it was read from.
fn group_output_rows(
    ir: &crate::NormalizedIr,
    plan_node_id: &str,
) -> (SummaryValue, Option<String>) {
    let groups = ir
        .operator_groups
        .iter()
        .filter(|group| group.plan_node_id.as_deref() == Some(plan_node_id))
        .collect::<Vec<_>>();
    let mut candidates = vec![];
    for group in &groups {
        let producing = ir
            .physical_operator_memberships
            .iter()
            .filter(|member| {
                member.plan_node_id.as_deref() == Some(plan_node_id)
                    && member.fragment_ref == group.fragment_ref
                    && member.source_role == RuntimeSourceRole::Native
            })
            .min_by_key(|member| member.source_occurrence_ordinal)
            .is_some_and(|member| {
                !member.canonical_type.ends_with("_SINK")
                    && !member.canonical_type.ends_with("_BUILD")
            });
        let rows = ir.metric_observations.iter().filter(|observation| {
            observation.metric_id.as_deref() == Some("OutputRows")
                && observation.subject_ref.as_deref() == Some(group.operator_group_id.as_str())
        });
        for observation in rows {
            candidates.push((producing, observation));
        }
    }
    let preferred = if candidates.iter().any(|(producing, _)| *producing) {
        candidates
            .iter()
            .filter(|(producing, _)| *producing)
            .collect::<Vec<_>>()
    } else {
        candidates.iter().collect::<Vec<_>>()
    };
    match preferred.as_slice() {
        [(_, observation)] => match count_value(observation) {
            Some(value) => (value, Some(observation.observation_id.clone())),
            None => (SummaryValue::unavailable(first_reason(observation)), None),
        },
        [] => (SummaryValue::unavailable("output_rows_unavailable"), None),
        _ => (SummaryValue::unavailable("output_rows_ambiguous"), None),
    }
}

fn first_reason(observation: &MetricObservation) -> &str {
    observation
        .diagnostics
        .first()
        .map_or("output_rows_unavailable", |diagnostic| {
            diagnostic.message_id.as_str()
        })
}

fn node_fragment(natives: &[&PhysicalOperatorMembership]) -> Option<String> {
    // The node lives in the fragment of its producing (non-sink) side. A node
    // whose only native operator is a sink (ASSERT_NUM_ROWS_SINK) has no other
    // side: its sinks' single fragment is the one ExplainAnalyzer prints it in.
    let sink_only = natives
        .iter()
        .all(|member| member.canonical_type.ends_with("_SINK"));
    let candidates = natives
        .iter()
        .filter(|member| sink_only || !member.canonical_type.ends_with("_SINK"))
        .filter_map(|member| member.fragment_ref.clone())
        .collect::<Vec<_>>();
    let first = candidates.first()?.clone();
    candidates
        .iter()
        .all(|fragment| *fragment == first)
        .then_some(first)
}

fn observations_by_operator<'a>(
    observations: &'a [MetricObservation],
    cancel: &CancelSignal<'_>,
) -> Result<BTreeMap<&'a str, Vec<&'a MetricObservation>>, Cancelled> {
    let mut index: BTreeMap<&str, Vec<&MetricObservation>> = BTreeMap::new();
    for observation in observations {
        cancel.tick()?;
        if let Some(reference) = observation.runtime_occurrence_ref.as_deref() {
            index.entry(reference).or_default().push(observation);
        }
    }
    Ok(index)
}

struct SelectedDuration {
    value: Pico,
    span: Span,
    source_refs: Vec<String>,
    observation_id: String,
}

/// `Ok(None)`: counter absent. `Err`: present but not usable as an exact value.
fn max_duration(
    observations: &BTreeMap<&str, Vec<&MetricObservation>>,
    operator_ref: &str,
    metric_id: &str,
) -> Result<Option<SelectedDuration>, &'static str> {
    let candidates = observations
        .get(operator_ref)
        .map(|items| {
            items
                .iter()
                .filter(|observation| observation.metric_id.as_deref() == Some(metric_id))
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let by_role = |role: &str| {
        candidates
            .iter()
            .filter(|observation| observation.value_role == role)
            .copied()
            .collect::<Vec<_>>()
    };
    let max = by_role("reported_max");
    let min = by_role("reported_min");
    if !max.is_empty() || !min.is_empty() {
        // RuntimeProfile prints __MIN_OF_/__MAX_OF_ together under their base
        // counter, or drops both when base == min == max.
        let base = by_role("reported_base");
        let ([minimum], [maximum], [base]) = (min.as_slice(), max.as_slice(), base.as_slice())
        else {
            return Err("operator_min_max_pair_incomplete_or_ambiguous");
        };
        observation_pico(base)?;
        if observation_pico(minimum)?.0 > observation_pico(maximum)?.0 {
            return Err("operator_min_max_range_invalid");
        }
    }
    let selected = if max.is_empty() {
        by_role("reported_base")
    } else {
        max
    };
    match selected.as_slice() {
        [] => Ok(None),
        [observation] => {
            let (value, span) = observation_pico(observation)?;
            Ok(Some(SelectedDuration {
                value,
                span,
                source_refs: observation.source_refs.clone(),
                observation_id: observation.observation_id.clone(),
            }))
        }
        _ => Err("metric_ambiguous"),
    }
}

fn base_count(
    observations: &BTreeMap<&str, Vec<&MetricObservation>>,
    operator_ref: &str,
    metric_id: &str,
) -> SummaryValue {
    let candidates = observations
        .get(operator_ref)
        .map(|items| {
            items
                .iter()
                .filter(|observation| {
                    observation.metric_id.as_deref() == Some(metric_id)
                        && observation.value_role == "reported_base"
                })
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match candidates.as_slice() {
        [observation] => count_value(observation)
            .unwrap_or_else(|| SummaryValue::unavailable("metric_not_exact")),
        [] => SummaryValue::unavailable("metric_missing"),
        _ => SummaryValue::unavailable("metric_ambiguous"),
    }
}

/// The official reading of one duration counter and the interval that contains
/// its runtime value.
fn observation_pico(observation: &MetricObservation) -> Result<(Pico, Span), &'static str> {
    if observation.parse_state != ParseState::Exact {
        return Err("metric_not_exact");
    }
    if observation.canonical_unit.as_deref() != Some("ns") {
        return Err("metric_unit_unexpected");
    }
    let number = observation
        .canonical_number
        .as_ref()
        .ok_or("metric_not_exact")?;
    let value = decimal_to_pico(number).ok_or("metric_value_out_of_range")?;
    let span = value_span(
        value,
        &observation.fidelity,
        observation.bounds.as_ref(),
        true,
    )?;
    Ok((value, span))
}

/// The interval of a typed value, in picoseconds for durations or in plain
/// units for counts.
fn value_span(
    value: i128,
    fidelity: &Fidelity,
    bounds: Option<&BoundedInterval>,
    duration: bool,
) -> Result<Span, &'static str> {
    let endpoint = |number: &CanonicalNumber| {
        if duration {
            decimal_to_pico(number)
        } else {
            integer_count(number)
        }
    };
    match (fidelity, bounds) {
        (Fidelity::Exact, _) => Ok(Span::point(value)),
        (Fidelity::Rounded | Fidelity::Truncated | Fidelity::Interval, Some(bounds)) => {
            let span = Span {
                lower: endpoint(&bounds.lower).ok_or("metric_bounds_invalid")?,
                upper: endpoint(&bounds.upper).ok_or("metric_bounds_invalid")?,
                lower_inclusive: bounds.lower_inclusive,
                upper_inclusive: bounds.upper_inclusive,
            };
            (span.lower <= value && value <= span.upper)
                .then_some(span)
                .ok_or("metric_bounds_invalid")
        }
        _ => Err("metric_fidelity_unknown"),
    }
}

fn decimal_to_pico(number: &CanonicalNumber) -> Option<Pico> {
    let scale = number.scale.unwrap_or(0);
    if scale > 3 {
        return None;
    }
    let digits: String = number.value.chars().filter(|c| *c != '.').collect();
    let mantissa = digits.parse::<i128>().ok()?;
    if mantissa < 0 {
        return None;
    }
    mantissa.checked_mul(10_i128.pow(3 - scale))
}

fn integer_count(number: &CanonicalNumber) -> Option<i128> {
    if number.kind != NumberKind::Integer || number.scale.is_some() {
        return None;
    }
    number
        .value
        .parse::<i128>()
        .ok()
        .filter(|value| *value >= 0)
}

fn count_value(observation: &MetricObservation) -> Option<SummaryValue> {
    if observation.parse_state != ParseState::Exact {
        return None;
    }
    let number = observation.canonical_number.as_ref()?;
    let value = integer_count(number)?;
    let span = value_span(
        value,
        &observation.fidelity,
        observation.bounds.as_ref(),
        false,
    )
    .ok()?;
    Some(SummaryValue {
        state: SummaryValueState::Exact,
        canonical_number: Some(number.clone()),
        canonical_unit: observation.canonical_unit.clone(),
        display_text: Some(group_thousands(value)),
        reasons: vec![],
        source_refs: observation.source_refs.clone(),
        bounds: interval(span, false),
    })
}

fn query_duration(ir: &crate::NormalizedIr, metric_id: &str) -> SummaryValue {
    let cells = ir
        .derived_metric_cells
        .iter()
        .filter(|cell| cell.metric_id == metric_id)
        .collect::<Vec<_>>();
    let [cell] = cells.as_slice() else {
        return SummaryValue::unavailable(if cells.is_empty() {
            "metric_missing"
        } else {
            "metric_ambiguous"
        });
    };
    let MetricCellValue::Exact {
        canonical_number,
        canonical_unit,
        ..
    } = &cell.value
    else {
        return SummaryValue::unavailable("metric_not_exact");
    };
    if canonical_unit != "ns" {
        return SummaryValue::unavailable("metric_unit_unexpected");
    }
    let Some(value) = decimal_to_pico(canonical_number) else {
        return SummaryValue::unavailable("metric_not_exact");
    };
    let fidelity = cell.fidelity().unwrap_or(Fidelity::Unknown);
    match value_span(value, &fidelity, cell.bounds(), true) {
        Ok(span) => duration_value(value, span, cell.source_refs.clone()),
        Err(reason) => SummaryValue::unavailable(reason),
    }
}

/// An exact duration summary value: its official reading and its interval.
pub(crate) fn summary_span(value: &SummaryValue) -> Option<(Pico, Span)> {
    if value.state != SummaryValueState::Exact || value.canonical_unit.as_deref() != Some("ns") {
        return None;
    }
    let point = decimal_to_pico(value.canonical_number.as_ref()?)?;
    let span = match &value.bounds {
        None => Span::point(point),
        Some(bounds) => Span {
            lower: decimal_to_pico(&bounds.lower)?,
            upper: decimal_to_pico(&bounds.upper)?,
            lower_inclusive: bounds.lower_inclusive,
            upper_inclusive: bounds.upper_inclusive,
        },
    };
    Some((point, span))
}

fn duration_value(pico: Pico, span: Span, mut source_refs: Vec<String>) -> SummaryValue {
    source_refs.sort();
    source_refs.dedup();
    SummaryValue {
        state: SummaryValueState::Exact,
        canonical_number: Some(pico_number(pico, false)),
        canonical_unit: Some("ns".to_owned()),
        display_text: Some(display_duration(pico)),
        reasons: vec![],
        source_refs,
        bounds: interval(span, true),
    }
}

/// Picoseconds as canonical nanoseconds: an integer when whole (unless
/// `force_decimal`), else a three-digit decimal.
fn pico_number(pico: Pico, force_decimal: bool) -> CanonicalNumber {
    let whole = pico / PICO_PER_NS;
    let fraction = pico % PICO_PER_NS;
    if fraction == 0 && !force_decimal {
        CanonicalNumber {
            kind: NumberKind::Integer,
            value: whole.to_string(),
            scale: None,
        }
    } else {
        CanonicalNumber {
            kind: NumberKind::Decimal,
            value: format!("{whole}.{fraction:03}"),
            scale: Some(3),
        }
    }
}

/// `None` for a lossless point. Both endpoints share one number kind, as
/// `BoundedInterval` requires.
fn interval(span: Span, duration: bool) -> Option<BoundedInterval> {
    if span.is_point() {
        return None;
    }
    let (lower, upper) = if duration {
        let fractional = span.lower % PICO_PER_NS != 0 || span.upper % PICO_PER_NS != 0;
        (
            pico_number(span.lower, fractional),
            pico_number(span.upper, fractional),
        )
    } else {
        let number = |value: i128| CanonicalNumber {
            kind: NumberKind::Integer,
            value: value.to_string(),
            scale: None,
        };
        (number(span.lower), number(span.upper))
    };
    Some(BoundedInterval {
        lower,
        upper,
        lower_inclusive: span.lower_inclusive,
        upper_inclusive: span.upper_inclusive,
    })
}

/// Node times in the product's unit policy (`display::duration_ns`), from picoseconds rounded half-up to ns.
pub(crate) fn display_duration(pico: Pico) -> String {
    crate::display::duration_ns((pico + PICO_PER_NS / 2) / PICO_PER_NS)
}

pub(crate) fn group_thousands(value: i128) -> String {
    crate::display::count(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_follow_the_display_policy() {
        assert_eq!(display_duration(1_206_000 * PICO_PER_NS), "1.206 ms");
        assert_eq!(display_duration(815_696 * PICO_PER_NS), "815.696 μs");
        assert_eq!(display_duration(950 * PICO_PER_NS), "950 ns");
        assert_eq!(display_duration(2_000_000_000 * PICO_PER_NS), "2 s");
    }

    #[test]
    fn rows_are_grouped_by_thousands() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(50_000), "50,000");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn decimal_counters_keep_sub_nanosecond_precision() {
        let number = CanonicalNumber {
            kind: NumberKind::Decimal,
            value: "10.5".to_owned(),
            scale: Some(1),
        };
        assert_eq!(decimal_to_pico(&number), Some(10_500));
        let too_fine = CanonicalNumber {
            kind: NumberKind::Decimal,
            value: "1.2345".to_owned(),
            scale: Some(4),
        };
        assert_eq!(decimal_to_pico(&too_fine), None);
    }

    fn half_open(lower: i128, upper: i128) -> Span {
        Span {
            lower,
            upper,
            lower_inclusive: true,
            upper_inclusive: false,
        }
    }

    #[test]
    fn share_classes_follow_strict_official_thresholds() {
        let denominator = Span::point(1_000);
        assert_eq!(
            classify_share(Span::point(301), denominator),
            Some(ShareClass::Above30)
        );
        // Exactly 30% is not "> 30%" (ExplainAnalyzer uses a strict comparison).
        assert_eq!(
            classify_share(Span::point(300), denominator),
            Some(ShareClass::Above15)
        );
        assert_eq!(
            classify_share(Span::point(150), denominator),
            Some(ShareClass::NotAbove15)
        );
        assert_eq!(classify_share(Span::point(1), Span::point(0)), None);
    }

    #[test]
    fn a_precision_interval_across_a_threshold_is_a_boundary() {
        // [299, 302) / 1000 contains 30%: neither tier is decidable.
        assert_eq!(
            classify_share(half_open(299, 302), Span::point(1_000)),
            Some(ShareClass::Boundary30)
        );
        // [140, 160) / 1000 crosses 15%.
        assert_eq!(
            classify_share(half_open(140, 160), Span::point(1_000)),
            Some(ShareClass::Boundary15)
        );
        // The whole interval above 30% stays decidable.
        assert_eq!(
            classify_share(half_open(310, 320), half_open(1_000, 1_001)),
            Some(ShareClass::Above30)
        );
        // A numerator whose excluded upper end touches 30% never exceeds it.
        assert_eq!(
            classify_share(half_open(290, 300), Span::point(1_000)),
            Some(ShareClass::Above15)
        );
    }
}
