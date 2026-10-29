//! 30-day load view over the local history store's hourly aggregates. Only
//! additive quantities are drawn: executions, failures and summed query time.
//! Hours that were not collected stay absent; version or rule changes split
//! the days into groups that are never compared across.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AuditMetricFact, AuditPerformanceValueTick};

pub const HISTORY_LOAD_MAX_DAYS: u16 = 30;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadRequest {
    /// The connected session whose target to show; otherwise the most
    /// recently collected target.
    pub connection_id: Option<String>,
    pub days: u16,
}

impl HistoryLoadRequest {
    pub fn validate(&self) -> bool {
        (7..=HISTORY_LOAD_MAX_DAYS).contains(&self.days)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum HistoryLoadCoverage {
    /// Every hour of the day was read completely.
    Complete,
    /// Some hours were read only partly or not at all.
    Partial,
    /// No hour of the day was read: nothing is drawn, nothing is zero.
    NotCollected,
}

/// One UTC day. Sums cover only the hours that were read.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadDay {
    pub start_seconds: u32,
    pub coverage: HistoryLoadCoverage,
    pub complete_hours: u16,
    pub partial_hours: u16,
    pub not_collected_hours: u16,
    /// `None` when the day was not collected.
    pub executions: Option<String>,
    pub errors: Option<String>,
    pub query_time_sum: Option<AuditMetricFact>,
    /// Index into `segments`: days in different segments are never compared.
    pub segment: u16,
    pub executions_permyriad: Option<u16>,
    pub errors_permyriad: Option<u16>,
    pub query_time_permyriad: Option<u16>,
}

/// A run of days under one StarRocks version and one history rule set.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadSegment {
    pub engine_versions: Vec<String>,
    pub rule_versions: Vec<String>,
    pub first_day_seconds: u32,
}

/// Average executions per collected hour for one UTC weekday and hour.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadHeatCell {
    /// 0 = Monday … 6 = Sunday.
    pub weekday: u8,
    pub hour: u8,
    pub collected_hours: u16,
    /// Rounded to a whole execution; `None` when no such hour was collected.
    pub average_executions: Option<String>,
    /// 0–5 relative to the busiest cell; `None` with the average.
    pub level: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum HistoryLoadComparison {
    Exact {
        delta: String,
        /// Change in permille of the earlier day; `None` when that day had 0.
        permille: Option<i32>,
    },
    NotComparable {
        reason: HistoryLoadNotComparableReason,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum HistoryLoadNotComparableReason {
    PartialCoverage,
    VersionOrRuleChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadWeekRow {
    pub current_day_seconds: u32,
    pub previous_day_seconds: u32,
    pub current_executions: Option<String>,
    pub previous_executions: Option<String>,
    pub comparison: HistoryLoadComparison,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadAxes {
    pub executions: Vec<AuditPerformanceValueTick>,
    pub errors: Vec<AuditPerformanceValueTick>,
    pub query_time: Vec<AuditPerformanceValueTick>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryLoadResponse {
    pub target: Option<String>,
    pub start_seconds: u32,
    pub end_seconds: u32,
    pub days: Vec<HistoryLoadDay>,
    pub segments: Vec<HistoryLoadSegment>,
    pub heatmap: Vec<HistoryLoadHeatCell>,
    pub week: Vec<HistoryLoadWeekRow>,
    pub axes: HistoryLoadAxes,
    pub collected_hours: u16,
    pub expected_hours: u16,
    /// Rows the collector read and the ones it could attribute to a template.
    pub rows_read: String,
    pub rows_attributed: String,
    pub executions: Option<String>,
    pub errors: Option<String>,
    pub limitations: Vec<String>,
}
