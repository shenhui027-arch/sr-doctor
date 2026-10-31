//! Task 58 (L1): the load history views over the local history store. Days
//! are UTC days; hours that were not collected are absent, never zero, and
//! a StarRocks version or history rule change starts a new segment.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    HistoryLoadCoverage, HistoryLoadSegment, LoadErrorCategory, LoadErrorCategoryCount,
    LoadErrorEntry, LoadKindGroup,
};

pub const LOAD_HISTORY_DAYS: [u16; 2] = [7, 30];
pub const LOAD_HISTORY_TABLES: usize = 12;
pub const LOAD_HISTORY_ERROR_LOGS_PAGE: u16 = 50;
pub const LOAD_HISTORY_REASONS: usize = 12;

fn valid_connection(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|value| {
        (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

fn valid_target(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|value| {
        (1..=300).contains(&value.len()) && !value.chars().any(char::is_control)
    })
}

fn valid_table(value: &Option<String>) -> bool {
    value
        .as_ref()
        .is_none_or(|value| (3..=257).contains(&value.len()) && value.contains('.'))
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryRequest {
    /// The connected session whose target to show; otherwise the most
    /// recently collected target.
    pub connection_id: Option<String>,
    /// The collection target to show (task 59: a load alert names it); it
    /// wins over the connected session when known.
    pub target: Option<String>,
    pub days: u16,
    pub kind: Option<LoadKindGroup>,
    /// `db.table`.
    pub table: Option<String>,
    pub include_system: bool,
}

impl LoadHistoryRequest {
    pub fn validate(&self) -> bool {
        LOAD_HISTORY_DAYS.contains(&self.days)
            && valid_connection(&self.connection_id)
            && valid_target(&self.target)
            && valid_table(&self.table)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryKindDay {
    pub kind: LoadKindGroup,
    pub loads: String,
    /// Bar height against `count_axis`.
    pub height_permyriad: u16,
}

/// One UTC day. Sums cover only the hours that were read.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryDay {
    pub start_seconds: u32,
    pub coverage: HistoryLoadCoverage,
    pub complete_hours: u16,
    pub partial_hours: u16,
    pub not_collected_hours: u16,
    /// Why hours were partial (`row_limit`, `stream_records_cleared`, ...).
    pub partial_reasons: Vec<String>,
    /// Index into `segments`.
    pub segment: u16,
    /// `None` when no hour of the day was read.
    pub loads: Option<String>,
    pub failed: Option<String>,
    pub loaded_rows: Option<String>,
    pub filtered_rows: Option<String>,
    /// Loads that did not report a count are left out of the sums.
    pub unknown_counts: u32,
    pub by_kind: Vec<LoadHistoryKindDay>,
    /// Failed ÷ finished in parts per million, against `rate_axis`.
    pub failed_rate_ppm: Option<u32>,
    pub failed_rate_y_permyriad: Option<u16>,
    /// Filtered ÷ (loaded + filtered) in parts per million.
    pub filtered_rate_ppm: Option<u32>,
    pub filtered_rate_y_permyriad: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryKindTotal {
    pub kind: LoadKindGroup,
    pub loads: String,
    pub failed: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryTotals {
    pub loads: String,
    pub failed: String,
    pub failed_ppm: Option<u32>,
    pub loaded_rows: String,
    pub loaded_unknown: u32,
    pub filtered_rows: String,
    pub filtered_unknown: u32,
    pub filtered_ppm: Option<u32>,
    pub tables_with_filtered: u32,
    pub partial_hours: u32,
    pub not_collected_hours: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryReason {
    pub category: LoadErrorCategory,
    pub column: Option<String>,
    pub target_type: Option<String>,
    /// Share of the sampled rows of this table, not of all filtered rows.
    pub sample_share_permyriad: u16,
    pub sampled_rows: u32,
    /// First seen within the last three days, after at least three days of
    /// saved logs without it.
    pub new_since_seconds: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryTable {
    pub database: String,
    pub table: String,
    pub kinds: Vec<LoadKindGroup>,
    pub loads: String,
    pub failed: String,
    pub loaded_rows: String,
    pub filtered_rows: String,
    pub unknown_counts: u32,
    pub top_reason: Option<LoadHistoryReason>,
    pub saved_logs: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorStorageView {
    pub logs: u32,
    pub saved: u32,
    pub raw_bytes: String,
    pub limit_bytes: String,
    pub used_permyriad: u16,
    pub earliest_saved_seconds: Option<u32>,
    /// Gone from the BE, or not read within StarRocks' retention.
    pub missed: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryCollection {
    /// Everything before this was read or recorded as a gap.
    pub watermark_seconds: Option<u32>,
    pub last_run_seconds: Option<u32>,
    pub last_rows_read: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryResponse {
    pub target: Option<String>,
    pub collection: LoadHistoryCollection,
    pub days: Vec<LoadHistoryDay>,
    pub segments: Vec<HistoryLoadSegment>,
    /// Counts per kind over the range, ignoring the kind filter.
    pub kinds: Vec<LoadHistoryKindTotal>,
    /// Every kind together, ignoring the kind filter.
    pub all_loads: String,
    /// Routine Load jobs with a snapshot in the range: the job history has
    /// its own entry, even where no task record was collected (3.2).
    pub routine_jobs: u32,
    pub totals: LoadHistoryTotals,
    pub count_axis: Vec<String>,
    /// Parts per million.
    pub rate_axis: Vec<String>,
    pub tables: Vec<LoadHistoryTable>,
    /// Every `db.table` with history, for the filter.
    pub table_options: Vec<String>,
    pub error_logs: LoadErrorStorageView,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryRoutineRequest {
    pub connection_id: Option<String>,
    /// The collection target to show (task 59: a load alert names it); it
    /// wins over the connected session when known.
    pub target: Option<String>,
    pub days: u16,
    /// The job whose detail to show; the first job otherwise.
    pub job_id: Option<String>,
}

impl LoadHistoryRoutineRequest {
    pub fn validate(&self) -> bool {
        LOAD_HISTORY_DAYS.contains(&self.days)
            && valid_connection(&self.connection_id)
            && valid_target(&self.target)
            && self.job_id.as_ref().is_none_or(|id| {
                (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
            })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RoutineHistoryState {
    Running,
    Scheduling,
    Paused,
    Stopped,
    Cancelled,
    /// Snapshots were taken, but this job was not among them.
    Absent,
    NotCollected,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineHistoryCell {
    pub start_seconds: u32,
    /// The worst state seen in the cell.
    pub state: RoutineHistoryState,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineHistoryJob {
    pub job_id: String,
    pub name: String,
    pub database: String,
    pub table: String,
    pub current_state: String,
    pub cells: Vec<RoutineHistoryCell>,
    /// Changes into PAUSED observed in the range.
    pub pauses: u32,
    /// Sums of counter increases between close snapshots; `None` when no
    /// interval of the range is known.
    pub loaded_rows: Option<String>,
    pub error_rows: Option<String>,
    pub committed_tasks: Option<String>,
    pub aborted_tasks: Option<String>,
    /// Intervals whose increase is unknown (counter reset or snapshots too
    /// far apart): the sums leave them out.
    pub unknown_intervals: u32,
    pub max_lag: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLagPoint {
    pub start_seconds: u32,
    pub max_lag: Option<String>,
    pub y_permyriad: Option<u16>,
    pub paused: bool,
    pub collected: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineHistoryEvent {
    pub at_seconds: u32,
    pub from_state: Option<String>,
    pub to_state: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineHistoryDay {
    pub start_seconds: u32,
    /// `None` when no increase of the day is known.
    pub loaded_rows: Option<String>,
    pub error_rows: Option<String>,
    pub loaded_height_permyriad: u16,
    pub error_height_permyriad: u16,
    pub unknown_intervals: u32,
    pub collected: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineHistoryDetail {
    pub job_id: String,
    pub name: String,
    pub lag: Vec<RoutineLagPoint>,
    pub lag_axis: Vec<String>,
    /// StarRocks before 3.3 reports no lag.
    pub lag_known: bool,
    pub events: Vec<RoutineHistoryEvent>,
    pub days: Vec<RoutineHistoryDay>,
    pub rows_axis: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryRoutineResponse {
    pub target: Option<String>,
    pub cell_seconds: u32,
    pub range_start_seconds: u32,
    pub jobs: Vec<RoutineHistoryJob>,
    pub selected: Option<RoutineHistoryDetail>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryErrorLogsRequest {
    pub connection_id: Option<String>,
    /// The collection target to show (task 59: a load alert names it); it
    /// wins over the connected session when known.
    pub target: Option<String>,
    pub days: u16,
    pub table: Option<String>,
    pub category: Option<LoadErrorCategory>,
    pub offset: u32,
}

impl LoadHistoryErrorLogsRequest {
    pub fn validate(&self) -> bool {
        LOAD_HISTORY_DAYS.contains(&self.days)
            && valid_connection(&self.connection_id)
            && valid_target(&self.target)
            && valid_table(&self.table)
            && self.offset <= 100_000
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum StoredLoadErrorLogState {
    Pending,
    Saved,
    Purged,
    NoLog,
    Failed,
    Expired,
    Trimmed,
}

/// Facts of one saved (or attempted) error log. No reason or row text.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StoredLoadErrorLog {
    pub job_id: String,
    pub label: String,
    pub database: String,
    pub table: String,
    pub kind: LoadKindGroup,
    pub finish_seconds: u32,
    pub filtered_rows: Option<String>,
    pub state: StoredLoadErrorLogState,
    pub attempts: u32,
    pub saved_seconds: Option<u32>,
    pub sampled: u32,
    pub truncated: bool,
    pub categories: Vec<LoadErrorCategoryCount>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorReasonSummary {
    pub database: String,
    pub table: String,
    pub category: LoadErrorCategory,
    pub column: Option<String>,
    pub target_type: Option<String>,
    pub sampled_rows: u32,
    pub loads: u32,
    pub first_seconds: u32,
    pub last_seconds: u32,
    pub new: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryErrorLogsResponse {
    pub target: Option<String>,
    pub storage: LoadErrorStorageView,
    pub reasons: Vec<LoadErrorReasonSummary>,
    pub logs: Vec<StoredLoadErrorLog>,
    /// Logs matching the filters; `logs` is one page of them.
    pub total: u32,
    pub table_options: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryErrorLogRequest {
    pub connection_id: Option<String>,
    /// The collection target to show (task 59: a load alert names it); it
    /// wins over the connected session when known.
    pub target: Option<String>,
    pub job_id: String,
}

impl LoadHistoryErrorLogRequest {
    pub fn validate(&self) -> bool {
        valid_connection(&self.connection_id)
            && valid_target(&self.target)
            && (1..=20).contains(&self.job_id.len())
            && self.job_id.bytes().all(|byte| byte.is_ascii_digit())
    }
}

/// The saved text of one load's error log: the only response carrying it.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHistoryErrorLogResponse {
    pub log: StoredLoadErrorLog,
    pub entries: Vec<LoadErrorEntry>,
    pub limitations: Vec<String>,
}
