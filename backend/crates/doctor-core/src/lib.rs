//! Internal IR ownership, strict metric parsing, and deterministic v0.1 rules.
//! HTTP handling remains intentionally deferred to later tasks.

mod adapters;
mod audit_concurrency;
mod audit_family_distribution;
mod audit_fingerprint_families;
mod audit_normalization;
mod cancel;
mod comparison;
pub mod display;
mod execution_shape;
mod execution_summary;
mod explain;
mod explain_snapshot;
mod graph;
mod history_collection;
mod history_trend;
pub use history_trend::{
    ChangeStats, ChangeVerdict, HISTORY_TREND_RULE_VERSION, SeriesPoint, SideStats, TREND_K_MAD,
    TREND_MIN_BASELINE_HOURS, TREND_MIN_CURRENT_SAMPLES, TREND_MIN_HOUR_SAMPLES,
    TREND_MIN_RELATIVE_PERCENT, TemplateTrend, TrendBucket, TrendWindows, WindowStats,
    evaluate_template_trend, history_rule_version, trend_series, verify_change, window_stats,
};
mod load_error_log;
pub use load_error_log::{LOAD_ERROR_LOG_MAX_BYTES, ParsedLoadErrorLog, parse_load_error_log};
mod load_history;
pub use load_history::{
    CollectedLoad, LOAD_PHASES, LoadBucketDelta, LoadCount, LoadHistoryKind, aggregate_loads,
};
mod log_histogram;
mod metrics;
mod report;
mod rules;
mod similar_sql;
mod sql_fingerprint;
mod sql_structure;
pub use audit_fingerprint_families::{
    AuditFingerprintEvent, AuditFingerprintGroup, AuditFingerprintStatement, AuditFingerprintTally,
    FamilyCells, MetricCells, compare_family_cells, group_audit_fingerprints,
    sort_audit_fingerprint_groups,
};
pub use history_collection::{
    CollectedExecution, CollectionTally, HISTORY_BUCKET_SECONDS, HISTORY_EXEMPLARS_PER_BUCKET,
    HistoryBucketDelta, HistoryExemplar, HistoryMetric, HistoryTemplate, HistoryTemplateKind,
    aggregate_collection, history_template_key,
};
pub use log_histogram::LogHistogram;
pub use similar_sql::{
    SimilarSqlClusterOut, SimilarSqlInput, SimilarSqlMemberOut, SimilarSqlOut, cluster_similar_sql,
};
pub use sql_fingerprint::{SqlFingerprintInput, product_sql_fingerprint};
mod sql_location;
pub use sql_location::attach_sql_locations;
mod version;

pub use adapters::{
    ANALYSIS_CANCELLED, AdapterContext, AdapterError, AdapterLimits, AdapterOutput, InputEnvelope,
    RedactionState, SourceLocator, SourceRef, adapt_input, adapt_input_cancellable,
    is_profile_topology_line,
};
pub use audit_concurrency::{
    AuditConcurrencyDelta, AuditConcurrencyError, AuditConcurrencyStep, AuditInterval,
    AuditIntervalExclusion, MAX_AUDIT_QUERY_TIME_MS, audit_concurrency_from_deltas,
    audit_concurrency_steps, audit_interval,
};
pub use audit_family_distribution::{
    AUDIT_FAMILY_HISTOGRAM_BINS, AuditFamilyDistributionError, AuditFamilyDistributionSource,
    AuditFamilyHistogramBin, project_audit_family_distribution,
};
pub use audit_normalization::normalize_audit_metric;
pub use cancel::{CancelSignal, Cancelled};
pub(crate) use cancel::{drop_polled, sort_by_polled};
pub use comparison::{analysis_attempt, compare_attempts, failed_attempt, v01_comparison_receipt};
pub use explain::{
    EXPLAIN_EXCHANGE_RECIPE_ID, EXPLAIN_EXCHANGE_RECIPE_VERSION, ExplainBudget,
    ExplainExchangeCandidate, ExplainExchangeLink, ExplainFragment, ExplainLimitation,
    ExplainLimits, ExplainNodeKind, ExplainPlanAggregatePhase, ExplainPlanChild,
    ExplainPlanDistribution, ExplainPlanFactKind, ExplainPlanFactValue, ExplainPlanFacts,
    ExplainPlanJoinStrategy, ExplainPlanNode, ExplainPlanNodeFact, ExplainPlanNodeFactSet,
    ExplainSafeFragment, ExplainSafeNode, ExplainSafeStructure, ExplainSinkKind, ExplainSourceSpan,
    ExplainStructureState, normalize_explain_costs,
};
pub use explain_snapshot::{
    ExplainSnapshotInput, assemble_explain_snapshot, assemble_explain_snapshot_for_mode,
};
pub use graph::{ExplicitGraphEvidence, GraphEvidence, GraphResolutionError, resolve_graph};
pub use metrics::{
    AnalysisAggregation, CounterTextFormat, InputPolicy, MetricDefinition, MetricDimension,
    MetricInput, MetricParseError, MetricParseOutcome, MetricRegistryLookup, ParseMode,
    SourceCounterAggregate, SourceCounterMerge, SourceCounterMinMax, SourceCounterStrategy,
    instance_report_sum_limitation, metric_lookup_for, metric_registry_for, parse_metric,
    parse_source_counter_strategy, sr3514_instance_report_sum_limitation, sr3514_metric_definition,
    sr3514_metric_lookup, sr3514_metric_registry,
};
pub use report::{
    PublicReportError, assemble_public_report, assemble_public_report_cancellable,
    v01_analysis_receipt,
};
pub use rules::{RuleEvaluationError, evaluate_rules};
/// Audited StarRocks releases: the 3.5.14 baseline plus per-release deltas.
pub use starrocks_version_pack_3_5_14::deltas::{
    ReleaseProfile, ReleaseResolution, resolve_release,
};
pub use version::{
    VersionResolutionError, resolve_version_and_graph, resolve_version_and_graph_cancellable,
    resolve_version_context,
};

use std::{cmp::Ordering, collections::BTreeMap};

use doctor_contracts::{
    AnalysisReceipt, AnalysisSnapshotReport, BoundedInterval, CanonicalNumber, Comparison,
    Compatibility, CompletenessReport, CompletenessState, Diagnostic, DimensionCompleteness,
    DirectionSemantics, EdgeSource, Fidelity, Finding, FindingConfidence, GraphEdge,
    GraphEntityKind, GraphNode, GraphRelation, GraphState, GraphView, InputRole, InputSummary,
    MergeState, MetricCell, MetricCellState, PackMatch, ParseState, PercentageState,
    ProfileContext, ProfileLevel, QueryState, QuerySummary, ReceiptComponent, RecipeRef,
    RuleEvaluation, RuleEvaluationStatus, RuntimeInventoryAcquisitionShape, RuntimeSourceRole,
    SelectedVersionPack, SnapshotKind, SourceQuality, SourceRole, SupportStatus, TopologySource,
    VersionContext, VersionSource, canonical_hash, canonical_hash_cancellable,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CounterStrategy {
    pub tunit: String,
    pub aggregate_type: String,
    pub merge_type: String,
    pub min_max_type: String,
    pub display_threshold: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricObservation {
    pub observation_id: String,
    pub metric_id: Option<String>,
    pub source_name: String,
    pub subject_ref: Option<String>,
    /// Backend-only link to an ordered native/subordinate operator. It is never wired publicly.
    pub operator_ref: Option<String>,
    /// Backend-only occurrence identity used by the runtime inventory. Unlike
    /// `operator_ref`, this does not assert a native/subordinate merge role.
    pub runtime_occurrence_ref: Option<String>,
    /// Structural counter identity published as `MetricCell.match_key`; the
    /// report keeps it only when it is unique.
    pub match_key: Option<String>,
    pub dimension: String,
    pub parse_state: ParseState,
    pub canonical_number: Option<CanonicalNumber>,
    pub canonical_unit: Option<String>,
    pub raw_text: Option<String>,
    pub source_refs: Vec<String>,
    pub consumed_ranges: Vec<String>,
    pub unconsumed_ranges: Vec<String>,
    pub fidelity: Fidelity,
    pub bounds: Option<BoundedInterval>,
    pub semantic_state: String,
    pub definition_ref: Option<String>,
    pub scope: String,
    pub value_role: String,
    pub strategy_origin: String,
    pub counter_strategy: Option<CounterStrategy>,
    pub source_merge_state: String,
    pub display_value: String,
    pub source_ordinal: Option<u32>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorGroup {
    pub operator_group_id: String,
    pub plan_node_id: Option<String>,
    pub fragment_ref: Option<String>,
    pub pipeline_ref: Option<String>,
    pub native_operator_refs: Vec<(String, u32)>,
    pub subordinate_operator_refs: Vec<(String, u32)>,
    pub canonical_type: String,
    pub raw_labels: Vec<String>,
    pub metric_observation_refs: Vec<String>,
    pub source_refs: Vec<String>,
}

/// Adapter-owned identity for one concrete Pipeline container occurrence.
///
/// `container_ref` deliberately reuses the matching internal Pipeline
/// `OperatorGroup` identity.  It is a typed join key, not a value callers may
/// split to reconstruct fragment, pipeline, or occurrence context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelineContainer {
    pub container_ref: String,
    pub fragment_ref: String,
    pub pipeline_ref: String,
    pub occurrence_ordinal: u32,
    pub source_refs: Vec<String>,
}

/// Adapter-owned identity for one explicit `PipelineDriver` occurrence.
/// Instance host/UUID data is intentionally excluded from every retained field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDriverOccurrence {
    pub driver_occurrence_ref: String,
    pub container_ref: String,
    pub occurrence_ordinal: u32,
    pub source_refs: Vec<String>,
}

/// Adapter-owned direct membership of one physical runtime operator in a
/// concrete Pipeline container.  Missing structural context remains explicit;
/// consumers must not infer it from an operator-group aggregate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalOperatorMembership {
    pub physical_operator_ref: String,
    /// Parse-order occurrence key captured by the Adapter. This is the only
    /// ordinal used by the public ref allocator; opaque internal refs are not
    /// hash inputs.
    pub source_occurrence_ordinal: u32,
    pub fragment_ref: Option<String>,
    pub container_ref: Option<String>,
    pub driver_occurrence_ref: Option<String>,
    pub driver_occurrence_ordinal: Option<u32>,
    pub emission_ordinal: Option<u32>,
    pub plan_node_id: Option<String>,
    pub canonical_type: String,
    pub source_role: RuntimeSourceRole,
    pub source_refs: Vec<String>,
    pub reasons: Vec<String>,
}

/// One categorical operator fact (`PartType`, `DistributionMode`) as
/// printed in an operator's UniqueMetrics. It is a condition for rules, not a
/// measurement, so it is never projected as a metric. `value: None` means the
/// printed value is not a plain identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorInfoFact {
    pub physical_operator_ref: String,
    pub key: String,
    pub value: Option<String>,
    pub source_refs: Vec<String>,
}

/// Backend-only normalization model. It deliberately has no serde derive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedIr {
    pub schema_version: String,
    pub analysis_receipt: AnalysisReceipt,
    pub analysis_key: String,
    pub input: InputSummary,
    pub version: VersionContext,
    pub profile_context: ProfileContext,
    pub completeness: CompletenessReport,
    /// Optional source-backed query metadata.  Synthetic fixtures intentionally
    /// leave this absent rather than inventing a query identity.
    pub query_summary: Option<QuerySummary>,
    pub operator_groups: Vec<OperatorGroup>,
    pub metric_observations: Vec<MetricObservation>,
    /// Query-scoped metrics are distinct from operator/pipeline observations.
    pub derived_metric_cells: Vec<MetricCell>,
    /// Concrete runtime Pipeline occurrences captured at the Adapter -> IR seam.
    pub pipeline_containers: Vec<PipelineContainer>,
    /// Explicit driver occurrences retained without instance host/UUID material.
    pub runtime_driver_occurrences: Vec<RuntimeDriverOccurrence>,
    /// Direct physical-runtime-operator membership in captured Pipeline containers.
    pub physical_operator_memberships: Vec<PhysicalOperatorMembership>,
    pub operator_info_facts: Vec<OperatorInfoFact>,
    /// Request-local acquisition evidence for the G4a allowlist.
    pub runtime_acquisition_shape: RuntimeInventoryAcquisitionShape,
    /// Parser-proven formatter grammar. Acquisition and a matching version pack
    /// are necessary but never sufficient to assert this evidence.
    pub runtime_formatter_shape: doctor_contracts::RuntimeInventoryFormatterShape,
    pub graph: GraphView,
    pub findings: Vec<Finding>,
    pub rule_evaluations: Vec<RuleEvaluation>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableMetricFact {
    pub metric_id: String,
    pub subject_ref: String,
    pub state: MetricCellState,
    pub canonical_number: Option<CanonicalNumber>,
    pub canonical_unit: Option<String>,
    pub fidelity: Option<Fidelity>,
    pub bounds: Option<BoundedInterval>,
    pub order_rank: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableGraphNodeFact {
    pub node_id: String,
    pub entity_kind: GraphEntityKind,
    pub plan_node_id: Option<String>,
    pub fragment_id: Option<String>,
    pub pipeline_id: Option<String>,
    pub plan_node_type: Option<String>,
    pub source_ordinal: Option<u32>,
    pub native_operator_refs: Vec<PortableOrderedOperatorRef>,
    pub subordinate_operator_refs: Vec<PortableOrderedOperatorRef>,
    pub metric_observation_refs: Vec<String>,
    pub execution_state: Option<String>,
    pub source_quality: SourceQuality,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableOrderedOperatorRef {
    pub operator_ref: String,
    pub source_ordinal: u32,
    pub source_role: SourceRole,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableGraphEdgeFact {
    pub edge_id: String,
    pub from: String,
    pub to: String,
    pub relation: GraphRelation,
    pub direction_semantics: DirectionSemantics,
    pub source: EdgeSource,
    pub quality: SourceQuality,
    pub source_ordinal: Option<u32>,
    pub input_role: Option<InputRole>,
    pub derivation_recipe: Option<RecipeRef>,
    pub restates_edge_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableUnresolvedRelationFact {
    pub relation_ref: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableGraphFacts {
    pub state: GraphState,
    pub topology_source: TopologySource,
    pub root_node_ids: Vec<String>,
    pub nodes: Vec<PortableGraphNodeFact>,
    pub edges: Vec<PortableGraphEdgeFact>,
    pub unconnected_node_ids: Vec<String>,
    pub unresolved_relations: Vec<PortableUnresolvedRelationFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FactsProjection {
    pub version: PortableVersionFacts,
    pub profile_context: PortableProfileFacts,
    pub receipt: Vec<ReceiptComponent>,
    pub metric_cells: Vec<PortableMetricFact>,
    pub graph: PortableGraphFacts,
    pub findings: Vec<PortableFindingFact>,
    pub rule_evaluations: Vec<PortableRuleEvaluationFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableProfileFacts {
    pub profile_level: ProfileLevel,
    pub query_state: QueryState,
    pub snapshot_kind: SnapshotKind,
    pub merge_state: MergeState,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableVersionFacts {
    pub effective_version: Option<String>,
    pub version_source: VersionSource,
    pub selected_pack: Option<SelectedVersionPack>,
    pub pack_match: PackMatch,
    pub compatibility: Compatibility,
    pub support_status: SupportStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableFindingFact {
    pub finding_id: String,
    pub rule_id: String,
    pub rule_version: String,
    pub category: String,
    pub severity: String,
    pub confidence: FindingConfidence,
    pub evidence_completeness: doctor_contracts::EvidenceCompleteness,
    pub evidence_completeness_reasons: Vec<String>,
    pub subject_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PortableRuleEvaluationFact {
    pub rule_id: String,
    pub rule_version: String,
    pub status: RuleEvaluationStatus,
    pub evidence_completeness: doctor_contracts::EvidenceCompleteness,
    pub evidence_completeness_reasons: Vec<String>,
    pub subject_ref: Option<String>,
    pub finding_id: Option<String>,
}

pub fn portable_facts<'a>(
    version: &VersionContext,
    profile_context: ProfileContext,
    receipt: Vec<ReceiptComponent>,
    metric_cells: impl IntoIterator<Item = &'a MetricCell>,
    graph: &GraphView,
    findings: &[Finding],
    rule_evaluations: &[RuleEvaluation],
) -> FactsProjection {
    never_cancelled(portable_facts_cancellable(
        version,
        profile_context,
        receipt,
        metric_cells,
        graph,
        findings,
        rule_evaluations,
        &CancelSignal::never(),
    ))
}

/// `portable_facts` polling `cancel` once per metric cell unit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn portable_facts_cancellable<'a>(
    version: &VersionContext,
    profile_context: ProfileContext,
    receipt: Vec<ReceiptComponent>,
    metric_cells: impl IntoIterator<Item = &'a MetricCell>,
    graph: &GraphView,
    findings: &[Finding],
    rule_evaluations: &[RuleEvaluation],
    cancel: &CancelSignal<'_>,
) -> Result<FactsProjection, Cancelled> {
    let mut metric_cells = metric_cells
        .into_iter()
        .map(|cell| cancel.tick().map(|()| portable_metric_fact(cell)))
        .collect::<Result<Vec<_>, _>>()?;
    sort_by_polled(
        &mut metric_cells,
        |left, right| {
            left.subject_ref
                .cmp(&right.subject_ref)
                .then(left.metric_id.cmp(&right.metric_id))
                .then(left.order_rank.cmp(&right.order_rank))
        },
        cancel,
    )?;
    let mut portable_findings = findings
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
    portable_findings.sort_by(|left, right| left.finding_id.cmp(&right.finding_id));
    let mut portable_rule_evaluations = rule_evaluations
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
    portable_rule_evaluations.sort_by(|left, right| {
        left.rule_id
            .cmp(&right.rule_id)
            .then(left.subject_ref.cmp(&right.subject_ref))
    });
    Ok(FactsProjection {
        version: PortableVersionFacts {
            effective_version: version.effective_version.clone(),
            version_source: version.version_source.clone(),
            selected_pack: version.selected_pack.clone(),
            pack_match: version.pack_match.clone(),
            compatibility: version.compatibility.clone(),
            support_status: version.support_status.clone(),
        },
        profile_context: PortableProfileFacts {
            profile_level: profile_context.profile_level,
            query_state: profile_context.query_state,
            snapshot_kind: profile_context.snapshot_kind,
            merge_state: profile_context.merge_state,
        },
        receipt,
        metric_cells,
        graph: PortableGraphFacts {
            state: graph.state.clone(),
            topology_source: graph.topology_source.clone(),
            root_node_ids: graph.root_node_ids.clone(),
            nodes: graph
                .nodes
                .iter()
                .map(|node| PortableGraphNodeFact {
                    node_id: node.node_id.clone(),
                    entity_kind: node.entity_kind.clone(),
                    plan_node_id: node.plan_node_id.clone(),
                    fragment_id: node.fragment_id.clone(),
                    pipeline_id: node.pipeline_id.clone(),
                    plan_node_type: node.plan_node_type.clone(),
                    source_ordinal: node.source_ordinal,
                    native_operator_refs: node
                        .native_operator_refs
                        .iter()
                        .map(|operator| PortableOrderedOperatorRef {
                            operator_ref: operator.operator_ref.clone(),
                            source_ordinal: operator.source_ordinal,
                            source_role: operator.source_role.clone(),
                        })
                        .collect(),
                    subordinate_operator_refs: node
                        .subordinate_operator_refs
                        .iter()
                        .map(|operator| PortableOrderedOperatorRef {
                            operator_ref: operator.operator_ref.clone(),
                            source_ordinal: operator.source_ordinal,
                            source_role: operator.source_role.clone(),
                        })
                        .collect(),
                    metric_observation_refs: node.metric_observation_refs.clone(),
                    execution_state: node.execution_state.clone(),
                    source_quality: node.source_quality.clone(),
                })
                .collect(),
            edges: graph
                .edges
                .iter()
                .map(|edge| PortableGraphEdgeFact {
                    edge_id: edge.edge_id.clone(),
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    relation: edge.relation.clone(),
                    direction_semantics: edge.direction_semantics.clone(),
                    source: edge.source.clone(),
                    quality: edge.quality.clone(),
                    source_ordinal: edge.source_ordinal,
                    input_role: edge.input_role.clone(),
                    derivation_recipe: edge.derivation_recipe.clone(),
                    restates_edge_id: edge.restates_edge_id.clone(),
                })
                .collect(),
            unconnected_node_ids: graph.unconnected_node_ids.clone(),
            unresolved_relations: graph
                .unresolved_relations
                .iter()
                .map(|relation| PortableUnresolvedRelationFact {
                    relation_ref: relation.relation_ref.clone(),
                    reason: relation.reason.clone(),
                })
                .collect(),
        },
        findings: portable_findings,
        rule_evaluations: portable_rule_evaluations,
    })
}

pub(crate) fn never_cancelled<T>(result: Result<T, Cancelled>) -> T {
    match result {
        Ok(value) => value,
        Err(Cancelled) => unreachable!("CancelSignal::never() is never raised"),
    }
}

fn portable_metric_fact(cell: &MetricCell) -> PortableMetricFact {
    PortableMetricFact {
        metric_id: cell.metric_id.clone(),
        subject_ref: cell.subject_ref.clone(),
        state: cell.state(),
        canonical_number: cell.canonical_number().cloned(),
        canonical_unit: cell.canonical_unit().map(str::to_owned),
        fidelity: cell.fidelity(),
        bounds: cell.bounds().cloned(),
        order_rank: cell.order_rank,
    }
}

/// Hashes only portable facts; filename, source hashes, locators and display-only input stay out.
pub fn facts_hash(facts: &FactsProjection) -> String {
    canonical_hash(facts)
}

/// `facts_hash` polling `cancel` while it writes and digests the canonical bytes.
pub(crate) fn facts_hash_cancellable(
    facts: &FactsProjection,
    cancel: &CancelSignal<'_>,
) -> Result<String, Cancelled> {
    canonical_hash_cancellable(facts, &|| cancel.tick().is_err()).ok_or(Cancelled)
}

/// Builds the portable fact projection from a safe report without source-specific presentation fields.
pub fn report_facts(report: &AnalysisSnapshotReport) -> FactsProjection {
    never_cancelled(report_facts_cancellable(report, &CancelSignal::never()))
}

pub(crate) fn report_facts_cancellable(
    report: &AnalysisSnapshotReport,
    cancel: &CancelSignal<'_>,
) -> Result<FactsProjection, Cancelled> {
    let metric_cells = report.derived_metric_cells.iter().chain(
        report
            .operator_rows
            .iter()
            .flat_map(|operator| &operator.metric_cells),
    );
    portable_facts_cancellable(
        &report.version,
        report.profile_context.clone(),
        report.analysis_receipt.components.clone(),
        metric_cells,
        &report.graph_view,
        &report.findings,
        &report.rule_evaluations,
        cancel,
    )
}

#[derive(serde::Serialize)]
struct AnalysisKeyMaterial<'a> {
    source_hash: &'a str,
    receipt: &'a AnalysisReceipt,
    options: &'a BTreeMap<String, String>,
}

/// Source-specific replay key. Unlike `facts_hash`, this intentionally includes the input hash.
pub fn analysis_key(
    source_hash: &str,
    receipt: &AnalysisReceipt,
    options: &BTreeMap<String, String>,
) -> String {
    canonical_hash(&AnalysisKeyMaterial {
        source_hash,
        receipt,
        options,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphContractError {
    UnavailableGraphHasEdges,
    VerifiedDerivedEdgeMissingRecipe,
}

/// Keeps source order intact. No ID sort is permitted for semantically ordered source arrays.
pub fn graph_view(
    state: GraphState,
    topology_source: TopologySource,
    root_node_ids: Vec<String>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    unresolved_relations: Vec<doctor_contracts::UnresolvedRelation>,
) -> Result<GraphView, GraphContractError> {
    if state == GraphState::Unavailable && !edges.is_empty() {
        return Err(GraphContractError::UnavailableGraphHasEdges);
    }
    if edges
        .iter()
        .any(|edge| edge.source == EdgeSource::VerifiedDerived && edge.derivation_recipe.is_none())
    {
        return Err(GraphContractError::VerifiedDerivedEdgeMissingRecipe);
    }
    let completeness_state = match &state {
        GraphState::Complete => CompletenessState::Complete,
        GraphState::Partial => CompletenessState::Partial,
        GraphState::Unavailable => CompletenessState::Unavailable,
    };
    Ok(GraphView {
        state,
        topology_source,
        root_node_ids,
        nodes,
        edges,
        unconnected_node_ids: Vec::new(),
        unresolved_relations,
        source_refs: Vec::new(),
        completeness: DimensionCompleteness {
            state: completeness_state,
            present_count: 0,
            expected_count: None,
            reasons: Vec::new(),
        },
    })
}

fn number_order_to_zero(number: &CanonicalNumber) -> Option<Ordering> {
    let value = number.value.as_str();
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    if unsigned.is_empty()
        || unsigned.matches('.').count() > 1
        || unsigned.starts_with('.')
        || unsigned.ends_with('.')
        || !unsigned
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let digits = unsigned.replace('.', "");
    if digits.bytes().all(|byte| byte == b'0') {
        return Some(Ordering::Equal);
    }
    Some(if value.starts_with('-') {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

/// Closure is deliberate: an open endpoint at zero still makes division unsafe.
pub fn interval_contains_zero_by_closure(bounds: &BoundedInterval) -> bool {
    match (
        number_order_to_zero(&bounds.lower),
        number_order_to_zero(&bounds.upper),
    ) {
        (Some(Ordering::Greater), _) | (_, Some(Ordering::Less)) => false,
        (Some(_), Some(_)) => true,
        _ => true,
    }
}

pub fn percentage_state_for_baseline(baseline: &MetricCell) -> PercentageState {
    let state = baseline.state();
    if state == MetricCellState::Exact
        && baseline.bounds().is_none()
        && baseline.canonical_number().and_then(number_order_to_zero) == Some(Ordering::Equal)
    {
        PercentageState::UndefinedBaselineZero
    } else if baseline
        .bounds()
        .is_some_and(interval_contains_zero_by_closure)
    {
        PercentageState::UndefinedBaselineIntervalContainsZero
    } else if baseline.bounds().is_some() {
        PercentageState::Approximate
    } else if state == MetricCellState::Exact {
        PercentageState::Computed
    } else if matches!(
        state,
        MetricCellState::Missing | MetricCellState::Unavailable | MetricCellState::Unknown
    ) {
        PercentageState::Missing
    } else if matches!(
        state,
        MetricCellState::Invalid | MetricCellState::Overflow | MetricCellState::Partial
    ) {
        PercentageState::Invalid
    } else if state == MetricCellState::Incompatible {
        PercentageState::SemanticMismatch
    } else {
        PercentageState::Approximate
    }
}

#[derive(serde::Serialize)]
struct AttemptKeyMaterial<'a> {
    outcome_facts_hash: Option<&'a str>,
    error_code: Option<&'a str>,
    error_stage: Option<&'a str>,
}

#[derive(serde::Serialize)]
struct ComparisonKeyMaterial<'a> {
    schema_version: &'a str,
    receipt: &'a doctor_contracts::ComparisonReceipt,
    baseline: AttemptKeyMaterial<'a>,
    candidate: AttemptKeyMaterial<'a>,
}

fn attempt_key_material(attempt: &doctor_contracts::AnalysisAttempt) -> AttemptKeyMaterial<'_> {
    match attempt {
        doctor_contracts::AnalysisAttempt::Outcome(attempt) => AttemptKeyMaterial {
            outcome_facts_hash: Some(attempt.outcome.report.facts_hash.as_str()),
            error_code: None,
            error_stage: None,
        },
        doctor_contracts::AnalysisAttempt::Error(attempt) => AttemptKeyMaterial {
            outcome_facts_hash: None,
            error_code: Some(attempt.error.code.as_str()),
            error_stage: Some(attempt.error.stage.as_str()),
        },
    }
}

/// The material excludes `comparison_key` itself and all display/provenance-only fields.
pub fn comparison_key(comparison: &Comparison) -> String {
    canonical_hash(&ComparisonKeyMaterial {
        schema_version: &comparison.schema_version,
        receipt: &comparison.comparison_receipt,
        baseline: attempt_key_material(&comparison.baseline),
        candidate: attempt_key_material(&comparison.candidate),
    })
}
