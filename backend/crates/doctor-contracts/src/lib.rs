//! Public, deterministic wire contracts for the offline v0.1 candidate.
//! Internal normalization types deliberately live in `doctor-core`.

use std::{collections::BTreeMap, io};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ts_rs::{TS, TypeVisitor};

mod audit_workload;
mod canonical;
pub use audit_workload::*;
mod audit_presentation;
pub use audit_presentation::*;
mod audit_family_detail;
mod audit_fingerprint;
mod audit_performance;
mod audit_similar;
mod governance;
mod history;
mod history_load;
mod history_loads;
mod history_trend;
mod load_alerts;
mod loads;
mod notify;
pub use audit_family_detail::*;
pub use audit_fingerprint::*;
pub use audit_performance::*;
pub use audit_similar::*;
pub use governance::*;
pub use history::*;
pub use history_load::*;
pub use history_loads::*;
pub use history_trend::*;
pub use load_alerts::*;
pub use loads::*;
pub use notify::*;
mod runtime_inventory;
pub use runtime_inventory::*;
mod official_profile;
pub use official_profile::*;
mod execution_summary;
pub use execution_summary::*;
mod execution_shape;
pub use execution_shape::*;

/// The schema carried by `AnalysisSnapshotReport`, `AnalysisAttempt`, and
/// `Capabilities`. Comparison has an independently versioned public shape.
pub const SCHEMA_VERSION: &str = "2.0";
pub const COMPARISON_SCHEMA_VERSION: &str = "2.0";
/// Connected-only DTOs are additive and do not revise the frozen analysis or
/// comparison schemas.
pub const CONNECTED_SCHEMA_VERSION: &str = "0.1";
/// Export DTOs are versioned independently from the analysis and comparison
/// snapshots.  The exporter does not own report assembly or redaction; those
/// remain implementation concerns of `doctor-export`.
pub const EXPORT_SCHEMA_VERSION: &str = "0.1";
pub const EXPORT_MEDIA_TYPE: &str = "application/vnd.sr-doctor.report+json;version=0.1";
/// Explain snapshots are independently versioned so a connected-contract
/// export cannot silently rewrite this key as its outer schema version.
pub const EXPLAIN_SCHEMA_VERSION: &str = "0.4";

macro_rules! wire_type {
    ($($item:item)*) => {$($item)*};
}

wire_type! {
    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum AnalysisState { Complete, Partial }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ParseState { Exact, Missing, Invalid, Partial, Overflow }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum MetricCellState { Exact, Missing, Invalid, Partial, Overflow, Unavailable, Incompatible, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum Fidelity { Exact, Rounded, Truncated, Interval, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum NumberKind { Integer, Decimal }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum GraphState { Complete, Partial, Unavailable }





    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ComparisonState { Comparable, PartiallyComparable, NotComparable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum MetricDeltaState { Comparable, NotComparable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum PercentageState {
        Computed,
        UndefinedBaselineZero,
        UndefinedBaselineIntervalContainsZero,
        Missing,
        Invalid,
        SemanticMismatch,
        Approximate,
    }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum VersionSource { Detected, UserDeclared, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum PackMatch { ExactPack, BaselineWithDeltas, NoPack }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum Compatibility { Compatible, Partial, Incompatible, Inconclusive }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum CompletenessState { Complete, Partial, Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum SourceKind { JsonProfile, TextProfile }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ProfileLevel {
        #[serde(rename = "level_1")]
        #[ts(rename = "level_1")]
        Level1,
        #[serde(rename = "level_2")]
        #[ts(rename = "level_2")]
        Level2,
        Unknown,
    }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum QueryState { Finished, Running, Error, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum SnapshotKind { Final, Runtime, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum MergeState { Merged, Unmerged, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum RuleEvaluationStatus { Triggered, Suspected, NotTriggered, NotApplicable, Suppressed }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum FindingConfidence { High, Medium, Low }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum EvidenceCompleteness { Complete, Partial, Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum TopologySource { ExecutionTopology, EmbeddedPlan, VerifiedProfileRelation, ExplainCosts, ExplainLogical, ExplainVerbose, None }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum GraphEntityKind { PlanNode, OperatorGroup, Pipeline, Fragment, Sink }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum GraphRelation { PlanChild, FragmentSink, ExchangeLink, Dependency, VerifiedDataFlow }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum DirectionSemantics { UpstreamToDownstream, ParentToChild, DependencyPrecedes }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum EdgeSource { Explicit, VerifiedDerived }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum SourceQuality { Exact, Partial }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum InputRole { Probe, Build, Child, Sink, Source }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum RequestedSourceKind { Auto, JsonProfile, TextProfile }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum SupportStatus { Supported, ParseOnly, Unsupported, Unknown }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum CompatibilityCheckState { Matched, Missing, Unknown, Conflict }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum SourceRole { Native, Subordinate }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CanonicalNumber {
    pub kind: NumberKind,
    /// Base-10 only: JSON numbers are never used for counters.
    pub value: String,
    pub scale: Option<u32>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct BoundedInterval {
    pub lower: CanonicalNumber,
    pub upper: CanonicalNumber,
    pub lower_inclusive: bool,
    pub upper_inclusive: bool,
}

#[derive(Deserialize)]
struct BoundedIntervalWire {
    lower: CanonicalNumber,
    upper: CanonicalNumber,
    lower_inclusive: bool,
    upper_inclusive: bool,
}

impl<'de> Deserialize<'de> for BoundedInterval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = BoundedIntervalWire::deserialize(deserializer)?;
        let ordering = compare_canonical_numbers(&wire.lower, &wire.upper).ok_or_else(|| {
            D::Error::custom("interval endpoints must be valid canonical numbers of the same kind")
        })?;
        if ordering == std::cmp::Ordering::Greater
            || (ordering == std::cmp::Ordering::Equal
                && (!wire.lower_inclusive || !wire.upper_inclusive))
        {
            return Err(D::Error::custom(
                "interval must be non-empty and lower must not exceed upper",
            ));
        }
        Ok(Self {
            lower: wire.lower,
            upper: wire.upper,
            lower_inclusive: wire.lower_inclusive,
            upper_inclusive: wire.upper_inclusive,
        })
    }
}

fn compare_canonical_numbers(
    left: &CanonicalNumber,
    right: &CanonicalNumber,
) -> Option<std::cmp::Ordering> {
    if left.kind != right.kind {
        return None;
    }
    let (left_negative, mut left_digits, left_scale) = canonical_parts(left)?;
    let (right_negative, mut right_digits, right_scale) = canonical_parts(right)?;
    if left_negative != right_negative {
        return Some(if left_negative {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        });
    }
    let scale = left_scale.max(right_scale);
    left_digits.extend(std::iter::repeat_n('0', scale - left_scale));
    right_digits.extend(std::iter::repeat_n('0', scale - right_scale));
    let left_digits = left_digits.trim_start_matches('0');
    let right_digits = right_digits.trim_start_matches('0');
    let magnitude = left_digits
        .len()
        .cmp(&right_digits.len())
        .then_with(|| left_digits.cmp(right_digits));
    Some(if left_negative {
        magnitude.reverse()
    } else {
        magnitude
    })
}

fn canonical_parts(number: &CanonicalNumber) -> Option<(bool, String, usize)> {
    let (negative, unsigned) = match number.value.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, number.value.as_str()),
    };
    match number.kind {
        NumberKind::Integer => {
            if number.scale.is_some()
                || unsigned.is_empty()
                || !unsigned.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let negative = negative && !unsigned.bytes().all(|byte| byte == b'0');
            Some((negative, unsigned.to_owned(), 0))
        }
        NumberKind::Decimal => {
            let scale: usize = number.scale?.try_into().ok()?;
            let (whole, fraction) = unsigned.split_once('.')?;
            if whole.is_empty()
                || fraction.len() != scale
                || fraction.is_empty()
                || !whole
                    .bytes()
                    .chain(fraction.bytes())
                    .all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let digits = format!("{whole}{fraction}");
            let negative = negative && !digits.bytes().all(|byte| byte == b'0');
            Some((negative, digits, scale))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct MetricCell {
    pub metric_cell_ref: String,
    pub metric_id: String,
    pub subject_ref: String,
    pub display_text: String,
    pub order_rank: Option<u32>,
    pub source_refs: Vec<String>,
    pub reasons: Vec<String>,
    /// Structural identity of the counter inside its subject: physical
    /// operator (fragment, pipeline, type, plan node, operator id) plus the raw
    /// counter name, which separates base/MIN/MAX statistics. Present only when
    /// unique within the report, so Compare can pair it across two Profiles
    /// of one plan; absent means it can not be paired unambiguously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub match_key: Option<String>,
    #[serde(flatten)]
    #[ts(flatten)]
    pub value: MetricCellValue,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum MetricCellValue {
    Exact {
        canonical_number: CanonicalNumber,
        canonical_unit: String,
        #[serde(flatten)]
        #[ts(flatten)]
        observation: ObservedMetricFidelity,
    },
    Missing {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Invalid {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Partial {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Overflow {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Unavailable {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Incompatible {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
    Unknown {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LosslessFidelity {
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BoundedFidelity {
    Rounded,
    Truncated,
    Interval,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UnknownFidelity {
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(untagged)]
#[ts(untagged)]
pub enum ObservedMetricFidelity {
    Lossless {
        fidelity: LosslessFidelity,
        bounds: (),
    },
    Bounded {
        fidelity: BoundedFidelity,
        bounds: BoundedInterval,
    },
    Unknown {
        fidelity: UnknownFidelity,
        bounds: (),
    },
}

impl ObservedMetricFidelity {
    fn fidelity(&self) -> Fidelity {
        match self {
            Self::Lossless { .. } => Fidelity::Exact,
            Self::Bounded {
                fidelity: BoundedFidelity::Rounded,
                ..
            } => Fidelity::Rounded,
            Self::Bounded {
                fidelity: BoundedFidelity::Truncated,
                ..
            } => Fidelity::Truncated,
            Self::Bounded {
                fidelity: BoundedFidelity::Interval,
                ..
            } => Fidelity::Interval,
            Self::Unknown { .. } => Fidelity::Unknown,
        }
    }

    fn bounds(&self) -> Option<&BoundedInterval> {
        match self {
            Self::Bounded { bounds, .. } => Some(bounds),
            _ => None,
        }
    }
}

impl MetricCell {
    pub fn state(&self) -> MetricCellState {
        match &self.value {
            MetricCellValue::Exact { .. } => MetricCellState::Exact,
            MetricCellValue::Missing { .. } => MetricCellState::Missing,
            MetricCellValue::Invalid { .. } => MetricCellState::Invalid,
            MetricCellValue::Partial { .. } => MetricCellState::Partial,
            MetricCellValue::Overflow { .. } => MetricCellState::Overflow,
            MetricCellValue::Unavailable { .. } => MetricCellState::Unavailable,
            MetricCellValue::Incompatible { .. } => MetricCellState::Incompatible,
            MetricCellValue::Unknown { .. } => MetricCellState::Unknown,
        }
    }

    pub fn canonical_number(&self) -> Option<&CanonicalNumber> {
        match &self.value {
            MetricCellValue::Exact {
                canonical_number, ..
            } => Some(canonical_number),
            _ => None,
        }
    }

    pub fn canonical_unit(&self) -> Option<&str> {
        match &self.value {
            MetricCellValue::Exact { canonical_unit, .. } => Some(canonical_unit),
            _ => None,
        }
    }

    pub fn fidelity(&self) -> Option<Fidelity> {
        match &self.value {
            MetricCellValue::Exact { observation, .. } => Some(observation.fidelity()),
            _ => None,
        }
    }

    pub fn bounds(&self) -> Option<&BoundedInterval> {
        match &self.value {
            MetricCellValue::Exact { observation, .. } => observation.bounds(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct GraphNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sql_location: Option<GraphSqlLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sql_facts: Option<Vec<GraphSqlFact>>,
    pub node_id: String,
    pub entity_kind: GraphEntityKind,
    pub plan_node_id: Option<String>,
    pub fragment_id: Option<String>,
    pub pipeline_id: Option<String>,
    pub plan_node_type: Option<String>,
    pub display_name: Option<String>,
    pub source_ordinal: Option<u32>,
    pub native_operator_refs: Vec<OrderedOperatorRef>,
    pub subordinate_operator_refs: Vec<OrderedOperatorRef>,
    pub metric_observation_refs: Vec<String>,
    pub execution_state: Option<String>,
    pub source_quality: SourceQuality,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct OrderedOperatorRef {
    pub operator_ref: String,
    pub source_ordinal: u32,
    pub source_role: SourceRole,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct RecipeRef {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct GraphEdge {
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
    /// Set on a `fragment_sink` edge whose relation an explicit `plan_child`
    /// edge (exchange -> sending fragment root) already states in the opposite
    /// direction. A renderer draws the referenced edge and skips this one.
    pub restates_edge_id: Option<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct UnresolvedRelation {
    pub relation_ref: String,
    pub reason: String,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct GraphView {
    pub state: GraphState,
    pub topology_source: TopologySource,
    pub root_node_ids: Vec<String>,
    /// This remains source ordered when the source relation is ordered.
    pub nodes: Vec<GraphNode>,
    /// This remains source ordered; canonicalization never sorts these edges by ID.
    pub edges: Vec<GraphEdge>,
    pub unconnected_node_ids: Vec<String>,
    pub unresolved_relations: Vec<UnresolvedRelation>,
    pub source_refs: Vec<String>,
    pub completeness: DimensionCompleteness,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ReceiptComponent {
    pub id: String,
    pub version: String,
    pub manifest_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct AnalysisReceipt {
    pub components: Vec<ReceiptComponent>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct InputSummary {
    pub requested_kind: RequestedSourceKind,
    pub detected_kind: SourceKind,
    pub filename: Option<String>,
    pub byte_length: u32,
    pub line_count: Option<u32>,
    pub source_hash: String,
    pub adapter_evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct VersionContext {
    pub reported_version: Option<VersionEvidence>,
    pub user_declared_version: Option<String>,
    pub effective_version: Option<String>,
    pub version_source: VersionSource,
    pub selected_pack: Option<SelectedVersionPack>,
    pub pack_match: PackMatch,
    pub compatibility: Compatibility,
    pub support_status: SupportStatus,
    pub detection_evidence: Vec<VersionEvidence>,
    pub compatibility_checks: Vec<CompatibilityCheck>,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SelectedVersionPack {
    pub pack_id: String,
    pub product_pack_version: String,
    pub upstream_release: String,
    pub upstream_commit: String,
    pub manifest_hash: String,
    /// A release other than the pack's own: the pack read with the audited
    /// deltas of that release (multi-version source audit). Absent for the
    /// pack's own release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub release_deltas: Option<ReleaseDeltas>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ReleaseDeltas {
    /// The reported release, `x.y.z`.
    pub release: String,
    /// Its minor line, `x.y`.
    pub line: String,
    /// Delta ids from `version-deltas.json` that apply, in file order.
    pub delta_ids: Vec<String>,
    pub deltas_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct VersionEvidence {
    pub value: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CompatibilityCheck {
    pub check_id: String,
    pub domain: String,
    pub state: CompatibilityCheckState,
    pub expected: Option<String>,
    pub observed: Option<String>,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ProfileContext {
    pub profile_level: ProfileLevel,
    pub query_state: QueryState,
    pub snapshot_kind: SnapshotKind,
    pub merge_state: MergeState,
    pub evidence_refs: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct DimensionCompleteness {
    pub state: CompletenessState,
    pub present_count: u32,
    pub expected_count: Option<u32>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CompletenessReport {
    pub summary: DimensionCompleteness,
    pub metrics: DimensionCompleteness,
    pub graph: DimensionCompleteness,
    pub version: DimensionCompleteness,
    pub provenance: DimensionCompleteness,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct OperatorRow {
    pub operator_ref: String,
    pub plan_node_id: Option<String>,
    pub operator_type: String,
    pub metric_cells: Vec<MetricCell>,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct QuerySummary {
    pub query_id: Option<String>,
    pub query_type: Option<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct Finding {
    pub finding_id: String,
    pub rule_id: String,
    pub rule_version: String,
    pub category: String,
    pub severity: String,
    pub confidence: FindingConfidence,
    pub confidence_reasons: Vec<String>,
    pub evidence_completeness: EvidenceCompleteness,
    pub evidence_completeness_reasons: Vec<String>,
    pub subject_ref: Option<String>,
    pub evidence: Vec<String>,
    pub limitations: Vec<String>,
    pub recommendation: Recommendation,
    pub related_finding_refs: Vec<String>,
    pub rule_trace_ref: String,
    /// What the rule measured on its subject, in the rule's own terms (a
    /// skew ratio, a fan-out ratio, bytes sent, a time share). Empty when the
    /// rule publishes no measurement; the browser never derives one.
    #[serde(default)]
    pub measurements: Vec<FindingMeasurement>,
}

/// One value a rule evaluator computed from the typed inputs it compared.
/// The text is formatted by the backend; bounded inputs give an interval.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct FindingMeasurement {
    /// Stable identifier, e.g. `slowest_over_mean_driver_time`.
    pub measurement_id: String,
    pub display_text: String,
    /// The rule's trigger condition on this value (e.g. `> 2×`); absent for
    /// context values that are not compared.
    pub threshold_text: Option<String>,
    /// Physical operator the value belongs to when narrower than the plan node.
    pub operator_type: Option<String>,
    /// Metric cells (`MetricCell.metric_cell_ref`) the value was computed from.
    pub input_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct Recommendation {
    pub risk: String,
    pub expected_direction: String,
    pub validation_steps: Vec<String>,
    /// Stable id of the recommendation text, e.g. `rec.scan.reduce_read@v1`.
    pub recommendation_id: String,
    pub kind: RecommendationKind,
    /// Short action title; the finding category names the problem instead.
    pub title: String,
    /// What to do first, in one sentence.
    pub next_step: String,
    pub risk_level: RecommendationRiskLevel,
    /// Whether a correct change is expected to keep the query result.
    pub result_effect: RecommendationResultEffect,
    pub steps: Vec<RecommendationStep>,
    pub expected_changes: Vec<ExpectedMetricChange>,
    /// What the recommendation did not check (the finding's own limitations
    /// describe its measurement instead).
    pub limitations: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RecommendationKind {
    /// Points at where the cost is (time-share rules).
    OptimizationTarget,
    /// A concrete check for an observed signal (spill, skew, fan-out…).
    Investigation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RecommendationRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RecommendationResultEffect {
    Unchanged,
    MayChange,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RecommendationStep {
    pub title: String,
    pub detail: String,
    /// Exact metric cells (`MetricCell.metric_cell_ref`) of the finding's own
    /// subject that this step reads; empty when none is printed.
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExpectedMetricChange {
    /// A registered metric id of the finding's subject.
    pub metric_id: String,
    pub direction: ExpectedChangeDirection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExpectedChangeDirection {
    Decrease,
    Increase,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct RuleEvaluation {
    pub rule_id: String,
    pub rule_version: String,
    pub status: RuleEvaluationStatus,
    pub evidence_completeness: EvidenceCompleteness,
    pub evidence_completeness_reasons: Vec<String>,
    pub subject_ref: Option<String>,
    pub applicability_checks: Vec<String>,
    pub input_observation_refs: Vec<String>,
    pub comparisons: Vec<String>,
    pub threshold_receipts: Vec<String>,
    pub missing_prerequisites: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub limitations: Vec<String>,
    pub finding_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct Diagnostic {
    pub code: String,
    pub stage: String,
    pub severity: String,
    pub recoverability: String,
    pub message_id: String,
    pub source_refs: Vec<String>,
    pub affected_ids: Vec<String>,
    pub details: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SourceView {
    pub source_kind: SourceKind,
    pub plain_text_lines: Vec<SourceLine>,
    pub json_tokens: Vec<SourceToken>,
    pub truncation_state: String,
    pub limitations: Vec<String>,
    /// The line of every text source reference in the report, from the
    /// adapter's typed locator. Names a reference whose line text is outside
    /// the safe allowlist above, so the browser never parses a reference.
    #[serde(default)]
    pub line_refs: Vec<SourceLineRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SourceLineRef {
    pub source_ref: String,
    pub line_no: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SourceLine {
    pub line_no: u32,
    pub text: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SourceToken {
    pub kind: String,
    pub text: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct AnalysisSnapshotReport {
    pub schema_version: String,
    pub analysis_state: AnalysisState,
    pub analysis_receipt: AnalysisReceipt,
    pub analysis_key: String,
    pub facts_hash: String,
    pub input: InputSummary,
    pub version: VersionContext,
    pub profile_context: ProfileContext,
    pub completeness: CompletenessReport,
    pub query_summary: Option<QuerySummary>,
    pub operator_rows: Vec<OperatorRow>,
    /// The single execution graph: plan nodes and their typed relations.
    pub graph_view: GraphView,
    /// Per-plan-node reading built with the official ExplainAnalyzer recipe.
    #[serde(default)]
    pub execution_summary: ExecutionSummary,
    /// Fragment / instance counts; Compare's parallelism gate reads only this.
    #[serde(default)]
    pub execution_shape: ExecutionShape,
    pub derived_metric_cells: Vec<MetricCell>,
    pub findings: Vec<Finding>,
    pub rule_evaluations: Vec<RuleEvaluation>,
    pub diagnostics: Vec<Diagnostic>,
    pub provenance_index: Vec<String>,
    pub source_view: SourceView,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct AnalysisOutcome {
    pub report: AnalysisSnapshotReport,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ErrorSummary {
    pub code: String,
    pub stage: String,
    pub message_id: String,
    pub recoverability: String,
    pub affected_side: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ErrorEnvelope {
    pub request_id: String,
    pub code: String,
    pub stage: String,
    pub message_id: String,
    pub recoverability: String,
    pub affected_side: Option<String>,
    pub source_refs: Vec<String>,
    pub details: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(untagged)]
#[ts(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum AnalysisAttempt {
    Outcome(AnalysisOutcomeAttempt),
    Error(AnalysisErrorAttempt),
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AnalysisOutcomeAttempt {
    pub outcome: AnalysisOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AnalysisErrorAttempt {
    pub error: ErrorSummary,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct MetricDelta {
    pub metric_id: String,
    pub subject_match: Option<String>,
    pub state: MetricDeltaState,
    pub baseline_metric_cell_ref: Option<String>,
    pub candidate_metric_cell_ref: Option<String>,
    pub absolute_delta: Option<CanonicalNumber>,
    pub percentage_state: PercentageState,
    pub percentage_value: Option<CanonicalNumber>,
    pub fidelity: Option<Fidelity>,
    /// Absolute-delta interval. This never carries percentage bounds.
    pub bounds: Option<BoundedInterval>,
    /// Percentage-delta interval. This is independent from `bounds`.
    pub percentage_bounds: Option<BoundedInterval>,
    pub order_rank: Option<u32>,
    pub limitations: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct MetricDeltaWire {
    metric_id: String,
    subject_match: Option<String>,
    state: MetricDeltaState,
    baseline_metric_cell_ref: Option<String>,
    candidate_metric_cell_ref: Option<String>,
    absolute_delta: Option<CanonicalNumber>,
    percentage_state: PercentageState,
    percentage_value: Option<CanonicalNumber>,
    fidelity: Option<Fidelity>,
    bounds: Option<BoundedInterval>,
    percentage_bounds: Option<BoundedInterval>,
    order_rank: Option<u32>,
    limitations: Vec<String>,
}

impl<'de> Deserialize<'de> for MetricDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MetricDeltaWire::deserialize(deserializer)?;
        let absolute_is_exact = wire.absolute_delta.is_some();
        let absolute_is_interval = wire.bounds.is_some();
        let absolute_is_absent = !absolute_is_exact && !absolute_is_interval;
        if !(absolute_is_absent && wire.fidelity.is_none()
            || absolute_is_exact && wire.fidelity == Some(Fidelity::Exact) && !absolute_is_interval
            || absolute_is_interval
                && wire.absolute_delta.is_none()
                && wire.fidelity == Some(Fidelity::Interval))
        {
            return Err(D::Error::custom(
                "MetricDelta absolute delta, fidelity and bounds must use the frozen exact/interval/absent matrix",
            ));
        }

        let percentage_valid = match wire.percentage_state {
            PercentageState::Computed => {
                wire.percentage_value.is_some() && wire.percentage_bounds.is_none()
            }
            PercentageState::Approximate => {
                wire.percentage_value.is_none() && wire.percentage_bounds.is_some()
            }
            _ => wire.percentage_value.is_none() && wire.percentage_bounds.is_none(),
        };
        if !percentage_valid {
            return Err(D::Error::custom(
                "MetricDelta percentage state, value and percentage_bounds must use the frozen matrix",
            ));
        }

        Ok(Self {
            metric_id: wire.metric_id,
            subject_match: wire.subject_match,
            state: wire.state,
            baseline_metric_cell_ref: wire.baseline_metric_cell_ref,
            candidate_metric_cell_ref: wire.candidate_metric_cell_ref,
            absolute_delta: wire.absolute_delta,
            percentage_state: wire.percentage_state,
            percentage_value: wire.percentage_value,
            fidelity: wire.fidelity,
            bounds: wire.bounds,
            percentage_bounds: wire.percentage_bounds,
            order_rank: wire.order_rank,
            limitations: wire.limitations,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ComparisonReceipt {
    pub components: Vec<ReceiptComponent>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct StructureDelta {
    pub state: String,
    pub matches: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub edge_delta: Vec<String>,
    pub unresolved: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct FindingDelta {
    pub added: Vec<String>,
    pub resolved: Vec<String>,
    pub persisting: Vec<String>,
    pub changed: Vec<String>,
    pub incomparable: Vec<String>,
    pub algorithm_changed: Vec<String>,
    /// Entries from `resolved` or `added` (`rule_id:subject_ref`) whose finding
    /// or unique negative evaluation has partial evidence. The backend labels
    /// these explicitly; consumers must not infer this from the report state.
    /// Older comparisons omit this field and deserialize as an empty list.
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub partial_evidence: Vec<String>,
    /// Rule measurements of the findings named by `added`, `resolved`,
    /// `persisting` and `changed`, paired by `measurement_id`. Display text
    /// is copied from each side's `Finding.measurements`; the comparator does
    /// not compute a measurement delta. Absent in payloads older than v0.6.
    #[serde(default)]
    pub measurement_changes: Vec<FindingMeasurementChange>,
}

/// One rule measurement of one finding entry, before and after.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct FindingMeasurementChange {
    /// The `FindingDelta` entry (`rule_id:subject_ref`) the measurement belongs to.
    pub finding_entry: String,
    pub measurement_id: String,
    /// `None` when that side has no such finding or the finding did not
    /// publish this measurement; never 0.
    pub baseline_display_text: Option<String>,
    pub candidate_display_text: Option<String>,
    /// Each side's rule condition on this value. They differ when the two
    /// findings were triggered on different severity tiers of one rule.
    pub baseline_threshold_text: Option<String>,
    pub candidate_threshold_text: Option<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct Comparison {
    pub schema_version: String,
    pub comparison_receipt: ComparisonReceipt,
    pub comparison_key: String,
    pub baseline: AnalysisAttempt,
    pub candidate: AnalysisAttempt,
    pub state: ComparisonState,
    pub reasons: Vec<String>,
    pub compatibility_delta: Vec<String>,
    pub structure_delta: StructureDelta,
    pub metric_deltas: Vec<MetricDelta>,
    pub finding_delta: FindingDelta,
    pub diagnostics: Vec<Diagnostic>,
    pub limitations: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ComparisonWire {
    schema_version: String,
    comparison_receipt: ComparisonReceipt,
    comparison_key: String,
    baseline: AnalysisAttempt,
    candidate: AnalysisAttempt,
    state: ComparisonState,
    reasons: Vec<String>,
    compatibility_delta: Vec<String>,
    structure_delta: StructureDelta,
    metric_deltas: Vec<MetricDelta>,
    finding_delta: FindingDelta,
    diagnostics: Vec<Diagnostic>,
    limitations: Vec<String>,
}

impl<'de> Deserialize<'de> for Comparison {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ComparisonWire::deserialize(deserializer)?;
        if wire.schema_version != COMPARISON_SCHEMA_VERSION {
            return Err(D::Error::custom(
                "Comparison must use COMPARISON_SCHEMA_VERSION",
            ));
        }
        if !valid_comparison_receipt(&wire.comparison_receipt) {
            return Err(D::Error::custom(
                "ComparisonReceipt must contain one structurally valid component",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            comparison_receipt: wire.comparison_receipt,
            comparison_key: wire.comparison_key,
            baseline: wire.baseline,
            candidate: wire.candidate,
            state: wire.state,
            reasons: wire.reasons,
            compatibility_delta: wire.compatibility_delta,
            structure_delta: wire.structure_delta,
            metric_deltas: wire.metric_deltas,
            finding_delta: wire.finding_delta,
            diagnostics: wire.diagnostics,
            limitations: wire.limitations,
        })
    }
}

fn valid_comparison_receipt(receipt: &ComparisonReceipt) -> bool {
    receipt.components.len() == 1
        && receipt.components.iter().all(|component| {
            !component.id.trim().is_empty()
                && !component.version.trim().is_empty()
                && component.manifest_hash.len() == 64
                && component
                    .manifest_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
        })
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportSchemaVersion {
    #[serde(rename = "0.1")]
    #[ts(rename = "0.1")]
    V0_1,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportKind {
    Analysis,
    Comparison,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportCanonicalization {
    #[serde(rename = "doctor-canonical-json-v1")]
    #[ts(rename = "doctor-canonical-json-v1")]
    DoctorCanonicalJsonV1,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportIntegrity {
    SelfConsistentNotSigned,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportBindingScope {
    Exporter,
    Analysis,
    Comparison,
    Baseline,
    Candidate,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExportEvidence {
    SyntheticContractOnlyNotDockerE2e,
    ProductRuntime,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExportFile {
    pub path: String,
    pub media_type: String,
    pub byte_length: u32,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct SchemaReceipt {
    pub schema_id: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ReceiptBinding {
    pub scope: ExportBindingScope,
    pub component: ReceiptComponent,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum SubjectHashAttempt {
    Success {
        analysis_key: String,
        facts_hash: String,
    },
    Failed {
        error_state: String,
    },
}

impl<'de> Deserialize<'de> for SubjectHashAttempt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("SubjectHashAttempt must be an object"))?;
        let state = object
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("SubjectHashAttempt requires state"))?;
        let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        match state {
            "success" => {
                if !has_exact_keys(&keys, &["state", "analysis_key", "facts_hash"]) {
                    return Err(D::Error::custom(
                        "successful SubjectHashAttempt has an invalid shape",
                    ));
                }
                let analysis_key = non_empty_string(object, "analysis_key")?;
                let facts_hash = hash_string(object, "facts_hash")?;
                Ok(Self::Success {
                    analysis_key,
                    facts_hash,
                })
            }
            "failed" => {
                if !has_exact_keys(&keys, &["state", "error_state"]) {
                    return Err(D::Error::custom(
                        "failed SubjectHashAttempt may contain only error_state",
                    ));
                }
                Ok(Self::Failed {
                    error_state: non_empty_string(object, "error_state")?,
                })
            }
            _ => Err(D::Error::custom("unknown SubjectHashAttempt state")),
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum SubjectHashes {
    Analysis {
        analysis_key: String,
        facts_hash: String,
    },
    Comparison {
        comparison_key: String,
        baseline: SubjectHashAttempt,
        candidate: SubjectHashAttempt,
    },
}

impl<'de> Deserialize<'de> for SubjectHashes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("SubjectHashes must be an object"))?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("SubjectHashes requires kind"))?;
        let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        match kind {
            "analysis" => {
                if !has_exact_keys(&keys, &["kind", "analysis_key", "facts_hash"]) {
                    return Err(D::Error::custom(
                        "analysis SubjectHashes has an invalid shape",
                    ));
                }
                Ok(Self::Analysis {
                    analysis_key: non_empty_string(object, "analysis_key")?,
                    facts_hash: hash_string(object, "facts_hash")?,
                })
            }
            "comparison" => {
                if !has_exact_keys(&keys, &["kind", "comparison_key", "baseline", "candidate"]) {
                    return Err(D::Error::custom(
                        "comparison SubjectHashes has an invalid shape",
                    ));
                }
                let baseline = serde_json::from_value(
                    object
                        .get("baseline")
                        .cloned()
                        .ok_or_else(|| D::Error::custom("missing baseline hash"))?,
                )
                .map_err(D::Error::custom)?;
                let candidate = serde_json::from_value(
                    object
                        .get("candidate")
                        .cloned()
                        .ok_or_else(|| D::Error::custom("missing candidate hash"))?,
                )
                .map_err(D::Error::custom)?;
                Ok(Self::Comparison {
                    comparison_key: non_empty_string(object, "comparison_key")?,
                    baseline,
                    candidate,
                })
            }
            _ => Err(D::Error::custom("unknown SubjectHashes kind")),
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExportManifest {
    pub schema_version: ExportSchemaVersion,
    pub canonicalization: ExportCanonicalization,
    pub integrity: ExportIntegrity,
    pub kind: ExportKind,
    pub evidence: ExportEvidence,
    pub files: Vec<ExportFile>,
    pub schemas: Vec<SchemaReceipt>,
    pub receipt_bindings: Vec<ReceiptBinding>,
    pub subject_hashes: SubjectHashes,
    pub manifest_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ExportManifestWire {
    schema_version: ExportSchemaVersion,
    canonicalization: ExportCanonicalization,
    integrity: ExportIntegrity,
    kind: ExportKind,
    evidence: ExportEvidence,
    files: Vec<ExportFile>,
    schemas: Vec<SchemaReceipt>,
    receipt_bindings: Vec<ReceiptBinding>,
    subject_hashes: SubjectHashes,
    manifest_hash: String,
}

impl<'de> Deserialize<'de> for ExportManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ExportManifestWire::deserialize(deserializer)?;
        if wire.files.len() != 1 {
            return Err(D::Error::custom(
                "ExportManifest must contain exactly one report file",
            ));
        }
        let report = &wire.files[0];
        if report.path != "report.json"
            || report.media_type != EXPORT_MEDIA_TYPE
            || report.byte_length == 0
        {
            return Err(D::Error::custom(
                "ExportManifest report file must be report.json with the frozen media type",
            ));
        }
        if !valid_hash(&report.sha256) || !valid_hash(&wire.manifest_hash) {
            return Err(D::Error::custom(
                "ExportManifest hashes must be SHA-256 hex strings",
            ));
        }
        if wire.schemas.is_empty()
            || wire.schemas.iter().any(|schema| {
                schema.schema_id.trim().is_empty()
                    || schema.version.trim().is_empty()
                    || !valid_hash(&schema.sha256)
            })
        {
            return Err(D::Error::custom(
                "ExportManifest schema receipts must be non-empty and hashed",
            ));
        }
        if wire.receipt_bindings.iter().any(|binding| {
            binding.component.id.trim().is_empty()
                || binding.component.version.trim().is_empty()
                || !valid_hash(&binding.component.manifest_hash)
        }) {
            return Err(D::Error::custom(
                "ExportManifest receipt bindings must contain valid components",
            ));
        }
        if wire
            .receipt_bindings
            .iter()
            .filter(|binding| binding.scope == ExportBindingScope::Exporter)
            .count()
            != 1
        {
            return Err(D::Error::custom(
                "ExportManifest must contain exactly one exporter receipt binding",
            ));
        }
        if !wire.receipt_bindings.iter().any(|binding| {
            binding.scope == ExportBindingScope::Exporter
                && binding.component.id == "doctor-export-v0.1"
        }) {
            return Err(D::Error::custom(
                "ExportManifest exporter receipt must be doctor-export-v0.1",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            canonicalization: wire.canonicalization,
            integrity: wire.integrity,
            kind: wire.kind,
            evidence: wire.evidence,
            files: wire.files,
            schemas: wire.schemas,
            receipt_bindings: wire.receipt_bindings,
            subject_hashes: wire.subject_hashes,
            manifest_hash: wire.manifest_hash,
        })
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ExportRequest {
    Analysis { snapshot: AnalysisSnapshotReport },
    Comparison { snapshot: Comparison },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ExportSubject {
    Analysis { snapshot: AnalysisSnapshotReport },
    Comparison { snapshot: Comparison },
}

fn deserialize_export_subject<'de, D, T>(deserializer: D, label: &str) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromExportWire,
{
    let value = Value::deserialize(deserializer)?;
    let object = value
        .as_object()
        .ok_or_else(|| D::Error::custom(format!("{label} must be an object")))?;
    let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    if !has_exact_keys(&keys, &["kind", "snapshot"]) {
        return Err(D::Error::custom(format!(
            "{label} requires exactly kind and snapshot"
        )));
    }
    T::from_export_value(value).map_err(D::Error::custom)
}

trait FromExportWire: Sized {
    fn from_export_value(value: Value) -> Result<Self, String>;
}

impl FromExportWire for ExportRequest {
    fn from_export_value(value: Value) -> Result<Self, String> {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("ExportRequest requires kind")?;
        let snapshot = value
            .get("snapshot")
            .cloned()
            .ok_or("ExportRequest requires snapshot")?;
        match kind {
            "analysis" => Ok(Self::Analysis {
                snapshot: serde_json::from_value(snapshot).map_err(|error| error.to_string())?,
            }),
            "comparison" => Ok(Self::Comparison {
                snapshot: serde_json::from_value(snapshot).map_err(|error| error.to_string())?,
            }),
            _ => Err("unknown ExportRequest kind".to_owned()),
        }
    }
}

impl FromExportWire for ExportSubject {
    fn from_export_value(value: Value) -> Result<Self, String> {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("ExportSubject requires kind")?;
        let snapshot = value
            .get("snapshot")
            .cloned()
            .ok_or("ExportSubject requires snapshot")?;
        match kind {
            "analysis" => Ok(Self::Analysis {
                snapshot: serde_json::from_value(snapshot).map_err(|error| error.to_string())?,
            }),
            "comparison" => Ok(Self::Comparison {
                snapshot: serde_json::from_value(snapshot).map_err(|error| error.to_string())?,
            }),
            _ => Err("unknown ExportSubject kind".to_owned()),
        }
    }
}

impl<'de> Deserialize<'de> for ExportRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_export_subject(deserializer, "ExportRequest")
    }
}

impl<'de> Deserialize<'de> for ExportSubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_export_subject(deserializer, "ExportSubject")
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExportArtifact {
    pub schema_version: ExportSchemaVersion,
    pub subject: ExportSubject,
    pub manifest: ExportManifest,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ExportArtifactWire {
    schema_version: ExportSchemaVersion,
    subject: ExportSubject,
    manifest: ExportManifest,
}

impl<'de> Deserialize<'de> for ExportArtifact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ExportArtifactWire::deserialize(deserializer)?;
        let subject_kind = match &wire.subject {
            ExportSubject::Analysis { .. } => ExportKind::Analysis,
            ExportSubject::Comparison { .. } => ExportKind::Comparison,
        };
        if wire.manifest.kind != subject_kind {
            return Err(D::Error::custom(
                "ExportArtifact subject and manifest kinds must match",
            ));
        }
        let hashes_kind = match &wire.manifest.subject_hashes {
            SubjectHashes::Analysis { .. } => ExportKind::Analysis,
            SubjectHashes::Comparison { .. } => ExportKind::Comparison,
        };
        if hashes_kind != subject_kind {
            return Err(D::Error::custom(
                "ExportArtifact subject and subject_hashes kinds must match",
            ));
        }
        if let (
            ExportSubject::Analysis { snapshot },
            SubjectHashes::Analysis {
                analysis_key,
                facts_hash,
            },
        ) = (&wire.subject, &wire.manifest.subject_hashes)
            && (&snapshot.analysis_key != analysis_key || &snapshot.facts_hash != facts_hash)
        {
            return Err(D::Error::custom(
                "analysis SubjectHashes must match the redacted snapshot",
            ));
        }
        if let (
            ExportSubject::Comparison { snapshot },
            SubjectHashes::Comparison { comparison_key, .. },
        ) = (&wire.subject, &wire.manifest.subject_hashes)
            && &snapshot.comparison_key != comparison_key
        {
            return Err(D::Error::custom(
                "comparison SubjectHashes must match the comparison snapshot",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            subject: wire.subject,
            manifest: wire.manifest,
        })
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn has_exact_keys(actual: &[&str], expected: &[&str]) -> bool {
    actual.len() == expected.len() && expected.iter().all(|key| actual.contains(key))
}

fn non_empty_string<E>(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, E>
where
    E: serde::de::Error,
{
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| E::custom(format!("{key} must be a non-empty string")))
}

fn hash_string<E>(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, E>
where
    E: serde::de::Error,
{
    let value = non_empty_string(object, key)?;
    if !valid_hash(&value) {
        return Err(E::custom(format!("{key} must be a SHA-256 hex string")));
    }
    Ok(value)
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct VersionPackCapability {
    pub pack_id: String,
    pub product_pack_version: String,
    pub upstream_commit: String,
    pub manifest_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct Capabilities {
    pub api_version: String,
    pub schema_version: String,
    pub supported_inputs: Vec<SourceKind>,
    pub version_packs: Vec<VersionPackCapability>,
    pub rule_sets: Vec<String>,
    pub limits: BTreeMap<String, u32>,
    pub csrf: CsrfCapability,
    pub build: BuildCapability,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CsrfCapability {
    pub nonce: String,
    pub required_when_origin_present: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct BuildCapability {
    pub git_commit: String,
    pub candidate_label: String,
}

wire_type! {
    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum InputOrigin { OfflineImport, ConnectedProfileFetch, CandidateProfileUserExternalRerun, ConnectedExplain, ConnectedSqlRuntime }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    /// `plaintext` reaches any host the user names (unencrypted, explicit choice);
    /// `docker_loopback_plaintext` is the literal-loopback test fixture mode.
    pub enum TlsMode { VerifyFull, DockerLoopbackPlaintext, Plaintext }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ConnectionSessionState { Ready, Unsupported, Expired, Closed, Failed }

    #[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ConnectionCapabilityState {
        Available,
        Unavailable,
        Forbidden,
        #[default]
        Unknown,
    }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum QueryDiscoverySource { Audit, ProfileList }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ConnectedQueryActionState { Available, Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    /// Records what the timestamp can prove. AuditLoader emits a wall-clock
    /// DATETIME without an origin offset, so a reader-session conversion is an
    /// interpretation rather than evidence of the producer's original instant.
    pub enum ConnectedTimestampState { ReaderSessionInterpreted, RawWallTimeOnly, Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ProfileFetchAvailableReason { CanBeAttempted, ProfileFetched }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ProfileFetchUnavailableReason { QueryIdInvalid, QueryIdDuplicate, QueryNotFinished }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum PlanCheckAvailableReason { StatementRetained }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum PlanCheckUnavailableReason { StatementMissingOrNotSelect, QueryMetadataUnavailable, QueryIdInvalid, QueryIdDuplicate }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum QueryFilterScope { AuditWindow, LatestProfileCacheRows }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ConnectedComparisonInterpretationState { ObservedOnly }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ProfileFetchKind { Baseline, CandidateUserExternalRerun }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    /// A `ProfileFetchResponse` is emitted only after the server-side fetch and
    /// analysis have succeeded. All unavailable/forbidden cases use a non-2xx
    /// `ErrorEnvelope`, so the success DTO has no nullable failure arms.
    pub enum ProfileFetchState { Fetched }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum RecommendationSource { ProductRuleSet, OfficialPlanAdvisor }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum AdvisorProvenance { OfficialExperimentalFeLocalVolatile }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainRuntimeState { Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    pub enum ExplainSchemaVersion {
        #[serde(rename = "0.4")]
        #[ts(rename = "0.4")]
        V0_4,
    }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainLevel { Logical, Verbose, Costs }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainSourceKind { ExplainLogical, ExplainVerbose, ExplainCosts }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainMetadataAvailability { Available, Unavailable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainSourceDisplayState { SafeStructureOnly, RawText }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ExplainSourceTruncationState { NotTruncated, InputLimited }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum PlanComparisonState { Comparable, PartiallyComparable, NotComparable }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum RuntimeDeltaState { NotComparableMissingRuntimeProfile }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum CandidateProfileResponsibility { UserExternalRerun }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum CandidateProfileState { AwaitingUserExternalRerun, FetchedAfterUserExternalRerun }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    #[serde(rename_all = "snake_case")]
    #[ts(rename_all = "snake_case")]
    pub enum ConnectedGoldenEvidence { SyntheticContractOnlyNotDockerE2e }

    #[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
    pub enum ConnectedSchemaVersion {
        #[serde(rename = "0.1")]
        #[ts(rename = "0.1")]
        V0_1,
    }
}

/// Input-only credentials. Serde's `skip_serializing` also emits JSON Schema
/// `writeOnly`, so a request can be accepted but cannot become an echo DTO.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectionCreateRequest {
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing)]
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub audit_table: Option<String>,
    pub tls_mode: TlsMode,
}

/// Whether this server asks for a login before the API answers.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct AccessSessionStatus {
    pub required: bool,
    pub authenticated: bool,
    /// The login is still the public default `admin` / `123456`, so the
    /// sign-in page may show it.
    pub default_account: bool,
}

/// Input-only login account; like credentials, it is never echoed.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct AccessLoginRequest {
    #[serde(skip_serializing)]
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
}

impl std::fmt::Debug for AccessLoginRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AccessLoginRequest { redacted: true }")
    }
}

impl std::fmt::Debug for ConnectionCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConnectionCreateRequest { redacted: true }")
    }
}

#[cfg(test)]
mod connection_capability_report_tests {
    use super::*;

    #[test]
    fn legacy_capability_report_defaults_new_user_filter_to_unknown() {
        let report: ConnectionCapabilityReport = serde_json::from_str(
            r#"{"version_probe":"available","time_zone":"available","audit":"available","profile_list":"available","profile_fetch":"available","explain_costs":"available","plan_advisor":"available"}"#,
        )
        .unwrap();
        assert_eq!(report.audit_user_filter, ConnectionCapabilityState::Unknown);
        assert_eq!(
            report.audit_statement_filter,
            ConnectionCapabilityState::Unknown
        );
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectionCapabilityReport {
    pub version_probe: ConnectionCapabilityState,
    pub time_zone: ConnectionCapabilityState,
    pub audit: ConnectionCapabilityState,
    #[serde(default)]
    pub audit_user_filter: ConnectionCapabilityState,
    /// `isQuery` (TINYINT) and `stmt` were both probed in this session, so the
    /// explicit statement-class filter can be applied without a fallback.
    #[serde(default)]
    pub audit_statement_filter: ConnectionCapabilityState,
    pub profile_list: ConnectionCapabilityState,
    pub profile_fetch: ConnectionCapabilityState,
    pub explain_costs: ConnectionCapabilityState,
    pub plan_advisor: ConnectionCapabilityState,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct TimeZoneEvidence {
    /// Effective time zone of the exact StarRocks session used by the connector.
    pub session_time_zone: String,
    /// FE host time zone reported by StarRocks. This resolves a session value
    /// of `SYSTEM` without guessing from the application host.
    pub system_time_zone: String,
    /// Session zone unless it is `SYSTEM`, then the server-reported system zone.
    pub effective_time_zone: String,
    /// Observed offset at connection time. Historical rows are never converted
    /// with this value because daylight-saving rules may differ at row time.
    pub current_utc_offset_seconds: i32,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectionSessionView {
    pub connection_id: String,
    pub state: ConnectionSessionState,
    pub tls_mode: TlsMode,
    pub idle_timeout_seconds: u32,
    pub detected_version: Option<VersionEvidence>,
    pub time_zone: Option<TimeZoneEvidence>,
    pub capabilities: ConnectionCapabilityReport,
    pub limitations: Vec<String>,
}

/// A Profile action is never an availability claim about the remote cache:
/// `available` means the bounded fetch may be attempted for this row.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum ConnectedProfileFetchAction {
    Available {
        reason: ProfileFetchAvailableReason,
    },
    Unavailable {
        reason: ProfileFetchUnavailableReason,
    },
}

/// The available arm must carry a session-owned capability reference. The
/// unavailable arm cannot carry one, so invalid state/reference matrices are
/// rejected by the wire contract rather than interpreted in the browser.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum ConnectedPlanCheckAction {
    Available {
        reason: PlanCheckAvailableReason,
        plan_sql_ref: String,
    },
    Unavailable {
        reason: PlanCheckUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedQueryRef {
    pub connection_id: String,
    pub query_id: String,
    pub discovery_source: Option<QueryDiscoverySource>,
    pub discovery_generation: Option<u32>,
    /// RFC 3339 text avoids a JavaScript-unsafe epoch-millisecond integer.
    /// `None` is required when a fetch response has only a query ID and no
    /// independently observed timestamp; the API must not invent UTC.
    pub observed_at_utc: Option<String>,
    /// Original bounded wall-clock text returned by Audit/ProfileList.
    pub observed_at_local: Option<String>,
    pub timestamp_state: ConnectedTimestampState,
    pub status: String,
    pub digest: Option<String>,
    pub database: Option<String>,
    pub resource_group: Option<String>,
    pub query_time_ms: Option<u32>,
    pub profile_fetch: ConnectedProfileFetchAction,
    pub plan_check: ConnectedPlanCheckAction,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedQueryListRequest {
    pub connection_id: String,
    pub limit: u16,
    pub status: Option<String>,
    pub digest: Option<String>,
    /// Explicit statement-class filter for the discovery list. An omitted
    /// field keeps the earlier unfiltered request semantics (`all`).
    #[serde(default)]
    #[schemars(default)]
    pub statement_filter: AuditStatementFilter,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedQueryListResponse {
    pub source: QueryDiscoverySource,
    pub discovery_generation: u32,
    pub filter_scope: QueryFilterScope,
    pub queries: Vec<ConnectedQueryRef>,
    pub limitations: Vec<String>,
    /// What the requested statement-class filter actually did on the resolved
    /// source. Hidden rows are counted, never silently dropped.
    pub statement_filter: QueryListStatementFilterCoverage,
}

/// Outcome of the discovery list's statement-class filter. Counts are exact
/// decimal source rows under the same window and status/digest filters and do
/// not depend on the list limit. "Not applied", "indistinguishable" and a real
/// zero stay distinct states.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum QueryListStatementFilterCoverage {
    /// `statement_filter = all`: nothing was hidden, so no hidden count exists.
    NotApplied,
    /// AuditLoader rows with a proven `isQuery` TINYINT and `stmt` column: the
    /// same predicate as the Audit history filter.
    Applied {
        /// Rows whose `isQuery` is not 1 (including NULL).
        hidden_non_query_row_count: String,
        /// `isQuery = 1` rows whose `stmt` starts with this tool's marker.
        hidden_doctor_row_count: String,
    },
    /// ProfileList rows: `Statement` keeps this tool's marker, but the source
    /// has no statement-class column, so non-query statements (for example a
    /// load with a Profile) cannot be distinguished and stay visible.
    DoctorOnlyApplied {
        /// Marked rows among the bounded ProfileList rows read this time.
        hidden_doctor_row_count: String,
    },
    /// The resolved source cannot prove which rows this tool sent, so nothing
    /// was hidden. `reason` is a stable limitation code.
    Indistinguishable { reason: String },
}

/// Narrow, session-owned lookup for SQL that was observed during the current
/// discovery generation. Query lists intentionally omit this field.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedQuerySqlRequest {
    pub connection_id: String,
    pub query_id: String,
    pub discovery_generation: u32,
}

/// Bounded SQL text returned only after an explicit lookup in the current
/// short-lived connection session. It is never part of query discovery rows.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedQuerySqlResponse {
    pub query_id: String,
    pub discovery_generation: u32,
    pub sql: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ProfileFetchRequest {
    pub connection_id: String,
    pub query_id: String,
    pub discovery_generation: u32,
    pub kind: ProfileFetchKind,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ProfileFetchResponse {
    pub query: ConnectedQueryRef,
    pub input_origin: InputOrigin,
    pub source_kind: SourceKind,
    pub state: ProfileFetchState,
    /// Present only after a verified standard execution in this same session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub official_profile_eligibility: Option<OfficialProfileEligibility>,
    pub source_hash: String,
    pub analysis_key: String,
    pub analysis_receipt: AnalysisReceipt,
    /// Reusing `AnalysisOutcome` keeps connected and offline analysis on one
    /// report contract without returning the connector's raw Profile bytes.
    pub outcome: AnalysisOutcome,
    pub source_refs: Vec<String>,
    pub limitations: Vec<String>,
}

/// A bounded ProfileList-only observation. `at_limit` reports the remote
/// ProfileList cap before Running filtering; `local_truncated` reports the
/// independent caller cap after filtering. Neither is a coverage claim.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RunningProfileCandidateListRequest {
    pub connection_id: String,
    pub limit: u16,
    /// Hides this tool's own marked statements from the Running list. An
    /// omitted field keeps the earlier unfiltered request semantics (`all`).
    #[serde(default)]
    #[schemars(default)]
    pub statement_filter: AuditStatementFilter,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RunningProfileCandidateListResponse {
    pub source: QueryDiscoverySource,
    pub discovery_generation: u32,
    /// Effective local maximum after bounded request normalization.
    pub limit: u16,
    pub returned: u32,
    /// The actual, unfiltered ProfileList result reached the remote request cap.
    pub at_limit: bool,
    /// Running candidates exceeded the independent local limit.
    pub local_truncated: bool,
    /// Running rows excluded because their retained shape, ID, or projection was invalid.
    pub rejected: u32,
    /// API receipt time, not a source sampling timestamp.
    pub read_at_utc: String,
    /// ProfileList does not supply a source sampling timestamp in this contract.
    pub profile_observed_at_utc: Option<String>,
    pub candidates: Vec<ConnectedQueryRef>,
    pub limitations: Vec<String>,
    /// What the requested statement filter did on the Running rows read this
    /// time: `not_applied` or `doctor_only_applied` with the hidden count.
    pub statement_filter: QueryListStatementFilterCoverage,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RunningProfileSnapshotRequest {
    pub connection_id: String,
    pub query_id: String,
    pub discovery_generation: u32,
}

/// A one-off raw Profile read for a query that was observed Running in the
/// supplied discovery generation. The source parser owns `profile_lifecycle`;
/// it is deliberately independent from the earlier ProfileList observation.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RunningProfileSnapshotResponse {
    pub query: ConnectedQueryRef,
    pub list_observed_state: String,
    pub profile_lifecycle: QueryState,
    /// API receipt time, independent from ProfileList's unknown source time.
    pub read_at_utc: String,
    pub profile_observed_at_utc: Option<String>,
    pub input_origin: InputOrigin,
    pub source_kind: SourceKind,
    pub source_hash: String,
    pub analysis_key: String,
    pub analysis_receipt: AnalysisReceipt,
    pub outcome: AnalysisOutcome,
    pub source_refs: Vec<String>,
    pub limitations: Vec<String>,
}

/// Explicit opt-in request that executes one bounded SELECT/WITH and analyzes
/// the Profile produced by that exact query. SQL is write-only at the API edge.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct RuntimeProfileRequest {
    pub connection_id: String,
    /// New C3 clients bind a generated UUID before submission. Legacy callers
    /// omit it and retain the pre-C3 runtime endpoint behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub execution_attempt_id: Option<String>,
    #[serde(skip_serializing)]
    pub candidate_sql: String,
}

impl std::fmt::Debug for RuntimeProfileRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RuntimeProfileRequest { redacted: true }")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExecutionAcquisitionMethod {
    StandardProfile,
}

/// Server-issued receipt that may bind a standard execution's completed query
/// to one later fixed `ANALYZE PROFILE` read in the same short-lived session.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct OfficialProfileEligibility {
    pub execution_attempt_id: String,
    pub method: OfficialProfileEligibilityMethod,
}

/// Strict input for the connected-only, server-fetched Profile comparison.
///
/// The client supplies only opaque IDs; it never submits reports or raw Profile
/// bytes. The API validates the path/body connection ID and both query IDs before
/// it asks the connector for either Profile.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedProfileComparisonRequest {
    pub connection_id: String,
    pub baseline_query_id: String,
    pub candidate_query_id: String,
    pub discovery_generation: u32,
}

/// Provenance for one server-fetched side of a connected Profile comparison.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedProfileComparisonSide {
    pub query: ConnectedQueryRef,
    pub input_origin: InputOrigin,
}

/// The comparison is assembled exclusively from two Profiles fetched through the
/// same opaque local connection session. `Comparison` already contains the typed
/// attempts and reports, so this wrapper deliberately does not duplicate them.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedProfileComparisonResponse {
    pub baseline: ConnectedProfileComparisonSide,
    pub candidate: ConnectedProfileComparisonSide,
    pub comparison: Comparison,
    pub validation_context: ConnectedComparisonValidationContext,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedComparisonValidationContext {
    pub interpretation_state: ConnectedComparisonInterpretationState,
    pub same_connection_session: bool,
    pub same_discovery_generation: bool,
    pub fetch_order: String,
    pub candidate_attestation: String,
}

/// Closed request grammar for ordinary EXPLAIN, not an execution method or
/// a claim that every mode already has a structured snapshot parser.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainMode {
    Logical,
    Verbose,
    #[default]
    Costs,
}

/// Exactly one input is accepted: a write-only candidate SQL statement, or a
/// short-lived server-side SQL reference emitted by query discovery.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "input_kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "input_kind", rename_all = "snake_case")]
pub enum ExplainRequest {
    UserSql {
        connection_id: String,
        #[serde(default)]
        mode: ExplainMode,
        #[serde(skip_serializing)]
        candidate_sql: String,
    },
    PlanSqlReference {
        connection_id: String,
        #[serde(default)]
        mode: ExplainMode,
        query_id: String,
        discovery_generation: u32,
        plan_sql_ref: String,
    },
}

impl std::fmt::Debug for ExplainRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExplainRequest { redacted: true }")
    }
}

/// The narrow, same-connection version envelope for a COSTS plan.  It is
/// intentionally not a `ProfileContext`: an Explain has no runtime profile.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainVersionContext {
    /// A registered release only.  Unknown server text is never reflected.
    pub detected_release: Option<String>,
    pub selected_pack: Option<SelectedVersionPack>,
    pub pack_match: PackMatch,
    pub source_refs: Vec<String>,
    pub limitations: Vec<String>,
}

/// Receipts that pin the independently versioned Explain surface.  Each
/// component hash is over its actual fixed semantic content.
/// These are available surface pins, not proof that every recipe was applied.
/// Raw-text modes explicitly mark COSTS recipes as not applied in limitations.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainReceipt {
    pub schema: ReceiptComponent,
    pub adapter: ReceiptComponent,
    pub exchange_recipe: ReceiptComponent,
    pub card_facts_recipe: ReceiptComponent,
    pub version_pack: Option<ReceiptComponent>,
}

/// The small, version-pinned set of plan facts that an Explain-only card may
/// render.  These are estimates or structural facts, never runtime metrics.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainFactKind {
    CardinalityEstimate,
    Distribution,
    JoinStrategy,
    AggregatePhase,
    PlanChildCount,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainFactEvidenceKind {
    Explicit,
    VerifiedDerived,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainDistribution {
    Broadcast,
    Gather,
    Shuffle,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainJoinStrategy {
    InnerBroadcast,
    InnerPartitioned,
    RightOuterBucketShuffle,
    RightOuterPartitioned,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainAggregatePhase {
    UpdateSerialize,
    UpdateFinalize,
    MergeFinalize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainCardinalityNumberKind {
    Integer,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainCardinalityEstimate {
    pub kind: ExplainCardinalityNumberKind,
    #[schemars(regex(pattern = r"^(0|[1-9][0-9]{0,38})$"))]
    pub value: String,
    pub scale: (),
}

/// The value arm remains type-safe while the outer fact tag always expresses
/// availability.  This prevents a formatter fact kind from being mistaken for
/// an availability state on the public wire.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(untagged)]
#[ts(untagged)]
pub enum ExplainFactValue {
    CardinalityEstimate(ExplainCardinalityEstimate),
    Distribution(ExplainDistribution),
    JoinStrategy(ExplainJoinStrategy),
    AggregatePhase(ExplainAggregatePhase),
    PlanChildCount(u32),
}

/// A fact is an exclusive `available`/`unavailable` wire arm.  The unavailable
/// arm carries only a typed kind and stable reason codes, never raw formatter
/// text.  Available facts keep `fact_kind` explicit for a renderer that does
/// not know the source grammar.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum ExplainNodeFact {
    Available {
        fact_ref: String,
        node_ref: String,
        fact_kind: ExplainFactKind,
        evidence_kind: ExplainFactEvidenceKind,
        value: ExplainFactValue,
        source_refs: Vec<String>,
    },
    Unavailable {
        fact_ref: String,
        node_ref: String,
        fact_kind: ExplainFactKind,
        source_refs: Vec<String>,
        #[schemars(length(min = 1))]
        #[ts(type = "[string, ...string[]]")]
        reason_codes: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ExplainNodeFactWire {
    Available {
        fact_ref: String,
        node_ref: String,
        fact_kind: ExplainFactKind,
        evidence_kind: ExplainFactEvidenceKind,
        value: ExplainFactValue,
        source_refs: Vec<String>,
    },
    Unavailable {
        fact_ref: String,
        node_ref: String,
        fact_kind: ExplainFactKind,
        source_refs: Vec<String>,
        reason_codes: Vec<String>,
    },
}

impl<'de> Deserialize<'de> for ExplainNodeFact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match ExplainNodeFactWire::deserialize(deserializer)? {
            ExplainNodeFactWire::Available {
                fact_ref,
                node_ref,
                fact_kind,
                evidence_kind,
                value,
                source_refs,
            } => {
                if let ExplainFactValue::CardinalityEstimate(number) = &value {
                    let digits = number.value.as_bytes();
                    let canonical_integer = (1..=39).contains(&digits.len())
                        && digits.iter().all(u8::is_ascii_digit)
                        && (digits.len() == 1 || digits[0] != b'0');
                    if !canonical_integer {
                        return Err(serde::de::Error::custom(
                            "Explain cardinality must be a canonical unsigned 1-39 digit integer",
                        ));
                    }
                }
                let matches_kind = matches!(
                    (&fact_kind, &value),
                    (
                        ExplainFactKind::CardinalityEstimate,
                        ExplainFactValue::CardinalityEstimate(_)
                    ) | (
                        ExplainFactKind::Distribution,
                        ExplainFactValue::Distribution(_)
                    ) | (
                        ExplainFactKind::JoinStrategy,
                        ExplainFactValue::JoinStrategy(_)
                    ) | (
                        ExplainFactKind::AggregatePhase,
                        ExplainFactValue::AggregatePhase(_)
                    ) | (
                        ExplainFactKind::PlanChildCount,
                        ExplainFactValue::PlanChildCount(_)
                    )
                );
                if fact_ref.is_empty() || node_ref.is_empty() || !matches_kind {
                    return Err(serde::de::Error::custom(
                        "Explain available fact kind/value XOR is invalid",
                    ));
                }
                Ok(Self::Available {
                    fact_ref,
                    node_ref,
                    fact_kind,
                    evidence_kind,
                    value,
                    source_refs,
                })
            }
            ExplainNodeFactWire::Unavailable {
                fact_ref,
                node_ref,
                fact_kind,
                source_refs,
                reason_codes,
            } => {
                if fact_ref.is_empty() || node_ref.is_empty() || reason_codes.is_empty() {
                    return Err(serde::de::Error::custom(
                        "Explain unavailable fact requires refs and reason codes",
                    ));
                }
                Ok(Self::Unavailable {
                    fact_ref,
                    node_ref,
                    fact_kind,
                    source_refs,
                    reason_codes,
                })
            }
        }
    }
}

impl ExplainNodeFact {
    pub fn fact_ref(&self) -> &str {
        match self {
            Self::Available { fact_ref, .. } | Self::Unavailable { fact_ref, .. } => fact_ref,
        }
    }

    pub fn node_ref(&self) -> &str {
        match self {
            Self::Available { node_ref, .. } | Self::Unavailable { node_ref, .. } => node_ref,
        }
    }

    pub fn kind(&self) -> ExplainFactKind {
        match self {
            Self::Available { fact_kind, .. } | Self::Unavailable { fact_kind, .. } => {
                fact_kind.clone()
            }
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainCardSlotId {
    Primary,
    Secondary,
    Tertiary,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplainCardFactSlotState {
    Selected,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ExplainCardFactSlot {
    pub slot_id: ExplainCardSlotId,
    pub expected_fact_kind: Option<ExplainFactKind>,
    pub selection_recipe: RecipeRef,
    pub state: ExplainCardFactSlotState,
    pub fact_ref: Option<String>,
    pub reason_codes: Vec<String>,
    pub source_refs: Vec<String>,
}

// Keep the public Rust value ergonomic while exporting the state-dependent
// wire shape as a real discriminated union in both JSON Schema and TypeScript.
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ExplainCardFactSlotSchema {
    Selected {
        slot_id: ExplainCardSlotId,
        expected_fact_kind: ExplainFactKind,
        selection_recipe: RecipeRef,
        fact_ref: String,
        reason_codes: [String; 0],
        source_refs: Vec<String>,
    },
    Unavailable {
        slot_id: ExplainCardSlotId,
        expected_fact_kind: ExplainFactKind,
        selection_recipe: RecipeRef,
        fact_ref: (),
        #[schemars(length(min = 1))]
        reason_codes: Vec<String>,
        source_refs: Vec<String>,
    },
    NotApplicable {
        slot_id: ExplainCardSlotId,
        expected_fact_kind: (),
        selection_recipe: RecipeRef,
        fact_ref: (),
        #[schemars(length(min = 1))]
        reason_codes: Vec<String>,
        source_refs: Vec<String>,
    },
}

impl JsonSchema for ExplainCardFactSlot {
    fn schema_name() -> String {
        "ExplainCardFactSlot".to_owned()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        ExplainCardFactSlotSchema::json_schema(generator)
    }
}

impl TS for ExplainCardFactSlot {
    type WithoutGenerics = Self;

    fn name() -> String {
        "ExplainCardFactSlot".to_owned()
    }

    fn inline() -> String {
        "{ slot_id: ExplainCardSlotId, expected_fact_kind: ExplainFactKind, selection_recipe: RecipeRef, state: \"selected\", fact_ref: string, reason_codes: [], source_refs: Array<string> } | { slot_id: ExplainCardSlotId, expected_fact_kind: ExplainFactKind, selection_recipe: RecipeRef, state: \"unavailable\", fact_ref: null, reason_codes: Array<string>, source_refs: Array<string> } | { slot_id: ExplainCardSlotId, expected_fact_kind: null, selection_recipe: RecipeRef, state: \"not_applicable\", fact_ref: null, reason_codes: Array<string>, source_refs: Array<string> }".to_owned()
    }

    fn inline_flattened() -> String {
        Self::inline()
    }

    fn decl() -> String {
        format!("type {} = {};", Self::name(), Self::inline())
    }

    fn decl_concrete() -> String {
        Self::decl()
    }

    fn visit_dependencies(visitor: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<ExplainCardSlotId>();
        visitor.visit::<ExplainFactKind>();
        visitor.visit::<RecipeRef>();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ExplainCardFactSlotWire {
    slot_id: ExplainCardSlotId,
    expected_fact_kind: Option<ExplainFactKind>,
    selection_recipe: RecipeRef,
    state: ExplainCardFactSlotState,
    fact_ref: Option<String>,
    reason_codes: Vec<String>,
    source_refs: Vec<String>,
}

impl<'de> Deserialize<'de> for ExplainCardFactSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ExplainCardFactSlotWire::deserialize(deserializer)?;
        let valid = match wire.state {
            ExplainCardFactSlotState::Selected => {
                wire.expected_fact_kind.is_some()
                    && wire
                        .fact_ref
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                    && wire.reason_codes.is_empty()
            }
            ExplainCardFactSlotState::Unavailable => {
                wire.expected_fact_kind.is_some()
                    && wire.fact_ref.is_none()
                    && !wire.reason_codes.is_empty()
            }
            ExplainCardFactSlotState::NotApplicable => {
                wire.expected_fact_kind.is_none()
                    && wire.fact_ref.is_none()
                    && !wire.reason_codes.is_empty()
            }
        };
        if !valid {
            return Err(serde::de::Error::custom(
                "Explain card slot violates its state XOR",
            ));
        }
        Ok(Self {
            slot_id: wire.slot_id,
            expected_fact_kind: wire.expected_fact_kind,
            selection_recipe: wire.selection_recipe,
            state: wire.state,
            fact_ref: wire.fact_ref,
            reason_codes: wire.reason_codes,
            source_refs: wire.source_refs,
        })
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainNodeFactSet {
    pub node_ref: String,
    pub facts: Vec<ExplainNodeFact>,
    pub card_slots: [ExplainCardFactSlot; 3],
    pub completeness: CompletenessState,
    pub reason_codes: Vec<String>,
    pub source_refs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ExplainNodeFactSetWire {
    node_ref: String,
    facts: Vec<ExplainNodeFact>,
    card_slots: [ExplainCardFactSlot; 3],
    completeness: CompletenessState,
    reason_codes: Vec<String>,
    source_refs: Vec<String>,
}

impl<'de> Deserialize<'de> for ExplainNodeFactSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ExplainNodeFactSetWire::deserialize(deserializer)?;
        let ids = [
            ExplainCardSlotId::Primary,
            ExplainCardSlotId::Secondary,
            ExplainCardSlotId::Tertiary,
        ];
        if wire.node_ref.is_empty()
            || wire
                .card_slots
                .iter()
                .zip(ids)
                .any(|(slot, id)| slot.slot_id != id)
            || wire
                .facts
                .iter()
                .any(|fact| fact.node_ref() != wire.node_ref)
        {
            return Err(serde::de::Error::custom(
                "Explain fact set ownership is invalid",
            ));
        }
        let mut fact_refs = std::collections::BTreeSet::new();
        if wire
            .facts
            .iter()
            .any(|fact| fact.fact_ref().is_empty() || !fact_refs.insert(fact.fact_ref()))
        {
            return Err(serde::de::Error::custom("Explain fact refs must be unique"));
        }
        for slot in &wire.card_slots {
            if matches!(slot.state, ExplainCardFactSlotState::Selected) {
                let selected = wire
                    .facts
                    .iter()
                    .find(|fact| Some(fact.fact_ref()) == slot.fact_ref.as_deref());
                if !selected.is_some_and(|fact| {
                    fact.is_available() && Some(fact.kind()) == slot.expected_fact_kind
                }) {
                    return Err(serde::de::Error::custom(
                        "Explain selected card slot must reference its available expected fact",
                    ));
                }
            }
        }
        Ok(Self {
            node_ref: wire.node_ref,
            facts: wire.facts,
            card_slots: wire.card_slots,
            completeness: wire.completeness,
            reason_codes: wire.reason_codes,
            source_refs: wire.source_refs,
        })
    }
}

/// A locator for Explain source material.  `row_line` ranges are byte offsets
/// in the original MySQL cell, never in a reconstructed joined string.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum ExplainSourceLocator {
    Result {
        source_ref: String,
        row_count: u32,
        byte_length: u32,
    },
    RowLine {
        source_ref: String,
        #[schemars(range(min = 1))]
        row_no: u32,
        #[schemars(range(min = 1))]
        row_line_no: u32,
        #[schemars(range(min = 1))]
        global_line_no: u32,
        byte_start: u32,
        byte_end: u32,
    },
    ConnectionMetadata {
        source_ref: String,
        field: String,
        availability: ExplainMetadataAvailability,
        evidence_hash: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExplainSourceLocatorWire {
    Result {
        source_ref: String,
        row_count: u32,
        byte_length: u32,
    },
    RowLine {
        source_ref: String,
        row_no: u32,
        row_line_no: u32,
        global_line_no: u32,
        byte_start: u32,
        byte_end: u32,
    },
    ConnectionMetadata {
        source_ref: String,
        field: String,
        availability: ExplainMetadataAvailability,
        evidence_hash: Option<String>,
    },
}

impl<'de> Deserialize<'de> for ExplainSourceLocator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match ExplainSourceLocatorWire::deserialize(deserializer)? {
            ExplainSourceLocatorWire::Result {
                source_ref,
                row_count,
                byte_length,
            } => Ok(Self::Result {
                source_ref,
                row_count,
                byte_length,
            }),
            ExplainSourceLocatorWire::RowLine {
                source_ref,
                row_no,
                row_line_no,
                global_line_no,
                byte_start,
                byte_end,
            } => {
                if row_no == 0 || row_line_no == 0 || global_line_no == 0 {
                    return Err(serde::de::Error::custom(
                        "Explain row_line numbers are one-based",
                    ));
                }
                if byte_start > byte_end {
                    return Err(serde::de::Error::custom(
                        "Explain row_line byte_start exceeds byte_end",
                    ));
                }
                Ok(Self::RowLine {
                    source_ref,
                    row_no,
                    row_line_no,
                    global_line_no,
                    byte_start,
                    byte_end,
                })
            }
            ExplainSourceLocatorWire::ConnectionMetadata {
                source_ref,
                field,
                availability,
                evidence_hash,
            } => Ok(Self::ConnectionMetadata {
                source_ref,
                field,
                availability,
                evidence_hash,
            }),
        }
    }
}

/// The only text exposed for an Explain is the registered structural grammar.
/// Attribute and SQL text remain outside the response contract.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainSourceView {
    pub source_kind: ExplainSourceKind,
    pub lines: Vec<SourceLine>,
    pub display_state: ExplainSourceDisplayState,
    pub truncation_state: ExplainSourceTruncationState,
    pub limitations: Vec<String>,
}

impl std::fmt::Debug for ExplainSourceView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExplainSourceView")
            .field("source_kind", &self.source_kind)
            .field("line_count", &self.lines.len())
            .field("redacted", &true)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainSnapshot {
    pub snapshot_id: String,
    pub explain_schema_version: ExplainSchemaVersion,
    pub explain_level: ExplainLevel,
    pub input_origin: InputOrigin,
    pub plan_hash: String,
    pub facts_hash: String,
    pub version: ExplainVersionContext,
    pub receipt: ExplainReceipt,
    /// Explain carries the plan graph only. Runtime remains explicitly
    /// unavailable in `runtime_state` and is never joined from a Profile.
    pub graph_view: GraphView,
    /// Source-ordered typed plan facts.  The client only renders these values;
    /// it never reparses raw Explain text or derives a fallback.
    pub plan_node_fact_sets: Vec<ExplainNodeFactSet>,
    /// Explain is plan-only; a runtime field cannot be populated here.
    pub runtime_state: ExplainRuntimeState,
    pub source_locators: Vec<ExplainSourceLocator>,
    pub source_view: ExplainSourceView,
    pub diagnostics: Vec<Diagnostic>,
    pub source_refs: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ExplainResponse {
    pub snapshot: ExplainSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedRecommendation {
    pub recommendation_id: String,
    pub source: RecommendationSource,
    pub source_version: String,
    pub advisor_provenance: Option<AdvisorProvenance>,
    pub action_kind: String,
    pub suggested_action: String,
    pub applicability: Vec<String>,
    pub evidence: Vec<String>,
    pub limitations: Vec<String>,
    /// The frozen analysis recommendation is composed, never widened.
    pub recommendation: Recommendation,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct PlanAdvisorRequest {
    pub connection_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct PlanAdvisorResponse {
    pub capability: ConnectionCapabilityState,
    pub recommendations: Vec<ConnectedRecommendation>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct CorrelationEvidence {
    pub baseline_origin: InputOrigin,
    pub baseline_source_kind: SourceKind,
    pub candidate_origin: InputOrigin,
    pub baseline_analysis_key: String,
    pub baseline_source_hash: String,
    pub baseline_analysis_receipt: AnalysisReceipt,
    pub candidate_explain_snapshot_id: String,
    pub candidate_plan_hash: String,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct PlanComparison {
    pub schema_version: ConnectedSchemaVersion,
    pub state: PlanComparisonState,
    pub correlation: CorrelationEvidence,
    pub structure_delta: StructureDelta,
    /// Profile/Explain never has a runtime delta until a candidate Profile exists.
    pub runtime_delta_state: RuntimeDeltaState,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct OptimizationExperimentContext {
    pub experiment_id: String,
    pub correlation: CorrelationEvidence,
    pub candidate_profile_responsibility: CandidateProfileResponsibility,
    pub candidate_profile_state: CandidateProfileState,
    pub plan_comparison: Option<PlanComparison>,
    pub validation_steps: Vec<String>,
    pub limitations: Vec<String>,
}

/// Synthetic-only contract evidence. It deliberately has no Docker or product
/// execution receipt; real Docker E2E is a later task's evidence.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[ts(rename_all = "snake_case")]
pub struct ConnectedContractGolden {
    pub schema_version: ConnectedSchemaVersion,
    pub evidence: ConnectedGoldenEvidence,
    pub connection: ConnectionSessionView,
    pub query_request: ConnectedQueryListRequest,
    pub query_response: ConnectedQueryListResponse,
    pub query_sql_request: ConnectedQuerySqlRequest,
    pub query_sql_response: ConnectedQuerySqlResponse,
    pub profile_fetch_request: ProfileFetchRequest,
    pub profile_fetch_response: ProfileFetchResponse,
    pub running_profile_candidate_list_request: RunningProfileCandidateListRequest,
    pub running_profile_candidate_list_response: RunningProfileCandidateListResponse,
    pub running_profile_snapshot_request: RunningProfileSnapshotRequest,
    pub running_profile_snapshot_response: RunningProfileSnapshotResponse,
    pub profile_comparison_request: ConnectedProfileComparisonRequest,
    pub profile_comparison_response: ConnectedProfileComparisonResponse,
    pub advisor_request: PlanAdvisorRequest,
    pub advisor_response: PlanAdvisorResponse,
    pub explain_response: ExplainResponse,
    pub plan_comparison: PlanComparison,
    pub experiment: OptimizationExperimentContext,
}

/// Canonical JSON is the only hash input. Object keys sort recursively; arrays stay ordered.
pub fn canonical_json(value: &Value) -> String {
    let mut output = Vec::new();
    write_canonical_json(value, &mut output).expect("write to memory");
    String::from_utf8(output).expect("canonical JSON is UTF-8")
}

/// Streams `canonical_json` byte for byte into one writer, so large reports
/// are not rebuilt as nested strings at every depth.
fn write_canonical_json(value: &Value, out: &mut impl io::Write) -> io::Result<()> {
    match value {
        Value::Null => out.write_all(b"null"),
        Value::Bool(value) => out.write_all(if *value { b"true" } else { b"false" }),
        Value::Number(value) => write!(out, "{value}"),
        Value::String(value) => Ok(serde_json::to_writer(&mut *out, value)?),
        Value::Array(values) => {
            out.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.write_all(b",")?;
                }
                write_canonical_json(value, out)?;
            }
            out.write_all(b"]")
        }
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            out.write_all(b"{")?;
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.write_all(b",")?;
                }
                serde_json::to_writer(&mut *out, key)?;
                out.write_all(b":")?;
                write_canonical_json(value, out)?;
            }
            out.write_all(b"}")
        }
    }
}

/// SHA-256 of `canonical_json(&serde_json::to_value(value))`, written directly
/// without building the intermediate `Value` (see `canonical.rs`).
pub fn canonical_hash(value: &impl Serialize) -> String {
    let bytes = canonical::canonical_bytes(value).expect("wire values serialize");
    format!("{:x}", Sha256::digest(&bytes))
}

/// `canonical_hash` that gives up with `None` once `stop` returns true. `stop`
/// is polled before every array element and every 4 KiB digested; the hash
/// of a completed run is byte-for-byte `canonical_hash`.
pub fn canonical_hash_cancellable(
    value: &impl Serialize,
    stop: &dyn Fn() -> bool,
) -> Option<String> {
    let stopped = std::cell::Cell::new(false);
    let poll = || {
        let stop = stop();
        stopped.set(stopped.get() || stop);
        stop
    };
    let bytes = match canonical::canonical_bytes_until(value, &poll) {
        Ok(bytes) => bytes,
        Err(_) if stopped.get() => return None,
        Err(error) => panic!("wire values serialize: {error}"),
    };
    let mut digest = Sha256::new();
    for chunk in bytes.chunks(4 * 1024) {
        if poll() {
            return None;
        }
        digest.update(chunk);
    }
    Some(format!("{:x}", digest.finalize()))
}

/// Stable v0.3 semantic projection shared by production assembly and generated
/// contract goldens. Provenance and display-only fields are intentionally absent.
pub fn explain_facts_hash_v1(
    graph: &GraphView,
    fact_sets: &[ExplainNodeFactSet],
    receipt: &ExplainReceipt,
) -> String {
    let graph_nodes = graph
        .nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "node_id": node.node_id, "entity_kind": node.entity_kind,
                "plan_node_id": node.plan_node_id, "fragment_id": node.fragment_id,
                "plan_node_type": node.plan_node_type, "display_name": node.display_name,
                "source_ordinal": node.source_ordinal, "source_quality": node.source_quality,
            })
        })
        .collect::<Vec<_>>();
    let graph_edges = graph
        .edges
        .iter()
        .map(|edge| {
            serde_json::json!({
                "edge_id": edge.edge_id, "from": edge.from, "to": edge.to,
                "relation": edge.relation, "direction_semantics": edge.direction_semantics,
                "source": edge.source, "quality": edge.quality,
                "derivation_recipe": edge.derivation_recipe,
            })
        })
        .collect::<Vec<_>>();
    let unresolved_relations = graph
        .unresolved_relations
        .iter()
        .map(|relation| {
            serde_json::json!({
                "relation_ref": relation.relation_ref, "reason": relation.reason,
            })
        })
        .collect::<Vec<_>>();
    let facts = fact_sets
        .iter()
        .map(|set| {
            serde_json::json!({
                "node_ref": set.node_ref,
                "facts": set.facts.iter().map(|fact| match fact {
                    ExplainNodeFact::Available { fact_ref, node_ref, fact_kind, evidence_kind, value, .. } => serde_json::json!({"state":"available","fact_ref":fact_ref,"node_ref":node_ref,"fact_kind":fact_kind,"evidence_kind":evidence_kind,"value":value}),
                    ExplainNodeFact::Unavailable { fact_ref, node_ref, fact_kind, reason_codes, .. } => serde_json::json!({"state":"unavailable","fact_ref":fact_ref,"node_ref":node_ref,"fact_kind":fact_kind,"reason_codes":reason_codes}),
                }).collect::<Vec<_>>(),
                "card_slots": set.card_slots.iter().map(|slot| serde_json::json!({
                    "slot_id":slot.slot_id,"expected_fact_kind":slot.expected_fact_kind,
                    "selection_recipe":slot.selection_recipe,"state":slot.state,
                    "fact_ref":slot.fact_ref,"reason_codes":slot.reason_codes,
                })).collect::<Vec<_>>(),
                "completeness": set.completeness,
                "reason_codes": set.reason_codes,
            })
        })
        .collect::<Vec<_>>();
    canonical_hash(&serde_json::json!({
        "projection_version": 1,
        "explain_schema_version": EXPLAIN_SCHEMA_VERSION,
        "adapter_receipt": receipt.adapter,
        "card_facts_receipt": receipt.card_facts_recipe,
        "graph": {
            "state": graph.state,
            "completeness_state": graph.completeness.state,
            "completeness_reasons": graph.completeness.reasons,
            "nodes": graph_nodes,
            "edges": graph_edges,
            "root_node_ids": graph.root_node_ids,
            "unconnected_node_ids": graph.unconnected_node_ids,
            "unresolved_relations": unresolved_relations,
        },
        "fact_sets": facts,
    }))
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
pub struct GraphSqlFact {
    pub kind: GraphSqlFactKind,
    pub value: String,
    pub source_refs: Vec<String>,
}

/// Request-local bounded source excerpt; never a reconstructed SQL expression.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum GraphSqlLocation {
    Located {
        kind: GraphSqlFactKind,
        before: String,
        highlight: String,
        after: String,
        source_refs: Vec<String>,
    },
    Ambiguous,
    Unavailable,
    Unsupported,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum GraphSqlFactKind {
    Table,
    Fields,
    Predicate,
    JoinCondition,
}
