use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    AnalysisAttempt, AnalysisErrorAttempt, AnalysisOutcome, AnalysisOutcomeAttempt,
    AnalysisSnapshotReport, BoundedInterval, COMPARISON_SCHEMA_VERSION, CanonicalNumber,
    Comparison, ComparisonReceipt, ComparisonState, ErrorSummary, ExecutionShapeState, Fidelity,
    FindingDelta, FindingMeasurement, FindingMeasurementChange, MetricCell, MetricCellState,
    MetricDelta, MetricDeltaState, NumberKind, PercentageState, ReceiptComponent, StructureDelta,
};

use crate::{comparison_key, interval_contains_zero_by_closure};

const COMPARATOR_ID: &str = "doctor-comparator-v0.1";
const COMPARATOR_VERSION: &str = "0.6.0";
const COMPARATOR_SEMANTIC_HASH: &str =
    "aff0559dc605f985de214353451d2ebe8bc8d610bae549f796bbe005498e6e5b";
const REGISTRY_ID: &str = "sr3514-metric-registry";
const REGISTRY_010_HASH: &str = "af257364f97f03bca0f0b9b6e4901dd9df871b133e58e623811f0ba210c104a9";
const REGISTRY_011_HASH: &str = "850fa6244f1235dbc370b92f8ecf2f53a28789a9214b89f1e41485c7c4098d62";
const REGISTRY_EQUIVALENCE: &str = "sr3514.registry.same_metric@1";

#[derive(Clone, Copy)]
struct NumberRange {
    lower: i128,
    upper: i128,
    lower_inclusive: bool,
    upper_inclusive: bool,
    scale: u32,
}

pub fn analysis_attempt(report: AnalysisSnapshotReport) -> AnalysisAttempt {
    AnalysisAttempt::Outcome(AnalysisOutcomeAttempt {
        outcome: AnalysisOutcome { report },
    })
}

pub fn failed_attempt(
    code: &str,
    stage: &str,
    message_id: &str,
    recoverability: &str,
    affected_side: &str,
) -> AnalysisAttempt {
    AnalysisAttempt::Error(AnalysisErrorAttempt {
        error: ErrorSummary {
            code: code.into(),
            stage: stage.into(),
            message_id: message_id.into(),
            recoverability: recoverability.into(),
            affected_side: Some(affected_side.into()),
        },
    })
}

/// Compares only typed public facts. Input text, source views, filenames and replay keys are never read.
pub fn compare_attempts(baseline: AnalysisAttempt, candidate: AnalysisAttempt) -> Comparison {
    let mut comparison = empty_comparison(baseline, candidate);
    match (&comparison.baseline, &comparison.candidate) {
        (AnalysisAttempt::Error(_), _) => comparison.reasons.push("baseline_failed".into()),
        (_, AnalysisAttempt::Error(_)) => comparison.reasons.push("candidate_failed".into()),
        _ => compare_outcomes(&mut comparison),
    }
    // Reasons are a set: per-metric pushes repeat the same code.
    let mut seen = BTreeSet::new();
    comparison
        .reasons
        .retain(|reason| seen.insert(reason.clone()));
    if !comparison.reasons.is_empty() {
        comparison.limitations = comparison.reasons.clone();
    }
    comparison.comparison_key = comparison_key(&comparison);
    comparison
}

pub fn v01_comparison_receipt() -> ComparisonReceipt {
    ComparisonReceipt {
        components: vec![ReceiptComponent {
            id: COMPARATOR_ID.into(),
            version: COMPARATOR_VERSION.into(),
            manifest_hash: COMPARATOR_SEMANTIC_HASH.into(),
        }],
    }
}

fn empty_comparison(baseline: AnalysisAttempt, candidate: AnalysisAttempt) -> Comparison {
    Comparison {
        schema_version: COMPARISON_SCHEMA_VERSION.into(),
        comparison_receipt: v01_comparison_receipt(),
        comparison_key: String::new(),
        baseline,
        candidate,
        state: ComparisonState::NotComparable,
        reasons: Vec::new(),
        compatibility_delta: Vec::new(),
        structure_delta: StructureDelta {
            state: "not_comparable".into(),
            matches: vec![],
            added: vec![],
            removed: vec![],
            changed: vec![],
            edge_delta: vec![],
            unresolved: vec![],
        },
        metric_deltas: Vec::new(),
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
        diagnostics: Vec::new(),
        limitations: Vec::new(),
    }
}

fn compare_outcomes(comparison: &mut Comparison) {
    let (baseline, candidate) = match (&comparison.baseline, &comparison.candidate) {
        (AnalysisAttempt::Outcome(baseline), AnalysisAttempt::Outcome(candidate)) => {
            (&baseline.outcome.report, &candidate.outcome.report)
        }
        _ => return,
    };
    let registry_relation = registry_relation(baseline, candidate);
    if let RegistryRelation::Mismatch = registry_relation {
        comparison.reasons.push("registry_semantic_mismatch".into());
        comparison
            .compatibility_delta
            .push("registry_semantic_mismatch".into());
    } else if let RegistryRelation::Equivalent = registry_relation {
        comparison
            .compatibility_delta
            .push(format!("registry_equivalence:{REGISTRY_EQUIVALENCE}"));
    }
    comparison.finding_delta = finding_delta(baseline, candidate);
    if !comparison.finding_delta.algorithm_changed.is_empty() {
        comparison.reasons.push("finding_algorithm_changed".into());
    }
    if !comparison.finding_delta.incomparable.is_empty() {
        comparison.reasons.push("finding_not_comparable".into());
    }
    comparison.metric_deltas = metric_deltas(
        baseline,
        candidate,
        registry_relation,
        parallelism_relation(baseline, candidate),
        &mut comparison.reasons,
    );
    append_partial_reasons(
        baseline,
        candidate,
        &comparison.metric_deltas,
        &mut comparison.reasons,
    );
    comparison.structure_delta = structure_delta(baseline, candidate, &comparison.metric_deltas);
    if comparison.structure_delta.state == "not_comparable" {
        comparison.reasons.push("structure_not_comparable".into());
    }
    comparison.state = if comparison
        .metric_deltas
        .iter()
        .any(|delta| delta.state == MetricDeltaState::Comparable)
        || (comparison.reasons.is_empty() && comparison.metric_deltas.is_empty())
    {
        if comparison.reasons.is_empty() {
            ComparisonState::Comparable
        } else {
            ComparisonState::PartiallyComparable
        }
    } else {
        ComparisonState::PartiallyComparable
    };
}

#[derive(Clone, Copy)]
enum RegistryRelation {
    Same,
    Equivalent,
    Mismatch,
}

fn registry_relation(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
) -> RegistryRelation {
    let (Some(left), Some(right)) = (registry_component(baseline), registry_component(candidate))
    else {
        return RegistryRelation::Mismatch;
    };
    if left == right {
        return RegistryRelation::Same;
    }
    let accepted_pair = (left.manifest_hash == REGISTRY_010_HASH
        && right.manifest_hash == REGISTRY_011_HASH)
        || (left.manifest_hash == REGISTRY_011_HASH && right.manifest_hash == REGISTRY_010_HASH);
    if accepted_pair && left.id == REGISTRY_ID && right.id == REGISTRY_ID {
        RegistryRelation::Equivalent
    } else {
        RegistryRelation::Mismatch
    }
}

/// Whether both Profiles ran with the same fragment and instance counts.
/// Query counters summed over instance reports (see
/// `sr3514_instance_report_sum_limitation`) are only comparable when they do.
#[derive(Clone, Copy, PartialEq)]
enum ParallelismRelation {
    Same,
    Different,
    Unknown,
}

fn parallelism_relation(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
) -> ParallelismRelation {
    let counts = |report: &AnalysisSnapshotReport| {
        let shape = &report.execution_shape;
        if shape.state != ExecutionShapeState::Complete {
            return None;
        }
        Some((shape.fragment_count?, shape.instance_count?))
    };
    match (counts(baseline), counts(candidate)) {
        (Some(left), Some(right)) if left == right => ParallelismRelation::Same,
        (Some(_), Some(_)) => ParallelismRelation::Different,
        _ => ParallelismRelation::Unknown,
    }
}

fn registry_component(report: &AnalysisSnapshotReport) -> Option<&ReceiptComponent> {
    report
        .analysis_receipt
        .components
        .iter()
        .find(|component| component.id == REGISTRY_ID)
}

fn append_partial_reasons(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
    deltas: &[MetricDelta],
    reasons: &mut Vec<String>,
) {
    if deltas
        .iter()
        .any(|delta| delta.state == MetricDeltaState::NotComparable || delta.bounds.is_some())
    {
        return;
    }
    if baseline.analysis_state == doctor_contracts::AnalysisState::Partial {
        reasons.push("baseline_partial".into());
    }
    if candidate.analysis_state == doctor_contracts::AnalysisState::Partial {
        reasons.push("candidate_partial".into());
    }
}

fn structure_delta(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
    deltas: &[MetricDelta],
) -> StructureDelta {
    if baseline.graph_view.state == doctor_contracts::GraphState::Unavailable
        || candidate.graph_view.state == doctor_contracts::GraphState::Unavailable
    {
        return StructureDelta {
            state: "not_comparable".into(),
            matches: vec![],
            added: vec![],
            removed: vec![],
            changed: vec![],
            edge_delta: vec![],
            unresolved: vec!["graph_unavailable".into()],
        };
    }
    let baseline_ids = baseline
        .graph_view
        .nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect::<BTreeSet<_>>();
    let candidate_ids = candidate
        .graph_view
        .nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect::<BTreeSet<_>>();
    let matches = baseline_ids
        .intersection(&candidate_ids)
        .map(|id| format!("{id}:stable_id@1"))
        .collect::<Vec<_>>();
    let added = candidate_ids
        .difference(&baseline_ids)
        .cloned()
        .collect::<Vec<_>>();
    let removed = baseline_ids
        .difference(&candidate_ids)
        .cloned()
        .collect::<Vec<_>>();
    let baseline_edges = baseline
        .graph_view
        .edges
        .iter()
        .map(|edge| (edge.edge_id.clone(), edge_semantics(edge)))
        .collect::<BTreeMap<_, _>>();
    let candidate_edges = candidate
        .graph_view
        .edges
        .iter()
        .map(|edge| (edge.edge_id.clone(), edge_semantics(edge)))
        .collect::<BTreeMap<_, _>>();
    let edge_delta = baseline_edges
        .keys()
        .chain(candidate_edges.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|id| baseline_edges.get(*id) != candidate_edges.get(*id))
        .cloned()
        .collect::<Vec<_>>();
    // Comparator 0.4.0: a node matched by stable id whose printed plan facts
    // (table, predicate, join condition) differ. Fact order is not compared.
    let changed = baseline
        .graph_view
        .nodes
        .iter()
        .filter(|node| candidate_ids.contains(&node.node_id))
        .filter(|node| {
            candidate
                .graph_view
                .nodes
                .iter()
                .find(|other| other.node_id == node.node_id)
                .is_some_and(|other| {
                    node.entity_kind != other.entity_kind
                        || node.plan_node_type != other.plan_node_type
                        || plan_facts(node) != plan_facts(other)
                })
        })
        .map(|node| node.node_id.clone())
        .collect::<Vec<_>>();
    let unchanged =
        added.is_empty() && removed.is_empty() && changed.is_empty() && edge_delta.is_empty();
    let partial = baseline.analysis_state == doctor_contracts::AnalysisState::Partial
        || candidate.analysis_state == doctor_contracts::AnalysisState::Partial;
    let unresolved = if !partial {
        vec![]
    } else if baseline.analysis_state == doctor_contracts::AnalysisState::Partial
        && candidate.analysis_state == doctor_contracts::AnalysisState::Partial
        && deltas.iter().any(|delta| delta.bounds.is_some())
    {
        vec!["partial_inputs".into()]
    } else {
        [
            (
                baseline.analysis_state == doctor_contracts::AnalysisState::Partial,
                "baseline_partial",
            ),
            (
                candidate.analysis_state == doctor_contracts::AnalysisState::Partial,
                "candidate_partial",
            ),
        ]
        .into_iter()
        .filter_map(|(present, reason)| present.then_some(reason.into()))
        .collect()
    };
    let state = if partial {
        "partial"
    } else if unchanged {
        "unchanged"
    } else {
        "changed"
    };
    StructureDelta {
        state: state.into(),
        matches,
        added,
        removed,
        changed,
        edge_delta,
        unresolved,
    }
}

fn edge_semantics(edge: &doctor_contracts::GraphEdge) -> String {
    format!(
        "{:?}",
        (
            &edge.from,
            &edge.to,
            &edge.relation,
            &edge.direction_semantics,
            &edge.source,
            &edge.quality,
            &edge.source_ordinal,
            &edge.input_role,
            &edge.derivation_recipe,
            &edge.restates_edge_id
        )
    )
}

/// The node's printed plan facts as an order-free set of (kind, value).
fn plan_facts(node: &doctor_contracts::GraphNode) -> BTreeSet<(String, String)> {
    node.sql_facts
        .iter()
        .flatten()
        .map(|fact| (format!("{:?}", fact.kind), fact.value.clone()))
        .collect()
}

fn finding_delta(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
) -> FindingDelta {
    let key = |finding: &doctor_contracts::Finding| {
        (
            finding.rule_id.clone(),
            finding.subject_ref.clone().unwrap_or_default(),
        )
    };
    let baseline_findings = baseline
        .findings
        .iter()
        .map(|finding| (key(finding), finding))
        .collect::<BTreeMap<_, _>>();
    let candidate_findings = candidate
        .findings
        .iter()
        .map(|finding| (key(finding), finding))
        .collect::<BTreeMap<_, _>>();
    let mut delta = FindingDelta {
        added: vec![],
        resolved: vec![],
        persisting: vec![],
        changed: vec![],
        incomparable: vec![],
        algorithm_changed: vec![],
        partial_evidence: vec![],
        measurement_changes: Vec::new(),
    };
    for (key, baseline_finding) in &baseline_findings {
        match candidate_findings.get(key) {
            None => {
                let entry = format!("{}:{}", key.0, key.1);
                push_measurement_changes(&mut delta, &entry, Some(baseline_finding), None);
                if let Some(evidence) = comparable_absence(baseline, candidate, baseline_finding) {
                    if evidence == doctor_contracts::EvidenceCompleteness::Partial {
                        delta.partial_evidence.push(entry.clone());
                    }
                    delta.resolved.push(entry);
                } else {
                    delta.incomparable.push(entry);
                }
            }
            Some(candidate_finding)
                if baseline_finding.rule_version != candidate_finding.rule_version =>
            {
                delta.algorithm_changed.push(format!(
                    "{}:{}:{}->{}",
                    key.0, key.1, baseline_finding.rule_version, candidate_finding.rule_version
                ));
            }
            Some(candidate_finding) => {
                let entry = format!("{}:{}", key.0, key.1);
                push_measurement_changes(
                    &mut delta,
                    &entry,
                    Some(baseline_finding),
                    Some(candidate_finding),
                );
                if *baseline_finding != *candidate_finding {
                    delta.changed.push(entry);
                } else {
                    delta.persisting.push(entry);
                }
            }
        }
    }
    for (key, candidate_finding) in candidate_findings
        .iter()
        .filter(|(key, _)| !baseline_findings.contains_key(*key))
    {
        let entry = format!("{}:{}", key.0, key.1);
        push_measurement_changes(&mut delta, &entry, None, Some(candidate_finding));
        if let Some(evidence) = comparable_absence(candidate, baseline, candidate_finding) {
            if evidence == doctor_contracts::EvidenceCompleteness::Partial {
                delta.partial_evidence.push(entry.clone());
            }
            delta.added.push(entry);
        } else {
            delta.incomparable.push(entry);
        }
    }
    delta
}

/// An absent finding proves nothing unless this exact rule/object/version
/// had a complete or partial negative evaluation with no missing prerequisites.
/// Partial evidence on either side is returned for explicit labelling.
/// NotApplicable, Suppressed, unavailable inputs and ambiguous evaluations
/// never prove resolution. This is a product decision, not an official rule.
fn comparable_absence(
    present: &AnalysisSnapshotReport,
    report: &AnalysisSnapshotReport,
    finding: &doctor_contracts::Finding,
) -> Option<doctor_contracts::EvidenceCompleteness> {
    use doctor_contracts::{EvidenceCompleteness, RuleEvaluationStatus};
    let mut matches = report.rule_evaluations.iter().filter(|evaluation| {
        evaluation.rule_id == finding.rule_id
            && evaluation.subject_ref == finding.subject_ref
            && evaluation.rule_version == finding.rule_version
    });
    let evaluation = matches.next()?;
    let comparable = matches.next().is_none()
        && finding.subject_ref.is_some()
        && evaluation.status == RuleEvaluationStatus::NotTriggered
        && evaluation.evidence_completeness != EvidenceCompleteness::Unavailable
        && evaluation.missing_prerequisites.is_empty()
        && finding.evidence_completeness != EvidenceCompleteness::Unavailable
        && matches!(
            report.version.compatibility,
            doctor_contracts::Compatibility::Compatible | doctor_contracts::Compatibility::Partial
        )
        && present.version.effective_version.is_some()
        && present.version.effective_version == report.version.effective_version
        && present.version.selected_pack == report.version.selected_pack
        && !matches!(
            registry_relation(present, report),
            RegistryRelation::Mismatch
        );
    comparable.then_some(
        if finding.evidence_completeness == EvidenceCompleteness::Partial
            || evaluation.evidence_completeness == EvidenceCompleteness::Partial
        {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Complete
        },
    )
}

/// Pairs the two sides' rule measurements by `measurement_id`, baseline order
/// first. Values are copied display text, never subtracted: a measurement is
/// already a ratio or bounded reading whose difference has no rule meaning.
fn push_measurement_changes(
    delta: &mut FindingDelta,
    entry: &str,
    baseline: Option<&doctor_contracts::Finding>,
    candidate: Option<&doctor_contracts::Finding>,
) {
    fn measurements(finding: Option<&doctor_contracts::Finding>) -> &[FindingMeasurement] {
        finding.map_or(&[], |finding| finding.measurements.as_slice())
    }
    let mut ids = Vec::<&str>::new();
    for measurement in measurements(baseline).iter().chain(measurements(candidate)) {
        if !ids.contains(&measurement.measurement_id.as_str()) {
            ids.push(&measurement.measurement_id);
        }
    }
    for id in ids {
        let find = |finding| {
            measurements(finding)
                .iter()
                .find(|measurement| measurement.measurement_id == id)
        };
        let before = find(baseline);
        let after = find(candidate);
        delta.measurement_changes.push(FindingMeasurementChange {
            finding_entry: entry.to_owned(),
            measurement_id: id.to_owned(),
            baseline_display_text: before.map(|measurement| measurement.display_text.clone()),
            candidate_display_text: after.map(|measurement| measurement.display_text.clone()),
            baseline_threshold_text: before
                .and_then(|measurement| measurement.threshold_text.clone()),
            candidate_threshold_text: after
                .and_then(|measurement| measurement.threshold_text.clone()),
        });
    }
}

fn metric_deltas(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
    registry: RegistryRelation,
    parallelism: ParallelismRelation,
    reasons: &mut Vec<String>,
) -> Vec<MetricDelta> {
    // Different releases never reach a metric delta (`comparable_reports`).
    let baseline_release = baseline.version.effective_version.as_deref();
    let baseline_groups = metric_map(baseline);
    let candidate_groups = metric_map(candidate);
    let groups = baseline_groups
        .keys()
        .chain(candidate_groups.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut deltas = Vec::new();
    for group in groups {
        let baseline_cells = baseline_groups.get(&group).map_or(&[][..], Vec::as_slice);
        let candidate_cells = candidate_groups.get(&group).map_or(&[][..], Vec::as_slice);
        for (baseline, candidate, ambiguous) in pair_cells(baseline_cells, candidate_cells) {
            deltas.push(metric_delta(
                group.0.clone(),
                group.1.clone(),
                baseline,
                candidate,
                ambiguous,
                ReportRelations {
                    registry,
                    parallelism,
                    instance_report_sum: crate::metrics::instance_report_sum_limitation(
                        baseline_release,
                        &group.0,
                        &group.1,
                    )
                    .is_some(),
                },
                reasons,
            ));
        }
    }
    deltas
}

type CellPair<'a> = (Option<&'a MetricCell>, Option<&'a MetricCell>, bool);

/// Pairs one (metric, subject) group. One cell per side pairs directly.
/// Several cells (physical operators or MIN/MAX statistics sharing a plan
/// node) pair only by `match_key`, and only when every cell on both sides
/// carries a unique one; otherwise the group stays a single ambiguous delta.
fn pair_cells<'a>(baseline: &[&'a MetricCell], candidate: &[&'a MetricCell]) -> Vec<CellPair<'a>> {
    if baseline.len() <= 1 && candidate.len() <= 1 {
        return vec![(baseline.first().copied(), candidate.first().copied(), false)];
    }
    let keyed = |cells: &[&'a MetricCell]| {
        let map = cells
            .iter()
            .map(|cell| cell.match_key.as_deref().map(|key| (key, *cell)))
            .collect::<Option<BTreeMap<_, _>>>()?;
        (map.len() == cells.len()).then_some(map)
    };
    match (keyed(baseline), keyed(candidate)) {
        (Some(baseline), Some(candidate)) => baseline
            .keys()
            .chain(candidate.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|key| {
                (
                    baseline.get(key).copied(),
                    candidate.get(key).copied(),
                    false,
                )
            })
            .collect(),
        _ => vec![(
            (baseline.len() == 1).then(|| baseline[0]),
            (candidate.len() == 1).then(|| candidate[0]),
            true,
        )],
    }
}

/// Report-level facts every metric delta of one comparison shares.
#[derive(Clone, Copy)]
struct ReportRelations {
    registry: RegistryRelation,
    parallelism: ParallelismRelation,
    /// The metric repeats a BE value once per fragment instance in this release.
    instance_report_sum: bool,
}

fn metric_delta(
    metric_id: String,
    subject_match: String,
    baseline: Option<&MetricCell>,
    candidate: Option<&MetricCell>,
    ambiguous: bool,
    relations: ReportRelations,
    reasons: &mut Vec<String>,
) -> MetricDelta {
    let ReportRelations {
        registry,
        parallelism,
        instance_report_sum,
    } = relations;
    if ambiguous {
        reasons.push("duplicate_metric_match".into());
        return not_comparable_delta(
            metric_id,
            subject_match,
            baseline,
            candidate,
            PercentageState::SemanticMismatch,
            "duplicate_metric_match",
        );
    }
    if matches!(registry, RegistryRelation::Mismatch) {
        return not_comparable_delta(
            metric_id,
            subject_match,
            baseline,
            candidate,
            PercentageState::SemanticMismatch,
            "registry_semantic_mismatch",
        );
    }
    let parallelism_gate = match parallelism {
        ParallelismRelation::Same => None,
        ParallelismRelation::Different => Some((
            PercentageState::SemanticMismatch,
            "parallelism_shape_mismatch",
        )),
        ParallelismRelation::Unknown => {
            Some((PercentageState::Missing, "parallelism_shape_unavailable"))
        }
    }
    .filter(|_| instance_report_sum);
    match (baseline, candidate) {
        (Some(baseline), Some(candidate))
            if baseline.canonical_unit() == candidate.canonical_unit() =>
        {
            if let Some((percentage, limitation)) = parallelism_gate {
                reasons.push(limitation.into());
                return not_comparable_delta(
                    metric_id,
                    subject_match,
                    Some(baseline),
                    Some(candidate),
                    percentage,
                    limitation,
                );
            }
            numeric_delta(metric_id, subject_match, baseline, candidate, reasons)
        }
        (Some(baseline), Some(candidate)) => not_comparable_delta(
            metric_id,
            subject_match,
            Some(baseline),
            Some(candidate),
            PercentageState::SemanticMismatch,
            "metric_unit_mismatch",
        ),
        _ => not_comparable_delta(
            metric_id,
            subject_match,
            baseline,
            candidate,
            PercentageState::Missing,
            "metric_missing_on_one_side",
        ),
    }
}

fn metric_map(report: &AnalysisSnapshotReport) -> BTreeMap<(String, String), Vec<&MetricCell>> {
    let mut values: BTreeMap<(String, String), Vec<&MetricCell>> = BTreeMap::new();
    for cell in report.derived_metric_cells.iter().chain(
        report
            .operator_rows
            .iter()
            .flat_map(|row| row.metric_cells.iter()),
    ) {
        values
            .entry((cell.metric_id.clone(), cell.subject_ref.clone()))
            .or_default()
            .push(cell);
    }
    values
}

fn numeric_delta(
    metric_id: String,
    subject_match: String,
    baseline: &MetricCell,
    candidate: &MetricCell,
    reasons: &mut Vec<String>,
) -> MetricDelta {
    let (Some(base), Some(candidate_range)) = (number_range(baseline), number_range(candidate))
    else {
        let state = baseline.state();
        let percentage = match state {
            MetricCellState::Missing | MetricCellState::Unavailable | MetricCellState::Unknown => {
                PercentageState::Missing
            }
            MetricCellState::Incompatible => PercentageState::SemanticMismatch,
            _ => PercentageState::Invalid,
        };
        reasons.push(format!("baseline_metric_{}", state_name(state)));
        return not_comparable_delta(
            metric_id,
            subject_match,
            Some(baseline),
            Some(candidate),
            percentage,
            "metric_state_not_comparable",
        );
    };
    let scale = base.scale.max(candidate_range.scale);
    let (Some(base), Some(candidate_range)) = (base.align(scale), candidate_range.align(scale))
    else {
        reasons.push("metric_delta_overflow".into());
        return not_comparable_delta(
            metric_id,
            subject_match,
            Some(baseline),
            Some(candidate),
            PercentageState::Invalid,
            "metric_delta_overflow",
        );
    };
    let bounded = baseline.bounds().is_some() || candidate.bounds().is_some();
    if !bounded {
        let Some(delta) = candidate_range.lower.checked_sub(base.lower) else {
            reasons.push("metric_delta_overflow".into());
            return not_comparable_delta(
                metric_id,
                subject_match,
                Some(baseline),
                Some(candidate),
                PercentageState::Invalid,
                "metric_delta_overflow",
            );
        };
        let absolute = number_from_scaled(delta, scale);
        let percentage = if base.lower == 0 {
            PercentageState::UndefinedBaselineZero
        } else {
            PercentageState::Computed
        };
        let computed_percentage = percentage == PercentageState::Computed;
        return MetricDelta {
            metric_id,
            subject_match: Some(subject_match),
            state: MetricDeltaState::Comparable,
            baseline_metric_cell_ref: Some(baseline.metric_cell_ref.clone()),
            candidate_metric_cell_ref: Some(candidate.metric_cell_ref.clone()),
            absolute_delta: Some(absolute),
            percentage_state: percentage,
            percentage_value: computed_percentage
                .then(|| {
                    candidate_range
                        .lower
                        .checked_sub(base.lower)
                        .and_then(|delta| rational_scaled(delta, base.lower, 9, Rounding::Nearest))
                })
                .flatten(),
            fidelity: Some(Fidelity::Exact),
            bounds: None,
            percentage_bounds: None,
            order_rank: candidate.order_rank.or(baseline.order_rank),
            limitations: vec![],
        };
    }
    let lower = candidate_range.lower.checked_sub(base.upper);
    let upper = candidate_range.upper.checked_sub(base.lower);
    let (Some(lower), Some(upper)) = (lower, upper) else {
        reasons.push("metric_delta_overflow".into());
        return not_comparable_delta(
            metric_id,
            subject_match,
            Some(baseline),
            Some(candidate),
            PercentageState::Invalid,
            "metric_delta_overflow",
        );
    };
    let bounds = BoundedInterval {
        lower: number_from_scaled(lower, scale),
        upper: number_from_scaled(upper, scale),
        lower_inclusive: candidate_range.lower_inclusive && base.upper_inclusive,
        upper_inclusive: candidate_range.upper_inclusive && base.lower_inclusive,
    };
    let percentage_state = if interval_contains_zero_by_closure(&BoundedInterval {
        lower: number_from_scaled(base.lower, scale),
        upper: number_from_scaled(base.upper, scale),
        lower_inclusive: base.lower_inclusive,
        upper_inclusive: base.upper_inclusive,
    }) {
        PercentageState::UndefinedBaselineIntervalContainsZero
    } else {
        PercentageState::Approximate
    };
    let percentage_bounds = (percentage_state == PercentageState::Approximate)
        .then(|| percentage_bounds(candidate_range, base, scale))
        .flatten();
    reasons.push("bounded_metric_delta".into());
    MetricDelta {
        metric_id,
        subject_match: Some(subject_match),
        state: MetricDeltaState::Comparable,
        baseline_metric_cell_ref: Some(baseline.metric_cell_ref.clone()),
        candidate_metric_cell_ref: Some(candidate.metric_cell_ref.clone()),
        absolute_delta: None,
        percentage_state,
        percentage_value: None,
        fidelity: Some(Fidelity::Interval),
        bounds: Some(bounds),
        percentage_bounds,
        order_rank: candidate.order_rank.or(baseline.order_rank),
        limitations: vec!["bounded_metric_delta".into()],
    }
}

fn not_comparable_delta(
    metric_id: String,
    subject_match: String,
    baseline: Option<&MetricCell>,
    candidate: Option<&MetricCell>,
    percentage_state: PercentageState,
    limitation: &str,
) -> MetricDelta {
    MetricDelta {
        metric_id,
        subject_match: Some(subject_match),
        state: MetricDeltaState::NotComparable,
        baseline_metric_cell_ref: baseline.map(|cell| cell.metric_cell_ref.clone()),
        candidate_metric_cell_ref: candidate.map(|cell| cell.metric_cell_ref.clone()),
        absolute_delta: None,
        percentage_state,
        percentage_value: None,
        fidelity: None,
        bounds: None,
        percentage_bounds: None,
        order_rank: candidate
            .and_then(|cell| cell.order_rank)
            .or_else(|| baseline.and_then(|cell| cell.order_rank)),
        limitations: vec![limitation.into()],
    }
}

impl NumberRange {
    fn align(self, scale: u32) -> Option<Self> {
        let factor = power10(scale.checked_sub(self.scale)?)?;
        Some(Self {
            lower: self.lower.checked_mul(factor)?,
            upper: self.upper.checked_mul(factor)?,
            scale,
            ..self
        })
    }
}

fn number_range(cell: &MetricCell) -> Option<NumberRange> {
    if cell.state() != MetricCellState::Exact {
        return None;
    }
    let number = cell.canonical_number()?;
    if let Some(bounds) = cell.bounds() {
        let (lower, lower_scale) = number_to_scaled(&bounds.lower)?;
        let (upper, upper_scale) = number_to_scaled(&bounds.upper)?;
        let scale = lower_scale.max(upper_scale);
        let lower = lower.checked_mul(power10(scale - lower_scale)?)?;
        let upper = upper.checked_mul(power10(scale - upper_scale)?)?;
        Some(NumberRange {
            lower,
            upper,
            lower_inclusive: bounds.lower_inclusive,
            upper_inclusive: bounds.upper_inclusive,
            scale,
        })
    } else {
        let (value, scale) = number_to_scaled(number)?;
        Some(NumberRange {
            lower: value,
            upper: value,
            lower_inclusive: true,
            upper_inclusive: true,
            scale,
        })
    }
}

fn number_to_scaled(number: &CanonicalNumber) -> Option<(i128, u32)> {
    let negative = number.value.starts_with('-');
    let unsigned = number.value.strip_prefix('-').unwrap_or(&number.value);
    let scale = match number.kind {
        NumberKind::Integer => 0,
        NumberKind::Decimal => number.scale?,
    };
    let digits = unsigned.replace('.', "");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = digits.parse::<i128>().ok()?;
    Some((if negative { -value } else { value }, scale))
}

fn number_from_scaled(value: i128, scale: u32) -> CanonicalNumber {
    if scale == 0 {
        return CanonicalNumber {
            kind: NumberKind::Integer,
            value: value.to_string(),
            scale: None,
        };
    }
    let negative = value.is_negative();
    let mut digits = value.unsigned_abs().to_string();
    let scale_usize = scale as usize;
    if digits.len() <= scale_usize {
        digits = format!("{}{}", "0".repeat(scale_usize + 1 - digits.len()), digits);
    }
    let split = digits.len() - scale_usize;
    CanonicalNumber {
        kind: NumberKind::Decimal,
        value: format!(
            "{}{}.{}",
            if negative { "-" } else { "" },
            &digits[..split],
            &digits[split..]
        ),
        scale: Some(scale),
    }
}

fn power10(scale: u32) -> Option<i128> {
    (0..scale).try_fold(1_i128, |value, _| value.checked_mul(10))
}

#[derive(Clone, Copy)]
enum Rounding {
    Floor,
    Ceil,
    Nearest,
}

fn rational_scaled(
    numerator: i128,
    denominator: i128,
    scale: u32,
    rounding: Rounding,
) -> Option<CanonicalNumber> {
    if denominator <= 0 {
        return None;
    }
    let scaled = numerator.checked_mul(power10(scale)?)?;
    let quotient = scaled / denominator;
    let remainder = scaled % denominator;
    let value = match rounding {
        Rounding::Floor if remainder < 0 => quotient.checked_sub(1)?,
        Rounding::Ceil if remainder > 0 => quotient.checked_add(1)?,
        Rounding::Nearest if remainder.unsigned_abs().checked_mul(2)? >= denominator as u128 => {
            if scaled.is_negative() {
                quotient.checked_sub(1)?
            } else {
                quotient.checked_add(1)?
            }
        }
        _ => quotient,
    };
    Some(number_from_scaled(value, scale))
}

fn percentage_bounds(
    candidate: NumberRange,
    baseline: NumberRange,
    _scale: u32,
) -> Option<BoundedInterval> {
    if baseline.lower <= 0 || baseline.upper <= 0 {
        return None;
    }
    let lower = rational_scaled(
        candidate.lower.checked_sub(baseline.upper)?,
        baseline.upper,
        9,
        Rounding::Floor,
    )?;
    let upper = rational_scaled(
        candidate.upper.checked_sub(baseline.lower)?,
        baseline.lower,
        9,
        Rounding::Ceil,
    )?;
    Some(BoundedInterval {
        lower,
        upper,
        lower_inclusive: true,
        upper_inclusive: true,
    })
}

fn state_name(state: MetricCellState) -> &'static str {
    match state {
        MetricCellState::Exact => "exact",
        MetricCellState::Missing => "missing",
        MetricCellState::Invalid => "invalid",
        MetricCellState::Partial => "partial",
        MetricCellState::Overflow => "overflow",
        MetricCellState::Unavailable => "unavailable",
        MetricCellState::Incompatible => "incompatible",
        MetricCellState::Unknown => "unknown",
    }
}
