//! Plan-node execution summary: the single per-node reading of a runtime
//! Profile, following the official StarRocks 3.5.14 `ExplainAnalyzer.NodeInfo`
//! recipe (time, share, hotspot tier, output rows). The browser renders these
//! values as-is and never re-aggregates operator metrics.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{BoundedInterval, CanonicalNumber};

/// Recipe identity for the plan-node summary. A new StarRocks release must
/// register its own recipe instead of silently reusing this one.
pub const EXECUTION_SUMMARY_RECIPE_ID: &str = "sr3514.explain_analyzer.node_info.v1";

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExecutionSummaryState {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SummaryValueState {
    Exact,
    Unavailable,
}

/// One backend-computed value. `Unavailable` never carries a number, so a
/// missing metric can not be read as zero.
///
/// `Exact` means the value equals the official 3.5.14 `ANALYZE PROFILE`
/// reading of the same Profile text (`RuntimeProfileParser` parses the
/// printed counters too). When a counter was printed with less precision
/// than the runtime value (e.g. `28.802ms` is truncated to the microsecond),
/// `bounds` is the interval in `canonical_unit` that contains the runtime
/// value; `None` means every input was printed losslessly.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct SummaryValue {
    pub state: SummaryValueState,
    pub canonical_number: Option<CanonicalNumber>,
    pub canonical_unit: Option<String>,
    pub display_text: Option<String>,
    pub reasons: Vec<String>,
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub bounds: Option<BoundedInterval>,
}

/// Official thresholds: share > 30% is most consuming, > 15% second.
/// `Boundary` (product contract): the official reading and the text
/// precision interval of the share disagree about one of those thresholds, so
/// the tier is not decidable; the time-share rules report the same input as
/// `suspected` at that boundary.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum HotspotTier {
    MostConsuming,
    SecondMostConsuming,
    Boundary,
    None,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SummaryOperatorRole {
    Native,
    Subordinate,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct PlanNodeOperatorSummary {
    pub operator_type: String,
    pub role: SummaryOperatorRole,
    pub fragment_ref: Option<String>,
    pub operator_time: SummaryValue,
    pub pull_rows: SummaryValue,
    pub push_rows: SummaryValue,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct PlanNodeSummary {
    /// Joins `GraphView.nodes[].node_id`.
    pub node_id: String,
    pub plan_node_id: String,
    pub fragment_ref: Option<String>,
    pub total_time: SummaryValue,
    /// Share of `QueryCumulativeOperatorTime` in basis points (1/100 percent).
    pub time_share_basis_points: Option<u32>,
    pub time_share_display: Option<String>,
    pub hotspot: HotspotTier,
    pub output_rows: SummaryValue,
    pub operators: Vec<PlanNodeOperatorSummary>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ExecutionSummary {
    pub state: ExecutionSummaryState,
    pub recipe_id: String,
    /// `Summary.Total` as reported by the FE; independent of operator time.
    pub query_total_time: SummaryValue,
    pub cumulative_operator_time: SummaryValue,
    /// `RESULT_SINK.PushRowNum`: rows returned to the client.
    pub result_rows: SummaryValue,
    pub nodes: Vec<PlanNodeSummary>,
    pub reasons: Vec<String>,
}

impl SummaryValue {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            state: SummaryValueState::Unavailable,
            canonical_number: None,
            canonical_unit: None,
            display_text: None,
            reasons: vec![reason.to_owned()],
            source_refs: vec![],
            bounds: None,
        }
    }
}

impl Default for ExecutionSummary {
    /// Retained reports assembled before the summary existed decode as
    /// explicitly unavailable, never as an empty "no hotspot" result.
    fn default() -> Self {
        Self {
            state: ExecutionSummaryState::Unavailable,
            recipe_id: EXECUTION_SUMMARY_RECIPE_ID.to_owned(),
            query_total_time: SummaryValue::unavailable("retained_report_without_summary"),
            cumulative_operator_time: SummaryValue::unavailable("retained_report_without_summary"),
            result_rows: SummaryValue::unavailable("retained_report_without_summary"),
            nodes: vec![],
            reasons: vec!["retained_report_without_summary".to_owned()],
        }
    }
}
