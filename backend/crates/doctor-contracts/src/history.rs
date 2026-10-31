//! Stage B: the local history store and its bounded incremental collection.
//! Times are epoch seconds in the audit session's time zone, as every audit
//! scope. Nothing here carries SQL text, Profile text or credentials.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryStatusRequest {
    /// The connected session to report collection for, if any.
    pub connection_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum HistoryCollectionAction {
    Pause,
    Resume,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryCollectionRequest {
    pub connection_id: String,
    pub action: HistoryCollectionAction,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum HistoryStoreState {
    Available {
        retention_days: u16,
        file_bytes: Option<String>,
        templates: String,
        buckets: String,
    },
    /// No history path configured, or the operator turned history off.
    Disabled,
    /// The configured file could not be opened; nothing is collected.
    /// Only a category and stable code are public, never paths or error text.
    Unavailable {
        /// Always emitted by current backends; TypeScript readers also accept
        /// older history responses that contain only the state.
        #[ts(as = "Option<HistoryStoreUnavailableReason>", optional)]
        reason: HistoryStoreUnavailableReason,
        #[ts(as = "Option<String>", optional)]
        code: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum HistoryStoreUnavailableReason {
    MigrationRequiresManualResolution,
    IoOrPermission,
    Other,
}

impl HistoryStoreUnavailableReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::MigrationRequiresManualResolution => {
                "history_store_migration_requires_manual_resolution"
            }
            Self::IoOrPermission => "history_store_io_or_permission",
            Self::Other => "history_store_open_failed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CollectionStoppedReason {
    /// No connected session: reconnect to continue from the watermark.
    NotConnected,
    HistoryDisabled,
    /// A remote (non-loopback) cluster: production collection waits for G1.
    ProductionGate,
    AuditUnavailable,
    /// Only one collector runs per process; another session already collects.
    AnotherSessionCollecting,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum CollectionState {
    Running {
        next_run_seconds: u32,
    },
    Paused,
    /// Failed reads back off exponentially; nothing is retried in a burst.
    Backoff {
        failures: u32,
        next_run_seconds: u32,
    },
    Stopped {
        reason: CollectionStoppedReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CollectionBudget {
    pub period_seconds: u32,
    pub slice_seconds: u32,
    /// A slice is read only once this long has passed after its end.
    pub lag_seconds: u32,
    pub slice_row_limit: u32,
    pub query_timeout_seconds: u32,
    /// Older unread spans are skipped and recorded as gaps.
    pub catch_up_seconds: u32,
    /// The production budget comes from the G1 pilot.
    pub production_budget_measured: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CollectionRunState {
    Complete,
    Partial,
    Failed,
    Gap,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CollectionRun {
    pub started_seconds: u32,
    pub slice_start_seconds: u32,
    pub slice_end_seconds: u32,
    pub state: CollectionRunState,
    /// e.g. `row_limit`, `read_failed:<stage>`, `beyond_catch_up`.
    pub reason: Option<String>,
    pub rows_read: String,
    pub attributed: String,
    pub partial_statements: String,
    pub unavailable_statements: String,
    pub duration_ms: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CoverageHourState {
    Complete,
    Partial,
    /// Not read (gap, failure, or never collected): not zero executions.
    NotCollected,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CoverageHour {
    pub hour_start_seconds: u32,
    pub state: CoverageHourState,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryStatusResponse {
    pub store: HistoryStoreState,
    pub target: Option<String>,
    pub collection: CollectionState,
    pub watermark_seconds: Option<u32>,
    pub budget: CollectionBudget,
    /// Most recent last; the hour holding `now` is the last entry.
    pub coverage: Vec<CoverageHour>,
    /// Newest first.
    pub runs: Vec<CollectionRun>,
    /// Remote clusters are collected only after the operator enables it post-G1.
    pub remote_collection_enabled: bool,
    /// Open (unacknowledged) in-app alerts of the shown target, for the badge.
    pub open_alerts: Option<String>,
    /// Task 59: changes when load collection moves or any load alert changes,
    /// so views re-read even when the open count stays the same.
    pub load_revision: Option<String>,
    pub limitations: Vec<String>,
}
