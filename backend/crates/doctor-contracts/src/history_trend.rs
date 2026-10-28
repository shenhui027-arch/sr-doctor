//! Stage C: per-template trends over the local history (ADR-5). Statistics
//! are medians, MAD and histogram quantiles; nothing is folded into a score.
//! Metric values are `AuditMetricFact`s through the one typed parser.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AuditMetricFact, CoverageHourState, EvidenceCompleteness};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryTrendsRequest {
    /// `host:port` as collected; `None` picks the most recently collected target.
    pub target: Option<String>,
    /// 1..=168 hours ending with the current hour.
    pub current_hours: u16,
    /// 24..=720 hours immediately before the current window.
    pub baseline_hours: u16,
    /// 1..=200 non-stable rows.
    pub limit: u16,
}

impl HistoryTrendsRequest {
    pub fn validate(&self) -> bool {
        (1..=168).contains(&self.current_hours)
            && (24..=720).contains(&self.baseline_hours)
            && (1..=200).contains(&self.limit)
            && self.target.as_deref().is_none_or(valid_key)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistorySeriesRequest {
    pub target: String,
    pub template_key: String,
    pub current_hours: u16,
    pub baseline_hours: u16,
    /// 24..=168 hourly points ending with the current hour.
    pub points: u16,
}

impl HistorySeriesRequest {
    pub fn validate(&self) -> bool {
        valid_key(&self.target)
            && valid_key(&self.template_key)
            && (1..=168).contains(&self.current_hours)
            && (24..=720).contains(&self.baseline_hours)
            && (24..=168).contains(&self.points)
    }
}

fn valid_key(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TrendSeverity {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TrendConfidence {
    High,
    Medium,
    Low,
}

/// Scan bytes or CPU per execution beside the latency change (AGENTS §3:
/// latency alone never decides).
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkloadSignal {
    Increased,
    Stable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TrendUnscoredReason {
    InsufficientBaseline,
    InsufficientCurrent,
    /// Baseline median is 0: only the absolute change is shown.
    ZeroBaseline,
    /// Latency moved but neither scan bytes nor CPU are available.
    WorkloadSignalMissing,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TrendNotComparableReason {
    EngineVersionChanged,
    RuleVersionChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum TrendVerdict {
    Regression {
        severity: TrendSeverity,
        workload: WorkloadSignal,
    },
    Improvement,
    /// Inside the baseline band.
    Stable,
    Unscored {
        reason: TrendUnscoredReason,
    },
    NotComparable {
        reason: TrendNotComparableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendSide {
    /// Baseline: median of hourly medians; current: median of the window.
    pub median: Option<AuditMetricFact>,
    /// Baseline only: median absolute deviation of hourly medians.
    pub mad: Option<AuditMetricFact>,
    pub executions: String,
    /// Executions with a valid query time; the rest are missing or invalid, never 0.
    pub valid_samples: String,
    /// Hours that entered the statistic.
    pub hours: u32,
    /// Hours with too few samples to enter it.
    pub thin_hours: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TrendDirection {
    Up,
    Down,
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendChange {
    pub direction: TrendDirection,
    pub magnitude: AuditMetricFact,
    /// Decimal text with one decimal; `None` when the baseline is 0.
    pub relative_percent: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendComparability {
    pub engine_version: String,
    pub rule_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendRow {
    pub template_key: String,
    /// `official_digest` or `product_fingerprint` (unofficial).
    pub kind: String,
    pub template: Option<String>,
    pub catalog: String,
    pub database: String,
    pub verdict: TrendVerdict,
    /// Only for a regression or an improvement.
    pub confidence: Option<TrendConfidence>,
    pub evidence: EvidenceCompleteness,
    pub baseline: TrendSide,
    pub current: TrendSide,
    pub change: Option<TrendChange>,
    pub comparability: TrendComparability,
    pub limitations: Vec<String>,
}

/// The rule in force; `calibrated` stays false until G2.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendRule {
    pub k_mad: String,
    pub min_relative_percent: u16,
    pub min_baseline_hours: u16,
    pub min_hour_samples: u16,
    pub min_current_samples: u16,
    pub calibrated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendCounts {
    pub regression: String,
    pub improvement: String,
    pub stable: String,
    pub unscored: String,
    pub not_comparable: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendWindow {
    pub current_start_seconds: u32,
    pub baseline_start_seconds: u32,
    pub end_seconds: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryTrendsResponse {
    pub target: Option<String>,
    pub targets: Vec<String>,
    pub window: TrendWindow,
    pub rule: TrendRule,
    /// Regressions first, then improvements, not comparable and unscored.
    pub rows: Vec<TrendRow>,
    pub truncated: bool,
    pub counts: TrendCounts,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendPoint {
    pub hour_start_seconds: u32,
    pub coverage: CoverageHourState,
    /// `None` when the hour was not collected; `"0"` only when it was.
    pub executions: Option<String>,
    pub median: Option<AuditMetricFact>,
    pub valid_samples: String,
    /// Too few samples to enter a statistic.
    pub thin: bool,
    /// Same engine and rule version as the current window.
    pub comparable: bool,
}

/// A slow execution kept by query ID only, to pull its Profile on demand.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrendExemplar {
    pub query_id: String,
    pub query_time: AuditMetricFact,
    pub event_seconds: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistorySeriesResponse {
    pub target: String,
    pub window: TrendWindow,
    pub rule: TrendRule,
    pub row: TrendRow,
    pub points: Vec<TrendPoint>,
    pub exemplars: Vec<TrendExemplar>,
}
