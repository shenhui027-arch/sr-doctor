// AdapterError keeps bounded diagnostics and provenance inline; boxing every parser boundary
// would add churn without reducing the public request budget.
#![allow(clippy::result_large_err)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use doctor_contracts::{
    AnalysisReceipt, AnalysisState, BoundedFidelity, CanonicalNumber, CompletenessReport,
    CompletenessState, Diagnostic, DimensionCompleteness, DirectionSemantics, EdgeSource, Fidelity,
    GraphEdge, GraphEntityKind, GraphNode, GraphRelation, InputSummary, LosslessFidelity,
    MergeState, MetricCell, MetricCellValue, NumberKind, ObservedMetricFidelity,
    OrderedOperatorRef, PackMatch, ParseState, ProfileContext, ProfileLevel, QueryState,
    QuerySummary, RequestedSourceKind, RuntimeInventoryAcquisitionShape,
    RuntimeInventoryFormatterShape, RuntimeSourceRole, SnapshotKind, SourceKind, SourceLine,
    SourceQuality, SourceRole, SourceToken, SourceView, SupportStatus, TopologySource,
    UnknownFidelity, UnresolvedRelation, VersionContext, VersionEvidence, VersionSource,
};
use serde::{
    Deserialize, Deserializer,
    de::{Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use starrocks_version_pack_3_5_14::deltas::TextStructure;

use crate::{
    CancelSignal, Cancelled, CounterTextFormat, ExplicitGraphEvidence, FactsProjection,
    GraphEvidence, MetricInput, MetricObservation, MetricParseOutcome, MetricRegistryLookup,
    NormalizedIr, OperatorGroup, OperatorInfoFact, ParseMode, PhysicalOperatorMembership,
    PipelineContainer, RuntimeDriverOccurrence, analysis_key, drop_polled, facts_hash, graph_view,
    metric_lookup_for, parse_metric, portable_facts_cancellable, resolve_graph,
    sr3514_metric_lookup,
};

/// Request-local input only. The adapter never uses the filename to choose a parser or hash facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputEnvelope {
    pub requested_kind: RequestedSourceKind,
    pub content: Vec<u8>,
    pub filename: Option<String>,
    pub version_hint: Option<String>,
    pub acquisition_shape: RuntimeInventoryAcquisitionShape,
}

impl InputEnvelope {
    pub fn new(requested_kind: RequestedSourceKind, content: impl Into<Vec<u8>>) -> Self {
        Self {
            requested_kind,
            content: content.into(),
            filename: None,
            version_hint: None,
            acquisition_shape: RuntimeInventoryAcquisitionShape::OfflineImport,
        }
    }
}

/// Core-owned parse limits. HTTP body and concurrency limits stay in doctor-api.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterLimits {
    pub max_bytes: usize,
    pub max_text_lines: usize,
    pub max_text_line_bytes: usize,
    pub max_json_depth: usize,
    pub max_json_nodes: usize,
    pub max_normalized_metrics: usize,
}

/// Frozen component receipt supplied by the analysis assembler; adapters never invent it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterContext {
    analysis_receipt: AnalysisReceipt,
}

impl AdapterContext {
    pub fn new(analysis_receipt: AnalysisReceipt) -> Result<Self, AdapterError> {
        let mut component_ids = BTreeSet::new();
        let valid = analysis_receipt.components.len() == 6
            && analysis_receipt.components.iter().all(|component| {
                !component.id.trim().is_empty()
                    && !component.version.trim().is_empty()
                    && component.manifest_hash.len() == 64
                    && component
                        .manifest_hash
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    && component_ids.insert(component.id.as_str())
            });
        if !valid {
            return Err(error(
                "invalid_request",
                "normalize",
                "analysis_receipt_invalid",
                "change_input",
                vec![],
                vec![],
            ));
        }
        Ok(Self { analysis_receipt })
    }

    fn receipt(&self) -> &AnalysisReceipt {
        &self.analysis_receipt
    }
}

impl Default for AdapterLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_text_lines: 250_000,
            max_text_line_bytes: 256 * 1024,
            max_json_depth: 128,
            max_json_nodes: 20_000,
            max_normalized_metrics: 250_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLocator {
    Json {
        json_pointer: String,
        byte_start: u32,
        byte_end: u32,
    },
    Text {
        line_start: u32,
        line_end: u32,
        byte_start: u32,
        byte_end: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RedactionState {
    OriginalLocal,
    Redacted,
}

/// Internal provenance retains a typed locator; wire DTOs carry only the stable ref id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRef {
    pub ref_id: String,
    pub source_kind: SourceKind,
    pub locator: SourceLocator,
    pub redaction_state: RedactionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterError {
    pub code: String,
    pub stage: String,
    pub message_id: String,
    pub recoverability: String,
    pub source_refs: Vec<SourceRef>,
    pub adapter_evidence: Vec<String>,
    pub details: BTreeMap<String, String>,
}

/// `AdapterError::code` of an adapt stopped by its `CancelSignal`.
pub const ANALYSIS_CANCELLED: &str = "analysis_cancelled";

impl AdapterError {
    /// The caller stopped the analysis; this is not an input problem.
    pub fn is_cancelled(&self) -> bool {
        self.code == ANALYSIS_CANCELLED
    }
}

impl From<Cancelled> for AdapterError {
    fn from(_: Cancelled) -> Self {
        error(
            ANALYSIS_CANCELLED,
            "cancel",
            ANALYSIS_CANCELLED,
            "retry_same",
            vec![],
            vec![],
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterOutput {
    /// This is an adapter-stage result, not an `AnalysisSnapshotReport`: a validated six-component
    /// receipt is injected by `AdapterContext`, while public report projection remains deferred.
    pub ir: NormalizedIr,
    /// Internal, validated graph evidence for the version resolver. This is not
    /// a wire DTO and must be reused instead of re-parsing source at the API seam.
    pub graph_evidence: GraphEvidence,
    pub analysis_state: AnalysisState,
    /// Adapter-local canonical facts; do not present this fingerprint as a completed report hash.
    pub facts: FactsProjection,
    pub source_refs: Vec<SourceRef>,
    pub source_view: SourceView,
}

impl AdapterOutput {
    /// Stage fingerprint of `facts`, computed on demand. Production reads only
    /// the final report hash, so stages no longer hash a large Profile up to
    /// three extra times; the value is the same `facts_hash(&self.facts)`.
    pub fn facts_hash(&self) -> String {
        facts_hash(&self.facts)
    }
}

#[derive(Clone, Debug)]
struct VersionFact {
    value: String,
    source_ref: SourceRef,
}

#[derive(Clone, Debug)]
struct MetricFact {
    outcome: MetricParseOutcome,
    source_ref: SourceRef,
    display_text: String,
}

/// Facts emitted by the official RuntimeProfile format.  This is kept separate
/// from the older synthetic singleton fixture so the latter remains a stable
/// compatibility path.
#[derive(Clone, Debug)]
struct RuntimeMetricFact {
    metric_id: String,
    scope: String,
    source_name: String,
    value_role: String,
    outcome: MetricParseOutcome,
    source_ref: SourceRef,
    display_text: String,
    source_ordinal: u32,
}

/// Non-metric recipe input retained only until version-pack normalization.  It
/// is never projected as a DTO metric because `PartType` is a categorical
/// condition, not a measurement.
#[derive(Clone, Debug)]
struct RuntimeInfoFact {
    value: Option<String>,
    source_ref: SourceRef,
}

#[derive(Clone, Debug)]
struct RuntimeOperator {
    operator_group_id: String,
    plan_node_id: Option<String>,
    fragment_ref: Option<String>,
    pipeline_ref: Option<String>,
    canonical_type: String,
    /// FE `(operator id=N)` header suffix, when printed.
    native_operator_id: Option<i32>,
    /// Source merge role. `None` only while parsing; after extraction an
    /// operator without `IsSubordinate` is native, as in the official
    /// ExplainAnalyzer (see `finalize_native_operator_roles`).
    source_role: Option<SourceRole>,
    runtime_source_role: RuntimeSourceRole,
    /// A malformed `IsSubordinate` marker keeps the role unknown (fail closed).
    role_marker_malformed: bool,
    source_ref: SourceRef,
    metrics: Vec<RuntimeMetricFact>,
    is_final_sink: bool,
    partition_type_facts: Vec<RuntimeInfoFact>,
    distribution_mode_facts: Vec<RuntimeInfoFact>,
    table_facts: Vec<RuntimeInfoFact>,
    predicate_facts: Vec<RuntimeInfoFact>,
    join_predicate_facts: Vec<RuntimeInfoFact>,
}

#[derive(Clone, Debug)]
struct RuntimeTopology {
    value: Value,
    source_ref: SourceRef,
}

#[derive(Clone, Debug)]
struct RuntimeProfileData {
    formatter_shape: RuntimeInventoryFormatterShape,
    query_summary: QuerySummary,
    query_metrics: Vec<RuntimeMetricFact>,
    /// Only exact direct `Planner` / `Total[1]` (text) or `Planner.Total`
    /// (JsonProfileFormatter) candidates are admitted here.  Resolution is
    /// intentionally deferred so absence and multiplicity stay unavailable.
    planning_time_candidates: Vec<RuntimeMetricFact>,
    operators: Vec<RuntimeOperator>,
    plan_node_types: BTreeMap<String, String>,
    pipeline_containers: Vec<PipelineContainer>,
    runtime_driver_occurrences: Vec<RuntimeDriverOccurrence>,
    physical_operator_memberships: Vec<PhysicalOperatorMembership>,
}

struct TextGraphNode {
    fields: String,
    source_ref: SourceRef,
}

struct ParsedGraphNode {
    node: GraphNode,
    children: Vec<String>,
    sink_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct ProfileFacts {
    profile_level: ProfileLevel,
    query_state: QueryState,
    snapshot_kind: SnapshotKind,
    merge_state: MergeState,
    profile_level_source: Option<SourceRef>,
    query_state_source: Option<SourceRef>,
    snapshot_kind_source: Option<SourceRef>,
    merge_state_source: Option<SourceRef>,
    profile_level_seen: bool,
    query_state_seen: bool,
    snapshot_kind_seen: bool,
    merge_state_seen: bool,
}

impl ProfileFacts {
    fn unknown() -> Self {
        Self {
            profile_level: ProfileLevel::Unknown,
            query_state: QueryState::Unknown,
            snapshot_kind: SnapshotKind::Unknown,
            merge_state: MergeState::Unknown,
            profile_level_source: None,
            query_state_source: None,
            snapshot_kind_source: None,
            merge_state_source: None,
            profile_level_seen: false,
            query_state_seen: false,
            snapshot_kind_seen: false,
            merge_state_seen: false,
        }
    }

    fn into_context(self) -> ProfileContext {
        let source_refs = [
            self.profile_level_source,
            self.query_state_source,
            self.snapshot_kind_source,
            self.merge_state_source,
        ]
        .into_iter()
        .flatten()
        .map(|source| source.ref_id)
        .fold(Vec::new(), |mut refs, ref_id| {
            if !refs.contains(&ref_id) {
                refs.push(ref_id);
            }
            refs
        });
        let mut limitations = Vec::new();
        if self.profile_level == ProfileLevel::Unknown {
            limitations.push("profile_level_unknown".into());
        }
        if self.query_state == QueryState::Unknown {
            limitations.push("query_state_unknown".into());
        }
        if self.snapshot_kind == SnapshotKind::Unknown {
            limitations.push("snapshot_kind_unknown".into());
        }
        if self.merge_state == MergeState::Unknown {
            limitations.push("merge_state_unknown".into());
        }
        ProfileContext {
            profile_level: self.profile_level,
            query_state: self.query_state,
            snapshot_kind: self.snapshot_kind,
            merge_state: self.merge_state,
            evidence_refs: source_refs,
            limitations,
        }
    }

    fn set_profile_level(
        &mut self,
        value: &str,
        source: SourceRef,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.profile_level_seen {
            self.profile_level = ProfileLevel::Unknown;
            self.profile_level_source = None;
            diagnostics.push(diagnostic(
                "profile_context_conflict",
                "parse",
                "warning",
                "partial",
                "profile_context_conflict",
                vec![source.ref_id],
            ));
            return;
        }
        self.profile_level_seen = true;
        match value.trim() {
            "1" => {
                self.profile_level = ProfileLevel::Level1;
                self.profile_level_source = Some(source);
            }
            "2" => {
                self.profile_level = ProfileLevel::Level2;
                self.profile_level_source = Some(source);
            }
            _ => diagnostics.push(diagnostic(
                "profile_context_invalid",
                "parse",
                "warning",
                "partial",
                "profile_context_invalid",
                vec![source.ref_id],
            )),
        }
    }

    fn set_query_state(
        &mut self,
        value: &str,
        source: SourceRef,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.query_state_seen {
            self.query_state = QueryState::Unknown;
            self.query_state_source = None;
            diagnostics.push(diagnostic(
                "profile_context_conflict",
                "parse",
                "warning",
                "partial",
                "profile_context_conflict",
                vec![source.ref_id],
            ));
            return;
        }
        self.query_state_seen = true;
        match value.trim().to_ascii_lowercase().as_str() {
            "finished" => {
                self.query_state = QueryState::Finished;
                self.query_state_source = Some(source);
            }
            "running" => {
                self.query_state = QueryState::Running;
                self.query_state_source = Some(source);
            }
            "error" => {
                self.query_state = QueryState::Error;
                self.query_state_source = Some(source);
            }
            _ => diagnostics.push(diagnostic(
                "profile_context_invalid",
                "parse",
                "warning",
                "partial",
                "profile_context_invalid",
                vec![source.ref_id],
            )),
        }
    }

    fn set_snapshot_kind(
        &mut self,
        value: &str,
        source: SourceRef,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.snapshot_kind_seen {
            self.snapshot_kind = SnapshotKind::Unknown;
            self.snapshot_kind_source = None;
            diagnostics.push(diagnostic(
                "profile_context_conflict",
                "parse",
                "warning",
                "partial",
                "profile_context_conflict",
                vec![source.ref_id],
            ));
            return;
        }
        self.snapshot_kind_seen = true;
        match value.trim().to_ascii_lowercase().as_str() {
            "final" => {
                self.snapshot_kind = SnapshotKind::Final;
                self.snapshot_kind_source = Some(source);
            }
            "runtime" => {
                self.snapshot_kind = SnapshotKind::Runtime;
                self.snapshot_kind_source = Some(source);
            }
            _ => diagnostics.push(diagnostic(
                "profile_context_invalid",
                "parse",
                "warning",
                "partial",
                "profile_context_invalid",
                vec![source.ref_id],
            )),
        }
    }

    fn set_merge_state(
        &mut self,
        value: &str,
        source: SourceRef,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.merge_state_seen {
            self.merge_state = MergeState::Unknown;
            self.merge_state_source = None;
            diagnostics.push(diagnostic(
                "profile_context_conflict",
                "parse",
                "warning",
                "partial",
                "profile_context_conflict",
                vec![source.ref_id],
            ));
            return;
        }
        self.merge_state_seen = true;
        match value.trim().to_ascii_lowercase().as_str() {
            "merged" => {
                self.merge_state = MergeState::Merged;
                self.merge_state_source = Some(source);
            }
            "unmerged" => {
                self.merge_state = MergeState::Unmerged;
                self.merge_state_source = Some(source);
            }
            _ => diagnostics.push(diagnostic(
                "profile_context_invalid",
                "parse",
                "warning",
                "partial",
                "profile_context_invalid",
                vec![source.ref_id],
            )),
        }
    }
}

/// Selects exactly one adapter and produces only the v0.1 synthetic facts this module can prove.
pub fn adapt_input(
    input: InputEnvelope,
    limits: &AdapterLimits,
    context: &AdapterContext,
) -> Result<AdapterOutput, AdapterError> {
    adapt_input_cancellable(input, limits, context, &CancelSignal::never())
}

/// `adapt_input` that polls `cancel` inside its long loops and returns an
/// `ANALYSIS_CANCELLED` error, never a partial output, once it is raised.
pub fn adapt_input_cancellable(
    input: InputEnvelope,
    limits: &AdapterLimits,
    context: &AdapterContext,
    cancel: &CancelSignal<'_>,
) -> Result<AdapterOutput, AdapterError> {
    check_bytes(input.content.len(), limits)?;
    let detected = detect_kind(&input.content)?;
    let adapter_evidence = match input.requested_kind {
        RequestedSourceKind::Auto => vec![match detected {
            SourceKind::JsonProfile => "auto:json:first_token".into(),
            SourceKind::TextProfile => "auto:text:first_token".into(),
        }],
        RequestedSourceKind::JsonProfile if detected != SourceKind::JsonProfile => {
            return Err(error(
                "kind_mismatch",
                "detect",
                "requested_json_mismatch",
                "change_input",
                vec![],
                vec!["explicit:json_profile".into()],
            ));
        }
        RequestedSourceKind::TextProfile if detected != SourceKind::TextProfile => {
            return Err(error(
                "kind_mismatch",
                "detect",
                "requested_text_mismatch",
                "change_input",
                vec![],
                vec!["explicit:text_profile".into()],
            ));
        }
        RequestedSourceKind::JsonProfile => vec!["explicit:json_profile".into()],
        RequestedSourceKind::TextProfile => vec!["explicit:text_profile".into()],
    };

    let extracted = match detected {
        SourceKind::JsonProfile => parse_json(&input.content, limits, &adapter_evidence, cancel)?,
        SourceKind::TextProfile => parse_text(&input.content, limits, &adapter_evidence, cancel)?,
    };
    build_output(
        input,
        detected,
        adapter_evidence,
        extracted,
        context,
        cancel,
    )
}

struct Extracted {
    line_count: Option<u32>,
    version: Option<VersionFact>,
    metric: Option<MetricFact>,
    profile_facts: ProfileFacts,
    source_refs: Vec<SourceRef>,
    diagnostics: Vec<Diagnostic>,
    graph_evidence: Option<GraphEvidence>,
    source_view: SourceView,
    runtime: Option<RuntimeProfileData>,
}

struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            values.insert(key, object.next_value::<StrictJsonValue>()?.0);
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

/// JSON input stays small in work (at most `max_json_nodes` values), so it
/// is polled around the parse and per fragment rather than per value.
fn parse_json(
    content: &[u8],
    limits: &AdapterLimits,
    evidence: &[String],
    cancel: &CancelSignal<'_>,
) -> Result<Extracted, AdapterError> {
    check_json_budget(content, limits, evidence)?;
    cancel.check()?;
    let value = serde_json::from_slice::<StrictJsonValue>(content)
        .map(|value| value.0)
        .map_err(|_| {
            error(
                "parse_failed",
                "parse",
                "json_invalid",
                "change_input",
                vec![],
                evidence.to_vec(),
            )
        })?;
    cancel.check()?;
    let object = value.as_object().ok_or_else(|| {
        error(
            "parse_failed",
            "parse",
            "json_object_required",
            "change_input",
            vec![],
            evidence.to_vec(),
        )
    })?;
    if object.get("Query").is_some_and(Value::is_object) {
        return parse_runtime_profile_json(content, object, limits, evidence, cancel);
    }
    let mut diagnostics = Vec::new();
    let mut source_refs = Vec::new();
    let mut tokens = Vec::new();
    let version = match object.get("profile_version") {
        Some(Value::String(value)) => {
            let source_ref = json_ref(content, "profile_version")?;
            source_refs.push(source_ref.clone());
            if is_version(value) {
                tokens.push(SourceToken {
                    kind: "string".into(),
                    text: value.into(),
                    source_ref: source_ref.ref_id.clone(),
                });
                Some(VersionFact {
                    value: value.into(),
                    source_ref,
                })
            } else {
                diagnostics.push(diagnostic(
                    "version_unrecognized",
                    "parse",
                    "warning",
                    "partial",
                    "version_unrecognized",
                    vec![source_ref.ref_id],
                ));
                None
            }
        }
        Some(_) => {
            let source_ref = json_ref(content, "profile_version")?;
            source_refs.push(source_ref.clone());
            diagnostics.push(diagnostic(
                "version_unrecognized",
                "parse",
                "warning",
                "partial",
                "version_unrecognized",
                vec![source_ref.ref_id],
            ));
            None
        }
        None => None,
    };

    let metric = match object.get("operator_total_time_ns") {
        None => None,
        Some(value) => {
            check_metric_budget(1, limits, evidence)?;
            let source_ref = json_ref(content, "operator_total_time_ns")?;
            source_refs.push(source_ref.clone());
            let raw = match value {
                Value::String(value) => value.clone(),
                Value::Number(_) => json_number_token(content, &source_ref).ok_or_else(|| {
                    error(
                        "parse_failed",
                        "parse",
                        "json_number_locator_missing",
                        "change_input",
                        vec![],
                        evidence.to_vec(),
                    )
                })?,
                Value::Null => String::new(),
                _ => String::new(),
            };
            let metric_input = if value.is_null() {
                MetricInput::Missing
            } else if value.is_string() || value.is_number() {
                // JSON strings preserve large counters; numeric values retain their source token above.
                MetricInput::JsonNumber(raw.clone())
            } else {
                MetricInput::Text(raw.clone())
            };
            let definition =
                match sr3514_metric_lookup(SourceKind::JsonProfile, "OperatorTotalTime") {
                    MetricRegistryLookup::Registered(definition) => definition,
                    MetricRegistryLookup::UnknownNonAdditiveUnavailable => {
                        return Err(error(
                            "internal_error",
                            "normalize",
                            "registry_metric_missing",
                            "report_bug",
                            vec![source_ref],
                            evidence.to_vec(),
                        ));
                    }
                };
            let outcome = parse_metric(&definition, metric_input, ParseMode::Strict);
            let display_text = outcome
                .canonical_number
                .as_ref()
                .zip(outcome.canonical_unit.as_ref())
                .map(|(number, unit)| format!("{}{}", number.value, unit))
                .unwrap_or_else(|| "—".into());
            if outcome.parse_state == ParseState::Exact && raw.len() <= 128 {
                tokens.push(SourceToken {
                    kind: "number".into(),
                    text: raw.clone(),
                    source_ref: source_ref.ref_id.clone(),
                });
            }
            if outcome.parse_state != ParseState::Exact {
                diagnostics.push(metric_diagnostic(
                    "OperatorTotalTime",
                    &outcome,
                    &source_ref,
                ));
            }
            Some(MetricFact {
                outcome,
                source_ref,
                display_text,
            })
        }
    };
    let profile_facts =
        parse_json_profile_context(content, object, &mut source_refs, &mut diagnostics)?;
    let graph_evidence = parse_json_graph(content, object, &mut source_refs, &mut diagnostics);
    for (index, key) in object
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "profile_version"
                    | "operator_total_time_ns"
                    | "fixture"
                    | "execution_topology"
                    | "executionTopology"
                    | "pipeline_profile_level"
                    | "query_state"
                    | "snapshot_kind"
                    | "merge_state"
            )
        })
        .enumerate()
    {
        let source_ref = opaque_json_ref(content, key, index)?;
        source_refs.push(source_ref.clone());
        diagnostics.push(diagnostic(
            "unknown_json_field",
            "parse",
            "info",
            "partial",
            "unknown_json_field",
            vec![source_ref.ref_id],
        ));
    }
    Ok(Extracted {
        line_count: None,
        version,
        metric,
        profile_facts,
        source_refs,
        diagnostics,
        graph_evidence,
        source_view: SourceView {
            source_kind: SourceKind::JsonProfile,
            plain_text_lines: vec![],
            json_tokens: tokens,
            truncation_state: "complete".into(),
            limitations: vec!["synthetic_adapter_tokens_only".into()],
            line_refs: vec![],
        },
        runtime: None,
    })
}

fn parse_json_profile_context(
    content: &[u8],
    object: &serde_json::Map<String, Value>,
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<ProfileFacts, AdapterError> {
    let mut facts = ProfileFacts::unknown();
    for key in [
        "pipeline_profile_level",
        "query_state",
        "snapshot_kind",
        "merge_state",
    ] {
        let Some(value) = object.get(key) else {
            continue;
        };
        let source_ref = json_ref(content, key)?;
        source_refs.push(source_ref.clone());
        let scalar = match value {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            _ => String::new(),
        };
        match key {
            "pipeline_profile_level" => {
                facts.set_profile_level(&scalar, source_ref, diagnostics);
            }
            "query_state" => facts.set_query_state(&scalar, source_ref, diagnostics),
            "snapshot_kind" => facts.set_snapshot_kind(&scalar, source_ref, diagnostics),
            "merge_state" => facts.set_merge_state(&scalar, source_ref, diagnostics),
            _ => {}
        }
    }
    Ok(facts)
}

fn parse_text(
    content: &[u8],
    limits: &AdapterLimits,
    evidence: &[String],
    cancel: &CancelSignal<'_>,
) -> Result<Extracted, AdapterError> {
    let line_count = check_text_budget(content, limits, evidence)?;
    let text = std::str::from_utf8(content).map_err(|_| {
        error(
            "parse_failed",
            "parse",
            "text_not_utf8",
            "change_input",
            vec![],
            evidence.to_vec(),
        )
    })?;
    if is_runtime_profile_text(text) {
        return parse_runtime_profile_text(text, line_count, limits, evidence, cancel);
    }
    let mut diagnostics = Vec::new();
    let mut source_refs = Vec::new();
    let mut lines = Vec::new();
    let mut version = None;
    let mut metric = None;
    let mut profile_facts = ProfileFacts::unknown();
    let mut topology_complete = None;
    let mut graph_roots = Vec::new();
    let mut graph_nodes = Vec::new();
    let mut start = 0usize;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        cancel.tick()?;
        let without_lf = raw.strip_suffix('\n').unwrap_or(raw);
        let line = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let end = start + line.len();
        let line_no = u32::try_from(index + 1).map_err(|_| {
            limit_error(
                "too_many_lines",
                index + 1,
                u32::MAX as usize,
                evidence.to_vec(),
            )
        })?;
        let version_line = line.starts_with("StarRocks Profile ");
        let metric_line = line.starts_with("OperatorTotalTime:");
        let topology_line = line.starts_with("GraphTopologyComplete:");
        let graph_root_line = line.starts_with("GraphRoot:");
        let graph_node_line = line.starts_with("GraphNode:");
        let profile_level_line = line.starts_with("PipelineProfileLevel:");
        let query_state_line = line.starts_with("QueryState:");
        let snapshot_kind_line = line.starts_with("SnapshotKind:");
        let merge_state_line = line.starts_with("MergeState:");
        if let Some(value) = line.strip_prefix("StarRocks Profile ") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            if is_version(value) && version.is_none() {
                lines.push(SourceLine {
                    line_no,
                    text: format!("StarRocks Profile {value}"),
                    source_ref: source_ref.ref_id.clone(),
                });
                version = Some(VersionFact {
                    value: value.into(),
                    source_ref,
                });
            } else if version.is_some() {
                diagnostics.push(diagnostic(
                    "duplicate_version",
                    "parse",
                    "warning",
                    "partial",
                    "duplicate_version",
                    vec![source_ref.ref_id],
                ));
            } else {
                diagnostics.push(diagnostic(
                    "version_unrecognized",
                    "parse",
                    "warning",
                    "partial",
                    "version_unrecognized",
                    vec![source_ref.ref_id],
                ));
            }
        }
        if let Some(value) = line.strip_prefix("OperatorTotalTime:") {
            check_metric_budget(1, limits, evidence)?;
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            if metric.is_some() {
                diagnostics.push(diagnostic(
                    "duplicate_metric",
                    "parse",
                    "warning",
                    "partial",
                    "duplicate_metric",
                    vec![source_ref.ref_id],
                ));
            } else {
                let raw = value.trim();
                let definition =
                    match sr3514_metric_lookup(SourceKind::TextProfile, "OperatorTotalTime") {
                        MetricRegistryLookup::Registered(definition) => definition,
                        MetricRegistryLookup::UnknownNonAdditiveUnavailable => {
                            return Err(error(
                                "internal_error",
                                "normalize",
                                "registry_metric_missing",
                                "report_bug",
                                vec![source_ref],
                                evidence.to_vec(),
                            ));
                        }
                    };
                let outcome = parse_metric(
                    &definition,
                    MetricInput::Text(raw.into()),
                    ParseMode::Strict,
                );
                let display_text = if outcome.parse_state == ParseState::Exact {
                    raw.into()
                } else {
                    "—".into()
                };
                if outcome.parse_state == ParseState::Exact && raw.len() <= 128 {
                    lines.push(SourceLine {
                        line_no,
                        text: format!("OperatorTotalTime: {raw}"),
                        source_ref: source_ref.ref_id.clone(),
                    });
                } else {
                    diagnostics.push(metric_diagnostic(
                        "OperatorTotalTime",
                        &outcome,
                        &source_ref,
                    ));
                }
                metric = Some(MetricFact {
                    outcome,
                    source_ref,
                    display_text,
                });
            }
        }
        if let Some(value) = line.strip_prefix("PipelineProfileLevel:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            profile_facts.set_profile_level(value, source_ref, &mut diagnostics);
        }
        if let Some(value) = line.strip_prefix("QueryState:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            profile_facts.set_query_state(value, source_ref, &mut diagnostics);
        }
        if let Some(value) = line.strip_prefix("SnapshotKind:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            profile_facts.set_snapshot_kind(value, source_ref, &mut diagnostics);
        }
        if let Some(value) = line.strip_prefix("MergeState:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            profile_facts.set_merge_state(value, source_ref, &mut diagnostics);
        }
        if let Some(value) = line.strip_prefix("GraphTopologyComplete:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            match value.trim() {
                "true" => topology_complete = Some((true, source_ref)),
                "false" => topology_complete = Some((false, source_ref)),
                _ => {
                    diagnostics.push(diagnostic(
                        "graph_evidence_invalid",
                        "graph",
                        "warning",
                        "partial",
                        "graph_evidence_invalid",
                        vec![source_ref.ref_id],
                    ));
                    topology_complete = None;
                }
            }
        }
        if let Some(value) = line.strip_prefix("GraphRoot:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            graph_roots.push((value.trim().into(), source_ref));
        }
        if let Some(value) = line.strip_prefix("GraphNode:") {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            graph_nodes.push(TextGraphNode {
                fields: value.trim().into(),
                source_ref,
            });
        }
        if !version_line
            && !metric_line
            && !topology_line
            && !graph_root_line
            && !graph_node_line
            && !profile_level_line
            && !query_state_line
            && !snapshot_kind_line
            && !merge_state_line
            && !line.is_empty()
        {
            let source_ref = text_ref(line_no, start, end)?;
            source_refs.push(source_ref.clone());
            let code = if line.starts_with("Fragment ") {
                "fragment_structure_unresolved"
            } else {
                "unconsumed_text_line"
            };
            diagnostics.push(diagnostic(
                code,
                "parse",
                "info",
                "partial",
                code,
                vec![source_ref.ref_id],
            ));
        }
        start += raw.len();
    }
    let graph_evidence = parse_text_graph(
        topology_complete,
        graph_roots,
        graph_nodes,
        &mut diagnostics,
    );
    Ok(Extracted {
        line_count: Some(u32::try_from(line_count).map_err(|_| {
            limit_error(
                "too_many_lines",
                line_count,
                u32::MAX as usize,
                evidence.to_vec(),
            )
        })?),
        version,
        metric,
        profile_facts,
        source_refs,
        diagnostics,
        graph_evidence,
        source_view: SourceView {
            source_kind: SourceKind::TextProfile,
            plain_text_lines: lines,
            json_tokens: vec![],
            truncation_state: "complete".into(),
            limitations: vec!["synthetic_adapter_lines_only".into()],
            line_refs: vec![],
        },
        runtime: None,
    })
}

fn is_runtime_profile_text(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "Query:")
        && text.lines().any(|line| line.trim() == "Summary:")
}

/// Parses the official `RuntimeProfile::prettyPrint` layout.  This deliberately
/// recognizes only stable structural headers and registry-backed counters.  Any
/// unrecognised line stays local and is neither exposed through SourceView nor
/// converted into a metric.
fn parse_runtime_profile_text(
    text: &str,
    line_count: usize,
    limits: &AdapterLimits,
    evidence: &[String],
    cancel: &CancelSignal<'_>,
) -> Result<Extracted, AdapterError> {
    let format = text_counter_format(text);
    let structure = text_structure(text);
    let mut diagnostics = Vec::new();
    let mut source_refs = Vec::new();
    let mut safe_lines = Vec::new();
    let mut version = None::<VersionFact>;
    let mut version_origin_ref = None::<String>;
    let mut version_conflicted = false;
    let mut profile_facts = ProfileFacts::unknown();
    let mut stored_profile_level = None::<(ProfileLevel, SourceRef)>;
    let mut stored_profile_level_conflicted = false;
    let mut final_profile_marker = None::<(bool, SourceRef)>;
    let mut final_profile_marker_conflicted = false;
    let mut query_id = None;
    let mut query_type = None;
    let mut query_metrics = Vec::new();
    let mut planning_time_candidates = Vec::new();
    let mut operators = Vec::new();
    let mut pipeline_containers = Vec::new();
    let mut runtime_driver_occurrences = Vec::new();
    let mut physical_operator_memberships = Vec::new();
    let mut pipeline_container_occurrences = BTreeMap::new();
    let mut topology = None;
    let mut topology_first_ref = None::<String>;
    let mut topology_conflict = None::<Vec<String>>;
    let mut section = RuntimeTextSection::Other;
    let mut fragment = None::<String>;
    let mut pipeline = None::<String>;
    // `prettyPrint` repeats a pipeline's operator headers for each PipelineDriver.
    // The source-order occurrences below are deliberately local structural context:
    // they distinguish repeated native rows without retaining instance UUIDs or hosts.
    let mut fragment_occurrence = None::<u32>;
    let mut instance_occurrence = None::<u32>;
    let mut pipeline_occurrence = None::<u32>;
    let mut pipeline_driver = None::<(String, u32, String)>;
    let mut driver_emission_ordinal = None::<u32>;
    let mut next_fragment_occurrence = 0usize;
    let mut next_instance_occurrence = 0usize;
    let mut next_pipeline_occurrence = 0usize;
    let mut next_driver_occurrence = 0usize;
    let mut next_operator_occurrence = 0usize;
    let mut current_fragment_container = None::<usize>;
    let mut current_pipeline_container = None::<usize>;
    // Indentation of the current `Pipeline (id=…):` or `PipelineDriver (id=…):`
    // header; a bare legacy operator header is only accepted as its direct child.
    let mut operator_parent_indent = None::<usize>;
    let mut current_operator = None::<usize>;
    let mut operator_metric_section = None::<RuntimeOperatorMetricSection>;
    let mut invalid_operator_indent = None::<usize>;
    let mut metric_count = 0usize;
    let mut start = 0usize;
    let mut formatter = RuntimePrettyPrintEvidence::default();

    for (index, raw) in text.split_inclusive('\n').enumerate() {
        cancel.tick()?;
        let without_lf = raw.strip_suffix('\n').unwrap_or(raw);
        let line = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let end = start + line.len();
        let line_no = u32::try_from(index + 1).map_err(|_| {
            limit_error(
                "too_many_lines",
                index + 1,
                u32::MAX as usize,
                evidence.to_vec(),
            )
        })?;
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if let Some(header_indent) = invalid_operator_indent {
            if indent > header_indent || !runtime_operator_recovery_boundary(trimmed, structure) {
                start += raw.len();
                continue;
            }
            invalid_operator_indent = None;
        }

        if line == "Query:" {
            formatter.query_root = true;
        }

        if trimmed == "Summary:" {
            formatter.summary = true;
            section = RuntimeTextSection::Summary;
            current_operator = None;
            operator_metric_section = None;
            start += raw.len();
            continue;
        }
        if trimmed == "Planner:" {
            section = RuntimeTextSection::Planner;
            current_operator = None;
            operator_metric_section = None;
            start += raw.len();
            continue;
        }
        if trimmed == "Execution:" {
            formatter.execution = true;
            section = RuntimeTextSection::Execution;
            current_operator = None;
            operator_metric_section = None;
            start += raw.len();
            continue;
        }
        if let Some(value) = trimmed
            .strip_prefix("Fragment ")
            .and_then(|value| value.strip_suffix(':'))
        {
            section = RuntimeTextSection::Execution;
            let source_ref = text_ref(line_no, start, end)?;
            let fragment_id = value.trim();
            if fragment_id.is_empty() {
                fragment = None;
                pipeline = None;
                fragment_occurrence = None;
                instance_occurrence = None;
                pipeline_occurrence = None;
                pipeline_driver = None;
                driver_emission_ordinal = None;
                current_fragment_container = None;
                current_pipeline_container = None;
                current_operator = None;
                operator_metric_section = None;
                diagnostics.push(diagnostic(
                    "runtime_fragment_ref_missing",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_fragment_ref_missing",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
                start += raw.len();
                continue;
            }
            fragment = Some(fragment_id.to_owned());
            formatter.fragment = true;
            pipeline = None;
            fragment_occurrence = Some(next_runtime_occurrence(
                &mut next_fragment_occurrence,
                limits,
                evidence,
            )?);
            instance_occurrence = None;
            pipeline_occurrence = None;
            pipeline_driver = None;
            driver_emission_ordinal = None;
            current_pipeline_container = None;
            let container = runtime_container(RuntimeContainerInput {
                operators: &mut operators,
                pipeline_containers: &mut pipeline_containers,
                pipeline_container_occurrences: &mut pipeline_container_occurrences,
                kind: "fragment",
                fragment_ref: fragment.as_deref(),
                pipeline_ref: None,
                canonical_type: "FRAGMENT",
                source_ref: source_ref.clone(),
                limits,
                evidence,
            })?;
            current_fragment_container = Some(container);
            current_operator = Some(container);
            operator_metric_section = None;
            source_refs.push(source_ref);
            start += raw.len();
            continue;
        }
        if let Some(value) = trimmed
            .strip_prefix("Pipeline (id=")
            .and_then(|value| value.strip_suffix("):"))
        {
            let source_ref = text_ref(line_no, start, end)?;
            let pipeline_id = value.trim();
            if pipeline_id.is_empty() {
                pipeline = None;
                pipeline_occurrence = None;
                pipeline_driver = None;
                driver_emission_ordinal = None;
                current_pipeline_container = None;
                current_operator = current_fragment_container;
                operator_metric_section = None;
                diagnostics.push(diagnostic(
                    "runtime_pipeline_ref_missing",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_pipeline_ref_missing",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
                start += raw.len();
                continue;
            }
            let Some(fragment_ref) = fragment.as_deref() else {
                diagnostics.push(diagnostic(
                    "runtime_pipeline_without_fragment",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_pipeline_without_fragment",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
                start += raw.len();
                continue;
            };
            pipeline = Some(pipeline_id.to_owned());
            formatter.pipeline = true;
            pipeline_occurrence = Some(next_runtime_occurrence(
                &mut next_pipeline_occurrence,
                limits,
                evidence,
            )?);
            pipeline_driver = None;
            driver_emission_ordinal = None;
            let container = runtime_container(RuntimeContainerInput {
                operators: &mut operators,
                pipeline_containers: &mut pipeline_containers,
                pipeline_container_occurrences: &mut pipeline_container_occurrences,
                kind: "pipeline",
                fragment_ref: Some(fragment_ref),
                pipeline_ref: pipeline.as_deref(),
                canonical_type: "PIPELINE",
                source_ref: source_ref.clone(),
                limits,
                evidence,
            })?;
            current_pipeline_container = Some(container);
            operator_parent_indent = Some(indent);
            current_operator = Some(container);
            operator_metric_section = None;
            source_refs.push(source_ref);
            start += raw.len();
            continue;
        }
        if runtime_instance_header(trimmed) {
            // Instance IDs carry host/UUID details. They define an occurrence boundary
            // but are intentionally never retained in DTO identity or SourceView.
            instance_occurrence = Some(next_runtime_occurrence(
                &mut next_instance_occurrence,
                limits,
                evidence,
            )?);
            pipeline = None;
            pipeline_occurrence = None;
            pipeline_driver = None;
            driver_emission_ordinal = None;
            current_pipeline_container = None;
            current_operator = current_fragment_container;
            operator_metric_section = None;
            formatter.instance = true;
            start += raw.len();
            continue;
        }
        if let Some(driver_id) = runtime_pipeline_driver_header(trimmed) {
            let source_ref = text_ref(line_no, start, end)?;
            match current_pipeline_container {
                Some(container) => {
                    // Driver counters have no one-to-one operator attribution in
                    // prettyPrint. Keep them on the explicit Pipeline scope.
                    let driver_occurrence =
                        next_runtime_occurrence(&mut next_driver_occurrence, limits, evidence)?;
                    let container_ref = operators
                        .get(container)
                        .map(|operator| operator.operator_group_id.clone())
                        .ok_or_else(|| {
                            error(
                                "internal_error",
                                "normalize",
                                "runtime_pipeline_container_missing",
                                "report_bug",
                                vec![source_ref.clone()],
                                evidence.to_vec(),
                            )
                        })?;
                    let driver_occurrence_ref =
                        format!("driver-occurrence:{container_ref}:{driver_occurrence}");
                    runtime_driver_occurrences.push(RuntimeDriverOccurrence {
                        driver_occurrence_ref: driver_occurrence_ref.clone(),
                        container_ref,
                        occurrence_ordinal: driver_occurrence,
                        source_refs: vec![source_ref.ref_id.clone()],
                    });
                    pipeline_driver = Some((driver_id, driver_occurrence, driver_occurrence_ref));
                    driver_emission_ordinal = Some(0);
                    operator_parent_indent = Some(indent);
                    current_operator = Some(container);
                    operator_metric_section = None;
                    source_refs.push(source_ref);
                    formatter.pipeline_driver = true;
                }
                None => diagnostics.push(diagnostic(
                    "runtime_driver_without_pipeline",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_driver_without_pipeline",
                    vec![source_ref.ref_id],
                )),
            }
            start += raw.len();
            continue;
        }
        // Before 3.1.3 an operator with a negative plan node id is printed by its
        // name alone or with `(pseudo_plan_node_id=…)`. It is accepted only as a
        // direct child of the current pipeline (or driver) and keeps no plan node
        // id: the bare id is not always -1 and a pseudo id names no plan node.
        let legacy_operator = (structure.legacy_negative_id_operator_header
            && matches!(section, RuntimeTextSection::Execution)
            && current_pipeline_container.is_some()
            && operator_parent_indent.is_some_and(|parent| indent == parent + 2))
        .then(|| runtime_legacy_negative_id_operator_header(trimmed))
        .flatten()
        .map(|operator_type| (operator_type, None, None));
        if let Some((operator_type, plan_node_id, native_operator_id)) =
            runtime_operator_header(trimmed).or(legacy_operator)
        {
            let source_ref = text_ref(line_no, start, end)?;
            // A (fragment, pipeline, type, plan_node_id) tuple is not a unique
            // native identity: every PipelineDriver can render that tuple.
            let operator_occurrence =
                next_runtime_occurrence(&mut next_operator_occurrence, limits, evidence)?;
            let group_id = format!(
                "operator:{}:fragment-occurrence-{}:instance-occurrence-{}:{}:pipeline-occurrence-{}:driver-{}:driver-occurrence-{}:{}:operator-occurrence-{}",
                fragment.as_deref().unwrap_or("unknown"),
                fragment_occurrence.map_or_else(|| "none".into(), |value| value.to_string()),
                instance_occurrence.map_or_else(|| "none".into(), |value| value.to_string()),
                pipeline.as_deref().unwrap_or("unknown"),
                pipeline_occurrence.map_or_else(|| "none".into(), |value| value.to_string()),
                pipeline_driver
                    .as_ref()
                    .map_or("none", |(driver, _, _)| driver),
                pipeline_driver
                    .as_ref()
                    .map_or_else(|| "none".into(), |(_, value, _)| value.to_string()),
                plan_node_id.as_deref().unwrap_or("none"),
                operator_occurrence
            );
            let container_ref = current_pipeline_container
                .and_then(|index| operators.get(index))
                .filter(|operator| operator.canonical_type == "PIPELINE")
                .map(|operator| operator.operator_group_id.clone());
            let mut membership_reasons = Vec::new();
            if fragment.is_none() {
                membership_reasons.push("runtime_fragment_ref_missing".into());
            }
            if pipeline.is_none() {
                membership_reasons.push("runtime_pipeline_ref_missing".into());
            }
            if current_pipeline_container.is_none() {
                membership_reasons.push("runtime_pipeline_container_missing".into());
            } else if container_ref.is_none() {
                membership_reasons.push("runtime_pipeline_container_identity_missing".into());
            }
            let membership_source_refs = vec![source_ref.ref_id.clone()];
            let canonical_type = operator_type;
            operators.push(RuntimeOperator {
                operator_group_id: group_id.clone(),
                plan_node_id: plan_node_id.clone(),
                fragment_ref: fragment.clone(),
                pipeline_ref: pipeline.clone(),
                canonical_type: canonical_type.clone(),
                native_operator_id,
                source_role: None,
                runtime_source_role: RuntimeSourceRole::Unknown,
                role_marker_malformed: false,
                source_ref,
                metrics: vec![],
                is_final_sink: false,
                partition_type_facts: vec![],
                distribution_mode_facts: vec![],
                table_facts: vec![],
                predicate_facts: vec![],
                join_predicate_facts: vec![],
            });
            physical_operator_memberships.push(PhysicalOperatorMembership {
                physical_operator_ref: group_id,
                source_occurrence_ordinal: operator_occurrence,
                fragment_ref: fragment.clone(),
                container_ref,
                driver_occurrence_ref: pipeline_driver
                    .as_ref()
                    .map(|(_, _, reference)| reference.clone()),
                driver_occurrence_ordinal: pipeline_driver.as_ref().map(|(_, ordinal, _)| *ordinal),
                emission_ordinal: driver_emission_ordinal,
                plan_node_id,
                canonical_type,
                source_role: RuntimeSourceRole::Unknown,
                source_refs: membership_source_refs,
                reasons: membership_reasons,
            });
            if current_pipeline_container.is_some() {
                formatter.physical_operator = true;
            }
            if let Some(ordinal) = driver_emission_ordinal.as_mut() {
                *ordinal = ordinal.checked_add(1).ok_or_else(|| {
                    limit_error(
                        "too_many_runtime_occurrences",
                        usize::MAX,
                        limits.max_normalized_metrics,
                        evidence.to_vec(),
                    )
                })?;
            }
            source_refs.push(
                operators
                    .last()
                    .map(|operator| operator.source_ref.clone())
                    .ok_or_else(|| {
                        error(
                            "internal_error",
                            "normalize",
                            "runtime_operator_missing",
                            "report_bug",
                            vec![],
                            evidence.to_vec(),
                        )
                    })?,
            );
            current_operator = Some(operators.len() - 1);
            operator_metric_section = None;
            start += raw.len();
            continue;
        }
        if matches!(section, RuntimeTextSection::Execution)
            && runtime_operator_header_candidate(trimmed)
        {
            let source_ref = text_ref(line_no, start, end)?;
            diagnostics.push(diagnostic(
                "runtime_operator_header_invalid",
                "parse",
                "warning",
                "partial",
                "runtime_operator_header_invalid",
                vec![source_ref.ref_id.clone()],
            ));
            source_refs.push(source_ref);
            // Quarantine this subtree, not just its header: otherwise its counters
            // attach to the previous operator (or to the query after clearing it).
            invalid_operator_indent = Some(indent);
            current_operator = None;
            operator_metric_section = None;
            // No proven adjacency across an unparsed operator in this driver.
            driver_emission_ordinal = None;
            start += raw.len();
            continue;
        }

        // `RuntimeProfile::prettyPrint` renders the Planner's direct named
        // counter as `- -- Total[1] <display-duration>`.  Do not use any
        // other `Total` spelling or branch: the Overview planning metric is a
        // direct alias, never query-total minus execution time.
        if matches!(section, RuntimeTextSection::Planner) {
            if let Some(value) = runtime_planner_total_text_value(trimmed) {
                let source_ref = text_ref(line_no, start, end)?;
                if let Some(metric) = runtime_metric_from_text(
                    "QueryPlanningTime",
                    value,
                    "query",
                    source_ref.clone(),
                    &mut metric_count,
                    limits,
                    format,
                    evidence,
                    &mut diagnostics,
                )? {
                    planning_time_candidates.push(metric);
                    source_refs.push(source_ref);
                }
            }
            start += raw.len();
            continue;
        }

        if matches!(section, RuntimeTextSection::Execution)
            && matches!(trimmed, "CommonMetrics:" | "UniqueMetrics:")
        {
            operator_metric_section = if trimmed == "CommonMetrics:" {
                formatter.common_metrics |= current_operator
                    .and_then(|index| operators.get(index))
                    .is_some_and(|operator| {
                        !matches!(operator.canonical_type.as_str(), "FRAGMENT" | "PIPELINE")
                    });
                Some(RuntimeOperatorMetricSection::Common)
            } else {
                formatter.unique_metrics |= current_operator
                    .and_then(|index| operators.get(index))
                    .is_some_and(|operator| {
                        !matches!(operator.canonical_type.as_str(), "FRAGMENT" | "PIPELINE")
                    });
                Some(RuntimeOperatorMetricSection::Unique)
            };
            start += raw.len();
            continue;
        }
        let Some((key, value)) = runtime_text_entry(trimmed) else {
            start += raw.len();
            continue;
        };
        let source_ref = text_ref(line_no, start, end)?;
        let incomplete_profile_code = if matches!(section, RuntimeTextSection::Execution) {
            match key {
                "NotIdentical" if trimmed == "- NotIdentical" => {
                    Some("runtime_profile_not_identical")
                }
                "NotIdentical" => Some("runtime_profile_incomplete_marker_malformed"),
                "MissingInstanceIds" if !value.trim().is_empty() => {
                    Some("runtime_profile_missing_instances")
                }
                "MissingInstanceIds" => Some("runtime_profile_incomplete_marker_malformed"),
                "IsRunning"
                    if operator_metric_section == Some(RuntimeOperatorMetricSection::Common)
                        && value.is_empty() =>
                {
                    Some("runtime_operator_running")
                }
                "IsRunning" => Some("runtime_profile_incomplete_marker_malformed"),
                _ => None,
            }
        } else {
            None
        };
        if let Some(code) = incomplete_profile_code {
            diagnostics.push(diagnostic(
                code,
                "parse",
                "warning",
                "partial",
                code,
                vec![source_ref.ref_id.clone()],
            ));
            source_refs.push(source_ref);
            start += raw.len();
            continue;
        }
        match section {
            RuntimeTextSection::Summary => match key {
                "Query ID" => {
                    if let Some(normalized) = runtime_query_id(value) {
                        runtime_safe_line(
                            &mut safe_lines,
                            line_no,
                            "Query ID",
                            &normalized,
                            &source_ref,
                        );
                        query_id = Some(normalized);
                    } else {
                        diagnostics.push(diagnostic(
                            "runtime_query_id_invalid",
                            "parse",
                            "warning",
                            "partial",
                            "runtime_query_id_invalid",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                "Query Type" => {
                    if let Some(normalized) = runtime_query_type(value) {
                        runtime_safe_line(
                            &mut safe_lines,
                            line_no,
                            "Query Type",
                            normalized,
                            &source_ref,
                        );
                        query_type = Some(normalized.into());
                    } else {
                        diagnostics.push(diagnostic(
                            "runtime_query_type_invalid",
                            "parse",
                            "warning",
                            "partial",
                            "runtime_query_type_invalid",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                "Query State" => {
                    profile_facts.set_query_state(value, source_ref.clone(), &mut diagnostics);
                    if let Some(normalized) =
                        runtime_accepted_query_state(&profile_facts, &source_ref)
                    {
                        runtime_safe_line(
                            &mut safe_lines,
                            line_no,
                            "Query State",
                            normalized,
                            &source_ref,
                        );
                    }
                    source_refs.push(source_ref);
                }
                "StarRocks Version" => {
                    if let Some(normalized) = runtime_version(value) {
                        if version_conflicted {
                            let mut refs = version_origin_ref.iter().cloned().collect::<Vec<_>>();
                            refs.push(source_ref.ref_id.clone());
                            diagnostics.push(diagnostic(
                                "version_conflict",
                                "parse",
                                "warning",
                                "partial",
                                "version_conflict",
                                refs,
                            ));
                        } else if let Some(existing) = version.as_ref() {
                            let refs = vec![
                                existing.source_ref.ref_id.clone(),
                                source_ref.ref_id.clone(),
                            ];
                            if existing.value == normalized {
                                diagnostics.push(diagnostic(
                                    "duplicate_version",
                                    "parse",
                                    "warning",
                                    "partial",
                                    "duplicate_version",
                                    refs,
                                ));
                            } else {
                                let conflicting_origin = existing.source_ref.ref_id.clone();
                                safe_lines.retain(|line| line.source_ref != conflicting_origin);
                                version = None;
                                version_conflicted = true;
                                diagnostics.push(diagnostic(
                                    "version_conflict",
                                    "parse",
                                    "warning",
                                    "partial",
                                    "version_conflict",
                                    refs,
                                ));
                            }
                        } else {
                            runtime_safe_line(
                                &mut safe_lines,
                                line_no,
                                "StarRocks Version",
                                &normalized,
                                &source_ref,
                            );
                            version = Some(VersionFact {
                                value: normalized,
                                source_ref: source_ref.clone(),
                            });
                            version_origin_ref = Some(source_ref.ref_id.clone());
                        }
                    } else {
                        diagnostics.push(diagnostic(
                            "version_unrecognized",
                            "parse",
                            "warning",
                            "partial",
                            "version_unrecognized",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                "NonDefaultSessionVariables" => {
                    record_unique_profile_evidence(
                        &mut stored_profile_level,
                        &mut stored_profile_level_conflicted,
                        sr3514_stored_profile_level(value),
                        source_ref.clone(),
                        &mut diagnostics,
                    );
                    source_refs.push(source_ref);
                }
                "IsProfileAsync" => {
                    record_unique_profile_evidence(
                        &mut final_profile_marker,
                        &mut final_profile_marker_conflicted,
                        match value {
                            "true" => Ok(true),
                            "false" => Ok(false),
                            _ => Err(()),
                        },
                        source_ref.clone(),
                        &mut diagnostics,
                    );
                    source_refs.push(source_ref);
                }
                // FE `Summary.Total`: end-to-end query time as reported. It is
                // a separate query metric, never derived from phase times.
                "Total" => {
                    if let Some(metric) = runtime_metric_from_text(
                        "QueryTotalTime",
                        value,
                        "query",
                        source_ref.clone(),
                        &mut metric_count,
                        limits,
                        format,
                        evidence,
                        &mut diagnostics,
                    )? {
                        source_refs.push(source_ref);
                        query_metrics.push(metric);
                    }
                }
                _ => {}
            },
            RuntimeTextSection::Execution => {
                if fragment.is_some() && key == "BackendAddresses" && !value.is_empty() {
                    formatter.backend_addresses = true;
                }
                if fragment.is_some() && key == "InstanceIds" && !value.is_empty() {
                    formatter.instance_ids = true;
                }
                if key == "Topology" {
                    // The FE prints one Topology per Profile. A second line (even an
                    // identical one) means the input was spliced: no line is trusted.
                    if let Some(first) = topology_first_ref.as_ref() {
                        let refs = topology_conflict.get_or_insert_with(|| vec![first.clone()]);
                        refs.push(source_ref.ref_id.clone());
                        topology = None;
                        diagnostics.push(diagnostic(
                            "runtime_topology_conflict",
                            "graph",
                            "warning",
                            "partial",
                            "runtime_topology_conflict",
                            refs.clone(),
                        ));
                        source_refs.push(source_ref);
                        start += raw.len();
                        continue;
                    }
                    topology_first_ref = Some(source_ref.ref_id.clone());
                    match serde_json::from_str::<StrictJsonValue>(value) {
                        Ok(value) => {
                            topology = Some(RuntimeTopology {
                                value: value.0,
                                source_ref: source_ref.clone(),
                            });
                            source_refs.push(source_ref);
                        }
                        Err(_) => diagnostics.push(diagnostic(
                            "runtime_topology_invalid",
                            "graph",
                            "warning",
                            "partial",
                            "runtime_topology_invalid",
                            vec![source_ref.ref_id],
                        )),
                    }
                } else if key == "IsSubordinate" {
                    let valid_marker = trimmed == "- IsSubordinate"
                        && operator_metric_section == Some(RuntimeOperatorMetricSection::Common)
                        && current_operator
                            .and_then(|index| operators.get(index))
                            .is_some_and(|operator| operator.source_role.is_none());
                    if valid_marker {
                        if let Some(index) = current_operator {
                            operators[index].source_role = Some(SourceRole::Subordinate);
                            operators[index].runtime_source_role = RuntimeSourceRole::Subordinate;
                            // The current operator's membership was pushed with its header,
                            // so a reverse search stops at once.
                            if let Some(membership) = physical_operator_memberships
                                .iter_mut()
                                .rev()
                                .find(|membership| {
                                    membership.physical_operator_ref
                                        == operators[index].operator_group_id
                                })
                            {
                                membership.source_role = RuntimeSourceRole::Subordinate;
                                membership.source_refs.push(source_ref.ref_id.clone());
                                membership.source_refs.sort();
                                membership.source_refs.dedup();
                            }
                        }
                    } else {
                        if let Some(operator) =
                            current_operator.and_then(|index| operators.get_mut(index))
                        {
                            operator.role_marker_malformed = true;
                        }
                        diagnostics.push(diagnostic(
                            "runtime_profile_incomplete_marker_malformed",
                            "parse",
                            "warning",
                            "partial",
                            "runtime_profile_incomplete_marker_malformed",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                } else if key == "IsFinalSink"
                    && value.is_empty()
                    && operator_metric_section == Some(RuntimeOperatorMetricSection::Common)
                {
                    if let Some(index) = current_operator {
                        operators[index].is_final_sink = true;
                        source_refs.push(source_ref);
                    }
                } else if key == "Table"
                    && operator_metric_section == Some(RuntimeOperatorMetricSection::Unique)
                {
                    if let Some(index) = current_operator
                        && operators[index].canonical_type == "OLAP_SCAN"
                    {
                        let value = runtime_table_value(value);
                        if let Some(table) = &value {
                            runtime_safe_line(
                                &mut safe_lines,
                                line_no,
                                "Table",
                                table,
                                &source_ref,
                            );
                        }
                        operators[index].table_facts.push(RuntimeInfoFact {
                            value,
                            source_ref: source_ref.clone(),
                        });
                        source_refs.push(source_ref);
                    }
                } else if matches!(key, "Predicates" | "JoinPredicates")
                    && operator_metric_section == Some(RuntimeOperatorMetricSection::Unique)
                {
                    if let Some(index) = current_operator {
                        let operator = &mut operators[index];
                        if runtime_predicate_applies(key, &operator.canonical_type) {
                            let value = runtime_expression_value(value);
                            let fact = RuntimeInfoFact {
                                value,
                                source_ref: source_ref.clone(),
                            };
                            if key == "Predicates" {
                                operator.predicate_facts.push(fact);
                            } else {
                                operator.join_predicate_facts.push(fact);
                            }
                            source_refs.push(source_ref);
                        }
                    }
                } else if key == "DistributionMode"
                    && operator_metric_section == Some(RuntimeOperatorMetricSection::Unique)
                {
                    if let Some(index) = current_operator {
                        operators[index]
                            .distribution_mode_facts
                            .push(RuntimeInfoFact {
                                value: runtime_identifier(value),
                                source_ref: source_ref.clone(),
                            });
                        source_refs.push(source_ref);
                    }
                } else if key == "PartType"
                    && operator_metric_section == Some(RuntimeOperatorMetricSection::Unique)
                {
                    if let Some(index) = current_operator {
                        let value = runtime_identifier(value);
                        if value.is_none() {
                            diagnostics.push(diagnostic(
                                "runtime_partition_type_invalid",
                                "parse",
                                "warning",
                                "partial",
                                "runtime_partition_type_invalid",
                                vec![source_ref.ref_id.clone()],
                            ));
                        }
                        operators[index].partition_type_facts.push(RuntimeInfoFact {
                            value,
                            source_ref: source_ref.clone(),
                        });
                        source_refs.push(source_ref);
                    }
                } else if let Some(metric) = runtime_metric_from_text(
                    key,
                    value,
                    runtime_scope_for(current_operator, &operators),
                    source_ref.clone(),
                    &mut metric_count,
                    limits,
                    format,
                    evidence,
                    &mut diagnostics,
                )? {
                    source_refs.push(source_ref);
                    if let Some(index) = current_operator {
                        operators[index].metrics.push(metric);
                    } else {
                        query_metrics.push(metric);
                    }
                }
            }
            RuntimeTextSection::Planner | RuntimeTextSection::Other => {}
        }
        start += raw.len();
    }

    let formatter_shape = formatter.shape(version.is_some(), profile_facts.query_state_seen);
    // The formatter shape is still checked line by line; the release only
    // has to be one the audit covered.
    let exact_sr3514 = version.as_ref().is_some_and(|version| {
        matches!(
            starrocks_version_pack_3_5_14::deltas::resolve_release(&version.value),
            starrocks_version_pack_3_5_14::deltas::ReleaseResolution::Audited(_)
        )
    });
    if exact_sr3514 && formatter_shape == RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514 {
        if let Some((level, source)) = stored_profile_level {
            let level_value = match level {
                ProfileLevel::Level1 => "1",
                ProfileLevel::Level2 => "2",
                ProfileLevel::Unknown => "",
            };
            let merge_value = match level {
                ProfileLevel::Level1 => "merged",
                ProfileLevel::Level2 => "unmerged",
                ProfileLevel::Unknown => "",
            };
            profile_facts.set_profile_level(level_value, source.clone(), &mut diagnostics);
            profile_facts.set_merge_state(merge_value, source, &mut diagnostics);
        }
        if profile_facts.query_state == QueryState::Finished
            && let Some((_, source)) = final_profile_marker
        {
            profile_facts.set_snapshot_kind("final", source, &mut diagnostics);
        }
    }

    cancel.check()?;
    finalize_native_operator_roles(&mut operators, &mut physical_operator_memberships);
    cancel.check()?;
    let plan_node_types = runtime_topology_node_types(topology.as_ref());
    let graph_evidence = match topology_conflict {
        Some(source_refs) => Some(GraphEvidence::Unavailable {
            reason: "runtime_topology_conflict".into(),
            source_refs,
        }),
        None => runtime_graph_evidence(topology.as_ref(), &operators, &mut diagnostics),
    };
    cancel.check()?;
    let data = RuntimeProfileData {
        formatter_shape,
        query_summary: QuerySummary {
            query_id,
            query_type,
            limitations: vec![
                "runtime_profile_summary_allowlist_only".into(),
                "sql_user_address_and_unknown_text_excluded".into(),
            ],
        },
        query_metrics,
        planning_time_candidates,
        operators,
        plan_node_types,
        pipeline_containers,
        runtime_driver_occurrences,
        physical_operator_memberships,
    };
    Ok(Extracted {
        line_count: Some(u32::try_from(line_count).map_err(|_| {
            limit_error(
                "too_many_lines",
                line_count,
                u32::MAX as usize,
                evidence.to_vec(),
            )
        })?),
        version,
        metric: None,
        profile_facts,
        source_refs,
        diagnostics,
        graph_evidence,
        source_view: SourceView {
            source_kind: SourceKind::TextProfile,
            plain_text_lines: safe_lines,
            json_tokens: vec![],
            truncation_state: "complete".into(),
            limitations: vec![
                "runtime_profile_safe_allowlist_only".into(),
                "raw_sql_user_address_and_unknown_text_excluded".into(),
            ],
            line_refs: vec![],
        },
        runtime: Some(data),
    })
}

/// StarRocks 3.5.14 records only non-default session values. The official
/// default is level 1; level 2 appears as an explicit `{defaultValue:1,
/// actualValue:2}` entry. This helper is applied only after the complete
/// 3.5.14 pretty-print shape and reported version have both been verified.
fn sr3514_stored_profile_level(value: &str) -> Result<ProfileLevel, ()> {
    let parsed = serde_json::from_str::<StrictJsonValue>(value)
        .map(|value| value.0)
        .map_err(|_| ())?;
    let variables = parsed.as_object().ok_or(())?;
    let Some(level) = variables.get("pipeline_profile_level") else {
        return Ok(ProfileLevel::Level1);
    };
    let level = level.as_object().ok_or(())?;
    match (
        level.get("defaultValue").and_then(Value::as_i64),
        level.get("actualValue").and_then(Value::as_i64),
    ) {
        (Some(1), Some(2)) => Ok(ProfileLevel::Level2),
        _ => Err(()),
    }
}

fn record_unique_profile_evidence<T>(
    slot: &mut Option<(T, SourceRef)>,
    conflicted: &mut bool,
    parsed: Result<T, ()>,
    source: SourceRef,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (message_id, refs) = if *conflicted {
        ("profile_context_conflict", vec![source.ref_id])
    } else if parsed.is_err() {
        *slot = None;
        *conflicted = true;
        ("profile_context_invalid", vec![source.ref_id])
    } else if let Some((_, existing)) = slot.as_ref() {
        let refs = vec![existing.ref_id.clone(), source.ref_id];
        *slot = None;
        *conflicted = true;
        ("profile_context_conflict", refs)
    } else {
        *slot = parsed.ok().map(|value| (value, source));
        return;
    };
    diagnostics.push(diagnostic(
        message_id, "parse", "warning", "partial", message_id, refs,
    ));
}

#[derive(Clone, Copy)]
enum RuntimeTextSection {
    Other,
    Summary,
    Planner,
    Execution,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RuntimeOperatorMetricSection {
    Common,
    Unique,
}

#[derive(Default)]
struct RuntimePrettyPrintEvidence {
    query_root: bool,
    summary: bool,
    execution: bool,
    fragment: bool,
    pipeline: bool,
    instance: bool,
    pipeline_driver: bool,
    backend_addresses: bool,
    instance_ids: bool,
    physical_operator: bool,
    common_metrics: bool,
    unique_metrics: bool,
}

impl RuntimePrettyPrintEvidence {
    fn shape(
        &self,
        has_reported_version: bool,
        has_query_state: bool,
    ) -> RuntimeInventoryFormatterShape {
        let hierarchy = self.query_root
            && self.summary
            && self.execution
            && self.fragment
            && self.pipeline
            && self.physical_operator
            && self.common_metrics
            && self.unique_metrics;
        let occurrence_shape = (self.backend_addresses && self.instance_ids)
            || (self.instance && self.pipeline_driver);
        if hierarchy && occurrence_shape && has_reported_version && has_query_state {
            RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514
        } else {
            RuntimeInventoryFormatterShape::Unknown
        }
    }
}

fn runtime_planner_total_text_value(line: &str) -> Option<&str> {
    line.strip_prefix("- -- Total[1] ")
        .filter(|value| !value.trim().is_empty())
}

fn runtime_text_entry(line: &str) -> Option<(&str, &str)> {
    let entry = line.strip_prefix("- ")?;
    match entry.split_once(':') {
        Some((key, value)) => Some((key.trim(), value.trim())),
        None => Some((entry.trim(), "")),
    }
}

fn next_runtime_occurrence(
    next: &mut usize,
    limits: &AdapterLimits,
    evidence: &[String],
) -> Result<u32, AdapterError> {
    let occurrence = u32::try_from(*next).map_err(|_| {
        limit_error(
            "too_many_runtime_occurrences",
            *next,
            limits.max_text_lines,
            evidence.to_vec(),
        )
    })?;
    *next = next.checked_add(1).ok_or_else(|| {
        limit_error(
            "too_many_runtime_occurrences",
            usize::MAX,
            limits.max_text_lines,
            evidence.to_vec(),
        )
    })?;
    Ok(occurrence)
}

fn runtime_instance_header(line: &str) -> bool {
    line.starts_with("Instance ") && line.ends_with(':')
}

fn runtime_pipeline_driver_header(line: &str) -> Option<String> {
    let value = line
        .strip_prefix("PipelineDriver (id=")?
        .strip_suffix("):")?
        .trim();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| value.into())
}

fn runtime_operator_header(line: &str) -> Option<(String, Option<String>, Option<i32>)> {
    runtime_json_operator_header(line.strip_suffix(':')?)
}

fn runtime_operator_header_candidate(name: &str) -> bool {
    if name.starts_with('-') {
        return false;
    }
    let name = name.strip_suffix(':').unwrap_or(name);
    name.contains("(plan_node_id")
        || runtime_identifier(name.split_once(" (").map_or(name, |(label, _)| label)).is_some()
}

/// Before 3.1.3 (BE `Operator::Operator`): `RESULT_SINK:` for a plan node id in
/// (-100, -1], `LOCAL_EXCHANGE_SOURCE (pseudo_plan_node_id=-101):` for an id
/// at or below -100. Neither names a plan node, so only the type is returned.
fn runtime_legacy_negative_id_operator_header(line: &str) -> Option<String> {
    let name = line.strip_suffix(':')?;
    match name.split_once(" (pseudo_plan_node_id=") {
        None => runtime_identifier(name),
        Some((label, suffix)) => {
            let id = suffix.strip_suffix(')')?.strip_prefix('-')?;
            let pseudo = !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_digit())
                && id.parse::<u32>().is_ok_and(|value| value >= 100);
            pseudo.then(|| runtime_identifier(label)).flatten()
        }
    }
}

fn runtime_operator_recovery_boundary(line: &str, structure: TextStructure) -> bool {
    // Dedentation alone is not evidence of a new owner. In particular, a
    // malformed sibling counter must not become a query metric after quarantine.
    matches!(line, "Summary:" | "Execution:" | "Planner:")
        || (structure.legacy_negative_id_operator_header
            && runtime_legacy_negative_id_operator_header(line).is_some())
        || runtime_operator_header(line).is_some()
        || runtime_pipeline_driver_header(line).is_some()
        || runtime_instance_header(line)
        || line
            .strip_prefix("Fragment ")
            .and_then(|value| value.strip_suffix(':'))
            .and_then(|value| runtime_header_id(value, false))
            .is_some()
        || line
            .strip_suffix(':')
            .and_then(runtime_pipeline_header)
            .and_then(|value| runtime_header_id(value, false))
            .is_some()
}

fn runtime_identifier(value: &str) -> Option<String> {
    (!value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'))
    .then(|| value.to_owned())
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value.trim())
}

fn runtime_query_id(value: &str) -> Option<String> {
    (value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                .then_some(byte == b'-')
                .unwrap_or_else(|| byte.is_ascii_hexdigit())
        }))
    .then(|| value.to_ascii_lowercase())
}

fn runtime_query_type(value: &str) -> Option<&'static str> {
    (value == "Query").then_some("Query")
}

fn runtime_accepted_query_state(
    facts: &ProfileFacts,
    source_ref: &SourceRef,
) -> Option<&'static str> {
    if !facts
        .query_state_source
        .as_ref()
        .is_some_and(|accepted| accepted.ref_id == source_ref.ref_id)
    {
        return None;
    }
    match &facts.query_state {
        QueryState::Finished => Some("Finished"),
        QueryState::Running => Some("Running"),
        QueryState::Error => Some("Error"),
        QueryState::Unknown => None,
    }
}

fn runtime_version(value: &str) -> Option<String> {
    let candidate = value.trim().split('-').next()?;
    is_version(candidate).then(|| candidate.to_owned())
}

/// Counters are read with the reported release's text format, so the version
/// is looked up before any counter. The full parse still validates the
/// version (conflicts, duplicates) and an unknown one isolates every metric.
fn text_counter_format(text: &str) -> CounterTextFormat {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("- StarRocks Version:"))
        .and_then(runtime_version)
        .map_or(CounterTextFormat::Padded, |release| {
            starrocks_version_pack_3_5_14::deltas::counter_text_format(&release)
        })
}

/// The reported release's text layout, looked up like the counter format. Only
/// an audited release whose delta says so accepts a legacy spelling; every
/// other input reads with the baseline layout.
fn text_structure(text: &str) -> TextStructure {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("- StarRocks Version:"))
        .and_then(runtime_version)
        .map_or(TextStructure::BASELINE, |release| {
            starrocks_version_pack_3_5_14::deltas::text_structure(&release)
        })
}

fn json_counter_format(query: &serde_json::Map<String, Value>) -> CounterTextFormat {
    query
        .get("Summary")
        .and_then(Value::as_object)
        .and_then(|summary| summary.get("StarRocks Version"))
        .and_then(Value::as_str)
        .and_then(runtime_version)
        .map_or(CounterTextFormat::Padded, |release| {
            starrocks_version_pack_3_5_14::deltas::counter_text_format(&release)
        })
}

fn runtime_safe_line(
    lines: &mut Vec<SourceLine>,
    line_no: u32,
    key: &str,
    value: &str,
    source_ref: &SourceRef,
) {
    if !value.contains('\n') && !value.contains('\r') && value.len() <= 256 {
        lines.push(SourceLine {
            line_no,
            text: format!("{key}: {value}"),
            source_ref: source_ref.ref_id.clone(),
        });
    }
}

struct RuntimeContainerInput<'a> {
    operators: &'a mut Vec<RuntimeOperator>,
    pipeline_containers: &'a mut Vec<PipelineContainer>,
    pipeline_container_occurrences: &'a mut BTreeMap<(String, String), usize>,
    kind: &'a str,
    fragment_ref: Option<&'a str>,
    pipeline_ref: Option<&'a str>,
    canonical_type: &'a str,
    source_ref: SourceRef,
    limits: &'a AdapterLimits,
    evidence: &'a [String],
}

fn runtime_container(input: RuntimeContainerInput<'_>) -> Result<usize, AdapterError> {
    let RuntimeContainerInput {
        operators,
        pipeline_containers,
        pipeline_container_occurrences,
        kind,
        fragment_ref,
        pipeline_ref,
        canonical_type,
        source_ref,
        limits,
        evidence,
    } = input;
    let ordinal = operators.len();
    let operator_group_id = format!(
        "{kind}:{}:{}:{ordinal}",
        fragment_ref.unwrap_or("unknown"),
        pipeline_ref.unwrap_or("none")
    );
    // Callers emit a source-backed diagnostic before reaching this defensive
    // guard. Never materialize an empty typed identity.
    if kind == "pipeline"
        && let (Some(fragment_ref), Some(pipeline_ref)) = (fragment_ref, pipeline_ref)
        && !fragment_ref.trim().is_empty()
        && !pipeline_ref.trim().is_empty()
    {
        let next = pipeline_container_occurrences
            .entry((fragment_ref.into(), pipeline_ref.into()))
            .or_insert(0);
        let occurrence_ordinal = next_runtime_occurrence(next, limits, evidence)?;
        pipeline_containers.push(PipelineContainer {
            // Reuse the owning Pipeline scope identity as the opaque typed join
            // key. Consumers must never parse this string for structural facts.
            container_ref: operator_group_id.clone(),
            fragment_ref: fragment_ref.into(),
            pipeline_ref: pipeline_ref.into(),
            occurrence_ordinal,
            source_refs: vec![source_ref.ref_id.clone()],
        });
    }
    operators.push(RuntimeOperator {
        operator_group_id,
        plan_node_id: None,
        fragment_ref: fragment_ref.map(str::to_owned),
        pipeline_ref: pipeline_ref.map(str::to_owned),
        canonical_type: canonical_type.into(),
        native_operator_id: None,
        source_role: None,
        runtime_source_role: RuntimeSourceRole::Unknown,
        role_marker_malformed: false,
        source_ref,
        metrics: vec![],
        is_final_sink: false,
        partition_type_facts: vec![],
        distribution_mode_facts: vec![],
        table_facts: vec![],
        predicate_facts: vec![],
        join_predicate_facts: vec![],
    });
    Ok(operators.len() - 1)
}

fn runtime_scope_for(current_operator: Option<usize>, operators: &[RuntimeOperator]) -> &str {
    current_operator
        .and_then(|index| operators.get(index))
        .map(|operator| match operator.canonical_type.as_str() {
            "FRAGMENT" => "fragment",
            "PIPELINE" => "pipeline",
            _ => "operator",
        })
        .unwrap_or("query")
}

#[allow(clippy::too_many_arguments)]
fn runtime_metric_from_text(
    key: &str,
    value: &str,
    scope: &str,
    source_ref: SourceRef,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<RuntimeMetricFact>, AdapterError> {
    // RuntimeProfile.mergeIsomorphicProfiles prints __MIN_OF_/__MAX_OF_ for
    // every counter whose strategy keeps min/max; the registry lookup below
    // admits only registered base counters.
    let (metric_id, value_role) = if let Some(base) = key.strip_prefix("__MAX_OF_") {
        (base, "reported_max")
    } else if let Some(base) = key.strip_prefix("__MIN_OF_") {
        (base, "reported_min")
    } else {
        (key, "reported_base")
    };
    let definition =
        match crate::metrics::registered_metric_for(SourceKind::TextProfile, format, metric_id) {
            Some(definition)
                if definition
                    .valid_scopes
                    .iter()
                    .any(|candidate| candidate == scope) =>
            {
                definition
            }
            _ => return Ok(None),
        };
    *metric_count = metric_count.saturating_add(1);
    check_metric_budget(*metric_count, limits, evidence)?;
    let outcome = parse_metric(
        definition,
        MetricInput::Text(value.to_owned()),
        ParseMode::Strict,
    );
    if outcome.parse_state != ParseState::Exact {
        diagnostics.push(metric_diagnostic(key, &outcome, &source_ref));
    }
    Ok(Some(RuntimeMetricFact {
        metric_id: metric_id.into(),
        scope: scope.into(),
        source_name: key.into(),
        value_role: value_role.into(),
        display_text: if outcome.parse_state == ParseState::Exact {
            value.into()
        } else {
            "—".into()
        },
        outcome,
        source_ref,
        source_ordinal: u32::try_from(*metric_count - 1).map_err(|_| {
            limit_error(
                "too_many_metrics",
                *metric_count,
                limits.max_normalized_metrics,
                evidence.to_vec(),
            )
        })?,
    }))
}

// Admit only explicit scoped table names from this same Profile; never choose between conflicts.
fn runtime_table_facts(
    operators: &[&RuntimeOperator],
) -> Option<Vec<doctor_contracts::GraphSqlFact>> {
    let mut result = Vec::new();
    for (kind, selector) in [
        (doctor_contracts::GraphSqlFactKind::Table, 0),
        (doctor_contracts::GraphSqlFactKind::Predicate, 1),
        (doctor_contracts::GraphSqlFactKind::JoinCondition, 2),
    ] {
        let facts = operators
            .iter()
            .flat_map(|operator| match selector {
                0 => &operator.table_facts,
                1 => &operator.predicate_facts,
                _ => &operator.join_predicate_facts,
            })
            .collect::<Vec<_>>();
        let Some(Some(value)) = facts.first().map(|fact| fact.value.as_ref()) else {
            continue;
        };
        if facts.iter().any(|fact| fact.value.as_ref() != Some(value)) {
            continue;
        }
        result.push(doctor_contracts::GraphSqlFact {
            kind,
            value: value.clone(),
            source_refs: facts
                .iter()
                .map(|fact| fact.source_ref.ref_id.clone())
                .collect(),
        });
    }
    (!result.is_empty()).then_some(result)
}
fn runtime_graph_evidence(
    topology: Option<&RuntimeTopology>,
    operators: &[RuntimeOperator],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<GraphEvidence> {
    let topology = topology?;
    let source_ref = &topology.source_ref;
    // One pass over the operators; every per-node lookup below is then a map hit.
    let operators_by_node = operators_by_plan_node(operators);
    let parsed = (|| {
        let object = topology.value.as_object().ok_or(())?;
        let root_node_id = runtime_topology_id(object.get("rootId").ok_or(())?)?;
        let nodes = object.get("nodes").and_then(Value::as_array).ok_or(())?;
        let mut parsed_nodes = Vec::with_capacity(nodes.len());
        let mut shape = Vec::with_capacity(nodes.len());
        for (index, value) in nodes.iter().enumerate() {
            let node = value.as_object().ok_or(())?;
            let node_id = runtime_topology_id(node.get("id").ok_or(())?)?;
            let name = node
                .get("name")
                .and_then(Value::as_str)
                .and_then(runtime_identifier);
            let children = runtime_topology_ids(node.get("children"))?;
            // ProfilingExecPlan.Topology.buildFrom puts `sinkIds` (possibly empty) on
            // exactly the fragment roots, so its presence is evidence too.
            let sink_ids = node
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get("sinkIds"))
                .map(|sink_ids| runtime_topology_ids(Some(sink_ids)))
                .transpose()?;
            let node_operators = operators_by_node
                .get(node_id.as_str())
                .map_or(&[][..], Vec::as_slice);
            let node_refs = vec![source_ref.ref_id.clone()];
            let native_operator_refs = node_operators
                .iter()
                // Native roles are finalized into runtime_source_role (finalize_native_operator_roles);
                // the parse-time source_role stays None for native operators.
                .filter(|operator| {
                    matches!(operator.runtime_source_role, RuntimeSourceRole::Native)
                })
                .enumerate()
                .map(|(ordinal, operator)| {
                    Ok(OrderedOperatorRef {
                        operator_ref: operator.operator_group_id.clone(),
                        source_ordinal: u32::try_from(ordinal).map_err(|_| ())?,
                        source_role: SourceRole::Native,
                        source_refs: node_refs.clone(),
                    })
                })
                .collect::<Result<Vec<_>, ()>>()?;
            let subordinate_operator_refs = node_operators
                .iter()
                .filter(|operator| {
                    matches!(operator.runtime_source_role, RuntimeSourceRole::Subordinate)
                })
                .enumerate()
                .map(|(ordinal, operator)| {
                    Ok(OrderedOperatorRef {
                        operator_ref: operator.operator_group_id.clone(),
                        source_ordinal: u32::try_from(ordinal).map_err(|_| ())?,
                        source_role: SourceRole::Subordinate,
                        source_refs: node_refs.clone(),
                    })
                })
                .collect::<Result<Vec<_>, ()>>()?;
            shape.push(TopologyShapeNode {
                id: node_id.clone(),
                name: name.clone(),
                children: children.clone(),
                sink_ids: sink_ids.clone().unwrap_or_default(),
                declares_fragment_root: sink_ids.is_some(),
            });
            parsed_nodes.push(ParsedGraphNode {
                node: GraphNode {
                    sql_location: None,
                    sql_facts: runtime_table_facts(node_operators),
                    node_id: node_id.clone(),
                    entity_kind: GraphEntityKind::PlanNode,
                    plan_node_id: Some(node_id),
                    fragment_id: None,
                    pipeline_id: None,
                    plan_node_type: name.clone(),
                    display_name: name,
                    source_ordinal: Some(u32::try_from(index).map_err(|_| ())?),
                    native_operator_refs,
                    subordinate_operator_refs,
                    metric_observation_refs: vec![],
                    execution_state: None,
                    source_quality: SourceQuality::Exact,
                    source_refs: node_refs,
                },
                children,
                sink_ids: sink_ids.unwrap_or_default(),
            });
        }
        let attribution = fragment_attribution(&root_node_id, &shape, &operators_by_node);
        for parsed in &mut parsed_nodes {
            parsed.node.fragment_id = attribution.fragments.get(&parsed.node.node_id).cloned();
        }
        let mut evidence = explicit_evidence(
            TopologySource::ExecutionTopology,
            vec![root_node_id.clone()],
            parsed_nodes,
            vec![source_ref.ref_id.clone()],
        )?;
        evidence.unresolved_relations = official_topology_contradictions(
            &root_node_id,
            &shape,
            &operators_by_node,
            attribution.conflicts,
            source_ref,
        );
        Ok(evidence)
    })();
    match parsed {
        // A Topology that satisfies every official invariant is closed: every plan node and
        // every fragment link is stated by the FE (ProfilingExecPlan.Topology.buildFrom).
        Ok(evidence) if evidence.unresolved_relations.is_empty() => {
            Some(GraphEvidence::ExplicitComplete(evidence))
        }
        Ok(evidence) => Some(GraphEvidence::Explicit(evidence)),
        Err(()) => {
            diagnostics.push(diagnostic(
                "runtime_topology_invalid",
                "graph",
                "warning",
                "partial",
                "runtime_topology_invalid",
                vec![source_ref.ref_id.clone()],
            ));
            Some(GraphEvidence::Unavailable {
                reason: "runtime_topology_invalid".into(),
                source_refs: vec![source_ref.ref_id.clone()],
            })
        }
    }
}

/// Operators keyed by their printed `plan_node_id`, in source order.
fn operators_by_plan_node(operators: &[RuntimeOperator]) -> BTreeMap<&str, Vec<&RuntimeOperator>> {
    let mut index = BTreeMap::<&str, Vec<&RuntimeOperator>>::new();
    for operator in operators {
        if let Some(plan_node_id) = operator.plan_node_id.as_deref() {
            index.entry(plan_node_id).or_default().push(operator);
        }
    }
    index
}

/// The two halves of an exchange live in different fragments: the sender's
/// EXCHANGE_SINK carries the destination exchange's id (`dest_node_id`).
fn is_cross_fragment_sink(operator: &RuntimeOperator) -> bool {
    operator.canonical_type == "EXCHANGE_SINK"
}

struct FragmentAttribution {
    /// Plan node id -> printed `Fragment N` id, only where it is proven.
    fragments: BTreeMap<String, String>,
    /// `(fragment root id, evidence refs)` for each segment that cannot be bound.
    conflicts: Vec<(String, Vec<String>)>,
}

/// Binds each plan node to the Profile `Fragment N` that executes it.
///
/// Official structure (3.5.14): `ProfilingExecPlan.Topology.buildFrom` puts
/// `sinkIds` on every fragment root, the topology root is fragment 0's root,
/// and an `ExchangeNode`'s only child is the sending fragment's root. Cutting
/// the plan below every fragment root yields one segment per fragment; the
/// receiving EXCHANGE node stays with its consumer, as ExplainAnalyzer prints it.
///
/// Product rule (own, not an FE rule): a segment is bound only when its native
/// non-`*_SINK` operators name exactly one fragment, every other operator of
/// the segment except the sender-side EXCHANGE_SINK agrees, and no other
/// segment claims the same fragment. A segment without operator evidence stays
/// unbound (`fragment_id = null`, not a contradiction); a disagreement is a
/// conflict and leaves every segment involved unbound.
fn fragment_attribution(
    root: &str,
    shape: &[TopologyShapeNode],
    operators_by_node: &BTreeMap<&str, Vec<&RuntimeOperator>>,
) -> FragmentAttribution {
    let by_id = shape
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut fragment_roots = BTreeSet::from([root]);
    for node in shape {
        if node.declares_fragment_root {
            fragment_roots.insert(node.id.as_str());
        }
        if is_exchange_node(node) {
            fragment_roots.extend(node.children.iter().map(String::as_str));
        }
    }

    let mut segments = Vec::<(&str, Vec<&str>, &str)>::new();
    let mut conflicts = Vec::new();
    let mut claims = BTreeMap::<&str, usize>::new();
    let mut assigned = BTreeSet::<&str>::new();
    // Source order keeps the result deterministic.
    for start in shape
        .iter()
        .map(|node| node.id.as_str())
        .filter(|id| fragment_roots.contains(id))
    {
        let mut members = Vec::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            if !assigned.insert(id) {
                continue;
            }
            members.push(id);
            if let Some(node) = by_id.get(id) {
                stack.extend(
                    node.children
                        .iter()
                        .map(String::as_str)
                        .filter(|child| !fragment_roots.contains(child)),
                );
            }
        }
        let operators = members
            .iter()
            .flat_map(|id| operators_by_node.get(id).into_iter().flatten())
            .filter(|operator| !is_cross_fragment_sink(operator))
            .collect::<Vec<_>>();
        let native = operators
            .iter()
            .filter(|operator| {
                operator.runtime_source_role == RuntimeSourceRole::Native
                    && !operator.canonical_type.ends_with("_SINK")
            })
            .filter_map(|operator| operator.fragment_ref.as_deref())
            .collect::<BTreeSet<_>>();
        let observed = operators
            .iter()
            .filter_map(|operator| operator.fragment_ref.as_deref())
            .collect::<BTreeSet<_>>();
        match (native.first(), native.len(), observed.len()) {
            (None, _, 0) => {}
            (Some(fragment), 1, 1) => {
                *claims.entry(fragment).or_default() += 1;
                segments.push((start, members, fragment));
            }
            _ => conflicts.push((
                start.to_owned(),
                operators
                    .iter()
                    .map(|operator| operator.source_ref.ref_id.clone())
                    .collect(),
            )),
        }
    }

    let mut fragments = BTreeMap::new();
    for (start, members, fragment) in segments {
        if claims.get(fragment).copied() == Some(1) {
            fragments.extend(
                members
                    .into_iter()
                    .map(|id| (id.to_owned(), fragment.to_owned())),
            );
        } else {
            conflicts.push((start.to_owned(), vec![]));
        }
    }
    FragmentAttribution {
        fragments,
        conflicts,
    }
}

/// One node of the FE `Topology` info string, exactly as printed.
struct TopologyShapeNode {
    id: String,
    name: Option<String>,
    children: Vec<String>,
    sink_ids: Vec<String>,
    /// `properties.sinkIds` is present (possibly empty): the FE marks a fragment root.
    declares_fragment_root: bool,
}

fn is_exchange_node(node: &TopologyShapeNode) -> bool {
    matches!(node.name.as_deref(), Some("EXCHANGE" | "MERGE_EXCHANGE"))
}

/// Contradictions against the invariants the 3.5.14 FE guarantees for the
/// `Topology` it prints (`ProfilingExecPlan.Topology.buildFrom`: a closed walk
/// over plan children; `ExchangeNode` has the sending fragment root as its one
/// child; `PlanFragment`/`DataStreamSink` and BE `data_sink.cpp` give the
/// sender's EXCHANGE_SINK the destination exchange's own `dest_node_id`).
///
/// A same-id EXCHANGE_SOURCE / EXCHANGE_SINK pair in two fragments is the two
/// halves of one exchange node (ExplainAnalyzer merges them), not an unproven
/// link, so it is never reported. Only an actual contradiction is; the graph
/// then stays partial instead of complete.
fn official_topology_contradictions(
    root: &str,
    shape: &[TopologyShapeNode],
    operators_by_node: &BTreeMap<&str, Vec<&RuntimeOperator>>,
    fragment_conflicts: Vec<(String, Vec<String>)>,
    source_ref: &SourceRef,
) -> Vec<UnresolvedRelation> {
    let by_id = shape
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut found: Vec<(&'static str, String, Vec<String>)> = Vec::new();

    // Closure and acyclicity from the root (a multicast producer is reached twice, legitimately).
    let mut visit = BTreeMap::<&str, bool>::new(); // true = on the current path
    let mut cycle = false;
    if by_id.contains_key(root) {
        let mut stack = vec![(root, 0_usize)];
        visit.insert(root, true);
        while let Some((id, index)) = stack.pop() {
            let children = by_id
                .get(id)
                .map_or(&[][..], |node| node.children.as_slice());
            if let Some(child) = children.get(index) {
                stack.push((id, index + 1));
                match visit.get(child.as_str()) {
                    Some(true) => cycle = true,
                    Some(false) => {}
                    None if by_id.contains_key(child.as_str()) => {
                        visit.insert(child.as_str(), true);
                        stack.push((child.as_str(), 0));
                    }
                    None => found.push(("topology_child_missing", child.clone(), vec![])),
                }
            } else {
                visit.insert(id, false);
            }
        }
    } else {
        found.push(("topology_root_missing", root.to_owned(), vec![]));
    }
    if cycle {
        found.push(("topology_cycle", root.to_owned(), vec![]));
    }
    for node in shape {
        if !visit.contains_key(node.id.as_str()) {
            found.push(("topology_node_unreachable", node.id.clone(), vec![]));
        }
    }

    let mut parents = BTreeMap::<&str, Vec<&str>>::new();
    for node in shape {
        for child in &node.children {
            parents
                .entry(child.as_str())
                .or_default()
                .push(node.id.as_str());
        }
        if is_exchange_node(node) && node.children.len() != 1 {
            found.push(("topology_exchange_child_count", node.id.clone(), vec![]));
        }
        if node
            .name
            .as_deref()
            .is_some_and(|name| name.ends_with("_SCAN"))
            && !node.children.is_empty()
        {
            found.push(("topology_scan_has_children", node.id.clone(), vec![]));
        }
        for target in &node.sink_ids {
            let consistent = by_id.get(target.as_str()).is_some_and(|exchange| {
                is_exchange_node(exchange) && exchange.children.contains(&node.id)
            });
            if !consistent {
                found.push(("topology_sink_target_mismatch", node.id.clone(), vec![]));
            }
        }
        // SplitCastDataSink prints no sinkIds (an official quirk), so only a listed, different target contradicts.
        if is_exchange_node(node)
            && let Some(child) = node
                .children
                .first()
                .and_then(|child| by_id.get(child.as_str()))
            && !child.sink_ids.is_empty()
            && !child.sink_ids.contains(&node.id)
        {
            found.push(("topology_sink_target_mismatch", child.id.clone(), vec![]));
        }
    }
    for (child, from) in &parents {
        if from.len() < 2 {
            continue;
        }
        // Only a multicast (CTE) producer root has several parents: each one an exchange it sinks to.
        let multicast = by_id.get(child).is_some_and(|node| {
            from.iter().all(|parent| {
                by_id
                    .get(parent)
                    .is_some_and(|exchange| is_exchange_node(exchange))
                    && node.sink_ids.iter().any(|sink| sink == parent)
            })
        });
        if !multicast {
            found.push(("topology_multiple_parents", (*child).to_owned(), vec![]));
        }
    }

    // Operators must sit where the Topology says: the sender's EXCHANGE_SINK in the
    // sending root's fragment, the receiver in a different one.
    let fragments = |plan_node_id: &str, sink: bool| {
        operators_by_node
            .get(plan_node_id)
            .into_iter()
            .flatten()
            .filter(|operator| is_cross_fragment_sink(operator) == sink)
            .filter_map(|operator| {
                operator
                    .fragment_ref
                    .clone()
                    .map(|fragment| (fragment, operator.source_ref.ref_id.clone()))
            })
            .collect::<Vec<_>>()
    };
    for node in shape.iter().filter(|node| !node.sink_ids.is_empty()) {
        let sender_home = fragments(&node.id, false)
            .into_iter()
            .map(|(fragment, _)| fragment)
            .collect::<BTreeSet<_>>();
        for target in &node.sink_ids {
            let sinks = fragments(target, true);
            let receivers = fragments(target, false);
            let refs = sinks
                .iter()
                .chain(&receivers)
                .map(|(_, reference)| reference.clone())
                .collect::<Vec<_>>();
            if !sender_home.is_empty()
                && sinks
                    .iter()
                    .any(|(fragment, _)| !sender_home.contains(fragment))
            {
                found.push((
                    "exchange_sink_fragment_mismatch",
                    target.clone(),
                    refs.clone(),
                ));
            }
            if sinks
                .iter()
                .any(|(fragment, _)| receivers.iter().any(|(other, _)| other == fragment))
            {
                found.push((
                    "exchange_sender_receiver_same_fragment",
                    target.clone(),
                    refs,
                ));
            }
        }
    }

    for (fragment_root, refs) in fragment_conflicts {
        found.push(("fragment_attribution_conflict", fragment_root, refs));
    }

    // Merge repeats of one finding, then order by kind and numeric plan node id ("4" before "11").
    let mut merged = BTreeMap::<(&'static str, String), Vec<String>>::new();
    for (kind, id, refs) in found {
        merged.entry((kind, id)).or_default().extend(refs);
    }
    let mut relations = merged
        .into_iter()
        .map(|((kind, id), mut source_refs)| {
            source_refs.push(source_ref.ref_id.clone());
            sort_natural(&mut source_refs);
            source_refs.dedup();
            UnresolvedRelation {
                relation_ref: format!("official_topology:{kind}:{id}"),
                reason: kind.into(),
                source_refs,
            }
        })
        .collect::<Vec<_>>();
    relations.sort_by(|left, right| natural_cmp(&left.relation_ref, &right.relation_ref));
    relations
}

/// Orders strings with embedded integers numerically: `text:4` before `text:11`.
pub(crate) fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    fn chunks(value: &str) -> impl Iterator<Item = (bool, &str)> {
        let mut rest = value;
        std::iter::from_fn(move || {
            let digit = rest.chars().next()?.is_ascii_digit();
            let end = rest
                .find(|c: char| c.is_ascii_digit() != digit)
                .unwrap_or(rest.len());
            let (chunk, tail) = rest.split_at(end);
            rest = tail;
            Some((digit, chunk))
        })
    }
    let (mut left_chunks, mut right_chunks) = (chunks(left), chunks(right));
    loop {
        let order = match (left_chunks.next(), right_chunks.next()) {
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((true, a)), Some((true, b))) => {
                let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                a.len().cmp(&b.len()).then_with(|| a.cmp(b))
            }
            (Some((_, a)), Some((_, b))) => a.cmp(b),
        };
        if order.is_ne() {
            return order;
        }
    }
}

pub(crate) fn sort_natural(values: &mut [String]) {
    values.sort_by(|left, right| natural_cmp(left, right));
}

fn runtime_topology_id(value: &Value) -> Result<String, ()> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Ok(value.clone()),
        Value::Number(value) if value.is_i64() || value.is_u64() => Ok(value.to_string()),
        _ => Err(()),
    }
}

fn runtime_topology_ids(value: Option<&Value>) -> Result<Vec<String>, ()> {
    match value {
        None => Ok(vec![]),
        Some(Value::Array(values)) => values.iter().map(runtime_topology_id).collect(),
        Some(_) => Err(()),
    }
}

/// Parses the official `RuntimeProfile.JsonProfileFormatter` nesting.  The
/// formatter emits rendered counter text, except for `Active_ns`; therefore a
/// JSON number is accepted only for that explicit raw companion field.
fn parse_runtime_profile_json(
    content: &[u8],
    object: &serde_json::Map<String, Value>,
    limits: &AdapterLimits,
    evidence: &[String],
    cancel: &CancelSignal<'_>,
) -> Result<Extracted, AdapterError> {
    let query = object
        .get("Query")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            error(
                "parse_failed",
                "parse",
                "runtime_query_object_required",
                "change_input",
                vec![],
                evidence.to_vec(),
            )
        })?;
    let format = json_counter_format(query);

    let mut diagnostics = Vec::new();
    let mut source_refs = Vec::new();
    let mut safe_tokens = Vec::new();
    let mut version = None;
    let mut profile_facts = ProfileFacts::unknown();
    let mut query_id = None;
    let mut query_type = None;
    let mut query_metrics = Vec::new();
    let mut operators = Vec::new();
    let mut pipeline_containers = Vec::new();
    let mut physical_operator_memberships = Vec::new();
    let mut pipeline_container_occurrences = BTreeMap::new();
    let mut topology = None;
    let mut metric_count = 0usize;
    let mut seen_operator_identity = BTreeSet::new();

    if let Some(summary) = query.get("Summary").and_then(Value::as_object) {
        for (key, value) in summary {
            let Some(value) = value.as_str() else {
                continue;
            };
            let pointer = format!("/Query/Summary/{}", json_pointer_component(key));
            let source_ref = runtime_json_ref(content, &pointer)?;
            match key.as_str() {
                "Query ID" => {
                    if let Some(normalized) = runtime_query_id(value) {
                        runtime_safe_token(&mut safe_tokens, &normalized, &source_ref);
                        query_id = Some(normalized);
                    } else {
                        diagnostics.push(diagnostic(
                            "runtime_query_id_invalid",
                            "parse",
                            "warning",
                            "partial",
                            "runtime_query_id_invalid",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                "Query Type" => {
                    if let Some(normalized) = runtime_query_type(value) {
                        runtime_safe_token(&mut safe_tokens, normalized, &source_ref);
                        query_type = Some(normalized.into());
                    } else {
                        diagnostics.push(diagnostic(
                            "runtime_query_type_invalid",
                            "parse",
                            "warning",
                            "partial",
                            "runtime_query_type_invalid",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                "Query State" => {
                    profile_facts.set_query_state(value, source_ref.clone(), &mut diagnostics);
                    if let Some(normalized) =
                        runtime_accepted_query_state(&profile_facts, &source_ref)
                    {
                        runtime_safe_token(&mut safe_tokens, normalized, &source_ref);
                    }
                    source_refs.push(source_ref);
                }
                "StarRocks Version" => {
                    if let Some(normalized) = runtime_version(value) {
                        version = Some(VersionFact {
                            value: normalized.clone(),
                            source_ref: source_ref.clone(),
                        });
                        runtime_safe_token(&mut safe_tokens, &normalized, &source_ref);
                    } else {
                        diagnostics.push(diagnostic(
                            "version_unrecognized",
                            "parse",
                            "warning",
                            "partial",
                            "version_unrecognized",
                            vec![source_ref.ref_id.clone()],
                        ));
                    }
                    source_refs.push(source_ref);
                }
                _ => {}
            }
        }
    }

    if let Some(execution) = query.get("Execution") {
        let mut incomplete_profile_markers = Vec::new();
        runtime_json_incomplete_profile_markers(
            content,
            execution,
            "/Query/Execution",
            &mut incomplete_profile_markers,
        )?;
        for (code, source_ref) in incomplete_profile_markers {
            diagnostics.push(diagnostic(
                &code,
                "parse",
                "warning",
                "partial",
                &code,
                vec![source_ref.ref_id.clone()],
            ));
            source_refs.push(source_ref);
        }
    }

    let planning_time_candidates = runtime_json_planning_candidates(
        content,
        query,
        &mut metric_count,
        limits,
        format,
        evidence,
        &mut source_refs,
        &mut diagnostics,
    )?;

    if let Some(execution) = query.get("Execution").and_then(Value::as_object) {
        let execution_path = "/Query/Execution";
        if let Some(value) = execution.get("Topology") {
            let source_ref = runtime_json_ref(content, &format!("{execution_path}/Topology"))?;
            match value {
                Value::String(raw) => match serde_json::from_str::<StrictJsonValue>(raw) {
                    Ok(value) => {
                        topology = Some(RuntimeTopology {
                            value: value.0,
                            source_ref: source_ref.clone(),
                        });
                        source_refs.push(source_ref);
                    }
                    Err(_) => diagnostics.push(diagnostic(
                        "runtime_topology_invalid",
                        "graph",
                        "warning",
                        "partial",
                        "runtime_topology_invalid",
                        vec![source_ref.ref_id],
                    )),
                },
                Value::Object(_) => {
                    topology = Some(RuntimeTopology {
                        value: value.clone(),
                        source_ref: source_ref.clone(),
                    });
                    source_refs.push(source_ref);
                }
                _ => diagnostics.push(diagnostic(
                    "runtime_topology_invalid",
                    "graph",
                    "warning",
                    "partial",
                    "runtime_topology_invalid",
                    vec![source_ref.ref_id],
                )),
            }
        }
        query_metrics.extend(runtime_json_metrics(
            content,
            execution,
            execution_path,
            "query",
            &mut metric_count,
            limits,
            format,
            evidence,
            &mut source_refs,
            &mut diagnostics,
        )?);
        for (name, value) in execution {
            cancel.check()?;
            let Some(fragment_id) = name
                .strip_prefix("Fragment ")
                .and_then(|value| value.strip_suffix(':').or(Some(value)))
            else {
                continue;
            };
            let fragment_id = nonempty(fragment_id);
            if fragment_id.is_none() {
                let source_ref = runtime_json_ref(
                    content,
                    &format!("{execution_path}/{}", json_pointer_component(name)),
                )?;
                diagnostics.push(diagnostic(
                    "runtime_fragment_ref_missing",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_fragment_ref_missing",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
            }
            let Some(fragment) = value.as_object() else {
                continue;
            };
            runtime_json_fragment(
                content,
                fragment,
                &format!("{execution_path}/{}", json_pointer_component(name)),
                fragment_id,
                &mut operators,
                &mut pipeline_containers,
                &mut physical_operator_memberships,
                &mut pipeline_container_occurrences,
                &mut metric_count,
                limits,
                format,
                evidence,
                &mut source_refs,
                &mut diagnostics,
                &mut seen_operator_identity,
            )?;
        }
    }

    finalize_native_operator_roles(&mut operators, &mut physical_operator_memberships);
    let plan_node_types = runtime_topology_node_types(topology.as_ref());
    let graph_evidence = runtime_graph_evidence(topology.as_ref(), &operators, &mut diagnostics);
    Ok(Extracted {
        line_count: None,
        version,
        metric: None,
        profile_facts,
        source_refs,
        diagnostics,
        graph_evidence,
        source_view: SourceView {
            source_kind: SourceKind::JsonProfile,
            plain_text_lines: vec![],
            json_tokens: safe_tokens,
            truncation_state: "complete".into(),
            limitations: vec![
                "runtime_profile_safe_allowlist_only".into(),
                "raw_sql_user_address_and_unknown_text_excluded".into(),
            ],
            line_refs: vec![],
        },
        runtime: Some(RuntimeProfileData {
            formatter_shape: RuntimeInventoryFormatterShape::Unknown,
            query_summary: QuerySummary {
                query_id,
                query_type,
                limitations: vec![
                    "runtime_profile_summary_allowlist_only".into(),
                    "sql_user_address_and_unknown_text_excluded".into(),
                ],
            },
            query_metrics,
            planning_time_candidates,
            operators,
            plan_node_types,
            pipeline_containers,
            runtime_driver_occurrences: vec![],
            physical_operator_memberships,
        }),
    })
}

#[allow(clippy::too_many_arguments)]
fn runtime_json_planning_candidates(
    content: &[u8],
    query: &serde_json::Map<String, Value>,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<RuntimeMetricFact>, AdapterError> {
    let Some(planner) = query.get("Planner").and_then(Value::as_object) else {
        return Ok(vec![]);
    };
    let Some(value) = planner.get("Total") else {
        return Ok(vec![]);
    };
    let source_ref = runtime_json_ref(content, "/Query/Planner/Total")?;
    let Value::String(value) = value else {
        diagnostics.push(diagnostic(
            "runtime_json_counter_type_invalid",
            "parse",
            "warning",
            "partial",
            "runtime_json_counter_type_invalid",
            vec![source_ref.ref_id.clone()],
        ));
        source_refs.push(source_ref);
        return Ok(vec![]);
    };
    let definition = match metric_lookup_for(SourceKind::JsonProfile, format, "QueryPlanningTime") {
        MetricRegistryLookup::Registered(definition) => definition,
        MetricRegistryLookup::UnknownNonAdditiveUnavailable => return Ok(vec![]),
    };
    *metric_count = metric_count.saturating_add(1);
    check_metric_budget(*metric_count, limits, evidence)?;
    let outcome = parse_metric(
        &definition,
        MetricInput::Text(value.clone()),
        ParseMode::Strict,
    );
    if outcome.parse_state != ParseState::Exact {
        diagnostics.push(metric_diagnostic(
            "QueryPlanningTime",
            &outcome,
            &source_ref,
        ));
    }
    let source_ordinal = u32::try_from(*metric_count - 1).map_err(|_| {
        limit_error(
            "too_many_metrics",
            *metric_count,
            limits.max_normalized_metrics,
            evidence.to_vec(),
        )
    })?;
    source_refs.push(source_ref.clone());
    Ok(vec![RuntimeMetricFact {
        metric_id: "QueryPlanningTime".into(),
        scope: "query".into(),
        source_name: "Planner/Total".into(),
        value_role: "reported_base".into(),
        display_text: if outcome.parse_state == ParseState::Exact {
            value.clone()
        } else {
            "—".into()
        },
        outcome,
        source_ref,
        source_ordinal,
    }])
}

fn runtime_topology_node_types(topology: Option<&RuntimeTopology>) -> BTreeMap<String, String> {
    let Some(nodes) = topology
        .and_then(|topology| topology.value.as_object())
        .and_then(|object| object.get("nodes"))
        .and_then(Value::as_array)
    else {
        return BTreeMap::new();
    };
    let mut node_types = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();
    for node in nodes {
        let Some(node) = node.as_object() else {
            continue;
        };
        let Some(id) = node.get("id").and_then(|id| runtime_topology_id(id).ok()) else {
            continue;
        };
        if !seen_ids.insert(id.clone()) {
            node_types.remove(&id);
            continue;
        }
        if let Some(node_type) = node
            .get("name")
            .and_then(Value::as_str)
            .and_then(runtime_identifier)
        {
            node_types.insert(id, node_type);
        }
    }
    node_types
}

fn runtime_semantic_operator_type(topology_type: &str) -> &str {
    // 3.5.14 ProfilingExecPlan emits SCHEMA_SCAN for a ScanNode-backed plan
    // node. Rules consume the semantic kind, while raw_labels retain the
    // source topology/operator names for evidence and later pack expansion.
    match topology_type {
        "SCHEMA_SCAN" => "SCAN",
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
fn runtime_json_fragment(
    content: &[u8],
    fragment: &serde_json::Map<String, Value>,
    path: &str,
    fragment_id: Option<&str>,
    operators: &mut Vec<RuntimeOperator>,
    pipeline_containers: &mut Vec<PipelineContainer>,
    physical_operator_memberships: &mut Vec<PhysicalOperatorMembership>,
    pipeline_container_occurrences: &mut BTreeMap<(String, String), usize>,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
    seen_operator_identity: &mut BTreeSet<String>,
) -> Result<(), AdapterError> {
    let fragment_ref = runtime_json_ref(content, path)?;
    let container = runtime_container(RuntimeContainerInput {
        operators,
        pipeline_containers,
        pipeline_container_occurrences,
        kind: "fragment",
        fragment_ref: fragment_id,
        pipeline_ref: None,
        canonical_type: "FRAGMENT",
        source_ref: fragment_ref.clone(),
        limits,
        evidence,
    })?;
    source_refs.push(fragment_ref);
    let metrics = runtime_json_metrics(
        content,
        fragment,
        path,
        "fragment",
        metric_count,
        limits,
        format,
        evidence,
        source_refs,
        diagnostics,
    )?;
    operators[container].metrics.extend(metrics);
    for (name, value) in fragment {
        let Some(pipeline_id) = runtime_pipeline_header(name) else {
            continue;
        };
        let Some(pipeline) = value.as_object() else {
            continue;
        };
        runtime_json_pipeline(
            content,
            pipeline,
            &format!("{path}/{}", json_pointer_component(name)),
            fragment_id,
            nonempty(pipeline_id),
            operators,
            pipeline_containers,
            physical_operator_memberships,
            pipeline_container_occurrences,
            metric_count,
            limits,
            format,
            evidence,
            source_refs,
            diagnostics,
            seen_operator_identity,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn runtime_json_pipeline(
    content: &[u8],
    pipeline: &serde_json::Map<String, Value>,
    path: &str,
    fragment_id: Option<&str>,
    pipeline_id: Option<&str>,
    operators: &mut Vec<RuntimeOperator>,
    pipeline_containers: &mut Vec<PipelineContainer>,
    physical_operator_memberships: &mut Vec<PhysicalOperatorMembership>,
    pipeline_container_occurrences: &mut BTreeMap<(String, String), usize>,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
    seen_operator_identity: &mut BTreeSet<String>,
) -> Result<(), AdapterError> {
    let pipeline_ref = runtime_json_ref(content, path)?;
    if pipeline_id.is_none() {
        diagnostics.push(diagnostic(
            "runtime_pipeline_ref_missing",
            "parse",
            "warning",
            "partial",
            "runtime_pipeline_ref_missing",
            vec![pipeline_ref.ref_id.clone()],
        ));
    }
    let container = runtime_container(RuntimeContainerInput {
        operators,
        pipeline_containers,
        pipeline_container_occurrences,
        kind: "pipeline",
        fragment_ref: fragment_id,
        pipeline_ref: pipeline_id,
        canonical_type: "PIPELINE",
        source_ref: pipeline_ref.clone(),
        limits,
        evidence,
    })?;
    source_refs.push(pipeline_ref);
    let metrics = runtime_json_metrics(
        content,
        pipeline,
        path,
        "pipeline",
        metric_count,
        limits,
        format,
        evidence,
        source_refs,
        diagnostics,
    )?;
    operators[container].metrics.extend(metrics);
    for (name, value) in pipeline {
        let Some((operator_type, plan_node_id, native_operator_id)) =
            runtime_json_operator_header(name)
        else {
            if runtime_operator_header_candidate(name) {
                let source_ref =
                    runtime_json_ref(content, &format!("{path}/{}", json_pointer_component(name)))?;
                diagnostics.push(diagnostic(
                    "runtime_operator_header_invalid",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_operator_header_invalid",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
            }
            continue;
        };
        let Some(operator) = value.as_object() else {
            continue;
        };
        let source_ref =
            runtime_json_ref(content, &format!("{path}/{}", json_pointer_component(name)))?;
        if let (Some(fragment_id), Some(pipeline_id)) = (fragment_id, pipeline_id) {
            let identity = format!(
                "{fragment_id}:{pipeline_id}:{operator_type}:{}:{native_operator_id:?}",
                plan_node_id.as_deref().unwrap_or("unknown"),
            );
            if !seen_operator_identity.insert(identity) {
                return Err(error(
                    "parse_failed",
                    "parse",
                    "runtime_operator_identity_duplicate",
                    "change_input",
                    vec![source_ref],
                    evidence.to_vec(),
                ));
            }
        }
        let ordinal = u32::try_from(operators.len()).map_err(|_| {
            limit_error(
                "too_many_metrics",
                operators.len(),
                limits.max_normalized_metrics,
                evidence.to_vec(),
            )
        })?;
        let index = operators.len();
        let operator_group_id = format!(
            "operator:{}:{}:{}:{ordinal}",
            fragment_id.unwrap_or("unknown"),
            pipeline_id.unwrap_or("unknown"),
            plan_node_id.as_deref().unwrap_or("none")
        );
        let pipeline_group_ref = operators
            .get(container)
            .filter(|operator| operator.canonical_type == "PIPELINE")
            .map(|operator| operator.operator_group_id.clone());
        // The enclosing pipeline's container is the most recently pushed one.
        let container_ref = pipeline_group_ref.and_then(|candidate| {
            pipeline_containers
                .iter()
                .rev()
                .find(|container| container.container_ref == candidate)
                .map(|container| container.container_ref.clone())
        });
        let mut membership_reasons = Vec::new();
        if fragment_id.is_none() {
            membership_reasons.push("runtime_fragment_ref_missing".into());
        }
        if pipeline_id.is_none() {
            membership_reasons.push("runtime_pipeline_ref_missing".into());
        }
        if container_ref.is_none() {
            membership_reasons.push("runtime_pipeline_container_missing".into());
        }
        let source_role = runtime_json_operator_role(operator);
        let runtime_source_role = if source_role == Some(SourceRole::Subordinate) {
            RuntimeSourceRole::Subordinate
        } else {
            RuntimeSourceRole::Unknown
        };
        // The same UniqueMetrics info strings the text grammar reads, with the same checks.
        let operator_path = format!("{path}/{}", json_pointer_component(name));
        let table_facts = if operator_type == "OLAP_SCAN" {
            runtime_json_operator_info(content, operator, &operator_path, "Table", source_refs)?
                .into_iter()
                .map(|fact| RuntimeInfoFact {
                    value: fact.value.as_deref().and_then(runtime_table_value),
                    ..fact
                })
                .collect()
        } else {
            vec![]
        };
        let predicate_facts = runtime_json_operator_expressions(
            content,
            operator,
            &operator_path,
            "Predicates",
            &operator_type,
            source_refs,
        )?;
        let join_predicate_facts = runtime_json_operator_expressions(
            content,
            operator,
            &operator_path,
            "JoinPredicates",
            &operator_type,
            source_refs,
        )?;
        let distribution_mode_facts = runtime_json_operator_info(
            content,
            operator,
            &operator_path,
            "DistributionMode",
            source_refs,
        )?
        .into_iter()
        .map(|fact| RuntimeInfoFact {
            value: fact.value.as_deref().and_then(runtime_identifier),
            ..fact
        })
        .collect();
        operators.push(RuntimeOperator {
            operator_group_id: operator_group_id.clone(),
            plan_node_id,
            fragment_ref: fragment_id.map(str::to_owned),
            pipeline_ref: pipeline_id.map(str::to_owned),
            canonical_type: operator_type,
            native_operator_id,
            source_role,
            runtime_source_role: runtime_source_role.clone(),
            role_marker_malformed: runtime_json_operator_role_malformed(operator),
            source_ref: source_ref.clone(),
            metrics: vec![],
            is_final_sink: runtime_json_operator_is_final_sink(operator),
            table_facts,
            predicate_facts,
            join_predicate_facts,
            partition_type_facts: runtime_json_operator_partition_types(
                content,
                operator,
                &format!("{path}/{}", json_pointer_component(name)),
                source_refs,
                diagnostics,
            )?,
            distribution_mode_facts,
        });
        physical_operator_memberships.push(PhysicalOperatorMembership {
            physical_operator_ref: operator_group_id,
            source_occurrence_ordinal: ordinal,
            fragment_ref: fragment_id.map(str::to_owned),
            container_ref,
            driver_occurrence_ref: None,
            driver_occurrence_ordinal: None,
            emission_ordinal: None,
            plan_node_id: operators[index].plan_node_id.clone(),
            canonical_type: operators[index].canonical_type.clone(),
            source_role: runtime_source_role,
            source_refs: vec![source_ref.ref_id.clone()],
            reasons: membership_reasons,
        });
        source_refs.push(source_ref);
        let metrics = runtime_json_operator_metrics(
            content,
            operator,
            &format!("{path}/{}", json_pointer_component(name)),
            metric_count,
            limits,
            format,
            evidence,
            source_refs,
            diagnostics,
        )?;
        operators[index].metrics.extend(metrics);
    }
    Ok(())
}

fn runtime_pipeline_header(name: &str) -> Option<&str> {
    name.strip_prefix("Pipeline (id=")
        .and_then(|value| value.strip_suffix(')'))
}

fn runtime_json_operator_header(name: &str) -> Option<(String, Option<String>, Option<i32>)> {
    let (label, suffix) = name.split_once(" (plan_node_id=")?;
    let label = runtime_identifier(label.trim())?;
    let (plan_node, trailing) = suffix.split_once(')')?;
    let plan_node = runtime_header_id(plan_node, true)?;
    let native_operator_id = if trailing.is_empty() {
        None
    } else {
        let value = trailing.strip_prefix(" (operator id=")?.strip_suffix(')')?;
        Some(runtime_header_id(value, false)?)
    };
    Some((label, Some(plan_node.to_string()), native_operator_id))
}

fn runtime_header_id(value: &str, allow_sentinel: bool) -> Option<i32> {
    if allow_sentinel && value == "-1" {
        return Some(-1);
    }
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Official 3.5.14 `ExplainAnalyzer.NodeIterator` classifies every operator
/// profile that lacks `CommonMetrics.IsSubordinate` as native (including one
/// without `CommonMetrics`). This sets the runtime (membership) role used by
/// the execution summary and the OutputRows recipe. The ordered group refs and
/// observation `operator_ref` keep the "unmarked" encoding that the rule
/// engine's verified official path consumes. Containers stay unset.
fn finalize_native_operator_roles(
    operators: &mut [RuntimeOperator],
    memberships: &mut [PhysicalOperatorMembership],
) {
    // First membership per physical operator ref, as the former linear `find` chose.
    let mut membership_index = BTreeMap::<String, usize>::new();
    for (index, membership) in memberships.iter().enumerate() {
        membership_index
            .entry(membership.physical_operator_ref.clone())
            .or_insert(index);
    }
    for operator in operators.iter_mut() {
        if operator.source_role.is_some()
            || operator.role_marker_malformed
            || operator.plan_node_id.is_none()
            || matches!(operator.canonical_type.as_str(), "FRAGMENT" | "PIPELINE")
        {
            continue;
        }
        operator.runtime_source_role = RuntimeSourceRole::Native;
        if let Some(index) = membership_index.get(&operator.operator_group_id) {
            memberships[*index].source_role = RuntimeSourceRole::Native;
        }
    }
}

/// `IsSubordinate` present with any value other than the empty marker.
fn runtime_json_operator_role_malformed(operator: &serde_json::Map<String, Value>) -> bool {
    operator
        .get("CommonMetrics")
        .and_then(Value::as_object)
        .and_then(|metrics| metrics.get("IsSubordinate"))
        .is_some_and(|value| value.as_str() != Some(""))
}

fn runtime_json_operator_role(operator: &serde_json::Map<String, Value>) -> Option<SourceRole> {
    if operator
        .get("CommonMetrics")
        .and_then(Value::as_object)
        .and_then(|metrics| metrics.get("IsSubordinate"))
        .and_then(Value::as_str)
        == Some("")
    {
        Some(SourceRole::Subordinate)
    } else {
        None
    }
}

fn runtime_json_operator_is_final_sink(operator: &serde_json::Map<String, Value>) -> bool {
    operator
        .get("CommonMetrics")
        .and_then(Value::as_object)
        .is_some_and(|metrics| metrics.contains_key("IsFinalSink"))
}

/// `Table` of an OLAP scan, as the text grammar accepts it (bounded, no control characters).
fn runtime_table_value(value: &str) -> Option<String> {
    let table = value.trim();
    (!table.is_empty() && table.len() <= 512 && !table.chars().any(char::is_control))
        .then(|| table.to_owned())
}

/// `Predicates` belong to OLAP scans, `JoinPredicates` to hash joins.
fn runtime_predicate_applies(key: &str, canonical_type: &str) -> bool {
    (key == "Predicates" && canonical_type == "OLAP_SCAN")
        || (key == "JoinPredicates"
            && matches!(
                canonical_type,
                "HASH_JOIN" | "HASH_JOIN_BUILD" | "HASH_JOIN_PROBE"
            ))
}

/// A predicate expression; a truncated (`...`) or oversized one is kept as unavailable.
fn runtime_expression_value(value: &str) -> Option<String> {
    let expression = value.trim();
    (!expression.is_empty()
        && expression.len() <= 4096
        && !expression.contains("...")
        && !expression.contains('…')
        && !expression.chars().any(char::is_control))
    .then(|| expression.to_owned())
}

/// One `UniqueMetrics` info string of a JsonProfileFormatter operator, raw.
/// A non-string value is kept as an unavailable fact, as a malformed text line is.
fn runtime_json_operator_info(
    content: &[u8],
    operator: &serde_json::Map<String, Value>,
    path: &str,
    key: &str,
    source_refs: &mut Vec<SourceRef>,
) -> Result<Vec<RuntimeInfoFact>, AdapterError> {
    let Some(value) = operator
        .get("UniqueMetrics")
        .and_then(Value::as_object)
        .and_then(|metrics| metrics.get(key))
    else {
        return Ok(vec![]);
    };
    let source_ref = runtime_json_ref(content, &format!("{path}/UniqueMetrics/{key}"))?;
    source_refs.push(source_ref.clone());
    Ok(vec![RuntimeInfoFact {
        value: value.as_str().map(str::to_owned),
        source_ref,
    }])
}

fn runtime_json_operator_expressions(
    content: &[u8],
    operator: &serde_json::Map<String, Value>,
    path: &str,
    key: &str,
    operator_type: &str,
    source_refs: &mut Vec<SourceRef>,
) -> Result<Vec<RuntimeInfoFact>, AdapterError> {
    if !runtime_predicate_applies(key, operator_type) {
        return Ok(vec![]);
    }
    Ok(
        runtime_json_operator_info(content, operator, path, key, source_refs)?
            .into_iter()
            .map(|fact| RuntimeInfoFact {
                value: fact.value.as_deref().and_then(runtime_expression_value),
                ..fact
            })
            .collect(),
    )
}

fn runtime_json_operator_partition_types(
    content: &[u8],
    operator: &serde_json::Map<String, Value>,
    path: &str,
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<RuntimeInfoFact>, AdapterError> {
    let Some(value) = operator
        .get("UniqueMetrics")
        .and_then(Value::as_object)
        .and_then(|metrics| metrics.get("PartType"))
    else {
        return Ok(vec![]);
    };
    let source_ref = runtime_json_ref(content, &format!("{path}/UniqueMetrics/PartType"))?;
    let value = value.as_str().and_then(runtime_identifier);
    if value.is_none() {
        diagnostics.push(diagnostic(
            "runtime_partition_type_invalid",
            "parse",
            "warning",
            "partial",
            "runtime_partition_type_invalid",
            vec![source_ref.ref_id.clone()],
        ));
    }
    source_refs.push(source_ref.clone());
    Ok(vec![RuntimeInfoFact { value, source_ref }])
}

#[allow(clippy::too_many_arguments)]
fn runtime_json_operator_metrics(
    content: &[u8],
    operator: &serde_json::Map<String, Value>,
    path: &str,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<RuntimeMetricFact>, AdapterError> {
    let mut metrics = runtime_json_metrics(
        content,
        operator,
        path,
        "operator",
        metric_count,
        limits,
        format,
        evidence,
        source_refs,
        diagnostics,
    )?;
    for section in ["CommonMetrics", "UniqueMetrics"] {
        if let Some(values) = operator.get(section).and_then(Value::as_object) {
            metrics.extend(runtime_json_metrics(
                content,
                values,
                &format!("{path}/{section}"),
                "operator",
                metric_count,
                limits,
                format,
                evidence,
                source_refs,
                diagnostics,
            )?);
        }
    }
    Ok(metrics)
}

#[allow(clippy::too_many_arguments)]
fn runtime_json_metrics(
    content: &[u8],
    object: &serde_json::Map<String, Value>,
    path: &str,
    scope: &str,
    metric_count: &mut usize,
    limits: &AdapterLimits,
    format: CounterTextFormat,
    evidence: &[String],
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<RuntimeMetricFact>, AdapterError> {
    let mut metrics = Vec::new();
    for (key, value) in object {
        // StarRocks 3.5.14 JsonProfileFormatter emits exactly one numeric raw
        // companion: `Active_ns` beside the rendered `Active` counter.  Do not
        // generalize the suffix to arbitrary metrics; that would turn an
        // untrusted JSON field into an exact counter that the official
        // formatter never promised.
        let raw_companion = key == "Active_ns";
        let (metric_id, value_role) = if raw_companion {
            ("Active", "reported_base")
        } else if let Some(base) = key.strip_prefix("__MAX_OF_") {
            (base, "reported_max")
        } else if let Some(base) = key.strip_prefix("__MIN_OF_") {
            (base, "reported_min")
        } else if let Some(base) = key.strip_suffix("_ns") {
            if matches!(
                metric_lookup_for(SourceKind::JsonProfile, format, base),
                MetricRegistryLookup::Registered(_)
            ) {
                let pointer = format!("{path}/{}", json_pointer_component(key));
                let source_ref = runtime_json_ref(content, &pointer)?;
                diagnostics.push(diagnostic(
                    "runtime_json_raw_counter_not_supported",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_json_raw_counter_not_supported",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
            }
            continue;
        } else {
            (key.as_str(), "reported_base")
        };
        let definition = match metric_lookup_for(SourceKind::JsonProfile, format, metric_id) {
            MetricRegistryLookup::Registered(definition)
                if definition
                    .valid_scopes
                    .iter()
                    .any(|candidate| candidate == scope) =>
            {
                definition
            }
            _ => continue,
        };
        // The formatter's raw companion wins whenever it is present.  If it
        // has the wrong type below, retain a diagnostic rather than silently
        // treating the rendered display value as exact raw data.
        if metric_id == "Active" && !raw_companion && object.contains_key("Active_ns") {
            continue;
        }
        let pointer = format!("{path}/{}", json_pointer_component(key));
        let source_ref = runtime_json_ref(content, &pointer)?;
        let input = match (raw_companion, value) {
            (true, Value::Number(value)) => MetricInput::JsonNumber(value.to_string()),
            (false, Value::String(value)) => MetricInput::Text(value.clone()),
            _ => {
                diagnostics.push(diagnostic(
                    "runtime_json_counter_type_invalid",
                    "parse",
                    "warning",
                    "partial",
                    "runtime_json_counter_type_invalid",
                    vec![source_ref.ref_id.clone()],
                ));
                source_refs.push(source_ref);
                continue;
            }
        };
        *metric_count = metric_count.saturating_add(1);
        check_metric_budget(*metric_count, limits, evidence)?;
        let outcome = parse_metric(&definition, input, ParseMode::Strict);
        if outcome.parse_state != ParseState::Exact {
            diagnostics.push(metric_diagnostic(metric_id, &outcome, &source_ref));
        }
        let display_text = match value {
            Value::String(value) if outcome.parse_state == ParseState::Exact => value.clone(),
            Value::Number(_) if outcome.parse_state == ParseState::Exact => outcome
                .canonical_number
                .as_ref()
                .zip(outcome.canonical_unit.as_ref())
                .map(|(number, unit)| format!("{}{}", number.value, unit))
                .unwrap_or_else(|| "—".into()),
            _ => "—".into(),
        };
        source_refs.push(source_ref.clone());
        metrics.push(RuntimeMetricFact {
            metric_id: metric_id.into(),
            scope: scope.into(),
            source_name: key.clone(),
            value_role: value_role.into(),
            outcome,
            source_ref,
            display_text,
            source_ordinal: u32::try_from(*metric_count - 1).map_err(|_| {
                limit_error(
                    "too_many_metrics",
                    *metric_count,
                    limits.max_normalized_metrics,
                    evidence.to_vec(),
                )
            })?,
        });
    }
    Ok(metrics)
}

fn runtime_json_ref(content: &[u8], pointer: &str) -> Result<SourceRef, AdapterError> {
    let byte_end = u32::try_from(content.len()).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "json_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    Ok(SourceRef {
        ref_id: format!("json:{pointer}"),
        source_kind: SourceKind::JsonProfile,
        locator: SourceLocator::Json {
            json_pointer: pointer.into(),
            byte_start: 0,
            byte_end,
        },
        redaction_state: RedactionState::OriginalLocal,
    })
}

fn runtime_json_incomplete_profile_markers(
    content: &[u8],
    value: &Value,
    path: &str,
    markers: &mut Vec<(String, SourceRef)>,
) -> Result<(), AdapterError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}/{}", json_pointer_component(key));
                let code = match key.as_str() {
                    "NotIdentical" if child.as_str() == Some("") => {
                        Some("runtime_profile_not_identical")
                    }
                    "NotIdentical" => Some("runtime_profile_incomplete_marker_malformed"),
                    "MissingInstanceIds"
                        if child.as_str().is_some_and(|value| !value.trim().is_empty()) =>
                    {
                        Some("runtime_profile_missing_instances")
                    }
                    "MissingInstanceIds" => Some("runtime_profile_incomplete_marker_malformed"),
                    "IsRunning"
                        if path.contains("/CommonMetrics") && child.as_str() == Some("") =>
                    {
                        Some("runtime_operator_running")
                    }
                    "IsRunning" => Some("runtime_profile_incomplete_marker_malformed"),
                    "IsSubordinate"
                        if path.ends_with("/CommonMetrics") && child.as_str() == Some("") =>
                    {
                        None
                    }
                    "IsSubordinate" => Some("runtime_profile_incomplete_marker_malformed"),
                    _ => None,
                };
                if let Some(code) = code {
                    markers.push((code.into(), runtime_json_ref(content, &child_path)?));
                }
                runtime_json_incomplete_profile_markers(content, child, &child_path, markers)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                runtime_json_incomplete_profile_markers(
                    content,
                    child,
                    &format!("{path}/{index}"),
                    markers,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn json_pointer_component(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn runtime_safe_token(tokens: &mut Vec<SourceToken>, value: &str, source_ref: &SourceRef) {
    if !value.contains('\n') && !value.contains('\r') && value.len() <= 256 {
        tokens.push(SourceToken {
            kind: "string".into(),
            text: value.into(),
            source_ref: source_ref.ref_id.clone(),
        });
    }
}

fn parse_json_graph(
    content: &[u8],
    object: &serde_json::Map<String, Value>,
    source_refs: &mut Vec<SourceRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<GraphEvidence> {
    let values = ["execution_topology", "executionTopology"]
        .into_iter()
        .filter_map(|key| object.get(key).map(|value| (key, value)))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    if values.len() != 1 {
        diagnostics.push(diagnostic(
            "graph_evidence_conflict",
            "graph",
            "warning",
            "partial",
            "graph_evidence_conflict",
            vec!["json:".into()],
        ));
        return Some(GraphEvidence::Unavailable {
            reason: "graph_evidence_conflict".into(),
            source_refs: vec!["json:".into()],
        });
    }
    let (key, topology) = values[0];
    let source_ref = match json_ref(content, key) {
        Ok(source_ref) => source_ref,
        Err(_) => {
            diagnostics.push(diagnostic(
                "graph_provenance_ambiguous",
                "graph",
                "warning",
                "partial",
                "graph_provenance_ambiguous",
                vec!["json:".into()],
            ));
            return Some(GraphEvidence::Unavailable {
                reason: "graph_provenance_ambiguous".into(),
                source_refs: vec!["json:".into()],
            });
        }
    };
    source_refs.push(source_ref.clone());
    match explicit_json_topology(topology, &source_ref) {
        Ok((evidence, complete)) => Some(if complete {
            GraphEvidence::ExplicitComplete(evidence)
        } else {
            GraphEvidence::Explicit(evidence)
        }),
        Err(()) => {
            diagnostics.push(diagnostic(
                "graph_evidence_invalid",
                "graph",
                "warning",
                "partial",
                "graph_evidence_invalid",
                vec![source_ref.ref_id.clone()],
            ));
            Some(GraphEvidence::Unavailable {
                reason: "graph_evidence_invalid".into(),
                source_refs: vec![source_ref.ref_id],
            })
        }
    }
}

fn explicit_json_topology(
    topology: &Value,
    source_ref: &SourceRef,
) -> Result<(ExplicitGraphEvidence, bool), ()> {
    let object = topology.as_object().ok_or(())?;
    let complete = match aliased_value(object, &["complete"])? {
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(()),
        None => false,
    };
    let root_node_ids = required_string_list(object, &["root_node_ids", "rootNodeIds"])?;
    let nodes = aliased_value(object, &["nodes"])?
        .and_then(Value::as_array)
        .ok_or(())?;
    let mut parsed_nodes = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        parsed_nodes.push(json_graph_node(node, index, source_ref)?);
    }
    explicit_evidence(
        TopologySource::ExecutionTopology,
        root_node_ids,
        parsed_nodes,
        vec![source_ref.ref_id.clone()],
    )
    .map(|evidence| (evidence, complete))
}

fn json_graph_node(
    value: &Value,
    index: usize,
    source_ref: &SourceRef,
) -> Result<ParsedGraphNode, ()> {
    let object = value.as_object().ok_or(())?;
    let node_id = required_string(object, &["node_id", "nodeId"])?;
    let entity_kind = optional_string(object, &["entity_kind", "entityKind"])?;
    let plan_node_id = optional_string(object, &["plan_node_id", "planNodeId"])?;
    let fragment_id = optional_string(object, &["fragment_id", "fragmentId"])?;
    let pipeline_id = optional_string(object, &["pipeline_id", "pipelineId"])?;
    let plan_node_type = optional_string(object, &["plan_node_type", "planNodeType"])?;
    let display_name = optional_string(object, &["display_name", "displayName"])?;
    let native_operator_refs =
        optional_string_list(object, &["operator_profiles", "operatorProfiles"])?;
    let subordinate_operator_refs = optional_string_list(
        object,
        &[
            "subordinate_operator_profiles",
            "subordinateOperatorProfiles",
        ],
    )?;
    let children = optional_string_list(object, &["children"])?;
    let sink_ids = optional_string_list(object, &["sink_ids", "sinkIds"])?;
    graph_node(
        node_id,
        entity_kind,
        plan_node_id,
        fragment_id,
        pipeline_id,
        plan_node_type,
        display_name,
        native_operator_refs,
        subordinate_operator_refs,
        u32::try_from(index).map_err(|_| ())?,
        source_ref,
    )
    .map(|node| ParsedGraphNode {
        node,
        children,
        sink_ids,
    })
}

fn parse_text_graph(
    topology_complete: Option<(bool, SourceRef)>,
    roots: Vec<(String, SourceRef)>,
    nodes: Vec<TextGraphNode>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<GraphEvidence> {
    if topology_complete.is_none() && roots.is_empty() && nodes.is_empty() {
        return None;
    }
    let mut evidence_refs = topology_complete
        .as_ref()
        .map(|(_, source)| vec![source.ref_id.clone()])
        .unwrap_or_default();
    let mut root_node_ids = Vec::new();
    for (root, source_ref) in roots {
        match split_explicit_list(&root) {
            Ok(values) => root_node_ids.extend(values),
            Err(()) => {
                diagnostics.push(diagnostic(
                    "graph_evidence_invalid",
                    "graph",
                    "warning",
                    "partial",
                    "graph_evidence_invalid",
                    vec![source_ref.ref_id.clone()],
                ));
                return Some(GraphEvidence::Unavailable {
                    reason: "graph_evidence_invalid".into(),
                    source_refs: vec![source_ref.ref_id],
                });
            }
        }
        evidence_refs.push(source_ref.ref_id);
    }
    let mut parsed_nodes = Vec::with_capacity(nodes.len());
    for (index, text_node) in nodes.iter().enumerate() {
        let fields = match parse_text_fields(&text_node.fields) {
            Ok(fields) => fields,
            Err(()) => {
                diagnostics.push(diagnostic(
                    "graph_evidence_invalid",
                    "graph",
                    "warning",
                    "partial",
                    "graph_evidence_invalid",
                    vec![text_node.source_ref.ref_id.clone()],
                ));
                return Some(GraphEvidence::Unavailable {
                    reason: "graph_evidence_invalid".into(),
                    source_refs: vec![text_node.source_ref.ref_id.clone()],
                });
            }
        };
        let parsed = text_graph_node(&fields, index, &text_node.source_ref);
        match parsed {
            Ok(parsed) => parsed_nodes.push(parsed),
            Err(()) => {
                diagnostics.push(diagnostic(
                    "graph_evidence_invalid",
                    "graph",
                    "warning",
                    "partial",
                    "graph_evidence_invalid",
                    vec![text_node.source_ref.ref_id.clone()],
                ));
                return Some(GraphEvidence::Unavailable {
                    reason: "graph_evidence_invalid".into(),
                    source_refs: vec![text_node.source_ref.ref_id.clone()],
                });
            }
        }
        evidence_refs.push(text_node.source_ref.ref_id.clone());
    }
    let complete = topology_complete
        .map(|(complete, _)| complete)
        .unwrap_or(false);
    match explicit_evidence(
        TopologySource::EmbeddedPlan,
        root_node_ids,
        parsed_nodes,
        evidence_refs,
    ) {
        Ok(evidence) if complete => Some(GraphEvidence::ExplicitComplete(evidence)),
        Ok(evidence) => Some(GraphEvidence::Explicit(evidence)),
        Err(()) => Some(GraphEvidence::Unavailable {
            reason: "graph_evidence_invalid".into(),
            source_refs: vec!["text:input".into()],
        }),
    }
}

fn text_graph_node(
    fields: &BTreeMap<String, String>,
    index: usize,
    source_ref: &SourceRef,
) -> Result<ParsedGraphNode, ()> {
    let node_id = required_text_field(fields, &["node_id"])?;
    let entity_kind = optional_text_field(fields, &["entity_kind"])?;
    let plan_node_id = optional_text_field(fields, &["plan_node_id"])?;
    let fragment_id = optional_text_field(fields, &["fragment_id"])?;
    let pipeline_id = optional_text_field(fields, &["pipeline_id"])?;
    let plan_node_type = optional_text_field(fields, &["plan_node_type"])?;
    let display_name = optional_text_field(fields, &["display_name"])?;
    let native_operator_refs = optional_text_list(fields, &["operator_profiles"])?;
    let subordinate_operator_refs = optional_text_list(fields, &["subordinate_operator_profiles"])?;
    let children = optional_text_list(fields, &["children"])?;
    let sink_ids = optional_text_list(fields, &["sink_ids"])?;
    graph_node(
        node_id,
        entity_kind,
        plan_node_id,
        fragment_id,
        pipeline_id,
        plan_node_type,
        display_name,
        native_operator_refs,
        subordinate_operator_refs,
        u32::try_from(index).map_err(|_| ())?,
        source_ref,
    )
    .map(|node| ParsedGraphNode {
        node,
        children,
        sink_ids,
    })
}

fn explicit_evidence(
    topology_source: TopologySource,
    root_node_ids: Vec<String>,
    parsed_nodes: Vec<ParsedGraphNode>,
    source_refs: Vec<String>,
) -> Result<ExplicitGraphEvidence, ()> {
    let mut nodes = Vec::with_capacity(parsed_nodes.len());
    let mut edges = Vec::new();
    for parsed in parsed_nodes {
        let source_ref = parsed.node.source_refs.first().cloned().ok_or(())?;
        let from = parsed.node.node_id.clone();
        // ExplainAnalyzer labels a join's children by order: index 0 PROBE, index 1 BUILD.
        let join = parsed.children.len() == 2
            && parsed
                .node
                .plan_node_type
                .as_deref()
                .is_some_and(|name| name.contains("JOIN"));
        nodes.push(parsed.node);
        for (index, to) in parsed.children.into_iter().enumerate() {
            let mut edge = explicit_edge(
                &from,
                to,
                GraphRelation::PlanChild,
                DirectionSemantics::ParentToChild,
                edges.len(),
                &source_ref,
            )?;
            if join {
                edge.input_role = Some(if index == 0 {
                    doctor_contracts::InputRole::Probe
                } else {
                    doctor_contracts::InputRole::Build
                });
            }
            edges.push(edge);
        }
        for to in parsed.sink_ids {
            edges.push(explicit_edge(
                &from,
                to,
                GraphRelation::FragmentSink,
                DirectionSemantics::UpstreamToDownstream,
                edges.len(),
                &source_ref,
            )?);
        }
    }
    if source_refs.is_empty() {
        return Err(());
    }
    // An exchange's plan_child edge to the sending fragment root and that root's
    // fragment_sink edge back to the exchange state one relation twice; mark the
    // sink edge so no renderer has to infer the duplicate.
    let plan_children = edges
        .iter()
        .filter(|edge| edge.relation == GraphRelation::PlanChild)
        .map(|edge| ((edge.from.clone(), edge.to.clone()), edge.edge_id.clone()))
        .collect::<BTreeMap<_, _>>();
    for edge in &mut edges {
        if edge.relation == GraphRelation::FragmentSink {
            edge.restates_edge_id = plan_children
                .get(&(edge.to.clone(), edge.from.clone()))
                .cloned();
        }
    }
    Ok(ExplicitGraphEvidence {
        topology_source,
        root_node_ids,
        nodes,
        edges,
        unresolved_relations: vec![],
        source_refs,
    })
}

#[allow(clippy::too_many_arguments)]
fn graph_node(
    node_id: String,
    entity_kind: Option<String>,
    plan_node_id: Option<String>,
    fragment_id: Option<String>,
    pipeline_id: Option<String>,
    plan_node_type: Option<String>,
    display_name: Option<String>,
    native_operator_refs: Vec<String>,
    subordinate_operator_refs: Vec<String>,
    source_ordinal: u32,
    source_ref: &SourceRef,
) -> Result<GraphNode, ()> {
    let source_refs = vec![source_ref.ref_id.clone()];
    Ok(GraphNode {
        sql_location: None,
        sql_facts: None,
        node_id,
        entity_kind: graph_entity_kind(entity_kind.as_deref())?,
        plan_node_id,
        fragment_id,
        pipeline_id,
        plan_node_type,
        display_name,
        source_ordinal: Some(source_ordinal),
        native_operator_refs: ordered_operator_refs(
            native_operator_refs,
            SourceRole::Native,
            &source_refs,
        )?,
        subordinate_operator_refs: ordered_operator_refs(
            subordinate_operator_refs,
            SourceRole::Subordinate,
            &source_refs,
        )?,
        metric_observation_refs: vec![],
        execution_state: None,
        source_quality: SourceQuality::Exact,
        source_refs,
    })
}

fn graph_entity_kind(value: Option<&str>) -> Result<GraphEntityKind, ()> {
    match value.unwrap_or("plan_node") {
        "plan_node" => Ok(GraphEntityKind::PlanNode),
        "operator_group" => Ok(GraphEntityKind::OperatorGroup),
        "pipeline" => Ok(GraphEntityKind::Pipeline),
        "fragment" => Ok(GraphEntityKind::Fragment),
        "sink" => Ok(GraphEntityKind::Sink),
        _ => Err(()),
    }
}

fn ordered_operator_refs(
    values: Vec<String>,
    source_role: SourceRole,
    source_refs: &[String],
) -> Result<Vec<OrderedOperatorRef>, ()> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, operator_ref)| {
            Ok(OrderedOperatorRef {
                operator_ref,
                source_ordinal: u32::try_from(index).map_err(|_| ())?,
                source_role: source_role.clone(),
                source_refs: source_refs.to_vec(),
            })
        })
        .collect()
}

fn explicit_edge(
    from: &str,
    to: String,
    relation: GraphRelation,
    direction_semantics: DirectionSemantics,
    index: usize,
    source_ref: &str,
) -> Result<GraphEdge, ()> {
    let source_ordinal = u32::try_from(index).map_err(|_| ())?;
    Ok(GraphEdge {
        edge_id: format!("explicit:{source_ordinal}:{from}"),
        from: from.into(),
        to,
        relation,
        direction_semantics,
        source: EdgeSource::Explicit,
        quality: SourceQuality::Exact,
        source_ordinal: Some(source_ordinal),
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec![source_ref.into()],
    })
}

fn aliased_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Result<Option<&'a Value>, ()> {
    let values = aliases
        .iter()
        .filter_map(|alias| object.get(*alias))
        .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(());
    }
    Ok(values.into_iter().next())
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Result<String, ()> {
    optional_string(object, aliases)?.ok_or(())
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Result<Option<String>, ()> {
    match aliased_value(object, aliases)? {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(_) => Err(()),
        None => Ok(None),
    }
}

fn required_string_list(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Result<Vec<String>, ()> {
    match aliased_value(object, aliases)? {
        Some(value) => json_string_list(value),
        None => Err(()),
    }
}

fn optional_string_list(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Result<Vec<String>, ()> {
    match aliased_value(object, aliases)? {
        Some(value) => json_string_list(value),
        None => Ok(vec![]),
    }
}

fn json_string_list(value: &Value) -> Result<Vec<String>, ()> {
    value
        .as_array()
        .ok_or(())?
        .iter()
        .map(|value| match value {
            Value::String(value) if !value.trim().is_empty() => Ok(value.clone()),
            _ => Err(()),
        })
        .collect()
}

fn parse_text_fields(value: &str) -> Result<BTreeMap<String, String>, ()> {
    value
        .split(';')
        .try_fold(BTreeMap::new(), |mut fields, field| {
            let (key, value) = field.split_once('=').ok_or(())?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty()
                || value.is_empty()
                || fields.insert(key.into(), value.into()).is_some()
            {
                return Err(());
            }
            Ok(fields)
        })
}

fn required_text_field(fields: &BTreeMap<String, String>, aliases: &[&str]) -> Result<String, ()> {
    optional_text_field(fields, aliases)?.ok_or(())
}

fn optional_text_field(
    fields: &BTreeMap<String, String>,
    aliases: &[&str],
) -> Result<Option<String>, ()> {
    let values = aliases
        .iter()
        .filter_map(|alias| fields.get(*alias))
        .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(());
    }
    Ok(values.into_iter().next().cloned())
}

fn optional_text_list(
    fields: &BTreeMap<String, String>,
    aliases: &[&str],
) -> Result<Vec<String>, ()> {
    match optional_text_field(fields, aliases)? {
        Some(value) => split_explicit_list(&value),
        None => Ok(vec![]),
    }
}

fn split_explicit_list(value: &str) -> Result<Vec<String>, ()> {
    value
        .split(',')
        .map(str::trim)
        .map(|value| (!value.is_empty()).then(|| value.to_owned()).ok_or(()))
        .collect()
}

fn graph_evidence_refs(evidence: &GraphEvidence) -> Vec<String> {
    match evidence {
        GraphEvidence::Explicit(input) | GraphEvidence::ExplicitComplete(input) => {
            input.source_refs.clone()
        }
        GraphEvidence::Unavailable { source_refs, .. } => source_refs.clone(),
    }
}

fn nonempty_graph_refs(source_refs: Vec<String>, fallback: &str) -> Vec<String> {
    if source_refs.is_empty() {
        vec![fallback.into()]
    } else {
        source_refs
    }
}

fn unavailable_graph(
    source_refs: Vec<String>,
) -> Result<doctor_contracts::GraphView, AdapterError> {
    let mut graph = graph_view(
        doctor_contracts::GraphState::Unavailable,
        TopologySource::None,
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .map_err(|_| {
        error(
            "internal_error",
            "graph",
            "unavailable_graph_rejected",
            "report_bug",
            vec![],
            vec![],
        )
    })?;
    graph.source_refs = source_refs;
    Ok(graph)
}

fn build_output(
    input: InputEnvelope,
    source_kind: SourceKind,
    adapter_evidence: Vec<String>,
    mut extracted: Extracted,
    context: &AdapterContext,
    cancel: &CancelSignal<'_>,
) -> Result<AdapterOutput, AdapterError> {
    cancel.check()?;
    let has_credible_fact = extracted.version.is_some()
        || extracted
            .metric
            .as_ref()
            .is_some_and(|metric| metric.outcome.parse_state == ParseState::Exact)
        || extracted.runtime.as_ref().is_some_and(|runtime| {
            runtime
                .planning_time_candidates
                .iter()
                .any(runtime_metric_is_exact)
                || runtime.query_metrics.iter().any(runtime_metric_is_exact)
                || runtime
                    .operators
                    .iter()
                    .flat_map(|operator| &operator.metrics)
                    .any(runtime_metric_is_exact)
        });
    if !has_credible_fact {
        return Err(error(
            "parse_failed",
            "parse",
            "no_trusted_facts",
            "change_input",
            extracted.source_refs,
            adapter_evidence,
        ));
    }
    let input_root = input_root_ref(&source_kind, input.content.len(), extracted.line_count)?;
    if !extracted
        .source_refs
        .iter()
        .any(|source_ref| source_ref.ref_id == input_root.ref_id)
    {
        extracted.source_refs.push(input_root.clone());
    }
    let source_hash = sha256_hex(&input.content);
    let input_summary = InputSummary {
        requested_kind: input.requested_kind.clone(),
        detected_kind: source_kind.clone(),
        filename: sanitize_filename(input.filename.as_deref()),
        byte_length: u32::try_from(input.content.len()).map_err(|_| {
            limit_error(
                "input_too_large",
                input.content.len(),
                u32::MAX as usize,
                vec![],
            )
        })?,
        line_count: extracted.line_count,
        source_hash: source_hash.clone(),
        adapter_evidence,
    };
    let (version, mut diagnostics) =
        version_context(extracted.version.as_ref(), input.version_hint.as_deref());
    for diagnostic in &mut diagnostics {
        if diagnostic.source_refs.is_empty() {
            diagnostic.source_refs.push(input_root.ref_id.clone());
        }
    }
    diagnostics.append(&mut extracted.diagnostics);
    let mut graph_evidence =
        extracted
            .graph_evidence
            .take()
            .unwrap_or_else(|| GraphEvidence::Unavailable {
                reason: "graph_unavailable".into(),
                source_refs: vec![input_root.ref_id.clone()],
            });
    let mut graph = match &graph_evidence {
        GraphEvidence::Unavailable {
            reason,
            source_refs,
        } => {
            let source_refs = nonempty_graph_refs(source_refs.clone(), &input_root.ref_id);
            diagnostics.push(diagnostic(
                "graph_unavailable",
                "graph",
                "info",
                "partial",
                reason,
                source_refs.clone(),
            ));
            unavailable_graph(source_refs)?
        }
        _ => {
            let evidence_refs = graph_evidence_refs(&graph_evidence);
            match resolve_graph(graph_evidence.clone()) {
                Ok(graph) => {
                    if graph.state == doctor_contracts::GraphState::Partial {
                        diagnostics.push(diagnostic(
                            "graph_partial",
                            "graph",
                            "info",
                            "partial",
                            "graph_partial",
                            evidence_refs,
                        ));
                    }
                    graph
                }
                Err(_) => {
                    let source_refs = nonempty_graph_refs(evidence_refs, &input_root.ref_id);
                    diagnostics.push(diagnostic(
                        "graph_evidence_conflict",
                        "graph",
                        "warning",
                        "partial",
                        "graph_evidence_conflict",
                        source_refs.clone(),
                    ));
                    graph_evidence = GraphEvidence::Unavailable {
                        reason: "graph_evidence_conflict".into(),
                        source_refs: source_refs.clone(),
                    };
                    unavailable_graph(source_refs)?
                }
            }
        }
    };
    cancel.check()?;
    let runtime = extracted.runtime.take();
    let (pipeline_containers, runtime_driver_occurrences, physical_operator_memberships) = runtime
        .as_ref()
        .map(|runtime| {
            (
                runtime.pipeline_containers.clone(),
                runtime.runtime_driver_occurrences.clone(),
                runtime.physical_operator_memberships.clone(),
            )
        })
        .unwrap_or_default();
    cancel.check()?;
    let operator_info_facts = runtime
        .as_ref()
        .map(|runtime| {
            runtime
                .operators
                .iter()
                .flat_map(|operator| {
                    let facts = |key: &str, facts: &[RuntimeInfoFact]| {
                        facts
                            .iter()
                            .map(|fact| OperatorInfoFact {
                                physical_operator_ref: operator.operator_group_id.clone(),
                                key: key.to_owned(),
                                value: fact.value.clone(),
                                source_refs: vec![fact.source_ref.ref_id.clone()],
                            })
                            .collect::<Vec<_>>()
                    };
                    let mut all = facts("PartType", &operator.partition_type_facts);
                    all.extend(facts("DistributionMode", &operator.distribution_mode_facts));
                    all
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    cancel.check()?;
    let runtime_formatter_shape = runtime
        .as_ref()
        .map(|runtime| runtime.formatter_shape.clone())
        .unwrap_or(RuntimeInventoryFormatterShape::Unknown);
    if runtime.is_none() {
        let subject_evidence = extracted
            .metric
            .as_ref()
            .map(|metric| metric.source_ref.ref_id.clone())
            .unwrap_or_else(|| input_root.ref_id.clone());
        diagnostics.push(diagnostic(
            "synthetic_subject_untyped",
            "normalize",
            "info",
            "partial",
            "synthetic_subject_untyped",
            vec![subject_evidence],
        ));
    }
    let mut profile_context = extracted.profile_facts.into_context();
    if profile_context.evidence_refs.is_empty() {
        profile_context
            .evidence_refs
            .push(input_root.ref_id.clone());
    }
    if runtime.is_none() {
        profile_context
            .limitations
            .push("synthetic_singleton_subject_operator_type_unknown".into());
    }
    for code in [
        "runtime_profile_not_identical",
        "runtime_profile_missing_instances",
        "runtime_operator_running",
        "runtime_profile_incomplete_marker_malformed",
    ] {
        if diagnostics.iter().any(|diagnostic| diagnostic.code == code) {
            profile_context.limitations.push(code.into());
        }
    }
    let (query_summary, derived_metric_cells, metric_cells, metric_observations, operator_groups) =
        match runtime.as_ref() {
            Some(runtime) => runtime_projection(
                runtime,
                matches!(version.version_source, VersionSource::Detected)
                    && version.reported_version.as_ref().is_some_and(|evidence| {
                        matches!(
                            starrocks_version_pack_3_5_14::deltas::resolve_release(&evidence.value),
                            starrocks_version_pack_3_5_14::deltas::ReleaseResolution::Audited(_)
                        )
                    }),
                cancel,
            )?,
            None => {
                let projection = extracted
                    .metric
                    .as_ref()
                    .map(metric_projection)
                    .transpose()?;
                match projection {
                    Some((cell, observation, group)) => {
                        (None, vec![], vec![cell], vec![observation], vec![group])
                    }
                    None => (None, vec![], vec![], vec![], vec![]),
                }
            }
        };
    if runtime.is_some() {
        let mut groups_by_plan_node = BTreeMap::<&str, Vec<&OperatorGroup>>::new();
        for group in &operator_groups {
            if let Some(plan_node_id) = group.plan_node_id.as_deref() {
                groups_by_plan_node
                    .entry(plan_node_id)
                    .or_default()
                    .push(group);
            }
        }
        for node in &mut graph.nodes {
            let Some(plan_node_id) = node.plan_node_id.as_deref() else {
                continue;
            };
            node.metric_observation_refs = groups_by_plan_node
                .get(plan_node_id)
                .into_iter()
                .flatten()
                .flat_map(|group| group.metric_observation_refs.iter().cloned())
                .collect();
        }
    }
    cancel.check()?;
    let runtime_reasons = if runtime.is_some() {
        vec!["runtime_profile_registry_backed_metrics".into()]
    } else {
        vec!["synthetic_adapter_only".into()]
    };
    let completeness = CompletenessReport {
        summary: dimension(
            CompletenessState::Partial,
            u32::from(extracted.version.is_some())
                + u32::from(
                    metric_cells
                        .iter()
                        .any(|cell| cell.state() == doctor_contracts::MetricCellState::Exact),
                ),
            None,
            runtime_reasons.clone(),
        ),
        metrics: dimension(
            CompletenessState::Partial,
            u32::from(
                metric_cells
                    .iter()
                    .any(|cell| cell.state() == doctor_contracts::MetricCellState::Exact),
            ),
            None,
            runtime_reasons,
        ),
        graph: graph.completeness.clone(),
        version: dimension(
            CompletenessState::Partial,
            u32::from(extracted.version.is_some()),
            Some(1),
            vec!["pack_selection_deferred".into()],
        ),
        provenance: dimension(
            CompletenessState::Partial,
            u32::try_from(extracted.source_refs.len()).map_err(|_| {
                error(
                    "internal_error",
                    "normalize",
                    "provenance_count_overflow",
                    "report_bug",
                    vec![],
                    vec![],
                )
            })?,
            None,
            vec!["core_fact_locators_only".into()],
        ),
    };
    let receipt = context.receipt().clone();
    let options = BTreeMap::from([
        (
            "requested_kind".into(),
            requested_kind_name(&input.requested_kind).into(),
        ),
        (
            "version_hint".into(),
            input.version_hint.as_deref().unwrap_or("auto").into(),
        ),
    ]);
    let facts = portable_facts_cancellable(
        &version,
        profile_context.clone(),
        receipt.components.clone(),
        &metric_cells,
        &graph,
        &[],
        &[],
        cancel,
    )?;
    let analysis_key = analysis_key(&source_hash, &receipt, &options);
    drop_polled(metric_cells, cancel)?;
    if let Some(runtime) = runtime {
        drop_polled(runtime.operators, cancel)?;
    }
    Ok(AdapterOutput {
        ir: NormalizedIr {
            schema_version: doctor_contracts::SCHEMA_VERSION.into(),
            analysis_receipt: receipt,
            analysis_key,
            input: input_summary,
            version,
            profile_context,
            completeness,
            query_summary,
            operator_groups,
            metric_observations,
            derived_metric_cells,
            pipeline_containers,
            runtime_driver_occurrences,
            physical_operator_memberships,
            operator_info_facts,
            runtime_acquisition_shape: input.acquisition_shape,
            runtime_formatter_shape,
            graph,
            findings: vec![],
            rule_evaluations: vec![],
            diagnostics,
        },
        graph_evidence,
        analysis_state: AnalysisState::Partial,
        facts,
        source_refs: extracted.source_refs,
        source_view: extracted.source_view,
    })
}

fn runtime_metric_is_exact(metric: &RuntimeMetricFact) -> bool {
    metric.outcome.parse_state == ParseState::Exact
}

type RuntimeProjection = (
    Option<QuerySummary>,
    Vec<MetricCell>,
    Vec<MetricCell>,
    Vec<MetricObservation>,
    Vec<OperatorGroup>,
);

fn runtime_projection(
    runtime: &RuntimeProfileData,
    has_sr3514_recipe_pack: bool,
    cancel: &CancelSignal<'_>,
) -> Result<RuntimeProjection, AdapterError> {
    let mut derived_metric_cells = Vec::with_capacity(runtime.query_metrics.len() + 1);
    let mut all_metric_cells = Vec::new();
    let mut observations = Vec::new();
    let mut groups = Vec::with_capacity(runtime.operators.len());
    let mut group_indexes = BTreeMap::new();
    for metric in &runtime.query_metrics {
        let cell = runtime_metric_cell(metric, "query", None)?;
        let observation = runtime_observation(metric, "query", None, None, None, None);
        derived_metric_cells.push(cell.clone());
        all_metric_cells.push(cell);
        observations.push(observation);
    }
    for operator in &runtime.operators {
        cancel.tick()?;
        let group_id = runtime_group_id(operator);
        let group_index = if let Some(index) = group_indexes.get(&group_id).copied() {
            index
        } else {
            let raw_type = operator
                .plan_node_id
                .as_ref()
                .and_then(|id| runtime.plan_node_types.get(id))
                .cloned()
                .unwrap_or_else(|| operator.canonical_type.clone());
            let index = groups.len();
            groups.push(OperatorGroup {
                operator_group_id: group_id.clone(),
                plan_node_id: operator.plan_node_id.clone(),
                fragment_ref: operator.fragment_ref.clone(),
                pipeline_ref: operator.pipeline_ref.clone(),
                native_operator_refs: Vec::new(),
                subordinate_operator_refs: Vec::new(),
                canonical_type: runtime_semantic_operator_type(&raw_type).into(),
                raw_labels: vec![raw_type],
                metric_observation_refs: Vec::new(),
                source_refs: Vec::new(),
            });
            group_indexes.insert(group_id.clone(), index);
            index
        };
        let member_ordinal = {
            let group = &mut groups[group_index];
            if !group.raw_labels.contains(&operator.canonical_type) {
                group.raw_labels.push(operator.canonical_type.clone());
            }
            if !group.source_refs.contains(&operator.source_ref.ref_id) {
                group.source_refs.push(operator.source_ref.ref_id.clone());
            }
            match &operator.source_role {
                Some(SourceRole::Native) => {
                    let ordinal =
                        u32::try_from(group.native_operator_refs.len()).map_err(|_| {
                            limit_error(
                                "too_many_metrics",
                                group.native_operator_refs.len(),
                                usize::MAX,
                                vec![operator.source_ref.ref_id.clone()],
                            )
                        })?;
                    if operator.plan_node_id.is_some() {
                        group
                            .native_operator_refs
                            .push((operator.operator_group_id.clone(), ordinal));
                    }
                    Some(ordinal)
                }
                Some(SourceRole::Subordinate) => {
                    let ordinal =
                        u32::try_from(group.subordinate_operator_refs.len()).map_err(|_| {
                            limit_error(
                                "too_many_metrics",
                                group.subordinate_operator_refs.len(),
                                usize::MAX,
                                vec![operator.source_ref.ref_id.clone()],
                            )
                        })?;
                    if operator.plan_node_id.is_some() {
                        group
                            .subordinate_operator_refs
                            .push((operator.operator_group_id.clone(), ordinal));
                    }
                    Some(ordinal)
                }
                None => None,
            }
        };
        let mut observation_refs = Vec::with_capacity(operator.metrics.len());
        for metric in &operator.metrics {
            cancel.tick()?;
            let observation = runtime_observation(
                metric,
                &group_id,
                Some(operator.operator_group_id.clone()),
                member_ordinal.map(|_| operator.operator_group_id.clone()),
                member_ordinal,
                runtime_counter_match_key(operator, metric),
            );
            observation_refs.push(observation.observation_id.clone());
            all_metric_cells.push(runtime_metric_cell(
                metric,
                &group_id,
                Some(&operator.operator_group_id),
            )?);
            observations.push(observation);
        }
        groups[group_index]
            .metric_observation_refs
            .extend(observation_refs);
    }

    let (planning_cell, planning_observation) = runtime_planning_projection(runtime)?;
    derived_metric_cells.push(planning_cell.clone());
    all_metric_cells.push(planning_cell);
    observations.push(planning_observation);

    // ExplainAnalyzer computes OutputRows from the source-merged native
    // operator list.  `prettyPrint` can still contain physical PipelineDriver
    // repeats, so a duplicate native label is not treated as a completed merge
    // and remains unavailable rather than being summed or selected.
    let index = OutputRowsIndex::new(&runtime.operators);
    for group in &mut groups {
        cancel.tick()?;
        if group.plan_node_id.is_none() {
            continue;
        }
        let (cell, observation) =
            runtime_output_rows_projection(runtime, &index, group, has_sr3514_recipe_pack);
        if !cell.source_refs.is_empty() {
            for source_ref in &cell.source_refs {
                if !group.source_refs.contains(source_ref) {
                    group.source_refs.push(source_ref.clone());
                }
            }
        }
        group
            .metric_observation_refs
            .push(observation.observation_id.clone());
        all_metric_cells.push(cell);
        observations.push(observation);
    }
    Ok((
        Some(runtime.query_summary.clone()),
        derived_metric_cells,
        all_metric_cells,
        observations,
        groups,
    ))
}

/// Plan-structural identity of one counter: the header tuple the FE prints for
/// a physical operator plus the raw counter name (`__MAX_OF_…` stays distinct
/// from the base value). Occurrence ordinals are never part of it, so an
/// unrelated earlier operator can not shift the pairing. Driver repeats may
/// still collide; the public report then drops the key instead of guessing.
fn runtime_counter_match_key(
    operator: &RuntimeOperator,
    metric: &RuntimeMetricFact,
) -> Option<String> {
    let (Some(fragment), Some(pipeline), Some(plan_node_id)) = (
        operator.fragment_ref.as_deref(),
        operator.pipeline_ref.as_deref(),
        operator.plan_node_id.as_deref(),
    ) else {
        return None;
    };
    let operator_id = operator
        .native_operator_id
        .map_or_else(|| "-".to_owned(), |id| id.to_string());
    Some(format!(
        "{fragment}:{pipeline}:{}:{plan_node_id}:{operator_id}/{}",
        operator.canonical_type, metric.source_name
    ))
}

fn runtime_group_id(operator: &RuntimeOperator) -> String {
    operator.plan_node_id.as_ref().map_or_else(
        || operator.operator_group_id.clone(),
        |plan_node_id| {
            format!(
                "operator-group:{}:{plan_node_id}",
                operator.fragment_ref.as_deref().unwrap_or("unknown")
            )
        },
    )
}

fn runtime_planning_projection(
    runtime: &RuntimeProfileData,
) -> Result<(MetricCell, MetricObservation), AdapterError> {
    match runtime.planning_time_candidates.as_slice() {
        [metric] => Ok((
            runtime_metric_cell(metric, "query", None)?,
            runtime_observation(metric, "query", None, None, None, None),
        )),
        [] => Ok(runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: "query",
            metric_id: "QueryPlanningTime",
            source_name: "Planner/Total",
            source_refs: runtime_fallback_source_refs(runtime),
            reason: "query_planning_time_unavailable",
            definition_ref: Some("sr3514-metric-registry:QueryPlanningTime"),
            scope: "query",
            strategy_origin: "registry",
        })),
        candidates => Ok(runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: "query",
            metric_id: "QueryPlanningTime",
            source_name: "Planner/Total",
            source_refs: candidates
                .iter()
                .map(|candidate| candidate.source_ref.ref_id.clone())
                .collect(),
            reason: "query_planning_time_ambiguous",
            definition_ref: Some("sr3514-metric-registry:QueryPlanningTime"),
            scope: "query",
            strategy_origin: "registry",
        })),
    }
}

/// Operator lookups for the OutputRows recipes, built once per Profile so that
/// each group reads its inputs from a map instead of scanning every operator.
struct OutputRowsIndex<'a> {
    native_by_group: BTreeMap<String, Vec<&'a RuntimeOperator>>,
    native_by_plan_node: BTreeMap<&'a str, Vec<&'a RuntimeOperator>>,
    fragment_containers: BTreeMap<Option<&'a str>, Vec<&'a RuntimeOperator>>,
}

impl<'a> OutputRowsIndex<'a> {
    fn new(operators: &'a [RuntimeOperator]) -> Self {
        let mut index = Self {
            native_by_group: BTreeMap::new(),
            native_by_plan_node: BTreeMap::new(),
            fragment_containers: BTreeMap::new(),
        };
        for operator in operators {
            if operator.canonical_type == "FRAGMENT" {
                index
                    .fragment_containers
                    .entry(operator.fragment_ref.as_deref())
                    .or_default()
                    .push(operator);
            }
            if operator.runtime_source_role != RuntimeSourceRole::Native {
                continue;
            }
            index
                .native_by_group
                .entry(runtime_group_id(operator))
                .or_default()
                .push(operator);
            if let Some(plan_node_id) = operator.plan_node_id.as_deref() {
                index
                    .native_by_plan_node
                    .entry(plan_node_id)
                    .or_default()
                    .push(operator);
            }
        }
        index
    }
}

fn runtime_output_rows_projection(
    runtime: &RuntimeProfileData,
    index: &OutputRowsIndex<'_>,
    group: &OperatorGroup,
    has_sr3514_recipe_pack: bool,
) -> (MetricCell, MetricObservation) {
    let fallback_refs = if group.source_refs.is_empty() {
        runtime_fallback_source_refs(runtime)
    } else {
        group.source_refs.clone()
    };
    if !has_sr3514_recipe_pack {
        return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: &group.operator_group_id,
            metric_id: "OutputRows",
            source_name: "OutputRows",
            source_refs: fallback_refs,
            reason: "output_rows_recipe_unavailable",
            definition_ref: None,
            scope: "operator",
            strategy_origin: "version_pack",
        });
    }
    let native = index
        .native_by_group
        .get(&group.operator_group_id)
        .cloned()
        .unwrap_or_default();
    if native.is_empty() {
        return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: &group.operator_group_id,
            metric_id: "OutputRows",
            source_name: "OutputRows",
            source_refs: fallback_refs,
            reason: "output_rows_native_source_missing",
            definition_ref: None,
            scope: "operator",
            strategy_origin: "version_pack",
        });
    }
    // Official ProfileNodeParser.parse (ExplainAnalyzer.java:1351-1378) makes
    // every pipeline's operator its own operatorProfile, so one label may
    // repeat across pipelines (UNION_PASSTHROUGH per child, INTERSECT_PROBE_SINK
    // per probe). The same label twice in one pipeline, or without pipeline
    // evidence, is an unmerged driver/instance source: never sum or pick it.
    let mut pipelines_by_label: BTreeMap<&str, BTreeSet<Option<&str>>> = BTreeMap::new();
    if native.iter().any(|operator| {
        let pipelines = pipelines_by_label
            .entry(operator.canonical_type.as_str())
            .or_default();
        !pipelines.insert(operator.pipeline_ref.as_deref())
            || (pipelines.len() > 1 && pipelines.contains(&None))
    }) {
        return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: &group.operator_group_id,
            metric_id: "OutputRows",
            source_name: "OutputRows",
            source_refs: native
                .iter()
                .map(|operator| operator.source_ref.ref_id.clone())
                .collect(),
            reason: "output_rows_source_merge_unverified",
            definition_ref: None,
            scope: "operator",
            strategy_origin: "version_pack",
        });
    }

    // ExplainAnalyzer branches on its ProfilingExecPlan node element, not a
    // pipeline operator label. A unique Topology node id/name proves that
    // identity; its graph edges are not recipe inputs. The pseudo final sink
    // has separate direct evidence: plan node -1 plus IsFinalSink.
    let topology_type = group
        .plan_node_id
        .as_deref()
        .filter(|plan_node_id| *plan_node_id != "-1")
        .and_then(|plan_node_id| runtime.plan_node_types.get(plan_node_id))
        .map(String::as_str);
    if group.plan_node_id.as_deref() != Some("-1") && topology_type.is_none() {
        return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
            subject_ref: &group.operator_group_id,
            metric_id: "OutputRows",
            source_name: "OutputRows",
            source_refs: fallback_refs,
            reason: "output_rows_operator_identity_unavailable",
            definition_ref: None,
            scope: "operator",
            strategy_origin: "version_pack",
        });
    }

    let (recipe_id, value, source_refs) = if group.plan_node_id.as_deref() == Some("-1") {
        let final_sinks = native
            .iter()
            .filter(|operator| operator.is_final_sink)
            .copied()
            .collect::<Vec<_>>();
        let Some(operator) = (final_sinks.len() == 1).then(|| final_sinks[0]) else {
            return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                subject_ref: &group.operator_group_id,
                metric_id: "OutputRows",
                source_name: "OutputRows",
                source_refs: native
                    .iter()
                    .map(|operator| operator.source_ref.ref_id.clone())
                    .collect(),
                reason: "output_rows_final_sink_unavailable",
                definition_ref: None,
                scope: "operator",
                strategy_origin: "version_pack",
            });
        };
        let Some(metric) = runtime_single_exact_metric(operator, "PushRowNum") else {
            return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                subject_ref: &group.operator_group_id,
                metric_id: "OutputRows",
                source_name: "OutputRows",
                source_refs: vec![operator.source_ref.ref_id.clone()],
                reason: "output_rows_selected_counter_unavailable",
                definition_ref: None,
                scope: "operator",
                strategy_origin: "version_pack",
            });
        };
        let Some(value) = runtime_exact_integer(metric) else {
            return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                subject_ref: &group.operator_group_id,
                metric_id: "OutputRows",
                source_name: "OutputRows",
                source_refs: vec![metric.source_ref.ref_id.clone()],
                reason: "output_rows_selected_counter_unavailable",
                definition_ref: None,
                scope: "operator",
                strategy_origin: "version_pack",
            });
        };
        (
            "sr3514.output_rows.sink@1",
            value,
            vec![metric.source_ref.ref_id.clone()],
        )
    } else if topology_type == Some("UNION") {
        let mut value = 0_i128;
        let mut source_refs = Vec::new();
        for operator in &native {
            let Some(metric) = runtime_single_exact_metric(operator, "PullRowNum") else {
                return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                    subject_ref: &group.operator_group_id,
                    metric_id: "OutputRows",
                    source_name: "OutputRows",
                    source_refs: native
                        .iter()
                        .map(|operator| operator.source_ref.ref_id.clone())
                        .collect(),
                    reason: "output_rows_selected_counter_unavailable",
                    definition_ref: None,
                    scope: "operator",
                    strategy_origin: "version_pack",
                });
            };
            let Some(input) = runtime_exact_integer(metric) else {
                return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                    subject_ref: &group.operator_group_id,
                    metric_id: "OutputRows",
                    source_name: "OutputRows",
                    source_refs: vec![metric.source_ref.ref_id.clone()],
                    reason: "output_rows_selected_counter_unavailable",
                    definition_ref: None,
                    scope: "operator",
                    strategy_origin: "version_pack",
                });
            };
            let Some(next) = value.checked_add(input) else {
                return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                    subject_ref: &group.operator_group_id,
                    metric_id: "OutputRows",
                    source_name: "OutputRows",
                    source_refs,
                    reason: "output_rows_aggregate_overflow",
                    definition_ref: None,
                    scope: "operator",
                    strategy_origin: "version_pack",
                });
            };
            value = next;
            source_refs.push(metric.source_ref.ref_id.clone());
        }
        ("sr3514.output_rows.union@1", value, source_refs)
    } else {
        let Some(metric) = runtime_selected_output_rows_metric(&native) else {
            return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                subject_ref: &group.operator_group_id,
                metric_id: "OutputRows",
                source_name: "OutputRows",
                source_refs: native
                    .iter()
                    .map(|operator| operator.source_ref.ref_id.clone())
                    .collect(),
                reason: "output_rows_selected_counter_unavailable",
                definition_ref: None,
                scope: "operator",
                strategy_origin: "version_pack",
            });
        };
        let Some(mut value) = runtime_exact_integer(metric) else {
            return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                subject_ref: &group.operator_group_id,
                metric_id: "OutputRows",
                source_name: "OutputRows",
                source_refs: vec![metric.source_ref.ref_id.clone()],
                reason: "output_rows_selected_counter_unavailable",
                definition_ref: None,
                scope: "operator",
                strategy_origin: "version_pack",
            });
        };
        let mut source_refs = vec![metric.source_ref.ref_id.clone()];
        // Official searchInfoString(NATIVE_ONLY, "PartType") reads the merged
        // node, so the sender side's PartType applies to the receiving group.
        let partition_facts = group
            .plan_node_id
            .as_deref()
            .and_then(|plan_node_id| index.native_by_plan_node.get(plan_node_id))
            .into_iter()
            .flatten()
            .flat_map(|operator| operator.partition_type_facts.iter())
            .collect::<Vec<_>>();
        let partitioned_exchange = topology_type == Some("EXCHANGE")
            && partition_facts.len() == 1
            && partition_facts[0]
                .value
                .as_deref()
                .is_some_and(|value| value != "UNPARTITIONED");
        let recipe_id = if topology_type == Some("EXCHANGE") && !partitioned_exchange {
            let Some(partition) = (partition_facts.len() == 1
                && partition_facts[0].value.as_deref() == Some("UNPARTITIONED"))
            .then(|| partition_facts[0]) else {
                return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                    subject_ref: &group.operator_group_id,
                    metric_id: "OutputRows",
                    source_name: "OutputRows",
                    source_refs: partition_facts
                        .iter()
                        .map(|fact| fact.source_ref.ref_id.clone())
                        .chain(source_refs)
                        .collect(),
                    reason: "output_rows_broadcast_condition_unavailable",
                    definition_ref: None,
                    scope: "operator",
                    strategy_origin: "version_pack",
                });
            };
            let fragment_metrics = index
                .fragment_containers
                .get(&group.fragment_ref.as_deref())
                .into_iter()
                .flatten()
                .flat_map(|operator| {
                    operator
                        .metrics
                        .iter()
                        .filter(|metric| metric.metric_id == "InstanceNum")
                })
                .collect::<Vec<_>>();
            let Some(instance) = (fragment_metrics.len() == 1)
                .then(|| fragment_metrics[0])
                .and_then(runtime_exact_integer)
                .filter(|value| *value > 0)
            else {
                return runtime_unavailable_metric(RuntimeUnavailableMetricInput {
                    subject_ref: &group.operator_group_id,
                    metric_id: "OutputRows",
                    source_name: "OutputRows",
                    source_refs: vec![partition.source_ref.ref_id.clone()],
                    reason: "output_rows_broadcast_condition_unavailable",
                    definition_ref: None,
                    scope: "operator",
                    strategy_origin: "version_pack",
                });
            };
            source_refs.push(partition.source_ref.ref_id.clone());
            let instance_ref = fragment_metrics[0].source_ref.ref_id.clone();
            source_refs.push(instance_ref);
            value /= instance;
            "sr3514.output_rows.broadcast_exchange@1"
        } else {
            if partitioned_exchange {
                source_refs.push(partition_facts[0].source_ref.ref_id.clone());
            }
            "sr3514.output_rows.general@1"
        };
        (recipe_id, value, source_refs)
    };
    runtime_derived_count_metric(RuntimeDerivedCountMetricInput {
        subject_ref: &group.operator_group_id,
        metric_id: "OutputRows",
        source_name: "OutputRows",
        value,
        source_refs,
        definition_ref: recipe_id,
        scope: "operator",
        strategy_origin: "version_pack",
    })
}

fn runtime_fallback_source_refs(runtime: &RuntimeProfileData) -> Vec<String> {
    runtime
        .query_metrics
        .first()
        .map(|metric| vec![metric.source_ref.ref_id.clone()])
        .or_else(|| {
            runtime
                .operators
                .first()
                .map(|operator| vec![operator.source_ref.ref_id.clone()])
        })
        .unwrap_or_default()
}

fn runtime_single_exact_metric<'a>(
    operator: &'a RuntimeOperator,
    metric_id: &str,
) -> Option<&'a RuntimeMetricFact> {
    // ExplainAnalyzer reads the merged base counter (getCounter(name)), never
    // its __MIN_OF_/__MAX_OF_ companions.
    let matches = operator
        .metrics
        .iter()
        .filter(|metric| metric.metric_id == metric_id && metric.value_role == "reported_base")
        .collect::<Vec<_>>();
    (matches.len() == 1 && runtime_metric_is_exact(matches[0])).then(|| matches[0])
}

fn runtime_selected_output_rows_metric<'a>(
    native: &[&'a RuntimeOperator],
) -> Option<&'a RuntimeMetricFact> {
    let operator = native.iter().enumerate().find_map(|(index, operator)| {
        let skip = index + 1 < native.len()
            && (operator.canonical_type.contains("_SINK")
                || operator.canonical_type.contains("_BUILD"));
        (!skip).then_some(*operator)
    })?;
    runtime_single_exact_metric(operator, "PullRowNum")
}

fn runtime_exact_integer(metric: &RuntimeMetricFact) -> Option<i128> {
    (metric.outcome.parse_state == ParseState::Exact
        && metric.outcome.canonical_unit.as_deref() == Some("item")
        && metric
            .outcome
            .canonical_number
            .as_ref()
            .is_some_and(|number| number.kind == NumberKind::Integer && number.scale.is_none()))
    .then(|| {
        metric
            .outcome
            .canonical_number
            .as_ref()
            .and_then(|number| number.value.parse::<i128>().ok())
    })
    .flatten()
}

struct RuntimeDerivedCountMetricInput<'a> {
    subject_ref: &'a str,
    metric_id: &'a str,
    source_name: &'a str,
    value: i128,
    source_refs: Vec<String>,
    definition_ref: &'a str,
    scope: &'a str,
    strategy_origin: &'a str,
}

fn runtime_derived_count_metric(
    input: RuntimeDerivedCountMetricInput<'_>,
) -> (MetricCell, MetricObservation) {
    let RuntimeDerivedCountMetricInput {
        subject_ref,
        metric_id,
        source_name,
        value,
        source_refs,
        definition_ref,
        scope,
        strategy_origin,
    } = input;
    let number = CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.to_string(),
        scale: None,
    };
    let observation_id = format!("observation:{subject_ref}:derived:{metric_id}");
    (
        MetricCell {
            match_key: None,
            metric_cell_ref: format!("metric:{subject_ref}:derived:{metric_id}"),
            metric_id: metric_id.into(),
            subject_ref: subject_ref.into(),
            display_text: value.to_string(),
            order_rank: None,
            source_refs: source_refs.clone(),
            reasons: vec![],
            value: MetricCellValue::Exact {
                canonical_number: number.clone(),
                canonical_unit: "item".into(),
                observation: ObservedMetricFidelity::Lossless {
                    fidelity: LosslessFidelity::Exact,
                    bounds: (),
                },
            },
        },
        MetricObservation {
            observation_id,
            metric_id: Some(metric_id.into()),
            source_name: source_name.into(),
            subject_ref: Some(subject_ref.into()),
            operator_ref: None,
            runtime_occurrence_ref: None,
            match_key: None,
            dimension: "count".into(),
            parse_state: ParseState::Exact,
            canonical_number: Some(number),
            canonical_unit: Some("item".into()),
            raw_text: None,
            source_refs,
            consumed_ranges: vec![],
            unconsumed_ranges: vec![],
            fidelity: Fidelity::Exact,
            bounds: None,
            semantic_state: "known".into(),
            definition_ref: Some(definition_ref.into()),
            scope: scope.into(),
            value_role: "derived".into(),
            strategy_origin: strategy_origin.into(),
            counter_strategy: None,
            source_merge_state: "merged".into(),
            display_value: value.to_string(),
            source_ordinal: None,
            diagnostics: vec![],
        },
    )
}

struct RuntimeUnavailableMetricInput<'a> {
    subject_ref: &'a str,
    metric_id: &'a str,
    source_name: &'a str,
    source_refs: Vec<String>,
    reason: &'a str,
    definition_ref: Option<&'a str>,
    scope: &'a str,
    strategy_origin: &'a str,
}

fn runtime_unavailable_metric(
    input: RuntimeUnavailableMetricInput<'_>,
) -> (MetricCell, MetricObservation) {
    let RuntimeUnavailableMetricInput {
        subject_ref,
        metric_id,
        source_name,
        source_refs,
        reason,
        definition_ref,
        scope,
        strategy_origin,
    } = input;
    let observation_id = format!("observation:{subject_ref}:derived:{metric_id}");
    let diagnostic = diagnostic(
        reason,
        "normalize",
        "info",
        "partial",
        reason,
        source_refs.clone(),
    );
    (
        MetricCell {
            match_key: None,
            metric_cell_ref: format!("metric:{subject_ref}:derived:{metric_id}"),
            metric_id: metric_id.into(),
            subject_ref: subject_ref.into(),
            display_text: "—".into(),
            order_rank: None,
            source_refs: source_refs.clone(),
            reasons: vec![reason.into()],
            value: MetricCellValue::Unavailable {
                canonical_number: (),
                canonical_unit: (),
                fidelity: (),
                bounds: (),
            },
        },
        MetricObservation {
            observation_id,
            metric_id: Some(metric_id.into()),
            source_name: source_name.into(),
            subject_ref: Some(subject_ref.into()),
            operator_ref: None,
            runtime_occurrence_ref: None,
            match_key: None,
            dimension: if metric_id == "OutputRows" {
                "count"
            } else {
                "duration"
            }
            .into(),
            parse_state: ParseState::Missing,
            canonical_number: None,
            canonical_unit: None,
            raw_text: None,
            source_refs,
            consumed_ranges: vec![],
            unconsumed_ranges: vec![],
            fidelity: Fidelity::Unknown,
            bounds: None,
            semantic_state: "unavailable".into(),
            definition_ref: definition_ref.map(str::to_owned),
            scope: scope.into(),
            value_role: "derived".into(),
            strategy_origin: strategy_origin.into(),
            counter_strategy: None,
            source_merge_state: "unknown".into(),
            display_value: "—".into(),
            source_ordinal: None,
            diagnostics: vec![diagnostic],
        },
    )
}

fn runtime_metric_cell(
    metric: &RuntimeMetricFact,
    subject_ref: &str,
    operator_ref: Option<&str>,
) -> Result<MetricCell, AdapterError> {
    let source_ref = metric.source_ref.ref_id.clone();
    Ok(MetricCell {
        match_key: None,
        metric_cell_ref: format!(
            "metric:{subject_ref}:{}:{}:{}",
            operator_ref.unwrap_or("query"),
            metric.metric_id,
            metric.source_ordinal
        ),
        metric_id: metric.metric_id.clone(),
        subject_ref: subject_ref.into(),
        display_text: metric.display_text.clone(),
        order_rank: metric.source_ordinal.checked_add(1),
        source_refs: vec![source_ref],
        reasons: metric
            .outcome
            .error_code
            .map(|code| vec![code.as_str().into()])
            .unwrap_or_default(),
        value: metric_cell_value(&metric.outcome, &metric.source_ref)?,
    })
}

fn runtime_observation(
    metric: &RuntimeMetricFact,
    subject_ref: &str,
    runtime_occurrence_ref: Option<String>,
    operator_ref: Option<String>,
    operator_ordinal: Option<u32>,
    match_key: Option<String>,
) -> MetricObservation {
    let source_ref = metric.source_ref.ref_id.clone();
    MetricObservation {
        observation_id: format!(
            "observation:{subject_ref}:{}:{}:{}",
            runtime_occurrence_ref.as_deref().unwrap_or("query"),
            metric.metric_id,
            metric.source_ordinal
        ),
        metric_id: Some(metric.metric_id.clone()),
        source_name: metric.source_name.clone(),
        subject_ref: Some(subject_ref.into()),
        operator_ref,
        runtime_occurrence_ref,
        match_key,
        dimension: match crate::metrics::sr3514_registered_metric(
            SourceKind::TextProfile,
            &metric.metric_id,
        ) {
            Some(definition) => match definition.dimension {
                crate::MetricDimension::Duration => "duration",
                crate::MetricDimension::Count => "count",
                crate::MetricDimension::Bytes => "bytes",
                crate::MetricDimension::Ratio => "ratio",
            }
            .into(),
            None => "unknown".into(),
        },
        parse_state: metric.outcome.parse_state.clone(),
        canonical_number: metric.outcome.canonical_number.clone(),
        canonical_unit: metric.outcome.canonical_unit.clone(),
        raw_text: None,
        source_refs: vec![source_ref.clone()],
        consumed_ranges: metric
            .outcome
            .consumed_ranges
            .iter()
            .map(|range| format!("{source_ref}#{}-{}", range.start, range.end))
            .collect(),
        unconsumed_ranges: metric
            .outcome
            .unconsumed_ranges
            .iter()
            .map(|range| format!("{source_ref}#{}-{}", range.start, range.end))
            .collect(),
        fidelity: metric.outcome.fidelity.clone().unwrap_or(Fidelity::Unknown),
        bounds: metric.outcome.bounds.clone(),
        semantic_state: "known".into(),
        definition_ref: Some(format!("sr3514-metric-registry:{}", metric.metric_id)),
        scope: metric.scope.clone(),
        value_role: metric.value_role.clone(),
        strategy_origin: "registry".into(),
        counter_strategy: None,
        source_merge_state: "unknown".into(),
        display_value: metric.display_text.clone(),
        source_ordinal: Some(operator_ordinal.unwrap_or(metric.source_ordinal)),
        diagnostics: (metric.outcome.parse_state != ParseState::Exact)
            .then(|| metric_diagnostic(&metric.metric_id, &metric.outcome, &metric.source_ref))
            .into_iter()
            .collect(),
    }
}

fn metric_projection(
    metric: &MetricFact,
) -> Result<(MetricCell, MetricObservation, OperatorGroup), AdapterError> {
    let source_ref = metric.source_ref.ref_id.clone();
    let value = metric_cell_value(&metric.outcome, &metric.source_ref)?;
    let cell = MetricCell {
        match_key: None,
        metric_cell_ref: "metric:operator-0:OperatorTotalTime".into(),
        metric_id: "OperatorTotalTime".into(),
        subject_ref: "operator-0".into(),
        display_text: metric.display_text.clone(),
        order_rank: (metric.outcome.parse_state == ParseState::Exact).then_some(1),
        source_refs: vec![source_ref.clone()],
        reasons: metric
            .outcome
            .error_code
            .map(|code| vec![code.as_str().into()])
            .unwrap_or_default(),
        value,
    };
    let observation = MetricObservation {
        observation_id: "observation:operator-0:OperatorTotalTime".into(),
        metric_id: Some("OperatorTotalTime".into()),
        source_name: "operator_total_time_ns".into(),
        subject_ref: Some("operator-0".into()),
        operator_ref: None,
        runtime_occurrence_ref: None,
        match_key: None,
        dimension: "duration".into(),
        parse_state: metric.outcome.parse_state.clone(),
        canonical_number: metric.outcome.canonical_number.clone(),
        canonical_unit: metric.outcome.canonical_unit.clone(),
        raw_text: None,
        source_refs: vec![source_ref.clone()],
        consumed_ranges: metric
            .outcome
            .consumed_ranges
            .iter()
            .map(|range| format!("{source_ref}#{}-{}", range.start, range.end))
            .collect(),
        unconsumed_ranges: metric
            .outcome
            .unconsumed_ranges
            .iter()
            .map(|range| format!("{source_ref}#{}-{}", range.start, range.end))
            .collect(),
        fidelity: metric.outcome.fidelity.clone().unwrap_or(Fidelity::Unknown),
        bounds: metric.outcome.bounds.clone(),
        semantic_state: "known".into(),
        definition_ref: Some("sr3514-metric-registry:OperatorTotalTime".into()),
        scope: "operator".into(),
        value_role: "reported_base".into(),
        strategy_origin: "registry".into(),
        counter_strategy: None,
        source_merge_state: "unknown".into(),
        display_value: metric.display_text.clone(),
        source_ordinal: Some(0),
        diagnostics: (metric.outcome.parse_state != ParseState::Exact)
            .then(|| metric_diagnostic("OperatorTotalTime", &metric.outcome, &metric.source_ref))
            .into_iter()
            .collect(),
    };
    let group = OperatorGroup {
        operator_group_id: "operator-0".into(),
        plan_node_id: None,
        fragment_ref: None,
        pipeline_ref: None,
        native_operator_refs: vec![],
        subordinate_operator_refs: vec![],
        canonical_type: "UNKNOWN".into(),
        raw_labels: vec![],
        metric_observation_refs: vec![observation.observation_id.clone()],
        source_refs: vec![source_ref.clone()],
    };
    Ok((cell, observation, group))
}

fn metric_cell_value(
    outcome: &MetricParseOutcome,
    source_ref: &SourceRef,
) -> Result<MetricCellValue, AdapterError> {
    Ok(match outcome.parse_state {
        ParseState::Exact => MetricCellValue::Exact {
            canonical_number: outcome.canonical_number.clone().ok_or_else(|| {
                error(
                    "internal_error",
                    "normalize",
                    "metric_number_missing",
                    "report_bug",
                    vec![source_ref.clone()],
                    vec![],
                )
            })?,
            canonical_unit: outcome.canonical_unit.clone().ok_or_else(|| {
                error(
                    "internal_error",
                    "normalize",
                    "metric_unit_missing",
                    "report_bug",
                    vec![source_ref.clone()],
                    vec![],
                )
            })?,
            observation: match (
                outcome.fidelity.clone().unwrap_or(Fidelity::Unknown),
                outcome.bounds.clone(),
            ) {
                (Fidelity::Exact, None) => ObservedMetricFidelity::Lossless {
                    fidelity: LosslessFidelity::Exact,
                    bounds: (),
                },
                (Fidelity::Rounded, Some(bounds)) => ObservedMetricFidelity::Bounded {
                    fidelity: BoundedFidelity::Rounded,
                    bounds,
                },
                (Fidelity::Truncated, Some(bounds)) => ObservedMetricFidelity::Bounded {
                    fidelity: BoundedFidelity::Truncated,
                    bounds,
                },
                (Fidelity::Interval, Some(bounds)) => ObservedMetricFidelity::Bounded {
                    fidelity: BoundedFidelity::Interval,
                    bounds,
                },
                _ => ObservedMetricFidelity::Unknown {
                    fidelity: UnknownFidelity::Unknown,
                    bounds: (),
                },
            },
        },
        ParseState::Missing => MetricCellValue::Missing {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        ParseState::Invalid => MetricCellValue::Invalid {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        ParseState::Partial => MetricCellValue::Partial {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        ParseState::Overflow => MetricCellValue::Overflow {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
    })
}

fn version_context(
    version: Option<&VersionFact>,
    version_hint: Option<&str>,
) -> (VersionContext, Vec<Diagnostic>) {
    let user_declared_version = version_hint
        .filter(|value| *value != "auto")
        .map(str::to_owned);
    match version {
        Some(version) => {
            let evidence = VersionEvidence {
                value: version.value.clone(),
                source_ref: version.source_ref.ref_id.clone(),
            };
            (
                VersionContext {
                    reported_version: Some(evidence.clone()),
                    user_declared_version,
                    effective_version: None,
                    version_source: VersionSource::Detected,
                    selected_pack: None,
                    pack_match: PackMatch::NoPack,
                    compatibility: doctor_contracts::Compatibility::Inconclusive,
                    support_status: SupportStatus::Unknown,
                    detection_evidence: vec![evidence],
                    compatibility_checks: vec![],
                    issues: vec!["pack_selection_deferred".into()],
                },
                vec![diagnostic(
                    "version_selection_deferred",
                    "normalize",
                    "info",
                    "partial",
                    "version_selection_deferred",
                    vec![version.source_ref.ref_id.clone()],
                )],
            )
        }
        None => (
            VersionContext {
                reported_version: None,
                user_declared_version: user_declared_version.clone(),
                effective_version: None,
                version_source: if user_declared_version.is_some() {
                    VersionSource::UserDeclared
                } else {
                    VersionSource::Unknown
                },
                selected_pack: None,
                pack_match: PackMatch::NoPack,
                compatibility: doctor_contracts::Compatibility::Inconclusive,
                support_status: SupportStatus::Unknown,
                detection_evidence: vec![],
                compatibility_checks: vec![],
                issues: vec!["reported_version_missing".into()],
            },
            vec![diagnostic(
                "reported_version_missing",
                "parse",
                "warning",
                "partial",
                "reported_version_missing",
                vec![],
            )],
        ),
    }
}

fn check_bytes(bytes: usize, limits: &AdapterLimits) -> Result<(), AdapterError> {
    if bytes > limits.max_bytes || bytes > u32::MAX as usize {
        return Err(limit_error(
            "input_too_large",
            bytes,
            limits.max_bytes,
            vec![],
        ));
    }
    Ok(())
}

fn check_text_budget(
    content: &[u8],
    limits: &AdapterLimits,
    evidence: &[String],
) -> Result<usize, AdapterError> {
    let mut lines = 0usize;
    let mut start = 0usize;
    for (index, byte) in content.iter().enumerate() {
        if *byte == b'\n' {
            lines += 1;
            let length = index - start - usize::from(index > start && content[index - 1] == b'\r');
            if length > limits.max_text_line_bytes
                && !is_profile_topology_line(&content[start..start + length])
            {
                return Err(limit_error(
                    "line_too_long",
                    length,
                    limits.max_text_line_bytes,
                    evidence.to_vec(),
                ));
            }
            if lines > limits.max_text_lines {
                return Err(limit_error(
                    "too_many_lines",
                    lines,
                    limits.max_text_lines,
                    evidence.to_vec(),
                ));
            }
            start = index + 1;
        }
    }
    if start < content.len() {
        lines += 1;
        let length = content.len() - start - usize::from(content.last() == Some(&b'\r'));
        if length > limits.max_text_line_bytes
            && !is_profile_topology_line(&content[start..start + length])
        {
            return Err(limit_error(
                "line_too_long",
                length,
                limits.max_text_line_bytes,
                evidence.to_vec(),
            ));
        }
        if lines > limits.max_text_lines {
            return Err(limit_error(
                "too_many_lines",
                lines,
                limits.max_text_lines,
                evidence.to_vec(),
            ));
        }
    }
    Ok(lines)
}

/// The FE `Topology` info line (`- Topology: {...}`) grows about 83 bytes per
/// plan node, so the per-line limit alone would reject a plan of about 3,100
/// nodes. Product decision (v0.6): this line is exempt from
/// `max_text_line_bytes`; it still counts toward `max_bytes` and
/// `max_text_lines`, and its JSON is parsed under serde_json's recursion limit.
pub fn is_profile_topology_line(line: &[u8]) -> bool {
    line.trim_ascii_start().starts_with(b"- Topology: ")
}

fn check_json_budget(
    content: &[u8],
    limits: &AdapterLimits,
    evidence: &[String],
) -> Result<(), AdapterError> {
    let mut depth = 0usize;
    let mut nodes = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in content {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'\"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                nodes += 1;
                if depth > limits.max_json_depth {
                    return Err(limit_error(
                        "too_deep",
                        depth,
                        limits.max_json_depth,
                        evidence.to_vec(),
                    ));
                }
                if nodes > limits.max_json_nodes {
                    return Err(limit_error(
                        "too_many_nodes",
                        nodes,
                        limits.max_json_nodes,
                        evidence.to_vec(),
                    ));
                }
            }
            b':' | b',' => {
                nodes += 1;
                if nodes > limits.max_json_nodes {
                    return Err(limit_error(
                        "too_many_nodes",
                        nodes,
                        limits.max_json_nodes,
                        evidence.to_vec(),
                    ));
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn check_metric_budget(
    count: usize,
    limits: &AdapterLimits,
    evidence: &[String],
) -> Result<(), AdapterError> {
    if count > limits.max_normalized_metrics {
        return Err(limit_error(
            "too_many_metrics",
            count,
            limits.max_normalized_metrics,
            evidence.to_vec(),
        ));
    }
    Ok(())
}

fn detect_kind(content: &[u8]) -> Result<SourceKind, AdapterError> {
    match content
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
    {
        Some(b'{' | b'[') => Ok(SourceKind::JsonProfile),
        Some(_) => Ok(SourceKind::TextProfile),
        None => Err(error(
            "parse_failed",
            "detect",
            "input_empty",
            "change_input",
            vec![],
            vec![],
        )),
    }
}

fn json_ref(content: &[u8], key: &str) -> Result<SourceRef, AdapterError> {
    let needle = format!("\"{key}\"");
    let positions = content
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle.as_bytes()).then_some(index))
        .collect::<Vec<_>>();
    let Some(key_start) = positions.first().copied().filter(|_| positions.len() == 1) else {
        return Err(error(
            "parse_failed",
            "parse",
            "json_locator_ambiguous",
            "change_input",
            vec![],
            vec![],
        ));
    };
    let value_range = json_value_range(content, key_start + needle.len()).ok_or_else(|| {
        error(
            "parse_failed",
            "parse",
            "json_value_locator_missing",
            "change_input",
            vec![],
            vec![],
        )
    })?;
    let byte_start = u32::try_from(value_range.start).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "json_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    let byte_end = u32::try_from(value_range.end).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "json_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    Ok(SourceRef {
        ref_id: format!("json:/{key}"),
        source_kind: SourceKind::JsonProfile,
        locator: SourceLocator::Json {
            json_pointer: format!("/{key}"),
            byte_start,
            byte_end,
        },
        redaction_state: RedactionState::OriginalLocal,
    })
}

fn opaque_json_ref(content: &[u8], key: &str, ordinal: usize) -> Result<SourceRef, AdapterError> {
    let mut source_ref = json_ref(content, key)?;
    let pointer = format!("/unknown_field/{ordinal}");
    source_ref.ref_id = format!("json:{pointer}");
    if let SourceLocator::Json { json_pointer, .. } = &mut source_ref.locator {
        *json_pointer = pointer;
    }
    source_ref.redaction_state = RedactionState::Redacted;
    Ok(source_ref)
}

fn json_value_range(content: &[u8], after_key: usize) -> Option<std::ops::Range<usize>> {
    let colon = content[after_key..].iter().position(|byte| *byte == b':')? + after_key;
    let start = content[colon + 1..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())?
        + colon
        + 1;
    if content.get(start) == Some(&b'\"') {
        let mut cursor = start + 1;
        let mut escaped = false;
        while let Some(byte) = content.get(cursor) {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\"' {
                return Some(start..cursor + 1);
            }
            cursor += 1;
        }
        return None;
    }
    let end = content[start..]
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || matches!(*byte, b',' | b'}' | b']'))
        .map(|offset| start + offset)
        .unwrap_or(content.len());
    (end > start).then_some(start..end)
}

fn json_number_token(content: &[u8], source_ref: &SourceRef) -> Option<String> {
    let SourceLocator::Json {
        byte_start,
        byte_end,
        ..
    } = &source_ref.locator
    else {
        return None;
    };
    let token = std::str::from_utf8(content.get(*byte_start as usize..*byte_end as usize)?).ok()?;
    (!token.starts_with('"')).then_some(token.to_owned())
}

fn text_ref(line_no: u32, byte_start: usize, byte_end: usize) -> Result<SourceRef, AdapterError> {
    let byte_start = u32::try_from(byte_start).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "text_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    let byte_end = u32::try_from(byte_end).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "text_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    Ok(SourceRef {
        ref_id: format!("text:{line_no}"),
        source_kind: SourceKind::TextProfile,
        locator: SourceLocator::Text {
            line_start: line_no,
            line_end: line_no,
            byte_start,
            byte_end,
        },
        redaction_state: RedactionState::OriginalLocal,
    })
}

fn input_root_ref(
    source_kind: &SourceKind,
    byte_length: usize,
    line_count: Option<u32>,
) -> Result<SourceRef, AdapterError> {
    let byte_end = u32::try_from(byte_length).map_err(|_| {
        error(
            "input_too_large",
            "limits",
            "root_locator_out_of_range",
            "reduce_input",
            vec![],
            vec![],
        )
    })?;
    Ok(match source_kind {
        SourceKind::JsonProfile => SourceRef {
            ref_id: "json:".into(),
            source_kind: SourceKind::JsonProfile,
            locator: SourceLocator::Json {
                json_pointer: "".into(),
                byte_start: 0,
                byte_end,
            },
            redaction_state: RedactionState::Redacted,
        },
        SourceKind::TextProfile => SourceRef {
            ref_id: "text:input".into(),
            source_kind: SourceKind::TextProfile,
            locator: SourceLocator::Text {
                line_start: 1,
                line_end: line_count.unwrap_or(1).max(1),
                byte_start: 0,
                byte_end,
            },
            redaction_state: RedactionState::Redacted,
        },
    })
}

fn sanitize_filename(filename: Option<&str>) -> Option<String> {
    filename.and_then(|filename| {
        let basename = filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_control())
            .take(255)
            .collect::<String>();
        (!basename.is_empty() && basename != "." && basename != "..").then_some(basename)
    })
}

fn is_version(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()), (Some(a), Some(b), Some(c), None) if !a.is_empty() && !b.is_empty() && !c.is_empty() && a.bytes().chain(b.bytes()).chain(c.bytes()).all(|byte| byte.is_ascii_digit()))
}

fn requested_kind_name(kind: &RequestedSourceKind) -> &'static str {
    match kind {
        RequestedSourceKind::Auto => "auto",
        RequestedSourceKind::JsonProfile => "json_profile",
        RequestedSourceKind::TextProfile => "text_profile",
    }
}

fn sha256_hex(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn dimension(
    state: CompletenessState,
    present_count: u32,
    expected_count: Option<u32>,
    reasons: Vec<String>,
) -> DimensionCompleteness {
    DimensionCompleteness {
        state,
        present_count,
        expected_count,
        reasons,
    }
}

fn metric_diagnostic(
    metric_id: &str,
    outcome: &MetricParseOutcome,
    source_ref: &SourceRef,
) -> Diagnostic {
    let code = outcome
        .error_code
        .map(|code| code.as_str())
        .unwrap_or("metric_partial");
    let mut diagnostic = diagnostic(
        code,
        "parse",
        "warning",
        "partial",
        "metric_value_unavailable",
        vec![source_ref.ref_id.clone()],
    );
    diagnostic.affected_ids = vec![metric_id.into()];
    diagnostic
}

fn diagnostic(
    code: &str,
    stage: &str,
    severity: &str,
    recoverability: &str,
    message_id: &str,
    source_refs: Vec<String>,
) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        stage: stage.into(),
        severity: severity.into(),
        recoverability: recoverability.into(),
        message_id: message_id.into(),
        source_refs,
        affected_ids: vec![],
        details: BTreeMap::new(),
    }
}

fn limit_error(code: &str, actual: usize, limit: usize, evidence: Vec<String>) -> AdapterError {
    let mut details = BTreeMap::new();
    details.insert("actual".into(), actual.to_string());
    details.insert("limit".into(), limit.to_string());
    AdapterError {
        code: code.into(),
        stage: "limits".into(),
        message_id: code.into(),
        recoverability: "reduce_input".into(),
        source_refs: vec![],
        adapter_evidence: evidence,
        details,
    }
}

fn error(
    code: &str,
    stage: &str,
    message_id: &str,
    recoverability: &str,
    source_refs: Vec<SourceRef>,
    adapter_evidence: Vec<String>,
) -> AdapterError {
    AdapterError {
        code: code.into(),
        stage: stage.into(),
        message_id: message_id.into(),
        recoverability: recoverability.into(),
        source_refs,
        adapter_evidence,
        details: BTreeMap::new(),
    }
}
