//! Task 59 (L2): load alerts, evaluated from the stored load history after
//! every collection cycle. Rules and thresholds are on trial until the G2
//! calibration; every number arrives counted by the server.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AlertState, LoadKindGroup, TrendSeverity};

/// Every state lists its newest `LOAD_ALERT_LIST_LIMIT` alerts; `more` lists
/// more of one state, so any alert can be reached.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertsRequest {
    pub target: Option<String>,
    #[serde(default)]
    pub more: Option<LoadAlertsMore>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertsMore {
    pub state: AlertState,
    /// How many of that state to list, newest first.
    pub limit: u32,
}

pub const LOAD_ALERT_LIST_LIMIT: u32 = 500;

impl LoadAlertsRequest {
    pub fn validate(&self) -> bool {
        self.target.as_deref().is_none_or(|target| {
            (1..=256).contains(&target.len()) && target.bytes().all(|byte| byte.is_ascii_graphic())
        }) && self
            .more
            .is_none_or(|more| (1..=1_000_000).contains(&more.limit))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadAlertRule {
    RoutinePaused,
    RoutineStopped,
    LagGrowing,
    FailureRate,
    FilterRate,
}

impl LoadAlertRule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoutinePaused => "routine_paused",
            Self::RoutineStopped => "routine_stopped",
            Self::LagGrowing => "lag_growing",
            Self::FailureRate => "failure_rate",
            Self::FilterRate => "filter_rate",
        }
    }
}

/// Counts of one window: loads and failures, or input and filtered rows.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertWindow {
    pub start_seconds: u32,
    pub end_seconds: u32,
    /// Loads, or input rows (loaded + filtered).
    pub total: String,
    /// Failed loads, or filtered rows.
    pub part: String,
    pub rate_ppm: u32,
    /// Completely collected hours that entered the counts.
    pub collected_hours: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertLagHour {
    pub start_seconds: u32,
    pub max_lag: String,
    pub height_permyriad: u16,
}

/// What the alert is about, per rule.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "rule", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "rule", rename_all = "snake_case")]
pub enum LoadAlertDetail {
    RoutinePaused {
        job_id: String,
        name: String,
        database: String,
        table: String,
        /// The snapshot that saw the state, not when it changed.
        observed_seconds: u32,
        /// `REASONS_OF_STATE_CHANGED`; shown in the app, never notified.
        reason: Option<String>,
        max_lag: Option<String>,
    },
    RoutineStopped {
        job_id: String,
        name: String,
        database: String,
        table: String,
        state: String,
        observed_seconds: u32,
        reason: Option<String>,
    },
    LagGrowing {
        job_id: String,
        name: String,
        database: String,
        table: String,
        /// The four complete hours the rule looked at, oldest first.
        hours: Vec<LoadAlertLagHour>,
        /// Latest ÷ first, in thousandths.
        ratio_permille: u32,
    },
    FailureRate {
        database: String,
        table: String,
        kind: LoadKindGroup,
        current: LoadAlertWindow,
        baseline: LoadAlertWindow,
        /// Current minus baseline rate, parts per million, from raw counts.
        delta_ppm: i32,
        /// Current ÷ baseline in thousandths; `None` when it is not available:
        /// the baseline has none, or the counts are too large to divide exactly.
        ratio_permille: Option<u32>,
    },
    FilterRate {
        database: String,
        table: String,
        kind: LoadKindGroup,
        current: LoadAlertWindow,
        baseline: LoadAlertWindow,
        /// Current minus baseline rate, parts per million, from raw counts.
        delta_ppm: i32,
        /// Current ÷ baseline in thousandths; `None` when it is not available:
        /// the baseline has none, or the counts are too large to divide exactly.
        ratio_permille: Option<u32>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertView {
    pub alert_id: String,
    pub rule: LoadAlertRule,
    pub state: AlertState,
    pub severity: TrendSeverity,
    pub muted_until_seconds: Option<u32>,
    pub first_seen_seconds: u32,
    pub last_seen_seconds: u32,
    pub resolved_seconds: Option<u32>,
    pub detail: LoadAlertDetail,
    pub engine_version: String,
    pub rule_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertCounts {
    pub open: String,
    pub acknowledged: String,
    pub resolved: String,
    pub muted: String,
}

/// The trial thresholds, for the rules panel.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertThresholds {
    pub current_hours: u16,
    pub baseline_hours: u16,
    pub mute_hours: u16,
    pub lag_hours: u16,
    pub lag_min: String,
    pub lag_high: String,
    pub failure_min_loads: u32,
    pub failure_min_baseline_loads: u32,
    pub failure_min_ppm: u32,
    pub failure_high_ppm: u32,
    pub filter_min_rows: u32,
    pub filter_min_baseline_rows: u32,
    pub filter_min_ppm: u32,
    pub filter_high_ppm: u32,
    pub calibrated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadAlertsResponse {
    pub target: Option<String>,
    /// Active first, then newest; each state up to `list_limit`, or the
    /// requested `more` limit for that state.
    pub alerts: Vec<LoadAlertView>,
    pub list_limit: u32,
    pub counts: LoadAlertCounts,
    pub thresholds: LoadAlertThresholds,
    pub limitations: Vec<String>,
}
