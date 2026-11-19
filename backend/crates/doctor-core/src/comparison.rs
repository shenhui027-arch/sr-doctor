use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    AnalysisAttempt, AnalysisErrorAttempt, AnalysisOutcome, AnalysisOutcomeAttempt,
    AnalysisSnapshotReport, BoundedInterval, COMPARISON_SCHEMA_VERSION, CanonicalNumber,
    Comparison, ComparisonReceipt, ComparisonState, Compatibility, ErrorSummary,
    ExecutionShapeState, Fidelity, FindingDelta, FindingMeasurement, FindingMeasurementChange,
    GraphState, MetricCell, MetricCellState, MetricDelta, MetricDeltaState, NumberKind,
    PercentageState, ReceiptComponent, SourceKind, StructureDelta, VersionContext, VersionSource,
};

use crate::{comparison_key, interval_contains_zero_by_closure};

const COMPARATOR_ID: &str = "doctor-comparator-v0.1";
const COMPARATOR_VERSION: &str = "0.7.0";
const COMPARATOR_SEMANTIC_HASH: &str =
    "88a8cdf6f4ce000172dd33363f2f3e0770a6469c8590fda95f553d4fe3a5228a";
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
    let version = version_relation(&baseline.version, &candidate.version);
    let version_reason = match version {
        VersionRelation::Same => None,
        VersionRelation::Unknown
            if [&baseline.version, &candidate.version]
                .iter()
                .all(|context| {
                    context.version_source == VersionSource::Detected
                        && context.reported_version.is_some()
                        && context.effective_version.is_some()
                        && matches!(
                            context.compatibility,
                            Compatibility::Compatible | Compatibility::Partial
                        )
                })
                && (baseline.version.selected_pack.is_none()
                    || candidate.version.selected_pack.is_none()) =>
        {
            Some("no_registered_pack")
        }
        VersionRelation::Unknown => Some("version_unknown"),
        VersionRelation::Incompatible => Some("version_incompatible"),
        VersionRelation::ReleaseMismatch => Some("starrocks_version_mismatch"),
        VersionRelation::PackMismatch => Some("version_pack_mismatch"),
    };
    let registry = registry_relation(baseline, candidate);
    if let Some(reason) = version_reason {
        comparison.reasons.push(reason.into());
        comparison.compatibility_delta.push(reason.into());
    } else {
        match registry {
            RegistryRelation::Same => {}
            RegistryRelation::Mismatch => {
                comparison.reasons.push("registry_semantic_mismatch".into());
                comparison
                    .compatibility_delta
                    .push("registry_semantic_mismatch".into());
            }
            RegistryRelation::Equivalent => comparison
                .compatibility_delta
                .push(format!("registry_equivalence:{REGISTRY_EQUIVALENCE}")),
        }
    }
    comparison.finding_delta = finding_delta(baseline, candidate, version, registry);
    if !comparison.finding_delta.algorithm_changed.is_empty() {
        comparison.reasons.push("finding_algorithm_changed".into());
    }
    if !comparison.finding_delta.incomparable.is_empty() {
        comparison.reasons.push("finding_not_comparable".into());
    }
    let precision_unknown = [baseline, candidate].iter().any(|report| {
        report.input.detected_kind == SourceKind::JsonProfile
            && report.query_summary.is_some()
            && !report.analysis_receipt.components.iter().any(|component| {
                component.id == "sr3514-runtime-profile-adapter" && component.version == "0.2.4"
            })
    });
    comparison.metric_deltas = metric_deltas(
        baseline,
        candidate,
        ReportRelations {
            version_reason,
            registry,
            precision_unknown,
            parallelism: parallelism_relation(baseline, candidate),
        },
        &mut comparison.reasons,
    );
    append_partial_reasons(
        baseline,
        candidate,
        &comparison.metric_deltas,
        &mut comparison.reasons,
    );
    comparison.structure_delta = structure_delta(
        baseline,
        candidate,
        version_reason,
        &comparison.metric_deltas,
    );
    if comparison.structure_delta.state == "not_comparable" {
        comparison.reasons.push("structure_not_comparable".into());
    }
    comparison.state = comparison_state(comparison);
}

#[derive(Clone, Copy, PartialEq)]
enum VersionRelation {
    Same,
    Unknown,
    Incompatible,
    ReleaseMismatch,
    PackMismatch,
}

fn version_relation(baseline: &VersionContext, candidate: &VersionContext) -> VersionRelation {
    if [baseline, candidate].iter().any(|context| {
        context.version_source != VersionSource::Detected
            || context.reported_version.is_none()
            || context.effective_version.is_none()
    }) {
        return VersionRelation::Unknown;
    }
    if [baseline, candidate].iter().any(|context| {
        context
            .reported_version
            .as_ref()
            .map(|evidence| &evidence.value)
            != context.effective_version.as_ref()
            || context.compatibility == Compatibility::Incompatible
    }) {
        return VersionRelation::Incompatible;
    }
    if baseline.effective_version != candidate.effective_version {
        return VersionRelation::ReleaseMismatch;
    }
    if [baseline, candidate].iter().any(|context| {
        context.selected_pack.is_none() || context.compatibility == Compatibility::Inconclusive
    }) {
        return VersionRelation::Unknown;
    }
    if baseline.selected_pack != candidate.selected_pack {
        return VersionRelation::PackMismatch;
    }
    VersionRelation::Same
}

fn comparison_state(comparison: &Comparison) -> ComparisonState {
    let structure = &comparison.structure_delta;
    let findings = &comparison.finding_delta;
    let comparable = if comparison.metric_deltas.is_empty() {
        (structure.state != "not_comparable"
            && [
                &structure.matches,
                &structure.added,
                &structure.removed,
                &structure.changed,
                &structure.edge_delta,
            ]
            .iter()
            .any(|entries| !entries.is_empty()))
            || [
                &findings.added,
                &findings.resolved,
                &findings.changed,
                &findings.persisting,
            ]
            .iter()
            .any(|entries| !entries.is_empty())
    } else {
        comparison
            .metric_deltas
            .iter()
            .any(|delta| delta.state == MetricDeltaState::Comparable)
    };
    if !comparable {
        ComparisonState::NotComparable
    } else if !comparison.reasons.is_empty()
        || structure.state == "partial"
        || !findings.partial_evidence.is_empty()
        || comparison
            .metric_deltas
            .iter()
            .any(|delta| delta.state == MetricDeltaState::NotComparable || delta.bounds.is_some())
    {
        ComparisonState::PartiallyComparable
    } else {
        ComparisonState::Comparable
    }
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
    version_reason: Option<&str>,
    deltas: &[MetricDelta],
) -> StructureDelta {
    let mut blockers = BTreeSet::new();
    if let Some(reason) = version_reason {
        blockers.insert(reason.to_owned());
    } else {
        for graph in [&baseline.graph_view, &candidate.graph_view] {
            match graph.state {
                GraphState::Unavailable => {
                    blockers.insert("graph_unavailable".into());
                }
                GraphState::Complete => blockers.extend(
                    crate::graph::plan_child_issues(
                        &graph.root_node_ids,
                        &graph.nodes,
                        &graph.edges,
                    )
                    .into_iter()
                    .map(|issue| issue.reason),
                ),
                GraphState::Partial => {}
            }
        }
    }
    if !blockers.is_empty() {
        return StructureDelta {
            state: "not_comparable".into(),
            matches: vec![],
            added: vec![],
            removed: vec![],
            changed: vec![],
            edge_delta: vec![],
            unresolved: blockers.into_iter().collect(),
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
    // Only a complete graph can prove the absence of a node or an edge.
    let baseline_complete = baseline.graph_view.state == GraphState::Complete;
    let candidate_complete = candidate.graph_view.state == GraphState::Complete;
    let added = candidate_ids
        .difference(&baseline_ids)
        .filter(|_| baseline_complete)
        .cloned()
        .collect::<Vec<_>>();
    let removed = baseline_ids
        .difference(&candidate_ids)
        .filter(|_| candidate_complete)
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
        .filter(
            |id| match (baseline_edges.get(*id), candidate_edges.get(*id)) {
                (Some(left), Some(right)) => left != right,
                (None, Some(_)) => baseline_complete,
                (Some(_), None) => candidate_complete,
                (None, None) => false,
            },
        )
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
    let baseline_partial = baseline.analysis_state == doctor_contracts::AnalysisState::Partial
        || baseline.graph_view.state == GraphState::Partial;
    let candidate_partial = candidate.analysis_state == doctor_contracts::AnalysisState::Partial
        || candidate.graph_view.state == GraphState::Partial;
    let partial = baseline_partial || candidate_partial;
    let unresolved = if !partial {
        vec![]
    } else if baseline_partial
        && candidate_partial
        && deltas.iter().any(|delta| delta.bounds.is_some())
    {
        vec!["partial_inputs".into()]
    } else {
        [
            (baseline_partial, "baseline_partial"),
            (candidate_partial, "candidate_partial"),
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
    version: VersionRelation,
    registry: RegistryRelation,
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
    if version != VersionRelation::Same {
        delta.incomparable = baseline_findings
            .keys()
            .chain(candidate_findings.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(rule_id, subject)| format!("{rule_id}:{subject}"))
            .collect();
        return delta;
    }
    for (key, baseline_finding) in &baseline_findings {
        match candidate_findings.get(key) {
            None => {
                let entry = format!("{}:{}", key.0, key.1);
                push_measurement_changes(&mut delta, &entry, Some(baseline_finding), None);
                if let Some(evidence) =
                    comparable_absence(candidate, baseline_finding, version, registry)
                {
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
            Some(candidate_finding)
                if [*baseline_finding, *candidate_finding]
                    .iter()
                    .any(|finding| {
                        finding.evidence_completeness
                            == doctor_contracts::EvidenceCompleteness::Unavailable
                    }) =>
            {
                delta.incomparable.push(format!("{}:{}", key.0, key.1));
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
        if let Some(evidence) = comparable_absence(baseline, candidate_finding, version, registry) {
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
    report: &AnalysisSnapshotReport,
    finding: &doctor_contracts::Finding,
    version: VersionRelation,
    registry: RegistryRelation,
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
        && version == VersionRelation::Same
        && matches!(registry, RegistryRelation::Same);
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
    relations: ReportRelations,
    reasons: &mut Vec<String>,
) -> Vec<MetricDelta> {
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
        let fixed_identity = relations.version_reason.is_none()
            && baseline_cells.len() == 1
            && candidate_cells.len() == 1
            && baseline_cells[0].match_key.is_none()
            && candidate_cells[0].match_key.is_none()
            && fixed_metric_identity(baseline, candidate, &group.0, &group.1);
        let instance_report_sum =
            crate::metrics::instance_report_sum_limitation(baseline_release, &group.0, &group.1)
                .is_some();
        let parallelism = if instance_report_sum {
            relations.parallelism
        } else {
            ParallelismRelation::Same
        };
        for (baseline, candidate, issue) in
            pair_cells(baseline_cells, candidate_cells, fixed_identity)
        {
            deltas.push(metric_delta(
                group.0.clone(),
                group.1.clone(),
                baseline,
                candidate,
                issue,
                ReportRelations {
                    parallelism,
                    ..relations
                },
                reasons,
            ));
        }
    }
    deltas
}

#[derive(Clone, Copy)]
enum CellPairingIssue {
    Duplicate,
    DifferentKeys,
    MissingKey,
}

type CellPair<'a> = (
    Option<&'a MetricCell>,
    Option<&'a MetricCell>,
    Option<CellPairingIssue>,
);

/// Pairs cells within one (metric, subject) group by counter identity.
fn pair_cells<'a>(
    baseline: &[&'a MetricCell],
    candidate: &[&'a MetricCell],
    fixed_identity: bool,
) -> Vec<CellPair<'a>> {
    if baseline.is_empty() {
        return candidate
            .iter()
            .map(|cell| (None, Some(*cell), None))
            .collect();
    }
    if candidate.is_empty() {
        return baseline
            .iter()
            .map(|cell| (Some(*cell), None, None))
            .collect();
    }
    if let ([baseline], [candidate]) = (baseline, candidate) {
        let issue = match (
            baseline.match_key.as_deref(),
            candidate.match_key.as_deref(),
        ) {
            (Some(left), Some(right)) if left == right => None,
            (Some(_), Some(_)) => Some(CellPairingIssue::DifferentKeys),
            (None, None) if fixed_identity => None,
            _ => Some(CellPairingIssue::MissingKey),
        };
        return vec![(Some(*baseline), Some(*candidate), issue)];
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
                    None,
                )
            })
            .collect(),
        _ => vec![(
            (baseline.len() == 1).then(|| baseline[0]),
            (candidate.len() == 1).then(|| candidate[0]),
            Some(CellPairingIssue::Duplicate),
        )],
    }
}

fn fixed_metric_identity(
    baseline: &AnalysisSnapshotReport,
    candidate: &AnalysisSnapshotReport,
    metric_id: &str,
    subject_ref: &str,
) -> bool {
    match metric_id {
        "OperatorTotalTime" if subject_ref == "operator-0" => {
            baseline.query_summary.is_none() && candidate.query_summary.is_none()
        }
        "OutputRows" => {
            let left = baseline
                .operator_rows
                .iter()
                .find(|row| row.operator_ref == subject_ref);
            let right = candidate
                .operator_rows
                .iter()
                .find(|row| row.operator_ref == subject_ref);
            matches!((left, right), (Some(left), Some(right))
                if left.plan_node_id.is_some() && left.plan_node_id == right.plan_node_id)
        }
        _ => false,
    }
}

/// Report-level facts every metric delta of one comparison shares.
#[derive(Clone, Copy)]
struct ReportRelations {
    version_reason: Option<&'static str>,
    registry: RegistryRelation,
    precision_unknown: bool,
    /// Set for a metric that repeats BE values across fragment instances.
    parallelism: ParallelismRelation,
}

fn metric_delta(
    metric_id: String,
    subject_match: String,
    baseline: Option<&MetricCell>,
    candidate: Option<&MetricCell>,
    pairing_issue: Option<CellPairingIssue>,
    relations: ReportRelations,
    reasons: &mut Vec<String>,
) -> MetricDelta {
    let blocker = relations
        .version_reason
        .or_else(|| {
            relations
                .precision_unknown
                .then_some("metric_precision_unknown")
        })
        .or_else(|| {
            matches!(relations.registry, RegistryRelation::Mismatch)
                .then_some("registry_semantic_mismatch")
        })
        .or_else(|| {
            pairing_issue.map(|issue| match issue {
                CellPairingIssue::Duplicate => "duplicate_metric_match",
                CellPairingIssue::DifferentKeys => "metric_match_key_mismatch",
                CellPairingIssue::MissingKey => "metric_match_key_unavailable",
            })
        });
    if let Some(reason) = blocker {
        reasons.push(reason.into());
        return not_comparable_delta(
            metric_id,
            subject_match,
            baseline,
            candidate,
            PercentageState::SemanticMismatch,
            reason,
        );
    }
    let baseline_issue = metric_cell_issue(baseline);
    let candidate_issue = metric_cell_issue(candidate);
    if baseline_issue.is_some() || candidate_issue.is_some() {
        let percentage = [
            PercentageState::SemanticMismatch,
            PercentageState::Invalid,
            PercentageState::Missing,
        ]
        .into_iter()
        .find(|percentage| {
            [baseline_issue.as_ref(), candidate_issue.as_ref()]
                .into_iter()
                .flatten()
                .any(|(_, issue)| issue == percentage)
        })
        .unwrap_or(PercentageState::Missing);
        let limitation = if baseline.is_none() || candidate.is_none() {
            "metric_missing_on_one_side"
        } else {
            "metric_state_not_comparable"
        };
        let mut delta = not_comparable_delta(
            metric_id,
            subject_match,
            baseline,
            candidate,
            percentage,
            limitation,
        );
        for (side, issue) in [("baseline", baseline_issue), ("candidate", candidate_issue)] {
            if let Some((state, _)) = issue {
                let reason = format!("{side}_metric_{}", state_name(state));
                reasons.push(reason.clone());
                delta.limitations.push(reason);
            }
        }
        return delta;
    }
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        unreachable!("absent cells are handled by metric_cell_issue");
    };
    let unit = baseline.canonical_unit();
    // The checked-in comparator recipe only proves equivalence for these
    // metric/unit pairs when the registry receipts differ.
    let equivalent = !matches!(relations.registry, RegistryRelation::Equivalent)
        || matches!(
            (metric_id.as_str(), unit),
            (
                "QueryCumulativeOperatorTime"
                    | "QueryCumulativeScanTime"
                    | "QueryCumulativeNetworkTime"
                    | "OperatorTotalTime"
                    | "NetworkTime"
                    | "ScanTime",
                Some("ns")
            ) | ("QuerySpillBytes", Some("byte"))
                | ("PushRowNum" | "PullRowNum" | "InstanceNum", Some("item"))
        );
    let parallelism_gate = match relations.parallelism {
        ParallelismRelation::Same => None,
        ParallelismRelation::Different => Some((
            PercentageState::SemanticMismatch,
            "parallelism_shape_mismatch",
        )),
        ParallelismRelation::Unknown => {
            Some((PercentageState::Missing, "parallelism_shape_unavailable"))
        }
    };
    let blocker = if unit != candidate.canonical_unit() {
        Some((PercentageState::SemanticMismatch, "metric_unit_mismatch"))
    } else if !equivalent {
        Some((
            PercentageState::SemanticMismatch,
            "registry_semantic_mismatch",
        ))
    } else {
        parallelism_gate
    };
    if let Some((percentage, reason)) = blocker {
        reasons.push(reason.into());
        return not_comparable_delta(
            metric_id,
            subject_match,
            Some(baseline),
            Some(candidate),
            percentage,
            reason,
        );
    }
    numeric_delta(metric_id, subject_match, baseline, candidate, reasons)
}

fn metric_cell_issue(cell: Option<&MetricCell>) -> Option<(MetricCellState, PercentageState)> {
    let state = cell.map_or(MetricCellState::Missing, MetricCell::state);
    let percentage = match state {
        MetricCellState::Exact => return None,
        MetricCellState::Missing | MetricCellState::Unavailable | MetricCellState::Unknown => {
            PercentageState::Missing
        }
        MetricCellState::Incompatible => PercentageState::SemanticMismatch,
        MetricCellState::Invalid | MetricCellState::Partial | MetricCellState::Overflow => {
            PercentageState::Invalid
        }
    };
    Some((state, percentage))
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
    let aligned = number_range(baseline)
        .zip(number_range(candidate))
        .and_then(|(base, candidate)| {
            let scale = base.scale.max(candidate.scale);
            Some((base.align(scale)?, candidate.align(scale)?, scale))
        });
    let Some((base, candidate_range, scale)) = aligned else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
        resolve_version_and_graph, v01_analysis_receipt,
    };
    use doctor_contracts::{AnalysisState, MetricCellValue, RequestedSourceKind};
    use serde_json::{Value, json};

    #[test]
    fn compare_rejects_cross_release_instance_summed_memory() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let analyze = |release: &str, memory: &str| {
            let input = json!({"Query":{
                "Summary":{"StarRocks Version":release},
                "Execution":{"QuerySumMemoryUsage":memory}
            }});
            let mut output = adapt_input(
                InputEnvelope::new(
                    RequestedSourceKind::JsonProfile,
                    serde_json::to_vec(&input).unwrap(),
                ),
                &AdapterLimits::default(),
                &context,
            )
            .unwrap();
            let graph = output.graph_evidence.clone();
            resolve_version_and_graph(&mut output, graph).unwrap();
            assemble_public_report(output).unwrap()
        };
        let baseline = analyze("3.5.14", "100.000 B");
        let candidate = analyze("3.3.5", "200.000 B");
        for (left, right) in [(&baseline, &candidate), (&candidate, &baseline)] {
            let comparison = compare_attempts(
                analysis_attempt(left.clone()),
                analysis_attempt(right.clone()),
            );
            assert_eq!(comparison.state, ComparisonState::NotComparable);
            assert_eq!(
                comparison.compatibility_delta,
                ["starrocks_version_mismatch"]
            );
            assert_eq!(comparison.structure_delta.state, "not_comparable");
            let metric = comparison
                .metric_deltas
                .iter()
                .find(|delta| delta.metric_id == "QuerySumMemoryUsage")
                .unwrap();
            assert_eq!(metric.state, MetricDeltaState::NotComparable);
            assert_eq!(metric.percentage_state, PercentageState::SemanticMismatch);
            assert_eq!(metric.limitations, ["starrocks_version_mismatch"]);
            assert!(
                metric.baseline_metric_cell_ref.is_some()
                    && metric.candidate_metric_cell_ref.is_some()
            );
            assert!(metric.absolute_delta.is_none() && metric.percentage_value.is_none());
            assert!(metric.bounds.is_none() && metric.percentage_bounds.is_none());
        }
    }

    #[test]
    fn compare_requires_release_and_full_pack_evidence() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let mut output = adapt_input(
            InputEnvelope::new(
                RequestedSourceKind::JsonProfile,
                &br#"{"profile_version":"3.3.5","operator_total_time_ns":500}"#[..],
            ),
            &AdapterLimits::default(),
            &context,
        )
        .unwrap();
        let graph = output.graph_evidence.clone();
        resolve_version_and_graph(&mut output, graph).unwrap();
        let baseline = assemble_public_report(output).unwrap();
        let mut changed_hash = baseline.clone();
        changed_hash
            .version
            .selected_pack
            .as_mut()
            .unwrap()
            .manifest_hash = "different".into();
        let mut changed_delta = baseline.clone();
        changed_delta
            .version
            .selected_pack
            .as_mut()
            .unwrap()
            .release_deltas
            .as_mut()
            .unwrap()
            .deltas_hash = "different".into();
        let mut missing_reported = baseline.clone();
        missing_reported.version.reported_version = None;
        let mut hint_only = missing_reported.clone();
        hint_only.version.version_source = VersionSource::UserDeclared;
        hint_only.version.user_declared_version = Some("3.3.5".into());
        let mut missing_pack = baseline.clone();
        missing_pack.version.selected_pack = None;
        for (candidate, reason) in [
            (changed_hash, "version_pack_mismatch"),
            (changed_delta, "version_pack_mismatch"),
            (missing_reported, "version_unknown"),
            (hint_only, "version_unknown"),
            (missing_pack, "no_registered_pack"),
        ] {
            let comparison = compare_attempts(
                analysis_attempt(baseline.clone()),
                analysis_attempt(candidate),
            );
            assert_eq!(comparison.state, ComparisonState::NotComparable);
            assert_eq!(comparison.compatibility_delta, [reason]);
            assert!(comparison.reasons.iter().any(|entry| entry == reason));
            let metric = &comparison.metric_deltas[0];
            assert_eq!(metric.limitations, [reason]);
            assert_eq!(metric.percentage_state, PercentageState::SemanticMismatch);
            assert!(metric.absolute_delta.is_none() && metric.percentage_value.is_none());
            assert!(metric.bounds.is_none() && metric.percentage_bounds.is_none());
        }
    }

    #[test]
    fn compare_aggregates_actual_comparability() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let analyze = |input: Value| {
            let mut output = adapt_input(
                InputEnvelope::new(
                    RequestedSourceKind::JsonProfile,
                    serde_json::to_vec(&input).unwrap(),
                ),
                &AdapterLimits::default(),
                &context,
            )
            .unwrap();
            let graph = output.graph_evidence.clone();
            resolve_version_and_graph(&mut output, graph).unwrap();
            let mut report = assemble_public_report(output).unwrap();
            report.analysis_state = AnalysisState::Complete;
            report
        };
        let compare =
            |left, right| compare_attempts(analysis_attempt(left), analysis_attempt(right));
        let topology = json!({"complete":true,"root_node_ids":["a"],"nodes":[{"node_id":"a"}]});
        let baseline = analyze(json!({
            "profile_version":"3.5.14","execution_topology":topology,"operator_total_time_ns":0
        }));
        let candidate = analyze(json!({
            "profile_version":"3.5.14","execution_topology":topology,"operator_total_time_ns":5
        }));
        let invalid = analyze(json!({
            "profile_version":"3.5.14","execution_topology":topology,"operator_total_time_ns":true
        }));
        let comparison = compare(baseline.clone(), invalid.clone());
        assert_eq!(comparison.state, ComparisonState::NotComparable);
        assert_eq!(
            comparison.metric_deltas[0].state,
            MetricDeltaState::NotComparable
        );
        assert!(!comparison.structure_delta.matches.is_empty());

        let comparison = compare(baseline.clone(), candidate.clone());
        assert_eq!(comparison.state, ComparisonState::Comparable);
        let metric = &comparison.metric_deltas[0];
        assert_eq!(metric.absolute_delta.as_ref().unwrap().value, "5");
        assert_eq!(
            metric.percentage_state,
            PercentageState::UndefinedBaselineZero
        );
        assert!(metric.percentage_value.is_none() && metric.percentage_bounds.is_none());

        let mut mixed = candidate;
        let mut invalid = invalid.operator_rows[0].metric_cells[0].clone();
        invalid.metric_id = "QuerySpillBytes".into();
        invalid.metric_cell_ref = "metric:operator-0:QuerySpillBytes".into();
        mixed.operator_rows[0].metric_cells.push(invalid);
        let comparison = compare(baseline, mixed);
        assert_eq!(comparison.state, ComparisonState::PartiallyComparable);
        assert_eq!(comparison.metric_deltas.len(), 2);
        assert_eq!(
            comparison
                .metric_deltas
                .iter()
                .filter(|metric| metric.state == MetricDeltaState::Comparable)
                .count(),
            1
        );

        let empty = analyze(json!({"profile_version":"3.5.14"}));
        let comparison = compare(empty.clone(), empty);
        assert!(comparison.metric_deltas.is_empty());
        assert_eq!(comparison.state, ComparisonState::NotComparable);
        let structure = analyze(json!({"profile_version":"3.5.14","execution_topology":topology}));
        let comparison = compare(structure.clone(), structure.clone());
        assert!(comparison.metric_deltas.is_empty());
        assert_eq!(comparison.structure_delta.matches, ["a:stable_id@1"]);
        assert_eq!(comparison.state, ComparisonState::Comparable);

        let partial = |root: &str| {
            analyze(json!({"profile_version":"3.5.14","execution_topology":{
                "complete":false,"root_node_ids":[root],"nodes":[{"node_id":root}]
            }}))
        };
        let comparison = compare(partial("a"), partial("b"));
        assert_eq!(comparison.state, ComparisonState::NotComparable);
        assert_eq!(comparison.structure_delta.state, "partial");
        assert!(comparison.structure_delta.matches.is_empty());
        assert!(comparison.structure_delta.added.is_empty());
        assert!(comparison.structure_delta.removed.is_empty());
        assert!(comparison.metric_deltas.is_empty());
        let comparison = compare(partial("a"), partial("a"));
        assert_eq!(comparison.state, ComparisonState::PartiallyComparable);
        assert_eq!(comparison.structure_delta.matches, ["a:stable_id@1"]);

        let mut old_cycle = analyze(json!({"profile_version":"3.5.14","execution_topology":{
            "complete":true,"root_node_ids":["a"],"nodes":[
                {"node_id":"a","children":["b"]},{"node_id":"b","children":["a"]}
            ]
        }}));
        old_cycle.graph_view.state = GraphState::Complete;
        old_cycle.graph_view.unresolved_relations.clear();
        let comparison = compare(structure, old_cycle);
        assert_eq!(comparison.structure_delta.state, "not_comparable");
        assert_eq!(comparison.structure_delta.unresolved, ["topology_cycle"]);
        assert_eq!(comparison.state, ComparisonState::NotComparable);
    }

    #[test]
    fn compare_singleton_base_and_max_do_not_pair() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let analyze = |key: &str, value: &str| {
            let input = json!({"Query":{
                "Summary":{"StarRocks Version":"3.5.14"},
                "Execution":{"Fragment 0":{"Pipeline (id=0)":{"SCAN (plan_node_id=0)":{
                    "CommonMetrics":{(key):value}
                }}}}
            }});
            let mut output = adapt_input(
                InputEnvelope::new(
                    RequestedSourceKind::JsonProfile,
                    serde_json::to_vec(&input).unwrap(),
                ),
                &AdapterLimits::default(),
                &context,
            )
            .unwrap();
            let graph = output.graph_evidence.clone();
            resolve_version_and_graph(&mut output, graph).unwrap();
            assemble_public_report(output).unwrap()
        };
        let baseline = analyze("OperatorTotalTime", "500ns");
        let max = analyze("__MAX_OF_OperatorTotalTime", "700ns");
        let base = analyze("OperatorTotalTime", "700ns");
        for (candidate, expected_delta) in [(max, None), (base, Some("200"))] {
            let comparison = compare_attempts(
                analysis_attempt(baseline.clone()),
                analysis_attempt(candidate),
            );
            let metric = comparison
                .metric_deltas
                .iter()
                .find(|delta| delta.metric_id == "OperatorTotalTime")
                .unwrap();
            assert!(
                metric.baseline_metric_cell_ref.is_some()
                    && metric.candidate_metric_cell_ref.is_some()
            );
            assert_eq!(
                metric
                    .absolute_delta
                    .as_ref()
                    .map(|value| value.value.as_str()),
                expected_delta
            );
            if expected_delta.is_none() {
                assert_eq!(metric.state, MetricDeltaState::NotComparable);
                assert_eq!(metric.limitations, ["metric_match_key_mismatch"]);
                assert!(
                    metric.percentage_value.is_none()
                        && metric.bounds.is_none()
                        && metric.percentage_bounds.is_none()
                );
            } else {
                assert_eq!(metric.state, MetricDeltaState::Comparable);
            }
        }
    }

    #[test]
    fn compare_requires_keys_except_for_fixed_metric_identity() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let analyze = |key: &str, value: &str, planner: Value| {
            let input = json!({"Query":{
                "Summary":{"StarRocks Version":"3.5.14"},
                "Execution":{(key):value},
                "Planner":planner
            }});
            let mut output = adapt_input(
                InputEnvelope::new(
                    RequestedSourceKind::JsonProfile,
                    serde_json::to_vec(&input).unwrap(),
                ),
                &AdapterLimits::default(),
                &context,
            )
            .unwrap();
            let graph = output.graph_evidence.clone();
            resolve_version_and_graph(&mut output, graph).unwrap();
            assemble_public_report(output).unwrap()
        };
        let baseline = analyze("QueryExecutionWallTime", "500ns", json!({}));
        let max = analyze("__MAX_OF_QueryExecutionWallTime", "700ns", json!({}));
        let base = analyze("QueryExecutionWallTime", "700ns", json!({}));
        let with_plan = analyze("QueryExecutionWallTime", "500ns", json!({"Total":"10ns"}));
        for (left, right, reason) in [
            (&baseline, &with_plan, "baseline_metric_unavailable"),
            (&with_plan, &baseline, "candidate_metric_unavailable"),
        ] {
            let comparison = compare_attempts(
                analysis_attempt(left.clone()),
                analysis_attempt(right.clone()),
            );
            let planning = comparison
                .metric_deltas
                .iter()
                .find(|delta| delta.metric_id == "QueryPlanningTime")
                .unwrap();
            assert_eq!(planning.state, MetricDeltaState::NotComparable);
            assert_eq!(
                planning.limitations,
                ["metric_state_not_comparable", reason]
            );
        }
        let base_comparison =
            compare_attempts(analysis_attempt(baseline.clone()), analysis_attempt(base));
        let wall_time = base_comparison
            .metric_deltas
            .iter()
            .find(|delta| delta.metric_id == "QueryExecutionWallTime")
            .unwrap();
        assert_eq!(wall_time.state, MetricDeltaState::Comparable);
        assert_eq!(wall_time.absolute_delta.as_ref().unwrap().value, "200");
        let max_comparison = compare_attempts(
            analysis_attempt(baseline.clone()),
            analysis_attempt(max.clone()),
        );
        assert_ne!(
            base_comparison.comparison_key,
            max_comparison.comparison_key
        );
        let index = baseline
            .derived_metric_cells
            .iter()
            .position(|cell| cell.metric_id == "QueryExecutionWallTime")
            .unwrap();
        assert_ne!(
            baseline.derived_metric_cells[index].match_key,
            max.derived_metric_cells
                .iter()
                .find(|cell| cell.metric_id == "QueryExecutionWallTime")
                .unwrap()
                .match_key
        );
        let mut no_key = baseline.clone();
        no_key.derived_metric_cells[index].match_key = None;
        let mut duplicate = baseline.clone();
        let mut duplicated_cell = duplicate.derived_metric_cells[index].clone();
        duplicated_cell.metric_cell_ref.push_str(":duplicate");
        duplicate.derived_metric_cells.push(duplicated_cell);
        for (left, right, reason) in [
            (&baseline, &max, "metric_match_key_mismatch"),
            (&baseline, &no_key, "metric_match_key_unavailable"),
            (&no_key, &no_key, "metric_match_key_unavailable"),
            (&baseline, &duplicate, "duplicate_metric_match"),
        ] {
            let comparison = compare_attempts(
                analysis_attempt(left.clone()),
                analysis_attempt(right.clone()),
            );
            let metric = comparison
                .metric_deltas
                .iter()
                .find(|delta| delta.metric_id == "QueryExecutionWallTime")
                .unwrap();
            assert_eq!(metric.state, MetricDeltaState::NotComparable);
            assert_eq!(metric.limitations, [reason]);
            assert!(
                metric.absolute_delta.is_none()
                    && metric.percentage_value.is_none()
                    && metric.bounds.is_none()
            );
        }

        let derived: AnalysisSnapshotReport = serde_json::from_str(include_str!(
            "../../../../fixtures/expected/3.5.14/docker_runtime_corpus_complex.report.json"
        ))
        .unwrap();
        let output_rows = derived
            .operator_rows
            .iter()
            .flat_map(|row| &row.metric_cells)
            .find(|cell| cell.metric_id == "OutputRows" && cell.state() == MetricCellState::Exact)
            .unwrap();
        assert!(output_rows.match_key.is_none());
        let subject = output_rows.subject_ref.clone();
        let comparison =
            compare_attempts(analysis_attempt(derived.clone()), analysis_attempt(derived));
        let metric = comparison
            .metric_deltas
            .iter()
            .find(|delta| {
                delta.metric_id == "OutputRows"
                    && delta.subject_match.as_deref() == Some(subject.as_str())
            })
            .unwrap();
        assert_eq!(metric.state, MetricDeltaState::Comparable);
        assert_eq!(metric.absolute_delta.as_ref().unwrap().value, "0");
    }

    #[test]
    fn compare_reports_the_actual_invalid_or_missing_side() {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let analyze = |value: Value| {
            let input = json!({"profile_version":"3.5.14","operator_total_time_ns":value});
            let mut output = adapt_input(
                InputEnvelope::new(
                    RequestedSourceKind::JsonProfile,
                    serde_json::to_vec(&input).unwrap(),
                ),
                &AdapterLimits::default(),
                &context,
            )
            .unwrap();
            let graph = output.graph_evidence.clone();
            resolve_version_and_graph(&mut output, graph).unwrap();
            let mut report = assemble_public_report(output).unwrap();
            report.operator_rows[0].metric_cells[0].match_key = Some("operator-total".into());
            report
        };
        let valid = analyze(json!(500));
        let invalid = analyze(json!(true));
        let missing = analyze(Value::Null);
        let mut incompatible = invalid.clone();
        incompatible.operator_rows[0].metric_cells[0].value = MetricCellValue::Incompatible {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        };
        for (left, right, percentage, reasons) in [
            (
                &valid,
                &invalid,
                PercentageState::Invalid,
                &["candidate_metric_invalid"][..],
            ),
            (
                &invalid,
                &valid,
                PercentageState::Invalid,
                &["baseline_metric_invalid"][..],
            ),
            (
                &valid,
                &missing,
                PercentageState::Missing,
                &["candidate_metric_missing"][..],
            ),
            (
                &missing,
                &valid,
                PercentageState::Missing,
                &["baseline_metric_missing"][..],
            ),
            (
                &missing,
                &invalid,
                PercentageState::Invalid,
                &["baseline_metric_missing", "candidate_metric_invalid"][..],
            ),
            (
                &invalid,
                &incompatible,
                PercentageState::SemanticMismatch,
                &["baseline_metric_invalid", "candidate_metric_incompatible"][..],
            ),
        ] {
            let comparison = compare_attempts(
                analysis_attempt(left.clone()),
                analysis_attempt(right.clone()),
            );
            let metric = &comparison.metric_deltas[0];
            assert_eq!(metric.state, MetricDeltaState::NotComparable);
            assert_eq!(metric.percentage_state, percentage);
            assert!(metric.absolute_delta.is_none() && metric.percentage_value.is_none());
            assert!(metric.bounds.is_none() && metric.percentage_bounds.is_none());
            assert!(
                reasons
                    .iter()
                    .all(|reason| metric.limitations.iter().any(|entry| entry == *reason))
            );
            let actual = comparison
                .reasons
                .iter()
                .filter(|reason| reason.contains("_metric_"))
                .map(String::as_str)
                .collect::<Vec<_>>();
            assert_eq!(actual, reasons);
            assert!(
                !comparison
                    .reasons
                    .iter()
                    .any(|reason| reason == "metric_unit_mismatch")
            );
            assert!(
                !metric
                    .limitations
                    .iter()
                    .any(|reason| reason == "metric_unit_mismatch")
            );
        }
    }
}
