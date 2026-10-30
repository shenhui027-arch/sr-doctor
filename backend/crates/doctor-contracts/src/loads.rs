//! Load workspace (L0, task 51): Routine Load jobs, recent load records and
//! one load's error log, read live from StarRocks `information_schema`
//! through the read-only connection. Nothing here is stored. Times are the
//! FE's own strings in its system time zone; they are never converted.
//! Error log rows are business data the user chose to see; they are only
//! returned for the one job asked for.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const LOAD_ERROR_ROW_MAX_BYTES: usize = 4 * 1024;
pub const LOAD_ERROR_REASON_MAX_BYTES: usize = 1024;
pub const LOAD_ERROR_MAX_ENTRIES: usize = 200;
pub const LOAD_RECORDS_MAX: usize = 500;

fn valid_connection_id(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// What the connected version can show. 3.3+ is complete; 3.2 has Routine
/// Load jobs without partition lag and no load records; 3.1 has neither.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadViewSupport {
    Full,
    WithoutLag,
    UnsupportedVersion,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadReadRequest {
    pub connection_id: String,
}

impl LoadReadRequest {
    pub fn validate(&self) -> bool {
        valid_connection_id(&self.connection_id)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RoutineLoadStateClass {
    Running,
    /// `NEED_SCHEDULE`: waiting for the next task.
    Scheduling,
    Paused,
    Stopped,
    Cancelled,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadPartition {
    pub partition: String,
    /// The consumed position as StarRocks writes it: an offset, or a marker
    /// such as `OFFSET_ZERO`.
    pub consumed: Option<String>,
    pub latest: Option<String>,
    /// From `OFFSET_LAG`; `None` when StarRocks reports none.
    pub lag: Option<String>,
    /// Epoch milliseconds of the newest consumed message (`TIMESTAMP_PROGRESS`).
    pub consumed_message_ms: Option<String>,
}

/// Lifetime counters from `STATISTICS`; every field may be missing.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadStatistics {
    pub received_bytes: Option<String>,
    pub received_bytes_rate: Option<String>,
    pub loaded_rows: Option<String>,
    pub load_rows_rate: Option<String>,
    pub error_rows: Option<String>,
    pub total_rows: Option<String>,
    pub unselected_rows: Option<String>,
    pub committed_tasks: Option<String>,
    pub aborted_tasks: Option<String>,
    pub task_execute_time_ms: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadJob {
    pub job_id: String,
    pub name: String,
    pub db: String,
    pub table: String,
    /// StarRocks' own state text.
    pub state: String,
    pub state_class: RoutineLoadStateClass,
    pub create_time: Option<String>,
    pub pause_time: Option<String>,
    pub end_time: Option<String>,
    /// `KAFKA`, `PULSAR`, ...
    pub source_type: String,
    /// Only the topic is taken from the data source properties; broker
    /// lists and client properties (which may hold credentials) are not.
    pub topic: Option<String>,
    pub current_tasks: Option<String>,
    pub statistics: RoutineLoadStatistics,
    pub max_error_number: Option<String>,
    /// `JOB_PROPERTIES.desireTaskConcurrentNum` and `maxBatchIntervalS`.
    pub desired_tasks: Option<String>,
    pub max_batch_interval_seconds: Option<String>,
    pub partitions: Vec<RoutineLoadPartition>,
    /// The partition with the largest lag and that lag.
    pub max_lag: Option<String>,
    pub max_lag_partition: Option<String>,
    pub reason: Option<String>,
    pub other_message: Option<String>,
    pub has_error_log: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadJobsResponse {
    pub support: LoadViewSupport,
    /// The FE system time zone the time strings are in.
    pub fe_time_zone: Option<String>,
    /// Newest job first.
    pub jobs: Vec<RoutineLoadJob>,
    /// Counts and lags over `jobs`, summed by the server (task 52).
    pub summary: RoutineLoadOverview,
    pub truncated: bool,
    pub limitations: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadKindFilter {
    StreamLoad,
    RoutineLoad,
    Broker,
    Insert,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadOutcomeFilter {
    Failed,
    Filtered,
    Running,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadRecordsRequest {
    pub connection_id: String,
    pub kind: Option<LoadKindFilter>,
    pub outcome: Option<LoadOutcomeFilter>,
    /// Case-insensitive substring of `db.table`.
    pub table: Option<String>,
    /// Statistics collection and audit-log loads are hidden unless asked.
    pub include_system: bool,
    /// One hour of creation time, `YYYY-MM-DD HH` in the FE time zone, as
    /// the overview's hourly buckets name it (task 52).
    #[serde(default)]
    pub created_hour: Option<String>,
}

/// `YYYY-MM-DD HH`: digits in the fixed places only.
pub fn valid_load_hour(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 13
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b' ',
            _ => byte.is_ascii_digit(),
        })
}

impl LoadRecordsRequest {
    pub fn validate(&self) -> bool {
        valid_connection_id(&self.connection_id)
            && self.table.as_deref().is_none_or(|table| {
                table.chars().count() <= 128 && !table.chars().any(char::is_control)
            })
            && self.created_hour.as_deref().is_none_or(valid_load_hour)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadStateClass {
    Finished,
    Cancelled,
    Running,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadRecord {
    pub job_id: String,
    pub label: String,
    pub db: String,
    pub table: String,
    /// StarRocks' `TYPE` text: `STREAM_LOAD`, `ROUTINE_LOAD`, `BROKER`, `INSERT`, ...
    pub kind: String,
    pub state: String,
    pub state_class: LoadStateClass,
    pub user: Option<String>,
    pub client_ip: Option<String>,
    pub create_time: Option<String>,
    pub finish_time: Option<String>,
    /// `None` where StarRocks reports -1 (unknown) or nothing.
    pub loaded_rows: Option<String>,
    pub filtered_rows: Option<String>,
    /// Filtered over filtered plus loaded rows, 0..=10000; `None` when
    /// either count is unknown or both are zero.
    pub filtered_share_permyriad: Option<u16>,
    pub unselected_rows: Option<String>,
    pub begin_txn_ms: Option<String>,
    pub plan_ms: Option<String>,
    pub receive_data_ms: Option<String>,
    /// Finish minus create, in whole seconds (both strings are second precision).
    pub total_seconds: Option<String>,
    /// Task 56: `SCAN_ROWS` / `SCAN_BYTES`. For Stream Load and Routine Load
    /// tasks the bytes are what the client or Kafka sent; for INSERT and
    /// Broker Load what was read at the source.
    pub scan_rows: Option<String>,
    pub scan_bytes: Option<String>,
    pub commit_time: Option<String>,
    /// Commit minus create, and finish minus commit, in whole seconds.
    pub execute_seconds: Option<String>,
    pub publish_seconds: Option<String>,
    /// `PROPERTIES.timeout`.
    pub timeout_seconds: Option<String>,
    /// Total over timeout in permyriad; above 10000 when the load ran past
    /// its timeout. `None` when either is unknown.
    pub timeout_share_permyriad: Option<u32>,
    pub priority: Option<String>,
    pub error_message: Option<String>,
    pub has_error_log: bool,
    pub profile_id: Option<String>,
    pub system: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadTableSummary {
    pub table: String,
    pub loads: String,
    pub failed: String,
    /// Sum over records that report the count.
    pub filtered_rows: String,
    /// Records whose filtered count StarRocks does not know; not in the sum.
    pub filtered_unknown: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadRecordsSummary {
    /// Records StarRocks still holds (system loads excluded unless asked).
    pub loads: String,
    pub failed: String,
    /// Sums over records that report the count; unknown ones are counted
    /// apart and never added as zero.
    pub loaded_rows: String,
    pub filtered_rows: String,
    pub loaded_rows_unknown: String,
    pub filtered_rows_unknown: String,
    /// Filtered over filtered plus loaded rows, 0..=10000; `None` when any
    /// count is unknown or both sums are zero.
    pub filtered_share_permyriad: Option<u16>,
    /// Nearest-rank P95 of begin-transaction plus plan time, over records
    /// that report both; `None` with fewer than 20 such records.
    pub begin_plan_p95_ms: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadRecordsResponse {
    pub support: LoadViewSupport,
    pub fe_time_zone: Option<String>,
    /// Newest first, after filters, at most `LOAD_RECORDS_MAX`.
    pub records: Vec<LoadRecord>,
    pub matched: String,
    pub summary: LoadRecordsSummary,
    /// Busiest tables by load count, at most 10.
    pub tables: Vec<LoadTableSummary>,
    /// The oldest record StarRocks still holds.
    pub earliest_create_time: Option<String>,
    /// The visible window is probably cut by StarRocks' retention limits.
    pub at_retention_limit: bool,
    pub hidden_system: String,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorLogRequest {
    pub connection_id: String,
    pub job_id: String,
}

impl LoadErrorLogRequest {
    pub fn validate(&self) -> bool {
        valid_connection_id(&self.connection_id)
            && (1..=19).contains(&self.job_id.len())
            && self.job_id.bytes().all(|byte| byte.is_ascii_digit())
    }
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadErrorCategory {
    OutOfRange,
    TooLong,
    ColumnCount,
    NullInNonNullable,
    OutOfPartition,
    JsonCast,
    JsonStructure,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorEntry {
    pub index: u16,
    pub category: LoadErrorCategory,
    pub column: Option<String>,
    pub target_type: Option<String>,
    pub reason: String,
    pub row: Option<String>,
    /// StarRocks printed converted values (`[4, 'x', NULL]`), not the source text.
    pub row_converted: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorCategoryCount {
    pub category: LoadErrorCategory,
    pub column: Option<String>,
    pub target_type: Option<String>,
    pub count: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadErrorLogState {
    Available,
    /// The BE already deleted the file (48 hours by default).
    Purged,
    /// The BE could not be reached for the file just now; try again.
    Unavailable,
    /// StarRocks has no error log for this job.
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadErrorLogResponse {
    pub job_id: String,
    pub state: LoadErrorLogState,
    pub entries: Vec<LoadErrorEntry>,
    /// Counts over the sampled entries, largest first.
    pub categories: Vec<LoadErrorCategoryCount>,
    /// More entries existed than `LOAD_ERROR_MAX_ENTRIES`, or the text was cut.
    pub truncated: bool,
    pub limitations: Vec<String>,
}

/// Task 52: the load overview tab and the audit overview's load card. One
/// read of `loads` and one of `routine_load_jobs`, summed by the server.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadOverviewRequest {
    pub connection_id: String,
    pub kind: Option<LoadKindFilter>,
    pub include_system: bool,
}

impl LoadOverviewRequest {
    pub fn validate(&self) -> bool {
        valid_connection_id(&self.connection_id)
    }
}

/// Hourly buckets drawn by the overview: at least 72, more (up to 96)
/// only when held records start earlier than 72 hours before the newest.
pub const LOAD_OVERVIEW_HOURS: usize = 72;
pub const LOAD_OVERVIEW_MAX_HOURS: usize = 96;
pub const LOAD_OVERVIEW_TABLES: usize = 8;
pub const LOAD_OVERVIEW_LAGS: usize = 10;
pub const LOAD_OVERVIEW_PAUSED: usize = 3;

/// How much of an hour the held records can speak for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadHourCoverage {
    /// Every FE load manager in scope still holds this hour.
    Complete,
    /// Some manager in scope may already have dropped part of this hour:
    /// the counts are what is still held, a lower bound.
    Partial,
    /// Before the oldest record in scope: nothing is held, counts are not
    /// zero loads.
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHourBucket {
    /// `YYYY-MM-DD HH` in the FE time zone.
    pub hour: String,
    pub coverage: LoadHourCoverage,
    pub loads: String,
    pub failed: String,
    pub filtered_rows: String,
    /// Records of this hour whose filtered count is unknown.
    pub filtered_unknown: String,
    /// Bar heights against the axis top, 0..=10000.
    pub loads_permyriad: u16,
    pub failed_permyriad: u16,
    /// The table with the most failures in this hour, if any failed.
    pub top_failed_table: Option<String>,
    pub top_failed_count: Option<String>,
    /// Task 56: loads of each group, stacked in `LoadKindGroup` order.
    pub segments: Vec<LoadHourSegment>,
    /// Bytes for Stream Load, rows written otherwise (`volume_unit`), over
    /// records that report it; `volume_unknown` counts the others. `None`
    /// when the hour has loads and none of them reports it.
    pub volume: Option<String>,
    pub volume_unknown: String,
    pub volume_permyriad: Option<u16>,
}

/// Task 56: the overview shows each group with only the measures StarRocks
/// reports for it.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadKindGroup {
    StreamLoad,
    Insert,
    RoutineLoad,
    Broker,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadHourSegment {
    pub group: LoadKindGroup,
    pub loads: String,
    /// Against the hour axis top, 0..=10000.
    pub permyriad: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadVolumeUnit {
    Bytes,
    Rows,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadGroupSummary {
    pub group: LoadKindGroup,
    pub loads: String,
    pub failed: String,
    /// `SINK_ROWS` summed over records that report it.
    pub sink_rows: String,
    pub sink_rows_unknown: String,
    pub filtered_rows: String,
    pub filtered_unknown: String,
    /// Nearest rank of finish minus create (whole seconds) over finished
    /// records; `None` below 20 of them.
    pub total_p95_seconds: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadDurationBin {
    pub lower_seconds: String,
    /// `None` for the open last bin.
    pub upper_seconds: Option<String>,
    pub loads: String,
    /// Against the fullest bin, 0..=10000.
    pub permyriad: u16,
}

/// Finish minus create in whole seconds, over finished records.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadDurationDistribution {
    pub bins: Vec<LoadDurationBin>,
    pub samples: String,
    /// Nearest rank; `None` below 20 samples.
    pub p50_seconds: Option<String>,
    pub p95_seconds: Option<String>,
    pub max_seconds: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadFailureReason {
    /// `filtered_ratio`, `timeout`, `label_exists` or `other` for Stream
    /// Load; the `type:` StarRocks writes (`TIMEOUT`, `LOAD_RUN_FAIL`, ...)
    /// for INSERT and Broker Load.
    pub reason: String,
    pub loads: String,
    /// When the newest failure of this reason finished (its creation time
    /// when the finish time is unknown); `latest_table` is the same record's.
    pub latest_time: Option<String>,
    pub latest_table: Option<String>,
    /// Against the most frequent reason, 0..=10000.
    pub width_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadSlowRecord {
    pub job_id: String,
    pub label: String,
    pub table: String,
    pub total_seconds: String,
    pub execute_seconds: Option<String>,
    pub publish_seconds: Option<String>,
    pub timeout_seconds: Option<String>,
    /// As `LoadRecord::timeout_share_permyriad`; may exceed 10000.
    pub timeout_share_permyriad: Option<u32>,
    /// Segment lengths against the slowest listed, 0..=10000.
    pub execute_permyriad: u16,
    pub publish_permyriad: u16,
}

/// One table of one group. Sums cover records that report the value;
/// `counts_unknown` records lack at least one of them.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadGroupTable {
    pub table: String,
    pub loads: String,
    pub failed: String,
    pub scan_rows: String,
    pub scan_bytes: String,
    pub sink_rows: String,
    pub filtered_rows: String,
    pub counts_unknown: String,
    pub total_p95_seconds: Option<String>,
    /// Stream Load only; `None` below 20 samples.
    pub plan_p95_ms: Option<String>,
    /// The most common priority (INSERT and Broker Load).
    pub priority: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadSource {
    /// `RUNTIME_DETAILS.client_ip`.
    pub client_ip: Option<String>,
    pub user: Option<String>,
    pub loads: String,
    pub failed: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadStateCount {
    pub state: String,
    pub loads: String,
}

pub const LOAD_OVERVIEW_SLOWEST: usize = 8;
/// A load that used this share of its timeout counts as near the timeout.
pub const LOAD_NEAR_TIMEOUT_PERMYRIAD: u32 = 8_000;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StreamLoadDetail {
    /// `SCAN_BYTES`: what clients sent.
    pub received_bytes: String,
    pub received_bytes_unknown: String,
    pub unselected_rows: String,
    pub unselected_unknown: String,
    pub total: LoadDurationDistribution,
    /// Finish minus commit; `None` below 20 samples.
    pub publish_p95_seconds: Option<String>,
    pub near_timeout_count: String,
    /// Highest timeout share first, at most `LOAD_OVERVIEW_SLOWEST`.
    pub near_timeout: Vec<LoadSlowRecord>,
    pub failure_reasons: Vec<LoadFailureReason>,
    /// Most loads first, at most `LOAD_OVERVIEW_TABLES`.
    pub tables: Vec<LoadGroupTable>,
    pub sources: Vec<LoadSource>,
}

/// INSERT or Broker Load: second-precision timings, source and sink counts.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct BatchLoadDetail {
    pub scan_rows: String,
    pub scan_bytes: String,
    pub counts_unknown: String,
    pub duration: LoadDurationDistribution,
    /// Slowest finished first, at most `LOAD_OVERVIEW_SLOWEST`.
    pub slowest: Vec<LoadSlowRecord>,
    pub near_timeout_count: String,
    pub failure_reasons: Vec<LoadFailureReason>,
    /// Loads not yet finished, by StarRocks' state.
    pub running_states: Vec<LoadStateCount>,
    /// Most rows written first, at most `LOAD_OVERVIEW_TABLES`.
    pub tables: Vec<LoadGroupTable>,
}

/// One Routine Load job's lifetime counters and what follows from them.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineJobHealth {
    pub job_id: String,
    pub name: String,
    pub db: String,
    pub table: String,
    pub state: String,
    pub state_class: RoutineLoadStateClass,
    pub current_tasks: Option<String>,
    pub desired_tasks: Option<String>,
    /// StarRocks' lifetime averages (rows or bytes over task execution
    /// time); `None` before the first committed task.
    pub load_rows_rate: Option<String>,
    pub received_bytes_rate: Option<String>,
    pub loaded_rows: Option<String>,
    pub total_rows: Option<String>,
    pub error_rows: Option<String>,
    /// Error over total rows, 0..=10000.
    pub error_share_permyriad: Option<u16>,
    pub committed_tasks: Option<String>,
    pub aborted_tasks: Option<String>,
    /// Aborted over committed plus aborted, 0..=10000.
    pub abort_share_permyriad: Option<u16>,
    /// Task execution time over committed tasks.
    pub average_task_ms: Option<String>,
    pub max_batch_interval_seconds: Option<String>,
    pub max_lag: Option<String>,
    pub max_lag_partition: Option<String>,
    /// The newest consumed message over partitions, epoch milliseconds.
    pub newest_consumed_ms: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineJobIssue {
    pub job_id: String,
    pub name: String,
    pub state: String,
    pub state_class: RoutineLoadStateClass,
    /// Pause time for a paused job, end time for an ended one.
    pub time: Option<String>,
    pub reason: Option<String>,
    pub error_share_permyriad: Option<u16>,
    pub aborted_tasks: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadDetail {
    /// Newest job first, as read.
    pub jobs: Vec<RoutineJobHealth>,
    /// Over jobs that report both counts; `None` when none does.
    pub error_rows: Option<String>,
    pub total_rows: Option<String>,
    /// Jobs left out of the row counts because one of them is unknown.
    pub rows_unknown: String,
    pub error_share_permyriad: Option<u16>,
    pub committed_tasks: Option<String>,
    pub aborted_tasks: Option<String>,
    /// Jobs left out of the task counts because one of them is unknown.
    pub tasks_unknown: String,
    pub abort_share_permyriad: Option<u16>,
    pub average_task_ms: Option<String>,
    /// Routine Load tasks among the held load records (3.3+).
    pub held_tasks: String,
    pub tasks: LoadDurationDistribution,
    /// Paused, then ended jobs, newest first.
    pub issues: Vec<RoutineJobIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadKindCount {
    /// StarRocks' `TYPE` text.
    pub kind: String,
    pub loads: String,
    pub failed: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadTableBar {
    pub table: String,
    pub loads: String,
    pub failed: String,
    pub filtered_rows: String,
    pub filtered_unknown: String,
    /// Log-scaled against the largest, 0..=10000.
    pub width_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadPhaseBin {
    pub lower_ms: String,
    /// `None` for the open last bin.
    pub upper_ms: Option<String>,
    pub begin_txn: String,
    pub plan: String,
    pub receive_data: String,
    pub begin_txn_permyriad: u16,
    pub plan_permyriad: u16,
    pub receive_data_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadPhaseQuantiles {
    /// Records that report this phase.
    pub samples: String,
    /// Nearest rank; `None` with fewer than 20 samples.
    pub p50_ms: Option<String>,
    pub p95_ms: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadPhaseDistribution {
    pub bins: Vec<LoadPhaseBin>,
    /// Top first: the axis top, its half and zero.
    pub axis_ticks: Vec<String>,
    pub begin_txn: LoadPhaseQuantiles,
    pub plan: LoadPhaseQuantiles,
    pub receive_data: LoadPhaseQuantiles,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLagBar {
    pub job_id: String,
    pub job_name: String,
    pub db: String,
    pub table: String,
    pub partition: String,
    pub lag: String,
    pub state_class: RoutineLoadStateClass,
    pub consumed_message_ms: Option<String>,
    /// Log-scaled against the largest, 0..=10000.
    pub width_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutinePausedJob {
    pub job_id: String,
    pub name: String,
    pub db: String,
    pub table: String,
    pub max_lag: Option<String>,
    pub pause_time: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutineLoadOverview {
    pub support: LoadViewSupport,
    /// Every job read.
    pub jobs: String,
    pub running: String,
    pub scheduling: String,
    pub paused: String,
    /// Stopped or cancelled; StarRocks drops them after 3 days.
    pub ended: String,
    /// Lifetime error rows summed over jobs that report them.
    pub error_rows: String,
    /// Jobs that do not report error rows; not in the sum.
    pub error_rows_unknown: String,
    /// Largest partition lags of jobs that have not ended.
    pub lags: Vec<RoutineLagBar>,
    pub max_running_lag: Option<RoutineLagBar>,
    /// Newest paused first, at most `LOAD_OVERVIEW_PAUSED`.
    pub paused_jobs: Vec<RoutinePausedJob>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoadOverviewResponse {
    /// Load records; Routine Load has its own in `routine`.
    pub support: LoadViewSupport,
    pub fe_time_zone: Option<String>,
    pub summary: LoadRecordsSummary,
    /// Failed over loads, 0..=10000; `None` with no loads.
    pub failed_share_permyriad: Option<u16>,
    /// Most loads first.
    pub by_kind: Vec<LoadKindCount>,
    /// Oldest first, ending at the hour of the newest record.
    pub hours: Vec<LoadHourBucket>,
    /// Records in scope that no drawn hour holds (no readable creation
    /// time, or older than `LOAD_OVERVIEW_MAX_HOURS`).
    pub records_outside_hours: String,
    /// Top first: the axis top, its half and zero.
    pub hour_axis_ticks: Vec<String>,
    /// Task 56: every group among the visible records, whatever `kind`
    /// selects, in `LoadKindGroup` order; groups without records are left out.
    pub groups: Vec<LoadGroupSummary>,
    /// Visible records of every group.
    pub all_loads: String,
    pub volume_unit: LoadVolumeUnit,
    pub volume_axis_ticks: Vec<String>,
    /// Only for the matching `kind`.
    pub stream: Option<StreamLoadDetail>,
    pub batch: Option<BatchLoadDetail>,
    pub routine_detail: Option<RoutineLoadDetail>,
    /// Tables with filtered rows, most first.
    pub tables: Vec<LoadTableBar>,
    pub phases: LoadPhaseDistribution,
    pub routine: RoutineLoadOverview,
    /// The oldest record in scope; hours before it are `unknown`.
    pub earliest_create_time: Option<String>,
    /// From here on every manager in scope still holds its records; hours
    /// between the two, and the hour holding this time unless it is on the
    /// hour, are `partial`. Equal to `earliest_create_time` when
    /// one manager covers the scope.
    pub complete_from: Option<String>,
    pub at_retention_limit: bool,
    pub hidden_system: String,
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_validate_their_identifiers() {
        assert!(
            LoadErrorLogRequest {
                connection_id: "c1".into(),
                job_id: "10256".into()
            }
            .validate()
        );
        assert!(
            !LoadErrorLogRequest {
                connection_id: "c1".into(),
                job_id: "1 OR 1=1".into()
            }
            .validate()
        );
        assert!(
            !LoadErrorLogRequest {
                connection_id: "c1".into(),
                job_id: "".into()
            }
            .validate()
        );
        let records = LoadRecordsRequest {
            connection_id: "c1".into(),
            kind: None,
            outcome: None,
            table: Some("ods.orders".into()),
            include_system: false,
            created_hour: Some("2026-10-30 06".into()),
        };
        assert!(records.validate());
        assert!(
            !LoadRecordsRequest {
                created_hour: Some("2026-10-30 6:00".into()),
                ..records.clone()
            }
            .validate()
        );
        assert!(
            !LoadRecordsRequest {
                table: Some("a\nb".into()),
                ..records
            }
            .validate()
        );
    }
}
