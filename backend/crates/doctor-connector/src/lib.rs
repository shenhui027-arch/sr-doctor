//! Narrow StarRocks 3.5.14 connection policy and short-lived session store.
//!
//! This crate deliberately has no generic SQL console and no Profile parser.
//! A MySQL transport must implement `RemoteFactory`; every operation reaches it
//! through the closed `RemoteOperation` enum below.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use doctor_contracts::{
    AUDIT_FINGERPRINT_READ_LIMIT, AUDIT_FINGERPRINT_STATEMENT_CHARS, AuditFamilyRequest,
    AuditFamilySort, AuditMetricId, AuditMetricInput, AuditOccurrenceRequest,
    AuditOverviewFamilyTopSort, AuditOverviewPrimary, AuditOverviewRequest, AuditScope,
    AuditStatementFilter, AuditStatementFilterCoverage, ExplainMode, OfficialProfileIdentity,
    QueryListStatementFilterCoverage, RequestedSourceKind, RuntimeInventoryAcquisitionShape,
};
use doctor_core::InputEnvelope;

mod operation_control;
pub use operation_control::OperationControl;
mod mysql_text;
pub use mysql_text::MysqlTextRemoteFactory;
mod audit_family_session;
mod audit_workload;
pub use audit_family_session::{AuditFamilyDetailWorkload, PreparedAuditFamilyDetail};
use audit_family_session::{
    SuccessfulProfileEvidence, family_bindings, occurrence_family_bindings, publish_family_bindings,
};
use audit_workload::ERROR_PROPORTION_EXACT_POPULATION_MAX;
pub use audit_workload::{
    AUDIT_ACTIVE_AT_PARTICIPANT_LIMIT, AUDIT_COLLECTION_SLICE_LIMIT, AUDIT_CONCURRENCY_MAX_STEPS,
    AUDIT_CONCURRENCY_MAX_TRANSITIONS, AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT,
    AUDIT_HISTORICAL_LOOKBACK_SECONDS, AUDIT_OVERVIEW_FAMILY_TOP_LIMIT,
    AUDIT_QUERY_TIMEOUT_SECONDS, AUDIT_STATEMENT_SAMPLE_CHARS, AuditActiveAtParticipantSource,
    AuditActiveAtSource, AuditColumn, AuditConcurrencyCoverageSource, AuditConcurrencyDeltaSource,
    AuditConcurrencySource, AuditErrorCodeSource, AuditFamilyDetailSource, AuditFamilySourceRow,
    AuditFamilyTarget, AuditOccurrenceSampleWindow, AuditOccurrenceSourceRow, AuditSchema,
    AuditSourceStatementSample, AuditSqlBuildError, AuditSqlLookupRequest,
    AuditStatementSampleSource, AuditTableName, DOCTOR_STATEMENT_MARKER, DOCTOR_STATEMENT_MARKERS,
    attach_family_samples, attach_occurrence_samples, audit_active_at_sql_lookup_statement,
    audit_active_at_statement, audit_collection_slice_statement, audit_concurrency_statement,
    audit_coverage_statement, audit_family_detail_sql_lookup_statement,
    audit_family_detail_statement, audit_family_sample_statement, audit_family_statement,
    audit_fingerprint_event_statement, audit_fingerprint_sample_statement,
    audit_historical_source_window, audit_occurrence_sample_statement, audit_occurrence_statement,
    audit_sql_lookup_statement, decode_active_at_source_rows, decode_concurrency_source_rows,
    decode_family_detail_source_rows, decode_family_sample_row, decode_family_source_row,
    decode_occurrence_sample_row, decode_occurrence_source_row, doctor_statement,
    family_sample_targets, occurrence_sample_targets,
};
pub use audit_workload::{
    AUDIT_PERFORMANCE_SUMMARY_COLUMNS, AUDIT_PERFORMANCE_TOP_LIMIT,
    AuditPerformanceDimensionSource, AuditPerformanceErrorSource, AuditPerformanceSource,
    AuditPerformanceSummarySource, audit_performance_bucket_count, audit_performance_statement,
    decode_performance_source_rows, summary_cell as audit_performance_summary_cell,
};

/// The release the connector fixtures report; every audited release (3.1–4.1) connects.
pub const SUPPORTED_VERSION: &str = "3.5.14";
pub const DEFAULT_AUDIT_TABLE: &str = "starrocks_audit_db__.starrocks_audit_tbl__";
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
pub const MAX_SESSIONS: usize = 3;
pub const MAX_QUERY_ROWS: u16 = 500;
pub const MAX_QUERY_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_PLAN_SQL_BYTES: usize = 100 * 1024;
pub const ANALYZE_PROFILE_MAX_NODE_IDS: usize = 32;
pub const ANALYZE_PROFILE_MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const ANALYZE_PROFILE_TIMEOUT_SECONDS: u64 = 10;
const MAX_EXECUTION_ATTEMPTS_PER_SESSION: usize = 64;

/// User input. `password` is intentionally write-only: it is never copied to a view or error.
pub struct CreateConnectionRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub default_catalog: Option<String>,
    pub default_database: Option<String>,
    pub audit_table: Option<String>,
    pub tls_mode: TlsMode,
}

/// A write-only credential.  Deliberately does not implement `Debug` or
/// `Display`, so it cannot be accidentally emitted by connector telemetry.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsMode {
    VerifyFull,
    /// The one-shot local Docker fixture: plaintext, literal loopback only.
    DisabledLoopbackDocker,
    /// Plaintext to any host the user names, like a desktop SQL client. The
    /// user chose it explicitly; credentials and SQL cross the network unencrypted.
    Plaintext,
}

impl TlsMode {
    pub fn is_plaintext(self) -> bool {
        !matches!(self, Self::VerifyFull)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Connected,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityState {
    Available,
    Unavailable,
    Forbidden,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub time_zone: CapabilityState,
    pub audit_loader: CapabilityState,
    pub audit_user_filter: CapabilityState,
    pub audit_statement_filter: CapabilityState,
    pub profile_list: CapabilityState,
    pub get_query_profile: CapabilityState,
    pub explain: CapabilityState,
    pub plan_advisor: CapabilityState,
}

impl Capabilities {
    fn unsupported() -> Self {
        Self {
            time_zone: CapabilityState::Unknown,
            audit_loader: CapabilityState::Unknown,
            audit_user_filter: CapabilityState::Unknown,
            audit_statement_filter: CapabilityState::Unknown,
            profile_list: CapabilityState::Unknown,
            get_query_profile: CapabilityState::Unknown,
            explain: CapabilityState::Unknown,
            plan_advisor: CapabilityState::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionSessionView {
    pub connection_id: String,
    pub state: ConnectionState,
    pub endpoint_label: String,
    pub detected_version: String,
    pub time_zone: Option<SessionTimeZone>,
    pub tls_mode: TlsMode,
    pub capabilities: Capabilities,
    pub limitations: Vec<String>,
    pub idle_timeout_seconds: u64,
}

/// Time-zone evidence from the exact MySQL session used for Audit/Profile reads.
/// The name is preserved as reported by StarRocks; the offset is observational
/// evidence at connection time and is not reused to reinterpret historical rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTimeZone {
    pub session_time_zone: String,
    pub system_time_zone: String,
    pub effective_time_zone: String,
    pub current_utc_offset_seconds: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuerySource {
    Auto,
    AuditLoader,
    ProfileList,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuerySearch {
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub state: Option<String>,
    pub digest: Option<String>,
    pub limit: u16,
    pub source: QuerySource,
    pub statement_filter: AuditStatementFilter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryBrowseResult {
    pub source: QuerySource,
    pub discovery_generation: u32,
    pub rows: Vec<Vec<String>>,
    pub plan_sql_references: BTreeMap<String, String>,
    pub ambiguous_query_ids: BTreeSet<String>,
    pub rejected_row_count: usize,
    pub degraded_reason: Option<String>,
    pub time_zone: Option<SessionTimeZone>,
    pub statement_filter: QueryListStatementFilterCoverage,
}

/// A bounded ProfileList observation dedicated to manual Running snapshots.
/// The source cap and local cap are intentionally separate so callers never
/// mistake an incomplete cache window for a complete Running-query inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningProfileCandidates {
    pub discovery_generation: u32,
    pub limit: u16,
    /// Number of Running source rows selected before strict retained-row checks.
    pub selected_row_count: usize,
    pub rows: Vec<Vec<String>>,
    pub plan_sql_references: BTreeMap<String, String>,
    pub ambiguous_query_ids: BTreeSet<String>,
    pub rejected_row_count: usize,
    pub at_limit: bool,
    pub local_truncated: bool,
    pub time_zone: Option<SessionTimeZone>,
    /// Marked rows of this tool hidden from the Running rows read this time.
    pub statement_filter: QueryListStatementFilterCoverage,
}

/// This connection-local identity is intentionally not a cluster ID. It keeps
/// a V2 audit response tied to the session that observed it until a trusted
/// native cluster identity is introduced by a separately verified capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditSessionIdentity {
    pub connection_id: String,
    pub detected_version: String,
    pub cluster_identity: AuditClusterIdentity,
    /// Evidence for interpreting the raw AuditLoader `timestamp` and the
    /// connector-produced `event_epoch_seconds`. The connector does not set or
    /// assume UTC: `event_epoch_seconds` is `UNIX_TIMESTAMP(timestamp)` in
    /// this same verified session.
    pub audit_timestamp_time_zone: Option<SessionTimeZone>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditClusterIdentity {
    SessionScoped,
}

/// Decimal source counts from one full requested window. They intentionally
/// remain strings so a downstream JSON/TypeScript boundary cannot lose u64
/// precision. `text_present_count` is not a typed-validity result: core owns
/// invalid/overflow/precision classification through its single parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditMetricInputCoverage {
    pub metric_id: AuditMetricId,
    pub sql_null_count: String,
    pub text_present_count: String,
    pub nonnegative_count: String,
    pub negative_count: String,
    pub aggregate_input: AuditMetricInput,
    pub aggregation: AuditMetricAggregation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditMetricAggregation {
    Sum,
    Max,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditWindowCoverage {
    pub raw_row_count: String,
    pub invalid_query_id_count: String,
    pub exact_duplicate_row_count: String,
    pub conflicting_duplicate_row_count: String,
    pub missing_digest_count: String,
    pub missing_context_count: String,
    pub retained_event_count: String,
    pub retained_family_count: String,
    pub error_count: String,
    pub metric_inputs: Vec<AuditMetricInputCoverage>,
    pub statement_filter: AuditStatementFilterCoverage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditOccurrenceWorkload {
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub session_identity: AuditSessionIdentity,
    pub rows: Vec<AuditOccurrenceSourceRow>,
    pub coverage: AuditWindowCoverage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilyWorkload {
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub session_identity: AuditSessionIdentity,
    pub rows: Vec<AuditFamilySourceRow>,
    pub coverage: AuditWindowCoverage,
}

/// Digest-missing executions for product fingerprints. Nothing here is
/// published to discovery, so these rows grant no SQL or Profile lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFingerprintWorkload {
    pub scope: AuditScope,
    pub session_identity: AuditSessionIdentity,
    /// Newest first, at most `AUDIT_FINGERPRINT_READ_LIMIT`, each carrying its
    /// fingerprint-length statement prefix.
    pub rows: Vec<AuditOccurrenceSourceRow>,
    /// Exact decimal count of eligible executions in the whole scope.
    pub eligible_count: String,
}

/// One stage B collection slice: the executions read (oldest first, each with
/// its statement prefix) and the slice's full canonical count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditCollectionSlice {
    pub session_identity: AuditSessionIdentity,
    pub rows: Vec<AuditOccurrenceSourceRow>,
    /// Exact decimal count of canonical executions in the whole slice.
    pub slice_count: String,
    /// The tool's own and non-query statements were excluded at the source.
    pub statement_filter_applied: bool,
}

/// A2 sources: one page of exact families with long samples, the window
/// coverage, and the digest-missing executions (scope and identity there).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditSimilarWorkload {
    pub families: Vec<AuditFamilySourceRow>,
    pub coverage: AuditWindowCoverage,
    pub fingerprint: AuditFingerprintWorkload,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditOverviewPrimaryWorkload {
    Occurrences {
        rows: Vec<AuditOccurrenceSourceRow>,
        limit: u16,
    },
    Families {
        rows: Vec<AuditFamilySourceRow>,
        limit: u16,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditOverviewWorkload {
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub session_identity: AuditSessionIdentity,
    pub primary: AuditOverviewPrimaryWorkload,
    pub coverage: AuditWindowCoverage,
    pub concurrency: AuditConcurrencySource,
    pub family_top_sort: AuditOverviewFamilyTopSort,
    pub family_top_rows: Vec<AuditFamilySourceRow>,
}

/// One bounded window aggregation. It publishes no SQL or family binding, so
/// it never moves the discovery generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPerformanceWorkload {
    pub scope: AuditScope,
    pub bucket_seconds: u32,
    pub session_identity: AuditSessionIdentity,
    pub user_column: bool,
    pub error_code_column: bool,
    pub pending_time_column: bool,
    pub source: AuditPerformanceSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditActiveAtWorkload {
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub at_milliseconds: i128,
    pub session_identity: AuditSessionIdentity,
    pub active_count: String,
    pub rows: Vec<AuditActiveAtParticipantSource>,
    pub truncated: bool,
}

/// The connector keeps Profile text opaque and hands it to the one core adapter.
#[derive(Clone, Eq, PartialEq)]
pub struct FetchedProfile {
    pub query_id: String,
    pub detected_version: String,
    pub query_context: FetchedQueryContext,
    profile_text: Vec<u8>,
}

/// Opaque Profile produced by the explicit, bounded runtime-analysis action.
/// SQL text is deliberately absent and cannot cross the connector boundary.
#[derive(Clone, Eq, PartialEq)]
pub struct ExecutedProfile {
    pub query_id: String,
    pub detected_version: String,
    /// Set only after this session recorded a completed standard attempt for
    /// the validated query ID. This remains absent for legacy and native paths.
    pub verified_standard_execution_attempt_id: Option<String>,
    profile_text: Vec<u8>,
}

/// Only one server-verified identity and bounded native plan node IDs are accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyzeProfileRequest {
    pub query_id: String,
    pub identity: OfficialProfileIdentity,
    pub node_ids: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficialProfileLifecycle {
    Finished,
    Running,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficialProfileUnavailableReason {
    LifecycleUnknown,
    Empty,
    PermissionDenied,
    RemoteUnsupported,
    /// StarRocks no longer retains the Profile (evicted or expired).
    NotRetained,
}

#[derive(Clone, Eq, PartialEq)]
pub enum OfficialProfileContent {
    Available {
        rows: Vec<String>,
    },
    Unavailable {
        reason: OfficialProfileUnavailableReason,
    },
}

/// Official formatter output, not raw Profile input. The API owns reading time
/// and source hashing; discovery lifecycle is not a fresh execution-status probe.
#[derive(Clone, Eq, PartialEq)]
pub struct OfficialProfileReading {
    pub query_id: String,
    pub identity: OfficialProfileIdentity,
    pub node_ids: Vec<u32>,
    pub detected_version: String,
    pub default_database: Option<String>,
    /// Present only for the discovery identity. Standard execution does not
    /// invent a discovery row or generation for its completed query.
    pub query_context: Option<FetchedQueryContext>,
    pub observed_lifecycle: Option<OfficialProfileLifecycle>,
    pub content: OfficialProfileContent,
}

impl std::fmt::Debug for OfficialProfileReading {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OfficialProfileReading { redacted: true }")
    }
}

impl ExecutedProfile {
    pub fn into_adapter_input(self) -> InputEnvelope {
        let mut input = InputEnvelope::new(RequestedSourceKind::TextProfile, self.profile_text);
        input.acquisition_shape = RuntimeInventoryAcquisitionShape::GetQueryProfile;
        input
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedQueryContext {
    pub source: QuerySource,
    pub discovery_generation: u32,
    pub row_index: usize,
    pub row: Vec<String>,
    pub time_zone: Option<SessionTimeZone>,
    pub plan_sql_ref: Option<String>,
}

impl FetchedProfile {
    pub fn into_adapter_input(self) -> InputEnvelope {
        // The server handshake version is connection evidence, not a user-declared
        // Profile version hint. The RuntimeProfile adapter extracts its own reported
        // version and keeps the two evidence sources semantically separate.
        let mut input = InputEnvelope::new(RequestedSourceKind::TextProfile, self.profile_text);
        input.acquisition_shape = RuntimeInventoryAcquisitionShape::GetQueryProfile;
        input
    }
}

/// SQL retained only for an explicit lookup from the current discovery
/// generation. It is intentionally separate from public query-list rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedQuerySql {
    pub query_id: String,
    pub discovery_generation: u32,
    pub sql: String,
}

/// Opaque COSTS rows and context captured under the same session lock.
/// Database is handshake-selected context, not evidence about tables in the plan.
/// The assembler must fingerprint/restrict it before including it in a public DTO.
#[derive(Clone, Eq, PartialEq)]
pub struct ExplainResult {
    pub rows: Vec<String>,
    pub mode: ExplainMode,
    pub detected_version: String,
    pub default_database: Option<String>,
}

impl std::fmt::Debug for ExplainResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExplainResult")
            .field("row_count", &self.rows.len())
            .field("mode", &self.mode)
            .field("version_detected", &!self.detected_version.is_empty())
            .field("database_selected", &self.default_database.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvisorResult {
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorErrorCode {
    InvalidRequest,
    TlsRequired,
    ConnectionFailed,
    TlsFailed,
    AuthenticationFailed,
    PermissionDenied,
    CapabilityUnavailable,
    UnsupportedVersion,
    ConnectionExpired,
    ProfileUnavailable,
    QueryDiscoveryStale,
    QuerySqlUnavailable,
    StatementNotAllowed,
    ExplainRejected,
    RemoteTimeout,
    SessionLimit,
    ExecutionAttemptSubmitted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorError {
    pub code: ConnectorErrorCode,
    pub stage: &'static str,
    pub limitation: Option<&'static str>,
    pub affected_side: Option<&'static str>,
}

impl ConnectorError {
    fn new(code: ConnectorErrorCode, stage: &'static str) -> Self {
        Self {
            code,
            stage,
            limitation: None,
            affected_side: None,
        }
    }
    fn with_limitation(
        code: ConnectorErrorCode,
        stage: &'static str,
        limitation: &'static str,
    ) -> Self {
        Self {
            code,
            stage,
            limitation: Some(limitation),
            affected_side: None,
        }
    }

    fn for_side(mut self, side: &'static str) -> Self {
        self.affected_side = Some(side);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteError {
    Connection,
    Tls,
    Authentication,
    PermissionDenied,
    Unsupported,
    Timeout,
    Protocol,
    OutputBudgetExceeded,
    /// `ANALYZE PROFILE` on a query whose Profile StarRocks no longer retains
    /// (exact official 1064 / HY000 / "query not exists"); the session stays usable.
    QueryProfileNotFound,
}

impl RemoteError {
    fn connector(self, stage: &'static str) -> ConnectorError {
        if self == Self::OutputBudgetExceeded {
            return ConnectorError::with_limitation(
                ConnectorErrorCode::ConnectionFailed,
                stage,
                "official_profile_output_budget_exceeded",
            );
        }
        let code = match self {
            Self::Connection => ConnectorErrorCode::ConnectionFailed,
            Self::Tls => ConnectorErrorCode::TlsFailed,
            Self::Authentication => ConnectorErrorCode::AuthenticationFailed,
            Self::PermissionDenied => ConnectorErrorCode::PermissionDenied,
            Self::Unsupported => ConnectorErrorCode::CapabilityUnavailable,
            Self::Timeout => ConnectorErrorCode::RemoteTimeout,
            Self::Protocol => ConnectorErrorCode::ConnectionFailed,
            Self::QueryProfileNotFound => ConnectorErrorCode::ProfileUnavailable,
            Self::OutputBudgetExceeded => unreachable!(),
        };
        ConnectorError::new(code, stage)
    }

    fn capability(self) -> CapabilityState {
        match self {
            Self::PermissionDenied => CapabilityState::Forbidden,
            Self::Unsupported => CapabilityState::Unavailable,
            Self::Connection
            | Self::Tls
            | Self::Authentication
            | Self::Timeout
            | Self::Protocol
            | Self::QueryProfileNotFound => CapabilityState::Unknown,
            Self::OutputBudgetExceeded => CapabilityState::Unknown,
        }
    }
}

/// Closed operation surface. User SQL reaches the transport only as either a
/// validated bare SELECT/WITH or the fixed native EXPLAIN ANALYZE wrapper.
pub enum RemoteOperation {
    CurrentVersion,
    SessionTimeZone,
    AuditProbe {
        table: String,
    },
    AuditSearch {
        table: String,
        parameters: QueryParameters,
    },
    AuditSearchHiddenCounts {
        table: String,
        parameters: QueryParameters,
    },
    AuditSchemaProbe {
        table: AuditTableName,
    },
    AuditOccurrences {
        table: AuditTableName,
        request: AuditOccurrenceRequest,
        schema: AuditSchema,
    },
    AuditFamilies {
        table: AuditTableName,
        request: AuditFamilyRequest,
        schema: AuditSchema,
    },
    AuditFamilyDetail {
        table: AuditTableName,
        parent: AuditScope,
        target: AuditFamilyTarget,
        candidates: Vec<String>,
        schema: AuditSchema,
    },
    /// One `stmt` prefix per returned family (the family page only).
    AuditFamilySamples {
        table: AuditTableName,
        scope: AuditScope,
        targets: Vec<AuditFamilyTarget>,
        schema: AuditSchema,
        /// Prefix length: the 240-character list sample, or the fingerprint length.
        chars: u16,
    },
    /// One `stmt` prefix per returned execution of an occurrence list.
    AuditOccurrenceSamples {
        table: AuditTableName,
        scope: AuditScope,
        window: AuditOccurrenceSampleWindow,
        query_ids: Vec<String>,
        schema: AuditSchema,
    },
    AuditFamilyDetailSqlLookup {
        table: AuditTableName,
        parent: AuditScope,
        target: AuditFamilyTarget,
        query_id: String,
        schema: AuditSchema,
    },
    /// One stage B collection slice, oldest first, with statement prefixes.
    AuditCollectionSlice {
        table: AuditTableName,
        scope: AuditScope,
        schema: AuditSchema,
    },
    /// The newest digest-missing executions, for product fingerprints.
    AuditFingerprintEvents {
        table: AuditTableName,
        scope: AuditScope,
        schema: AuditSchema,
    },
    /// The fingerprint-length `stmt` prefix of each of those executions.
    AuditFingerprintSamples {
        table: AuditTableName,
        scope: AuditScope,
        query_ids: Vec<String>,
        schema: AuditSchema,
    },
    AuditCoverage {
        table: AuditTableName,
        scope: AuditScope,
        schema: AuditSchema,
    },
    AuditConcurrency {
        table: AuditTableName,
        scope: AuditScope,
        schema: AuditSchema,
    },
    AuditActiveAt {
        table: AuditTableName,
        scope: AuditScope,
        at_milliseconds: i128,
        schema: AuditSchema,
    },
    /// The one bounded performance-overview aggregation of a window.
    AuditPerformance {
        table: AuditTableName,
        scope: AuditScope,
        bucket_seconds: u32,
        schema: AuditSchema,
    },
    AuditOverviewFamilyTop {
        table: AuditTableName,
        request: AuditFamilyRequest,
        schema: AuditSchema,
    },
    AuditSqlLookup {
        table: AuditTableName,
        request: AuditSqlLookupRequest,
        schema: AuditSchema,
    },
    AuditActiveAtSqlLookup {
        table: AuditTableName,
        request: AuditSqlLookupRequest,
        at_milliseconds: i128,
        schema: AuditSchema,
    },
    ShowProfileList {
        limit: u16,
    },
    GetQueryProfile {
        query_id: String,
    },
    AnalyzeProfile {
        query_id: String,
        node_ids: Vec<u32>,
    },
    ShowPlanAdvisor,
    Explain {
        candidate_sql: String,
        mode: ExplainMode,
    },
    ProfiledSelect {
        candidate_sql: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParameters {
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub state: Option<String>,
    pub digest: Option<String>,
    pub limit: u16,
    pub statement_filter: AuditStatementFilter,
}

pub enum RemoteResponse {
    Version(String),
    TimeZone(SessionTimeZone),
    Rows(Vec<Vec<String>>),
    NullableRows(Vec<Vec<Option<String>>>),
    Profile(String),
    AnalyzeProfile(Vec<String>),
    Explain(Vec<String>),
    ProfiledQuery { query_id: String, profile: String },
}

/// Boundary for the future MySQL client. Implementations must disable multi-statements.
pub trait Remote: Send {
    fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError>;
    fn close(&mut self) {}
    fn set_audit_control(
        &mut self,
        _control: Option<Arc<OperationControl>>,
    ) -> Result<(), RemoteError> {
        Ok(())
    }
}

pub trait RemoteFactory: Send + Sync {
    fn open(&self, options: RemoteConnectOptions) -> Result<Box<dyn Remote>, RemoteError>;
}

/// Transport-only fields, with TLS and multi-statement policy already fixed by validation.
pub struct RemoteConnectOptions {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    pub default_catalog: Option<String>,
    pub default_database: Option<String>,
    pub tls_mode: TlsMode,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub struct MonotonicClock(Instant);

impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

pub struct SessionManager {
    factory: Arc<dyn RemoteFactory>,
    clock: Arc<dyn Clock>,
    idle_ttl: Duration,
    sessions: Mutex<SessionStore>,
    operation_done: std::sync::Condvar,
}

struct SessionStore {
    active: BTreeMap<String, Session>,
    pending_connects: usize,
}

struct Session {
    audit_operation: Option<Arc<OperationControl>>,
    view: ConnectionSessionView,
    audit_table: AuditTableName,
    audit_schema: Option<AuditSchema>,
    default_database: Option<String>,
    remote: Box<dyn Remote>,
    plan_sql_by_ref: BTreeMap<String, StoredPlanSql>,
    observed_queries: BTreeMap<String, ObservedQuery>,
    discovery_generation: u32,
    audit_discovery: Option<AuditDiscovery>,
    successful_profiles: SuccessfulProfileEvidence,
    execution_attempts: BTreeMap<String, ExecutionAttempt>,
    last_used: Duration,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExecutionAttemptMethod {
    StandardProfile,
}

#[derive(Clone, Eq, PartialEq)]
struct ExecutionAttempt {
    method: ExecutionAttemptMethod,
    /// A claimed attempt stays incomplete when its remote execution fails or
    /// yields an invalid/empty Profile. Only a completed standard execution
    /// can later authorize a fixed official Profile read.
    completed_query_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuditDiscovery {
    generation: u32,
    scope: AuditScope,
    unique_query_ids: BTreeSet<String>,
    selector: AuditDiscoverySelector,
    families_by_key: BTreeMap<String, AuditFamilyTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuditDiscoverySelector {
    OccurrenceList,
    OverviewPrimary,
    ActiveAt {
        at_milliseconds: i128,
    },
    FamilyDetail {
        parent: AuditScope,
        family_key: String,
    },
}

struct StoredPlanSql {
    query_id: String,
    discovery_generation: u32,
    sql: String,
}

#[derive(Clone)]
struct ObservedQuery {
    source: QuerySource,
    discovery_generation: u32,
    row_index: usize,
    row: Vec<String>,
    profile_eligible: bool,
    plan_sql_ref: Option<String>,
    sql: Option<String>,
}

/// Reservation prevents concurrent slow handshakes from bypassing the
/// process-wide session limit. It is released on every early return and is
/// atomically converted into an active session only after all probes succeed.
struct SessionReservation<'a> {
    sessions: &'a Mutex<SessionStore>,
    active: bool,
}

impl SessionReservation<'_> {
    fn activate(mut self, connection_id: String, session: Session) {
        let mut sessions = lock_sessions(self.sessions);
        sessions.pending_connects = sessions.pending_connects.saturating_sub(1);
        sessions.active.insert(connection_id, session);
        self.active = false;
    }
}

impl Drop for SessionReservation<'_> {
    fn drop(&mut self) {
        if self.active {
            let mut sessions = lock_sessions(self.sessions);
            sessions.pending_connects = sessions.pending_connects.saturating_sub(1);
        }
    }
}

impl SessionManager {
    pub fn new(factory: Arc<dyn RemoteFactory>) -> Self {
        Self::with_seams(
            factory,
            Arc::new(MonotonicClock::default()),
            DEFAULT_IDLE_TTL,
        )
    }

    pub fn with_seams(
        factory: Arc<dyn RemoteFactory>,
        clock: Arc<dyn Clock>,
        idle_ttl: Duration,
    ) -> Self {
        Self {
            factory,
            clock,
            idle_ttl,
            operation_done: std::sync::Condvar::new(),
            sessions: Mutex::new(SessionStore {
                active: BTreeMap::new(),
                pending_connects: 0,
            }),
        }
    }

    pub fn connect(
        &self,
        request: CreateConnectionRequest,
    ) -> Result<ConnectionSessionView, ConnectorError> {
        let validated = ValidatedRequest::new(request)?;
        let now = self.clock.now();
        let reservation = self.reserve_connection_slot(now)?;
        let audit_table = AuditTableName::parse(validated.audit_table.clone())
            .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
        let tls_mode = validated.tls_mode;
        let mut remote = self
            .factory
            .open(validated.connect_options())
            .map_err(|error| error.connector("connection"))?;
        let detected_version = match remote
            .run(RemoteOperation::CurrentVersion)
            .map_err(|error| error.connector("detect"))?
        {
            RemoteResponse::Version(version) => version,
            _ => {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::ConnectionFailed,
                    "detect",
                ));
            }
        };
        let release = supported_release(&detected_version);
        let supported = release.is_some();
        let mut capabilities = Capabilities::unsupported();
        let mut limitations = Vec::new();
        let mut time_zone = None;
        let mut audit_schema = None;
        if supported {
            match remote.run(RemoteOperation::SessionTimeZone) {
                Ok(RemoteResponse::TimeZone(evidence)) => {
                    capabilities.time_zone = CapabilityState::Available;
                    time_zone = Some(evidence);
                }
                Ok(_) => {
                    return Err(ConnectorError::new(
                        ConnectorErrorCode::ConnectionFailed,
                        "time_zone_probe",
                    ));
                }
                Err(error) if remote_must_be_discarded(error) => {
                    return Err(error.connector("time_zone_probe"));
                }
                Err(error) => {
                    capabilities.time_zone = error.capability();
                    limitations.push("session_time_zone_unavailable".into());
                }
            }
            let (audit_capability, probed_schema) =
                probe_audit_schema(&mut *remote, audit_table.clone())
                    .map_err(|error| error.connector("capability_probe"))?;
            capabilities.audit_loader = audit_capability;
            audit_schema = probed_schema;
            capabilities.audit_user_filter =
                audit_user_filter_capability(audit_capability, audit_schema.as_ref());
            capabilities.audit_statement_filter =
                audit_statement_filter_capability(audit_capability, audit_schema.as_ref());
            if audit_schema.is_none() {
                limitations.push("audit_schema_unavailable".into());
            }
            capabilities.profile_list =
                probe(&mut *remote, RemoteOperation::ShowProfileList { limit: 1 })
                    .map_err(|error| error.connector("capability_probe"))?;
            capabilities.plan_advisor = probe(&mut *remote, RemoteOperation::ShowPlanAdvisor)
                .map_err(|error| error.connector("capability_probe"))?;
            capabilities.explain = CapabilityState::Unknown; // no synthetic Explain against user clusters
        } else {
            limitations.push("unsupported_version".into());
        }
        let connection_id = opaque_id()?;
        let view = ConnectionSessionView {
            connection_id: connection_id.clone(),
            state: if supported {
                ConnectionState::Connected
            } else {
                ConnectionState::Unsupported
            },
            endpoint_label: endpoint_label(&validated.host, validated.port),
            detected_version,
            time_zone,
            tls_mode,
            capabilities,
            limitations,
            idle_timeout_seconds: self.idle_ttl.as_secs(),
        };
        reservation.activate(
            connection_id,
            Session {
                audit_operation: None,
                view: view.clone(),
                audit_table,
                // This is populated only by the scoped schema probe, never by
                // a successful `SELECT 1`.
                audit_schema,
                default_database: validated.default_database.clone(),
                remote,
                plan_sql_by_ref: BTreeMap::new(),
                observed_queries: BTreeMap::new(),
                discovery_generation: 0,
                audit_discovery: None,
                successful_profiles: SuccessfulProfileEvidence::default(),
                execution_attempts: BTreeMap::new(),
                last_used: now,
            },
        );
        Ok(view)
    }

    /// Whether the session still exists and is within its idle TTL. Does not
    /// count as use, so a paused collector cannot keep a session alive.
    pub fn is_active(&self, connection_id: &str) -> bool {
        let now = self.clock.now();
        self.lock_sessions()
            .active
            .get(connection_id)
            .is_some_and(|session| now.saturating_sub(session.last_used) <= self.idle_ttl)
    }

    /// Closing an unknown ID is intentionally idempotent and reveals no history.
    pub fn close(&self, connection_id: &str) {
        let mut sessions = self.lock_sessions();
        while sessions
            .active
            .get(connection_id)
            .is_some_and(|session| session.audit_operation.is_some())
        {
            sessions = self
                .operation_done
                .wait_timeout(sessions, Duration::from_millis(25))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if let Some(mut session) = sessions.active.remove(connection_id) {
            session.remote.close();
        }
    }

    /// API wiring may call this from its own timer. The connector itself does
    /// not start a background thread or claim autonomous expiry.
    pub fn expire_idle(&self) -> usize {
        self.expire(self.clock.now())
    }

    pub fn search(
        &self,
        connection_id: &str,
        search: QuerySearch,
    ) -> Result<QueryBrowseResult, ConnectorError> {
        validate_search(&search)?;
        self.with_supported_session(connection_id, |session| {
            let audit_available =
                session.view.capabilities.audit_loader == CapabilityState::Available;
            match (search.source, audit_available) {
                (QuerySource::AuditLoader, false) => Err(ConnectorError::new(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "query_source",
                )),
                (QuerySource::AuditLoader | QuerySource::Auto, true) => {
                    // The Audit history predicate needs proven `isQuery`
                    // TINYINT and `stmt` columns. Without them nothing is
                    // hidden and the response says so instead of guessing.
                    let statement_filter_proven = session
                        .audit_schema
                        .as_ref()
                        .is_some_and(AuditSchema::supports_statement_filter);
                    let statement_filter =
                        if search.statement_filter.is_applied() && statement_filter_proven {
                            AuditStatementFilter::ExcludeDoctorAndNonQuery
                        } else {
                            AuditStatementFilter::All
                        };
                    let table = session.audit_table.as_source_name().to_owned();
                    let parameters = QueryParameters {
                        start_seconds: search.start_seconds,
                        end_seconds: search.end_seconds,
                        state: search.state.clone(),
                        digest: search.digest.clone(),
                        limit: normalized_limit(search.limit),
                        statement_filter,
                    };
                    let raw_rows = rows(
                        session
                            .remote
                            .run(RemoteOperation::AuditSearch {
                                table: table.clone(),
                                parameters: parameters.clone(),
                            })
                            .map_err(|error| error.connector("query_source"))?,
                    )?;
                    let statement_filter = if statement_filter.is_applied() {
                        decode_audit_search_hidden_counts(rows(
                            session
                                .remote
                                .run(RemoteOperation::AuditSearchHiddenCounts { table, parameters })
                                .map_err(|error| error.connector("query_source"))?,
                        )?)?
                    } else if search.statement_filter.is_applied() {
                        QueryListStatementFilterCoverage::Indistinguishable {
                            reason: "audit_statement_filter_unavailable".into(),
                        }
                    } else {
                        QueryListStatementFilterCoverage::NotApplied
                    };
                    let retained =
                        retain_plan_sql(session, raw_rows, 13, QuerySource::AuditLoader)?;
                    Ok(QueryBrowseResult {
                        source: QuerySource::AuditLoader,
                        discovery_generation: retained.discovery_generation,
                        rows: retained.rows,
                        plan_sql_references: retained.plan_sql_references,
                        ambiguous_query_ids: retained.ambiguous_query_ids,
                        rejected_row_count: retained.rejected_row_count,
                        degraded_reason: None,
                        time_zone: session.view.time_zone.clone(),
                        statement_filter,
                    })
                }
                (QuerySource::Auto, false) | (QuerySource::ProfileList, _) => {
                    if session.view.capabilities.profile_list != CapabilityState::Available {
                        return Err(ConnectorError::new(
                            ConnectorErrorCode::CapabilityUnavailable,
                            "query_source",
                        ));
                    }
                    if search.digest.is_some() {
                        return Err(ConnectorError::with_limitation(
                            ConnectorErrorCode::CapabilityUnavailable,
                            "query_source",
                            "profile_list_digest_filter_unavailable",
                        ));
                    }
                    let raw_rows = rows(
                        session
                            .remote
                            .run(RemoteOperation::ShowProfileList {
                                limit: MAX_QUERY_ROWS,
                            })
                            .map_err(|error| error.connector("query_source"))?,
                    )?;
                    let state_rows = filter_profile_list_rows(raw_rows, search.state.as_deref());
                    let (state_rows, statement_filter) = if search.statement_filter.is_applied() {
                        let before = state_rows.len();
                        let kept = state_rows
                            .into_iter()
                            .filter(|row| !profile_list_row_is_doctor_statement(row))
                            .collect::<Vec<_>>();
                        let hidden = before - kept.len();
                        (
                            kept,
                            QueryListStatementFilterCoverage::DoctorOnlyApplied {
                                hidden_doctor_row_count: hidden.to_string(),
                            },
                        )
                    } else {
                        (state_rows, QueryListStatementFilterCoverage::NotApplied)
                    };
                    let raw_rows = state_rows
                        .into_iter()
                        .take(normalized_limit(search.limit) as usize)
                        .collect();
                    let retained = retain_plan_sql(session, raw_rows, 5, QuerySource::ProfileList)?;
                    Ok(QueryBrowseResult {
                        source: QuerySource::ProfileList,
                        discovery_generation: retained.discovery_generation,
                        rows: retained.rows,
                        plan_sql_references: retained.plan_sql_references,
                        ambiguous_query_ids: retained.ambiguous_query_ids,
                        rejected_row_count: retained.rejected_row_count,
                        degraded_reason: (search.source == QuerySource::Auto)
                            .then(|| "audit_loader_unavailable".into()),
                        time_zone: session.view.time_zone.clone(),
                        statement_filter,
                    })
                }
            }
        })
    }

    pub fn fetch_profile(
        &self,
        connection_id: &str,
        query_id: &str,
        discovery_generation: u32,
    ) -> Result<FetchedProfile, ConnectorError> {
        if !is_valid_query_id(query_id) {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        self.with_supported_session(connection_id, |session| {
            fetch_profile_session(session, query_id, discovery_generation)
        })
    }

    /// Observes bounded ProfileList Running candidates once without fetching
    /// any raw Profile. The returned generation is required for a later manual
    /// snapshot, so each observation supersedes the prior session discovery.
    pub fn list_running_profile_candidates(
        &self,
        connection_id: &str,
        limit: u16,
        statement_filter: AuditStatementFilter,
    ) -> Result<RunningProfileCandidates, ConnectorError> {
        self.with_supported_session(connection_id, |session| {
            if session.view.capabilities.profile_list != CapabilityState::Available {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "query_source",
                ));
            }
            let raw_rows = rows(
                session
                    .remote
                    .run(RemoteOperation::ShowProfileList {
                        limit: MAX_QUERY_ROWS,
                    })
                    .map_err(|error| error.connector("query_source"))?,
            )?;
            let at_limit = raw_rows.len() >= MAX_QUERY_ROWS as usize;
            let running_rows = raw_rows
                .into_iter()
                .filter(|row| {
                    row.get(3)
                        .is_some_and(|state| state.eq_ignore_ascii_case("Running"))
                })
                .collect::<Vec<_>>();
            // Same marker rule as the Profile list: hidden before the local
            // limit, counted, never silently dropped.
            let (running_rows, statement_filter) = if statement_filter.is_applied() {
                let before = running_rows.len();
                let kept = running_rows
                    .into_iter()
                    .filter(|row| !profile_list_row_is_doctor_statement(row))
                    .collect::<Vec<_>>();
                let hidden = before - kept.len();
                (
                    kept,
                    QueryListStatementFilterCoverage::DoctorOnlyApplied {
                        hidden_doctor_row_count: hidden.to_string(),
                    },
                )
            } else {
                (running_rows, QueryListStatementFilterCoverage::NotApplied)
            };
            let local_limit = normalized_limit(limit) as usize;
            let local_truncated = running_rows.len() > local_limit;
            let selected_row_count = running_rows.len().min(local_limit);
            let retained = retain_plan_sql(
                session,
                running_rows.into_iter().take(local_limit).collect(),
                5,
                QuerySource::ProfileList,
            )?;
            Ok(RunningProfileCandidates {
                discovery_generation: retained.discovery_generation,
                limit: local_limit as u16,
                selected_row_count,
                rows: retained.rows,
                plan_sql_references: retained.plan_sql_references,
                ambiguous_query_ids: retained.ambiguous_query_ids,
                rejected_row_count: retained.rejected_row_count,
                at_limit,
                local_truncated,
                time_zone: session.view.time_zone.clone(),
                statement_filter,
            })
        })
    }

    /// Reads one opaque raw snapshot for a currently observed ProfileList Running query.
    /// This explicit action neither polls nor reinterprets the returned Profile text.
    pub fn fetch_running_profile_snapshot(
        &self,
        connection_id: &str,
        query_id: &str,
        discovery_generation: u32,
    ) -> Result<FetchedProfile, ConnectorError> {
        if !is_valid_query_id(query_id) {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        self.with_supported_session(connection_id, |session| {
            fetch_running_profile_snapshot_session(session, query_id, discovery_generation)
        })
    }

    /// Reads official interpretation once; never executes user SQL or fetches raw Profile.
    pub fn analyze_profile(
        &self,
        connection_id: &str,
        request: AnalyzeProfileRequest,
    ) -> Result<OfficialProfileReading, ConnectorError> {
        let query_id = canonical_query_id(&request.query_id)
            .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
        validate_analyze_profile_node_ids(&request.node_ids)?;
        let identity = match request.identity {
            OfficialProfileIdentity::Discovery {
                discovery_generation,
            } => OfficialProfileIdentity::Discovery {
                discovery_generation,
            },
            OfficialProfileIdentity::VerifiedStandardAttempt {
                execution_attempt_id,
            } => OfficialProfileIdentity::VerifiedStandardAttempt {
                execution_attempt_id: valid_execution_attempt_id(&execution_attempt_id)?,
            },
        };
        self.with_supported_session(connection_id, move |session| {
            let (query_context, observed_lifecycle) = match &identity {
                OfficialProfileIdentity::Discovery {
                    discovery_generation,
                } => {
                    let observed =
                        validate_observed_query(session, &query_id, *discovery_generation)?;
                    let observed_lifecycle =
                        observed.row.get(3).and_then(|state| match observed.source {
                            QuerySource::AuditLoader if state.eq_ignore_ascii_case("EOF") => {
                                Some(OfficialProfileLifecycle::Finished)
                            }
                            QuerySource::ProfileList if state.eq_ignore_ascii_case("Finished") => {
                                Some(OfficialProfileLifecycle::Finished)
                            }
                            QuerySource::ProfileList if state.eq_ignore_ascii_case("Running") => {
                                Some(OfficialProfileLifecycle::Running)
                            }
                            _ => None,
                        });
                    (
                        Some(FetchedQueryContext {
                            source: observed.source,
                            discovery_generation: *discovery_generation,
                            row_index: observed.row_index,
                            row: observed.row,
                            time_zone: session.view.time_zone.clone(),
                            plan_sql_ref: observed.plan_sql_ref,
                        }),
                        observed_lifecycle,
                    )
                }
                OfficialProfileIdentity::VerifiedStandardAttempt {
                    execution_attempt_id,
                } => {
                    validate_verified_standard_attempt(session, execution_attempt_id, &query_id)?;
                    (None, None)
                }
            };
            let content = if query_context.is_some() && observed_lifecycle.is_none() {
                OfficialProfileContent::Unavailable {
                    reason: OfficialProfileUnavailableReason::LifecycleUnknown,
                }
            } else {
                match session.remote.run(RemoteOperation::AnalyzeProfile {
                    query_id: query_id.clone(),
                    node_ids: request.node_ids.clone(),
                }) {
                    Ok(RemoteResponse::AnalyzeProfile(rows)) => {
                        validate_official_profile_rows(&rows)?;
                        if rows.iter().all(|row| row.trim().is_empty()) {
                            OfficialProfileContent::Unavailable {
                                reason: OfficialProfileUnavailableReason::Empty,
                            }
                        } else {
                            OfficialProfileContent::Available { rows }
                        }
                    }
                    Ok(_) => return Err(RemoteError::Protocol.connector("analyze_profile")),
                    Err(RemoteError::PermissionDenied) => OfficialProfileContent::Unavailable {
                        reason: OfficialProfileUnavailableReason::PermissionDenied,
                    },
                    Err(RemoteError::Unsupported) => OfficialProfileContent::Unavailable {
                        reason: OfficialProfileUnavailableReason::RemoteUnsupported,
                    },
                    Err(RemoteError::QueryProfileNotFound) => OfficialProfileContent::Unavailable {
                        reason: OfficialProfileUnavailableReason::NotRetained,
                    },
                    Err(error) => return Err(error.connector("analyze_profile")),
                }
            };
            Ok(OfficialProfileReading {
                query_id,
                identity,
                node_ids: request.node_ids,
                detected_version: session.view.detected_version.clone(),
                default_database: session.default_database.clone(),
                query_context,
                observed_lifecycle,
                content,
            })
        })
    }

    /// Returns SQL observed during this session's current discovery generation.
    /// The lookup is local-only and never issues or executes a remote statement.
    pub fn query_sql(
        &self,
        connection_id: &str,
        query_id: &str,
        discovery_generation: u32,
    ) -> Result<RetainedQuerySql, ConnectorError> {
        self.with_supported_session(connection_id, |session| {
            let Some(query_id) = canonical_query_id(query_id) else {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QuerySqlUnavailable,
                    "query_sql",
                    "query_sql_not_retained",
                ));
            };
            if session.discovery_generation != discovery_generation {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QueryDiscoveryStale,
                    "query_sql",
                    "query_discovery_stale",
                ));
            }
            let Some(observed) = session.observed_queries.get(&query_id) else {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QuerySqlUnavailable,
                    "query_sql",
                    "query_sql_not_retained",
                ));
            };
            if observed.discovery_generation != session.discovery_generation {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QueryDiscoveryStale,
                    "query_sql",
                    "query_discovery_stale",
                ));
            }
            let Some(sql) = observed.sql.clone() else {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QuerySqlUnavailable,
                    "query_sql",
                    "query_sql_not_retained",
                ));
            };
            Ok(RetainedQuerySql {
                query_id,
                discovery_generation,
                sql,
            })
        })
    }

    /// Bounded AuditLoader Top occurrences. The raw cells deliberately remain
    /// nullable source inputs until the core normalizer applies its one metric
    /// parser.
    pub fn audit_occurrences(
        &self,
        connection_id: &str,
        request: AuditOccurrenceRequest,
    ) -> Result<AuditOccurrenceWorkload, ConnectorError> {
        self.audit_occurrences_controlled(connection_id, request, None)
    }

    pub fn audit_occurrences_controlled(
        &self,
        connection_id: &str,
        request: AuditOccurrenceRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditOccurrenceWorkload, ConnectorError> {
        request.validate().map_err(audit_request_error)?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let scope = request.scope.clone();
            audit_occurrence_statement(&session.audit_table, &request, &schema)
                .map_err(audit_schema_error)?;
            let coverage = audit_coverage(session, scope.clone(), schema.clone())?;
            let table = session.audit_table.clone();
            let decode_schema = schema.clone();
            let raw_rows = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditOccurrences {
                        table,
                        request,
                        schema,
                    })
                    .map_err(|error| error.connector("audit_occurrences"))?,
                "audit_occurrences",
            )?;
            let mut rows = raw_rows
                .into_iter()
                .map(|row| decode_occurrence_source_row(row, &decode_schema))
                .collect::<Result<Vec<_>, _>>()
                .map_err(audit_response_error)?;
            audit_occurrence_samples(
                session,
                &scope,
                AuditOccurrenceSampleWindow::Scope,
                &decode_schema,
                rows.iter_mut().collect(),
            )?;
            let discovery_generation = {
                let bindings = occurrence_family_bindings(session, &rows)?;
                let generation = register_audit_occurrence_discovery(session, scope.clone(), &rows);
                publish_family_bindings(session, bindings);
                generation
            };
            Ok(AuditOccurrenceWorkload {
                discovery_generation,
                scope,
                session_identity: audit_session_identity(session),
                rows,
                coverage,
            })
        })
    }

    /// Bounded server aggregation by the full catalog/database/digest key.
    /// Starting a family view deliberately invalidates an earlier member
    /// discovery: callers must request current members before SQL lookup.
    pub fn audit_families(
        &self,
        connection_id: &str,
        request: AuditFamilyRequest,
    ) -> Result<AuditFamilyWorkload, ConnectorError> {
        self.audit_families_controlled(connection_id, request, None)
    }

    pub fn audit_families_controlled(
        &self,
        connection_id: &str,
        request: AuditFamilyRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditFamilyWorkload, ConnectorError> {
        request.validate().map_err(audit_request_error)?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let schema_for_samples = schema.clone();
            let scope = request.scope.clone();
            let sort = request.sort.clone();
            audit_family_statement(&session.audit_table, &request, &schema)
                .map_err(audit_schema_error)?;
            let coverage = audit_coverage(session, scope.clone(), schema.clone())?;
            let table = session.audit_table.clone();
            let raw_rows = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditFamilies {
                        table,
                        request,
                        schema,
                    })
                    .map_err(|error| error.connector("audit_families"))?,
                "audit_families",
            )?;
            let mut rows = raw_rows
                .into_iter()
                .map(decode_family_source_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(audit_response_error)?;
            guard_error_proportion_population(&rows, &sort, "audit_families")?;
            audit_family_samples(
                session,
                &scope,
                &schema_for_samples,
                &mut rows,
                AUDIT_STATEMENT_SAMPLE_CHARS,
            )?;
            let bindings = family_bindings(
                session,
                rows.iter().map(|row| {
                    (
                        row.native_digest.as_deref(),
                        row.catalog.as_deref(),
                        row.database.as_deref(),
                    )
                }),
            )?;
            let discovery_generation =
                next_audit_generation(session, scope.clone(), BTreeSet::new());
            publish_family_bindings(session, bindings);
            Ok(AuditFamilyWorkload {
                discovery_generation,
                scope,
                session_identity: audit_session_identity(session),
                rows,
                coverage,
            })
        })
    }

    /// Two bounded reads: the newest digest-missing executions in scope, then
    /// each one's fingerprint-length `stmt` prefix. The family sort and limit
    /// are applied after in-process grouping, not here.
    pub fn audit_fingerprint_events_controlled(
        &self,
        connection_id: &str,
        request: AuditFamilyRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditFingerprintWorkload, ConnectorError> {
        request.validate().map_err(audit_request_error)?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let scope = request.scope;
            let (rows, eligible_count) = read_fingerprint_events(session, &scope, schema)?;
            Ok(AuditFingerprintWorkload {
                scope,
                session_identity: audit_session_identity(session),
                rows,
                eligible_count,
            })
        })
    }

    /// Stage B: one bounded, read-only slice `[start, end)` of the audit table
    /// (session time-zone seconds, as every scope). Nothing is published to
    /// discovery. The caller owns watermarks, retries and storage.
    pub fn audit_collection_slice_controlled(
        &self,
        connection_id: &str,
        start_seconds: u32,
        end_seconds: u32,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditCollectionSlice, ConnectorError> {
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let statement_filter_applied = matches!(
                session.view.capabilities.audit_statement_filter,
                CapabilityState::Available
            );
            let scope = AuditScope {
                window: doctor_contracts::AuditWindow {
                    start_seconds,
                    end_seconds,
                },
                state: None,
                user: None,
                native_digest: None,
                catalog: None,
                database: None,
                resource_group: None,
                statement_filter: if statement_filter_applied {
                    AuditStatementFilter::ExcludeDoctorAndNonQuery
                } else {
                    AuditStatementFilter::All
                },
            };
            scope.validate().map_err(audit_request_error)?;
            audit_collection_slice_statement(&session.audit_table, &scope, &schema)
                .map_err(audit_schema_error)?;
            let table = session.audit_table.clone();
            let raw_rows = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditCollectionSlice {
                        table,
                        scope,
                        schema: schema.clone(),
                    })
                    .map_err(|error| error.connector("audit_collection_slice"))?,
                "audit_collection_slice",
            )?;
            if raw_rows.len() > usize::from(AUDIT_COLLECTION_SLICE_LIMIT) {
                return Err(audit_response_error("audit_collection_slice_limit"));
            }
            let mut slice_count = String::from("0");
            let mut rows = Vec::with_capacity(raw_rows.len());
            for mut row in raw_rows {
                if row.len() != 23 {
                    return Err(audit_response_error("audit_collection_slice_shape"));
                }
                let flag = |cell: Option<String>| match cell.as_deref() {
                    Some("1") => Ok(true),
                    Some("0") => Ok(false),
                    _ => Err(audit_response_error("audit_collection_slice_flag")),
                };
                let count = row
                    .pop()
                    .flatten()
                    .filter(|count| {
                        !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    .ok_or_else(|| audit_response_error("audit_collection_slice_count"))?;
                let agrees = flag(row.pop().flatten())?;
                let truncated = flag(row.pop().flatten())?;
                let null = flag(row.pop().flatten())?;
                let prefix = row.pop().flatten();
                let mut decoded =
                    decode_occurrence_source_row(row, &schema).map_err(audit_response_error)?;
                decoded.statement_sample = if !agrees {
                    AuditSourceStatementSample::ConflictingSourceRows
                } else if null {
                    AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull)
                } else {
                    AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                        value: prefix
                            .ok_or_else(|| audit_response_error("audit_collection_slice_prefix"))?,
                        truncated,
                    })
                };
                rows.push(decoded);
                slice_count = count;
            }
            Ok(AuditCollectionSlice {
                session_identity: audit_session_identity(session),
                rows,
                slice_count,
                statement_filter_applied,
            })
        })
    }

    /// A2 sources, all bounded and read-only: the requested page of exact
    /// families with each one's fingerprint-length sample, the window
    /// coverage, and the digest-missing executions. Nothing is published to
    /// discovery: a similar-SQL view grants no SQL or Profile lookup.
    pub fn audit_similar_sources_controlled(
        &self,
        connection_id: &str,
        request: AuditFamilyRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditSimilarWorkload, ConnectorError> {
        request.validate().map_err(audit_request_error)?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            if !schema.has(AuditColumn::Statement) {
                return Err(audit_schema_error(AuditSqlBuildError::SchemaMissing(
                    "stmt",
                )));
            }
            let scope = request.scope.clone();
            let sort = request.sort.clone();
            audit_family_statement(&session.audit_table, &request, &schema)
                .map_err(audit_schema_error)?;
            let coverage = audit_coverage(session, scope.clone(), schema.clone())?;
            let table = session.audit_table.clone();
            let mut families = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditFamilies {
                        table,
                        request,
                        schema: schema.clone(),
                    })
                    .map_err(|error| error.connector("audit_families"))?,
                "audit_families",
            )?
            .into_iter()
            .map(decode_family_source_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(audit_response_error)?;
            guard_error_proportion_population(&families, &sort, "audit_families")?;
            audit_family_samples(
                session,
                &scope,
                &schema,
                &mut families,
                AUDIT_FINGERPRINT_STATEMENT_CHARS,
            )?;
            let (fingerprint_rows, eligible_count) =
                read_fingerprint_events(session, &scope, schema)?;
            Ok(AuditSimilarWorkload {
                families,
                coverage,
                fingerprint: AuditFingerprintWorkload {
                    scope,
                    session_identity: audit_session_identity(session),
                    rows: fingerprint_rows,
                    eligible_count,
                },
            })
        })
    }

    /// Overview is intentionally one locked, four-operation batch. No state
    /// changes until every sealed read and decoder has succeeded, so a late
    /// failure leaves SQL/Profile bindings on the prior generation.
    pub fn audit_overview(
        &self,
        connection_id: &str,
        request: AuditOverviewRequest,
    ) -> Result<AuditOverviewWorkload, ConnectorError> {
        self.audit_overview_controlled(connection_id, request, None)
    }

    pub fn audit_overview_controlled(
        &self,
        connection_id: &str,
        request: AuditOverviewRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditOverviewWorkload, ConnectorError> {
        request.scope.validate().map_err(audit_request_error)?;
        let scope = request.scope.clone();
        match &request.primary {
            AuditOverviewPrimary::Occurrences { sort, limit } => AuditOccurrenceRequest {
                scope: scope.clone(),
                sort: sort.clone(),
                limit: *limit,
            }
            .validate()
            .map_err(audit_request_error)?,
            AuditOverviewPrimary::Families { sort, limit } => AuditFamilyRequest {
                scope: scope.clone(),
                sort: sort.clone(),
                limit: *limit,
            }
            .validate()
            .map_err(audit_request_error)?,
        }
        let family_top_request = AuditFamilyRequest {
            scope: scope.clone(),
            sort: match request.family_top_sort {
                AuditOverviewFamilyTopSort::CumulativeQueryTimeDesc => {
                    doctor_contracts::AuditFamilySort::CumulativeQueryTimeDesc
                }
                AuditOverviewFamilyTopSort::CumulativeScanBytesDesc => {
                    doctor_contracts::AuditFamilySort::CumulativeScanBytesDesc
                }
                AuditOverviewFamilyTopSort::CumulativeCpuCostDesc => {
                    doctor_contracts::AuditFamilySort::CumulativeCpuCostDesc
                }
                AuditOverviewFamilyTopSort::ExecutionCountDesc => {
                    doctor_contracts::AuditFamilySort::ExecutionCountDesc
                }
            },
            limit: AUDIT_OVERVIEW_FAMILY_TOP_LIMIT,
        };
        family_top_request.validate().map_err(audit_request_error)?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let coverage = audit_coverage(session, scope.clone(), schema.clone())?;
            let table = session.audit_table.clone();
            let primary = match request.primary {
                AuditOverviewPrimary::Occurrences { sort, limit } => {
                    let request = AuditOccurrenceRequest {
                        scope: scope.clone(),
                        sort,
                        limit,
                    };
                    audit_occurrence_statement(&table, &request, &schema)
                        .map_err(audit_schema_error)?;
                    let mut rows = nullable_rows(
                        session
                            .remote
                            .run(RemoteOperation::AuditOccurrences {
                                table: table.clone(),
                                request,
                                schema: schema.clone(),
                            })
                            .map_err(|error| error.connector("audit_overview_primary"))?,
                        "audit_overview_primary",
                    )?
                    .into_iter()
                    .map(|row| decode_occurrence_source_row(row, &schema))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(audit_response_error)?;
                    audit_occurrence_samples(
                        session,
                        &scope,
                        AuditOccurrenceSampleWindow::Scope,
                        &schema,
                        rows.iter_mut().collect(),
                    )?;
                    AuditOverviewPrimaryWorkload::Occurrences { rows, limit }
                }
                AuditOverviewPrimary::Families { sort, limit } => {
                    let request = AuditFamilyRequest {
                        scope: scope.clone(),
                        sort,
                        limit,
                    };
                    let sort = request.sort.clone();
                    audit_family_statement(&table, &request, &schema)
                        .map_err(audit_schema_error)?;
                    let mut rows = nullable_rows(
                        session
                            .remote
                            .run(RemoteOperation::AuditFamilies {
                                table: table.clone(),
                                request,
                                schema: schema.clone(),
                            })
                            .map_err(|error| error.connector("audit_overview_primary"))?,
                        "audit_overview_primary",
                    )?
                    .into_iter()
                    .map(decode_family_source_row)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(audit_response_error)?;
                    guard_error_proportion_population(&rows, &sort, "audit_overview_primary")?;
                    audit_family_samples(
                        session,
                        &scope,
                        &schema,
                        &mut rows,
                        AUDIT_STATEMENT_SAMPLE_CHARS,
                    )?;
                    AuditOverviewPrimaryWorkload::Families { rows, limit }
                }
            };
            let concurrency = if schema.supports_historical_concurrency() {
                audit_concurrency_statement(&table, &scope, &schema).map_err(audit_schema_error)?;
                decode_concurrency_source_rows(nullable_rows(
                    session
                        .remote
                        .run(RemoteOperation::AuditConcurrency {
                            table: table.clone(),
                            scope: scope.clone(),
                            schema: schema.clone(),
                        })
                        .map_err(|error| error.connector("audit_overview_concurrency"))?,
                    "audit_overview_concurrency",
                )?)
                .map_err(audit_response_error)?
            } else {
                AuditConcurrencySource::SchemaTypeUnsupported
            };
            audit_family_statement(&table, &family_top_request, &schema)
                .map_err(audit_schema_error)?;
            let family_top_rows = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditOverviewFamilyTop {
                        table,
                        request: family_top_request,
                        schema,
                    })
                    .map_err(|error| error.connector("audit_overview_family_top"))?,
                "audit_overview_family_top",
            )?
            .into_iter()
            .map(decode_family_source_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(audit_response_error)?;
            let mut bindings = family_bindings(
                session,
                family_top_rows.iter().map(|row| {
                    (
                        row.native_digest.as_deref(),
                        row.catalog.as_deref(),
                        row.database.as_deref(),
                    )
                }),
            )?;
            let primary_bindings = match &primary {
                AuditOverviewPrimaryWorkload::Occurrences { rows, .. } => {
                    occurrence_family_bindings(session, rows)?
                }
                AuditOverviewPrimaryWorkload::Families { rows, .. } => family_bindings(
                    session,
                    rows.iter().map(|row| {
                        (
                            row.native_digest.as_deref(),
                            row.catalog.as_deref(),
                            row.database.as_deref(),
                        )
                    }),
                )?,
            };
            audit_family_session::merge_family_bindings(&mut bindings, primary_bindings)?;
            let discovery_generation = match &primary {
                AuditOverviewPrimaryWorkload::Occurrences { rows, .. } => {
                    register_audit_occurrence_discovery_with_selector(
                        session,
                        scope.clone(),
                        rows,
                        AuditDiscoverySelector::OverviewPrimary,
                    )
                }
                AuditOverviewPrimaryWorkload::Families { .. } => {
                    next_audit_generation_with_selector(
                        session,
                        scope.clone(),
                        BTreeSet::new(),
                        AuditDiscoverySelector::OverviewPrimary,
                    )
                }
            };
            publish_family_bindings(session, bindings);
            Ok(AuditOverviewWorkload {
                discovery_generation,
                scope,
                session_identity: audit_session_identity(session),
                primary,
                coverage,
                concurrency,
                family_top_sort: request.family_top_sort,
                family_top_rows,
            })
        })
    }

    /// Reads one explicitly selected historical point from the current
    /// Overview discovery. A successful drill deliberately becomes the next
    /// discovery generation so only returned identities can reveal SQL/Profile.
    pub fn audit_active_at(
        &self,
        connection_id: &str,
        scope: AuditScope,
        at_milliseconds: String,
        discovery_generation: u32,
    ) -> Result<AuditActiveAtWorkload, ConnectorError> {
        self.audit_active_at_controlled(
            connection_id,
            scope,
            at_milliseconds,
            discovery_generation,
            None,
        )
    }

    pub fn audit_performance_controlled(
        &self,
        connection_id: &str,
        scope: AuditScope,
        bucket_seconds: u32,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditPerformanceWorkload, ConnectorError> {
        scope.validate().map_err(audit_request_error)?;
        audit_performance_bucket_count(&scope, bucket_seconds).map_err(|_| {
            ConnectorError::new(ConnectorErrorCode::InvalidRequest, "audit_performance")
        })?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let schema = verified_audit_schema(session)?;
            let table = session.audit_table.clone();
            audit_performance_statement(&table, &scope, bucket_seconds, &schema)
                .map_err(audit_schema_error)?;
            let rows = nullable_rows(
                session
                    .remote
                    .run(RemoteOperation::AuditPerformance {
                        table,
                        scope: scope.clone(),
                        bucket_seconds,
                        schema: schema.clone(),
                    })
                    .map_err(|error| error.connector("audit_performance"))?,
                "audit_performance",
            )?;
            let source = decode_performance_source_rows(rows, &scope, bucket_seconds, &schema)
                .map_err(audit_response_error)?;
            Ok(AuditPerformanceWorkload {
                scope: scope.clone(),
                bucket_seconds,
                session_identity: audit_session_identity(session),
                user_column: schema.has(AuditColumn::User),
                error_code_column: schema.has(AuditColumn::ErrorCode),
                pending_time_column: schema.has(AuditColumn::PendingTimeMs),
                source,
            })
        })
    }

    pub fn audit_active_at_controlled(
        &self,
        connection_id: &str,
        scope: AuditScope,
        at_milliseconds: String,
        discovery_generation: u32,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<AuditActiveAtWorkload, ConnectorError> {
        scope.validate().map_err(audit_request_error)?;
        let at_milliseconds = at_milliseconds
            .parse::<i128>()
            .ok()
            .filter(|value| *value >= 0)
            .ok_or_else(|| {
                ConnectorError::new(ConnectorErrorCode::InvalidRequest, "audit_active_at")
            })?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let Some(discovery) = session.audit_discovery.as_ref() else {
                return Err(audit_sql_unavailable("audit_overview_not_discovered"));
            };
            if session.discovery_generation != discovery_generation
                || discovery.generation != discovery_generation
            {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QueryDiscoveryStale,
                    "audit_active_at",
                    "audit_discovery_stale",
                ));
            }
            if discovery.scope != scope {
                return Err(audit_sql_unavailable("audit_scope_mismatch"));
            }
            if !matches!(
                &discovery.selector,
                AuditDiscoverySelector::OverviewPrimary | AuditDiscoverySelector::ActiveAt { .. }
            ) {
                return Err(audit_sql_unavailable("audit_overview_not_discovered"));
            }
            let schema = verified_audit_schema(session)?;
            if !schema.supports_historical_concurrency() {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "audit_active_at",
                    "audit_concurrency_schema_type_unsupported",
                ));
            }
            audit_active_at_statement(&session.audit_table, &scope, at_milliseconds, &schema)
                .map_err(audit_schema_error)?;
            let table = session.audit_table.clone();
            let mut source = decode_active_at_source_rows(
                nullable_rows(
                    session
                        .remote
                        .run(RemoteOperation::AuditActiveAt {
                            table,
                            scope: scope.clone(),
                            at_milliseconds,
                            schema: schema.clone(),
                        })
                        .map_err(|error| error.connector("audit_active_at"))?,
                    "audit_active_at",
                )?,
                at_milliseconds,
                &schema,
            )
            .map_err(audit_response_error)?;
            audit_occurrence_samples(
                session,
                &scope,
                AuditOccurrenceSampleWindow::HistoricalLookback,
                &schema,
                source
                    .rows
                    .iter_mut()
                    .map(|participant| &mut participant.occurrence)
                    .collect(),
            )?;
            let occurrences = source
                .rows
                .iter()
                .map(|participant| participant.occurrence.clone())
                .collect::<Vec<_>>();
            let bindings = occurrence_family_bindings(session, &occurrences)?;
            let next_generation = register_audit_occurrence_discovery_with_selector(
                session,
                scope.clone(),
                &occurrences,
                AuditDiscoverySelector::ActiveAt { at_milliseconds },
            );
            publish_family_bindings(session, bindings);
            Ok(AuditActiveAtWorkload {
                discovery_generation: next_generation,
                scope,
                at_milliseconds,
                session_identity: audit_session_identity(session),
                active_count: source.active_count,
                rows: source.rows,
                truncated: source.truncated,
            })
        })
    }

    /// Explicit remote SQL lookup for one identity discovered in this session's
    /// current occurrence generation. This is separate from legacy local
    /// `query_sql`, which retains its no-remote-call behavior.
    pub fn audit_query_sql(
        &self,
        connection_id: &str,
        request: AuditSqlLookupRequest,
        discovery_generation: u32,
    ) -> Result<RetainedQuerySql, ConnectorError> {
        self.audit_query_sql_controlled(connection_id, request, discovery_generation, None)
    }

    pub fn audit_query_sql_controlled(
        &self,
        connection_id: &str,
        request: AuditSqlLookupRequest,
        discovery_generation: u32,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<RetainedQuerySql, ConnectorError> {
        request.validate().map_err(audit_request_error)?;
        let query_id = canonical_query_id(&request.query_id)
            .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "audit_sql"))?;
        self.with_supported_session_controlled(connection_id, control, |session| {
            let Some(discovery) = session.audit_discovery.as_ref() else {
                return Err(audit_sql_unavailable("audit_identity_not_discovered"));
            };
            if session.discovery_generation != discovery_generation
                || discovery.generation != discovery_generation
            {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::QueryDiscoveryStale,
                    "audit_sql",
                    "audit_discovery_stale",
                ));
            }
            if discovery.scope != request.scope {
                return Err(audit_sql_unavailable("audit_scope_mismatch"));
            }
            if !discovery.unique_query_ids.contains(&query_id) {
                return Err(audit_sql_unavailable("audit_identity_not_unique"));
            }
            let selector = discovery.selector.clone();
            let family_target = match &selector {
                AuditDiscoverySelector::FamilyDetail { family_key, .. } => Some(
                    discovery
                        .families_by_key
                        .get(family_key)
                        .cloned()
                        .ok_or_else(|| audit_sql_unavailable("audit_family_not_discovered"))?,
                ),
                _ => None,
            };
            let schema = verified_audit_schema(session)?;
            let table = session.audit_table.clone();
            let operation = match selector {
                AuditDiscoverySelector::ActiveAt { at_milliseconds } => {
                    audit_active_at_sql_lookup_statement(
                        &table,
                        &request,
                        at_milliseconds,
                        &schema,
                    )
                    .map_err(audit_schema_error)?;
                    RemoteOperation::AuditActiveAtSqlLookup {
                        table,
                        request,
                        at_milliseconds,
                        schema,
                    }
                }
                AuditDiscoverySelector::FamilyDetail { parent, .. } => {
                    let target = family_target
                        .ok_or_else(|| audit_sql_unavailable("audit_family_not_discovered"))?;
                    audit_family_detail_sql_lookup_statement(
                        &table, &parent, &target, &query_id, &schema,
                    )
                    .map_err(audit_schema_error)?;
                    RemoteOperation::AuditFamilyDetailSqlLookup {
                        table,
                        parent,
                        target,
                        query_id: query_id.clone(),
                        schema,
                    }
                }
                AuditDiscoverySelector::OccurrenceList
                | AuditDiscoverySelector::OverviewPrimary => {
                    audit_sql_lookup_statement(&table, &request, &schema)
                        .map_err(audit_schema_error)?;
                    RemoteOperation::AuditSqlLookup {
                        table,
                        request,
                        schema,
                    }
                }
            };
            let rows = nullable_rows(
                session
                    .remote
                    .run(operation)
                    .map_err(|error| error.connector("audit_sql"))?,
                "audit_sql",
            )?;
            let [row] = rows.as_slice() else {
                return Err(audit_sql_unavailable("audit_sql_not_unique_or_missing"));
            };
            let [returned_query_id, _event_time, statement] = row.as_slice() else {
                return Err(audit_response_error(AuditSqlBuildError::RowShape {
                    expected: 3,
                    actual: row.len(),
                }));
            };
            if returned_query_id
                .as_deref()
                .and_then(canonical_query_id)
                .as_deref()
                != Some(query_id.as_str())
            {
                return Err(audit_sql_unavailable("audit_identity_mismatch"));
            }
            let Some(sql) = statement.as_deref().and_then(bounded_retained_sql) else {
                return Err(audit_sql_unavailable("audit_sql_not_returnable"));
            };
            Ok(RetainedQuerySql {
                query_id,
                discovery_generation,
                sql,
            })
        })
    }

    /// Validate both members before issuing either read, then fetch baseline
    /// followed by candidate while holding one session lease.
    pub fn fetch_profile_pair(
        &self,
        connection_id: &str,
        baseline_query_id: &str,
        candidate_query_id: &str,
        discovery_generation: u32,
    ) -> Result<(FetchedProfile, FetchedProfile), ConnectorError> {
        if !is_valid_query_id(baseline_query_id) {
            return Err(
                ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request")
                    .for_side("baseline"),
            );
        }
        if !is_valid_query_id(candidate_query_id) {
            return Err(
                ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request")
                    .for_side("candidate"),
            );
        }
        self.with_supported_session(connection_id, |session| {
            validate_observed_profile(session, baseline_query_id, discovery_generation)
                .map_err(|error| error.for_side("baseline"))?;
            validate_observed_profile(session, candidate_query_id, discovery_generation)
                .map_err(|error| error.for_side("candidate"))?;
            let baseline = fetch_profile_session(session, baseline_query_id, discovery_generation)
                .map_err(|error| error.for_side("baseline"))?;
            let candidate =
                fetch_profile_session(session, candidate_query_id, discovery_generation)
                    .map_err(|error| error.for_side("candidate"))?;
            Ok((baseline, candidate))
        })
    }

    pub fn advisor(&self, connection_id: &str) -> Result<AdvisorResult, ConnectorError> {
        self.with_supported_session(connection_id, |session| {
            if session.view.capabilities.plan_advisor != CapabilityState::Available {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "query_source",
                ));
            }
            Ok(AdvisorResult {
                rows: rows(
                    session
                        .remote
                        .run(RemoteOperation::ShowPlanAdvisor)
                        .map_err(|error| error.connector("query_source"))?,
                )?,
            })
        })
    }

    pub fn explain(
        &self,
        connection_id: &str,
        candidate_sql: &str,
        mode: ExplainMode,
    ) -> Result<ExplainResult, ConnectorError> {
        let candidate_sql = allowed_candidate_sql(candidate_sql)?;
        self.with_supported_session(connection_id, |session| {
            explain_session(session, candidate_sql, mode)
        })
    }

    pub fn explain_costs(
        &self,
        connection_id: &str,
        candidate_sql: &str,
    ) -> Result<ExplainResult, ConnectorError> {
        self.explain(connection_id, candidate_sql, ExplainMode::Costs)
    }

    /// Explicit runtime mode: execute one validated SELECT/WITH in the current
    /// short-lived session and return only its opaque Profile, never its rows.
    pub fn execute_profiled_select(
        &self,
        connection_id: &str,
        candidate_sql: &str,
    ) -> Result<ExecutedProfile, ConnectorError> {
        self.execute_profiled_select_with_attempt(connection_id, candidate_sql, None)
    }

    /// The optional attempt ID keeps legacy callers wire-compatible while new
    /// C3 callers make standard and native execution mutually exclusive.
    pub fn execute_profiled_select_with_attempt(
        &self,
        connection_id: &str,
        candidate_sql: &str,
        execution_attempt_id: Option<&str>,
    ) -> Result<ExecutedProfile, ConnectorError> {
        let candidate_sql = allowed_candidate_sql(candidate_sql)?;
        let execution_attempt_id = execution_attempt_id
            .map(valid_execution_attempt_id)
            .transpose()?;
        self.with_supported_session(connection_id, |session| {
            if let Some(execution_attempt_id) = execution_attempt_id.as_ref() {
                claim_execution_attempt(
                    session,
                    execution_attempt_id.clone(),
                    ExecutionAttemptMethod::StandardProfile,
                )?;
            }
            let (query_id, profile) = match session
                .remote
                .run(RemoteOperation::ProfiledSelect { candidate_sql })
                .map_err(|error| error.connector("runtime_execute"))?
            {
                RemoteResponse::ProfiledQuery { query_id, profile }
                    if is_valid_query_id(&query_id) && !profile.is_empty() =>
                {
                    (query_id, profile)
                }
                _ => {
                    return Err(ConnectorError::with_limitation(
                        ConnectorErrorCode::ProfileUnavailable,
                        "runtime_execute",
                        "runtime_profile_not_returned",
                    ));
                }
            };
            if let Some(execution_attempt_id) = execution_attempt_id.as_deref() {
                complete_standard_execution_attempt(session, execution_attempt_id, &query_id)?;
            }
            session.view.capabilities.get_query_profile = CapabilityState::Available;
            Ok(ExecutedProfile {
                query_id,
                detected_version: session.view.detected_version.clone(),
                verified_standard_execution_attempt_id: execution_attempt_id,
                profile_text: profile.into_bytes(),
            })
        })
    }

    pub fn explain_reference(
        &self,
        connection_id: &str,
        query_id: &str,
        discovery_generation: u32,
        plan_sql_ref: &str,
        mode: ExplainMode,
    ) -> Result<ExplainResult, ConnectorError> {
        let query_id = canonical_query_id(query_id)
            .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
        if !valid_plan_sql_ref(plan_sql_ref) {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        self.with_supported_session(connection_id, |session| {
            let Some(stored) = session.plan_sql_by_ref.get(plan_sql_ref) else {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "explain",
                    "plan_sql_reference_expired_or_unknown",
                ));
            };
            if stored.query_id != query_id || stored.discovery_generation != discovery_generation {
                return Err(ConnectorError::with_limitation(
                    ConnectorErrorCode::CapabilityUnavailable,
                    "explain",
                    "plan_sql_reference_binding_mismatch",
                ));
            }
            let sql = stored.sql.clone();
            explain_session(session, sql, mode)
        })
    }

    pub fn explain_costs_reference(
        &self,
        connection_id: &str,
        query_id: &str,
        discovery_generation: u32,
        plan_sql_ref: &str,
    ) -> Result<ExplainResult, ConnectorError> {
        self.explain_reference(
            connection_id,
            query_id,
            discovery_generation,
            plan_sql_ref,
            ExplainMode::Costs,
        )
    }

    fn with_supported_session<T>(
        &self,
        connection_id: &str,
        operation: impl FnOnce(&mut Session) -> Result<T, ConnectorError>,
    ) -> Result<T, ConnectorError> {
        self.with_supported_session_controlled(connection_id, None, operation)
    }

    fn with_supported_session_controlled<T>(
        &self,
        connection_id: &str,
        control: Option<&Arc<OperationControl>>,
        operation: impl FnOnce(&mut Session) -> Result<T, ConnectorError>,
    ) -> Result<T, ConnectorError> {
        let mut sessions = if let Some(control) = control {
            loop {
                match self.sessions.try_lock() {
                    Ok(sessions) => break sessions,
                    Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => {
                        control.check()?;
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        } else {
            self.lock_sessions()
        };
        while sessions
            .active
            .get(connection_id)
            .and_then(|session| session.audit_operation.as_ref())
            .is_some_and(|owner| !control.is_some_and(|control| Arc::ptr_eq(owner, control)))
        {
            // A waiting request never owns (and must never evict) another operation's session.
            if let Some(control) = control {
                control.check()?;
            }
            sessions = self
                .operation_done
                .wait_timeout(sessions, Duration::from_millis(25))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let now = self.clock.now();
        let expired = sessions
            .active
            .get(connection_id)
            .is_none_or(|session| now.saturating_sub(session.last_used) > self.idle_ttl);
        if expired {
            if let Some(mut session) = sessions.active.remove(connection_id) {
                session.remote.close();
            }
            return Err(ConnectorError::new(
                ConnectorErrorCode::ConnectionExpired,
                "connection",
            ));
        }
        let result = {
            let Some(session) = sessions.active.get_mut(connection_id) else {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::ConnectionExpired,
                    "connection",
                ));
            };
            if session.view.state != ConnectionState::Connected {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::UnsupportedVersion,
                    "detect",
                ));
            }
            session.last_used = now;
            (|| {
                if let Some(control) = control {
                    control.acquire_session()?;
                    session.audit_operation = Some(Arc::clone(control));
                    control.check()?;
                    session
                        .remote
                        .set_audit_control(Some(Arc::clone(control)))
                        .map_err(|error| error.connector("audit_operation"))?;
                }
                let result = operation(session);
                if let Some(control) = control {
                    control.check()?;
                }
                result
            })()
        };
        if result.as_ref().is_err_and(session_must_be_discarded)
            && control.is_none_or(|control| control.acquired_session())
            && let Some(mut session) = sessions.active.remove(connection_id)
        {
            session.remote.close();
            self.operation_done.notify_all();
        }
        result
    }

    /// Finish while holding the session lock. Arc identity fences late completion,
    /// and ownership prevents reuse during a two-phase Audit publication.
    pub fn finish_audit_operation(
        &self,
        connection_id: &str,
        control: &Arc<OperationControl>,
    ) -> Result<(), ConnectorError> {
        // A queue timeout has no session to finalize and must not queue behind its owner again.
        if !control.acquired_session() {
            return control.finish();
        }
        let mut sessions = self.lock_sessions();
        let result = control.finish();
        let owns = sessions
            .active
            .get(connection_id)
            .and_then(|session| session.audit_operation.as_ref())
            .is_some_and(|owner| Arc::ptr_eq(owner, control));
        if owns {
            if result.is_err() {
                if let Some(mut session) = sessions.active.remove(connection_id) {
                    session.remote.close();
                }
            } else if let Some(session) = sessions.active.get_mut(connection_id) {
                let _ = session.remote.set_audit_control(None);
                session.audit_operation = None;
            }
        }
        control.unbind();
        self.operation_done.notify_all();
        result
    }

    fn reserve_connection_slot(
        &self,
        now: Duration,
    ) -> Result<SessionReservation<'_>, ConnectorError> {
        self.expire(now);
        let mut sessions = self.lock_sessions();
        if sessions.active.len() + sessions.pending_connects >= MAX_SESSIONS {
            return Err(ConnectorError::new(
                ConnectorErrorCode::SessionLimit,
                "connection",
            ));
        }
        sessions.pending_connects += 1;
        drop(sessions);
        Ok(SessionReservation {
            sessions: &self.sessions,
            active: true,
        })
    }

    fn expire(&self, now: Duration) -> usize {
        let mut sessions = self.lock_sessions();
        let expired = sessions
            .active
            .iter()
            .filter_map(|(id, session)| {
                (session.audit_operation.is_none()
                    && now.saturating_sub(session.last_used) > self.idle_ttl)
                    .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        let expired_count = expired.len();
        for id in expired {
            if let Some(mut session) = sessions.active.remove(&id) {
                session.remote.close();
            }
        }
        expired_count
    }

    fn lock_sessions(&self) -> MutexGuard<'_, SessionStore> {
        lock_sessions(&self.sessions)
    }
}

fn lock_sessions(sessions: &Mutex<SessionStore>) -> MutexGuard<'_, SessionStore> {
    sessions.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn session_must_be_discarded(error: &ConnectorError) -> bool {
    matches!(
        error.code,
        ConnectorErrorCode::ConnectionFailed
            | ConnectorErrorCode::TlsFailed
            | ConnectorErrorCode::AuthenticationFailed
            | ConnectorErrorCode::RemoteTimeout
    )
}

struct ValidatedRequest {
    host: String,
    port: u16,
    username: String,
    password: SecretString,
    default_catalog: Option<String>,
    default_database: Option<String>,
    audit_table: String,
    tls_mode: TlsMode,
}

impl ValidatedRequest {
    fn new(request: CreateConnectionRequest) -> Result<Self, ConnectorError> {
        if !valid_host(&request.host)
            || request.port == 0
            || request.username.trim().is_empty()
            || request.username.len() > 256
            || request.username.contains('\0')
        {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        if request
            .default_catalog
            .as_deref()
            .is_some_and(|value| !valid_identifier(value))
            || request
                .default_database
                .as_deref()
                .is_some_and(|value| !valid_identifier(value))
        {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        // v0.1 must not issue a post-connect `USE CATALOG`: it would expand
        // the remote command surface beyond the closed read-only template
        // set.  Reject rather than silently ignoring a requested catalog.
        if request.default_catalog.is_some() {
            return Err(ConnectorError::with_limitation(
                ConnectorErrorCode::InvalidRequest,
                "request",
                "default_catalog_not_supported_v0_1",
            ));
        }
        let audit_table = request
            .audit_table
            .unwrap_or_else(|| DEFAULT_AUDIT_TABLE.into());
        if !valid_identifier_path(&audit_table) {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "request",
            ));
        }
        if request.tls_mode == TlsMode::DisabledLoopbackDocker
            && !is_literal_loopback(&request.host)
        {
            return Err(ConnectorError::new(
                ConnectorErrorCode::TlsRequired,
                "security",
            ));
        }
        Ok(Self {
            host: request.host,
            port: request.port,
            username: request.username,
            password: SecretString::new(request.password),
            default_catalog: request.default_catalog,
            default_database: request.default_database,
            audit_table,
            tls_mode: request.tls_mode,
        })
    }

    fn connect_options(&self) -> RemoteConnectOptions {
        RemoteConnectOptions {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            default_catalog: self.default_catalog.clone(),
            default_database: self.default_database.clone(),
            tls_mode: self.tls_mode,
        }
    }
}

fn probe(
    remote: &mut dyn Remote,
    operation: RemoteOperation,
) -> Result<CapabilityState, RemoteError> {
    match remote.run(operation) {
        Ok(_) => Ok(CapabilityState::Available),
        Err(error) if remote_must_be_discarded(error) => Err(error),
        Err(error) => Ok(error.capability()),
    }
}

fn probe_audit_schema(
    remote: &mut dyn Remote,
    table: AuditTableName,
) -> Result<(CapabilityState, Option<AuditSchema>), RemoteError> {
    match remote.run(RemoteOperation::AuditSchemaProbe { table }) {
        Ok(RemoteResponse::NullableRows(rows)) => AuditSchema::from_schema_probe_rows(&rows)
            .map(|schema| {
                let capability = if audit_schema_supports_workload(&schema) {
                    CapabilityState::Available
                } else {
                    CapabilityState::Unavailable
                };
                (capability, Some(schema))
            })
            .map_err(|_| RemoteError::Protocol),
        Ok(_) => Err(RemoteError::Protocol),
        Err(error) if remote_must_be_discarded(error) => Err(error),
        Err(error) => Ok((error.capability(), None)),
    }
}

fn audit_schema_supports_workload(schema: &AuditSchema) -> bool {
    [
        AuditColumn::QueryId,
        AuditColumn::Timestamp,
        AuditColumn::State,
        AuditColumn::QueryTime,
        AuditColumn::ScanBytes,
        AuditColumn::ScanRows,
        AuditColumn::CpuCostNs,
        AuditColumn::MemCostBytes,
        AuditColumn::ReturnRows,
        AuditColumn::Catalog,
        AuditColumn::Database,
        AuditColumn::ResourceGroup,
        AuditColumn::Digest,
    ]
    .into_iter()
    .all(|column| schema.has(column))
}

fn audit_statement_filter_capability(
    audit_capability: CapabilityState,
    schema: Option<&AuditSchema>,
) -> CapabilityState {
    match audit_capability {
        CapabilityState::Available => {
            if schema.is_some_and(AuditSchema::supports_statement_filter) {
                CapabilityState::Available
            } else {
                CapabilityState::Unavailable
            }
        }
        unavailable => unavailable,
    }
}

fn audit_user_filter_capability(
    audit_capability: CapabilityState,
    schema: Option<&AuditSchema>,
) -> CapabilityState {
    match audit_capability {
        CapabilityState::Available => {
            if schema.is_some_and(|schema| schema.has(AuditColumn::User)) {
                CapabilityState::Available
            } else {
                CapabilityState::Unavailable
            }
        }
        unavailable => unavailable,
    }
}

fn remote_must_be_discarded(error: RemoteError) -> bool {
    matches!(
        error,
        RemoteError::Connection
            | RemoteError::Tls
            | RemoteError::Authentication
            | RemoteError::Timeout
            | RemoteError::Protocol
            | RemoteError::OutputBudgetExceeded
    )
}

fn rows(response: RemoteResponse) -> Result<Vec<Vec<String>>, ConnectorError> {
    match response {
        RemoteResponse::Rows(rows) => Ok(rows),
        _ => Err(ConnectorError::new(
            ConnectorErrorCode::CapabilityUnavailable,
            "query_source",
        )),
    }
}

fn nullable_rows(
    response: RemoteResponse,
    stage: &'static str,
) -> Result<Vec<Vec<Option<String>>>, ConnectorError> {
    match response {
        RemoteResponse::NullableRows(rows) => Ok(rows),
        _ => Err(ConnectorError::new(
            ConnectorErrorCode::ConnectionFailed,
            stage,
        )),
    }
}

fn audit_request_error(_: impl std::fmt::Display) -> ConnectorError {
    ConnectorError::new(ConnectorErrorCode::InvalidRequest, "audit_request")
}

fn audit_response_error(_: impl std::fmt::Display) -> ConnectorError {
    ConnectorError::new(ConnectorErrorCode::ConnectionFailed, "audit_response")
}

/// The DECIMAL order key is only exact within the frozen population bound. Its
/// max is projected by the same server-side family statement, never inferred
/// from coverage or returned rows. Invalid source guard cells are a connector
/// response fault; a valid but out-of-range population is typed unavailable.
fn guard_error_proportion_population(
    rows: &[AuditFamilySourceRow],
    sort: &AuditFamilySort,
    stage: &'static str,
) -> Result<(), ConnectorError> {
    if !matches!(sort, AuditFamilySort::ErrorProportionDesc) {
        return Ok(());
    }
    let mut population_max = None;
    for row in rows {
        let Some(value) = row
            .error_proportion_population_max
            .as_deref()
            .and_then(canonical_ascii_decimal)
        else {
            return Err(audit_response_error("invalid error proportion population"));
        };
        if let Some(expected) = population_max {
            if value != expected {
                return Err(audit_response_error(
                    "inconsistent error proportion population",
                ));
            }
        } else {
            population_max = Some(value);
        }
    }
    let Some(population_max) = population_max else {
        return Ok(());
    };
    let maximum = ERROR_PROPORTION_EXACT_POPULATION_MAX.to_string();
    if (population_max.len(), population_max) > (maximum.len(), maximum.as_str()) {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::CapabilityUnavailable,
            stage,
            "audit_family_error_proportion_population_limit_exceeded",
        ));
    }
    Ok(())
}

fn canonical_ascii_decimal(value: &str) -> Option<&str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let normalized = value.trim_start_matches('0');
    Some(if normalized.is_empty() {
        "0"
    } else {
        normalized
    })
}

fn audit_schema_error(error: AuditSqlBuildError) -> ConnectorError {
    let limitation = match error {
        AuditSqlBuildError::SchemaMissing("user") => "audit_user_filter_unavailable",
        AuditSqlBuildError::StatementFilterUnavailable => "audit_statement_filter_unavailable",
        AuditSqlBuildError::SchemaTypeUnsupported => {
            "audit_schema_required_column_type_unsupported"
        }
        _ => "audit_schema_required_column_missing",
    };
    ConnectorError::with_limitation(
        ConnectorErrorCode::CapabilityUnavailable,
        "audit_schema",
        limitation,
    )
}

fn verified_audit_schema(session: &Session) -> Result<AuditSchema, ConnectorError> {
    (session.view.capabilities.audit_loader == CapabilityState::Available)
        .then(|| session.audit_schema.clone())
        .flatten()
        .ok_or_else(|| {
            ConnectorError::with_limitation(
                ConnectorErrorCode::CapabilityUnavailable,
                "audit_schema",
                "audit_schema_unavailable",
            )
        })
}

/// Reads one `stmt` sample per returned family and attaches it in place.
/// Without a probed `stmt` column every row says so instead of reading.
/// The newest digest-missing executions in scope and each one's
/// fingerprint-length statement prefix, plus the full-window eligible count.
fn read_fingerprint_events(
    session: &mut Session,
    scope: &AuditScope,
    schema: AuditSchema,
) -> Result<(Vec<AuditOccurrenceSourceRow>, String), ConnectorError> {
    let scope = scope.clone();
    audit_fingerprint_event_statement(&session.audit_table, &scope, &schema)
        .map_err(audit_schema_error)?;
    let table = session.audit_table.clone();
    let raw_rows = nullable_rows(
        session
            .remote
            .run(RemoteOperation::AuditFingerprintEvents {
                table: table.clone(),
                scope: scope.clone(),
                schema: schema.clone(),
            })
            .map_err(|error| error.connector("audit_fingerprint_events"))?,
        "audit_fingerprint_events",
    )?;
    if raw_rows.len() > usize::from(AUDIT_FINGERPRINT_READ_LIMIT) {
        return Err(audit_response_error("audit_fingerprint_read_limit"));
    }
    let mut eligible_count = String::from("0");
    let mut rows = Vec::with_capacity(raw_rows.len());
    for mut row in raw_rows {
        let count = row
            .pop()
            .flatten()
            .filter(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| audit_response_error("audit_fingerprint_eligible_count"))?;
        rows.push(decode_occurrence_source_row(row, &schema).map_err(audit_response_error)?);
        eligible_count = count;
    }
    let query_ids = occurrence_sample_targets(rows.iter());
    if !query_ids.is_empty() {
        let samples = nullable_rows(
            session
                .remote
                .run(RemoteOperation::AuditFingerprintSamples {
                    table,
                    scope: scope.clone(),
                    query_ids,
                    schema,
                })
                .map_err(|error| error.connector("audit_fingerprint_samples"))?,
            "audit_fingerprint_samples",
        )?
        .into_iter()
        .map(decode_occurrence_sample_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(audit_response_error)?;
        attach_occurrence_samples(rows.iter_mut(), samples);
    }
    Ok((rows, eligible_count))
}

fn audit_family_samples(
    session: &mut Session,
    scope: &AuditScope,
    schema: &AuditSchema,
    rows: &mut [AuditFamilySourceRow],
    chars: u16,
) -> Result<(), ConnectorError> {
    if !schema.has(AuditColumn::Statement) {
        for row in rows.iter_mut() {
            row.statement_sample = AuditSourceStatementSample::SourceColumnAbsent;
        }
        return Ok(());
    }
    let targets = family_sample_targets(rows);
    if targets.is_empty() {
        return Ok(());
    }
    let table = session.audit_table.clone();
    audit_family_sample_statement(&table, scope, &targets, schema, chars)
        .map_err(audit_schema_error)?;
    let samples = nullable_rows(
        session
            .remote
            .run(RemoteOperation::AuditFamilySamples {
                table,
                scope: scope.clone(),
                targets,
                schema: schema.clone(),
                chars,
            })
            .map_err(|error| error.connector("audit_family_samples"))?,
        "audit_family_samples",
    )?
    .into_iter()
    .map(decode_family_sample_row)
    .collect::<Result<Vec<_>, _>>()
    .map_err(audit_response_error)?;
    attach_family_samples(rows, samples);
    Ok(())
}

/// Reads each returned execution's own `stmt` prefix and attaches it in place.
/// Without a probed `stmt` column every row says so instead of reading.
fn audit_occurrence_samples(
    session: &mut Session,
    scope: &AuditScope,
    window: AuditOccurrenceSampleWindow,
    schema: &AuditSchema,
    rows: Vec<&mut AuditOccurrenceSourceRow>,
) -> Result<(), ConnectorError> {
    if !schema.has(AuditColumn::Statement) {
        for row in rows {
            row.statement_sample = AuditSourceStatementSample::SourceColumnAbsent;
        }
        return Ok(());
    }
    let query_ids = occurrence_sample_targets(rows.iter().map(|row| &**row));
    if query_ids.is_empty() {
        return Ok(());
    }
    let table = session.audit_table.clone();
    audit_occurrence_sample_statement(&table, scope, window, &query_ids, schema)
        .map_err(audit_schema_error)?;
    let samples = nullable_rows(
        session
            .remote
            .run(RemoteOperation::AuditOccurrenceSamples {
                table,
                scope: scope.clone(),
                window,
                query_ids,
                schema: schema.clone(),
            })
            .map_err(|error| error.connector("audit_occurrence_samples"))?,
        "audit_occurrence_samples",
    )?
    .into_iter()
    .map(decode_occurrence_sample_row)
    .collect::<Result<Vec<_>, _>>()
    .map_err(audit_response_error)?;
    attach_occurrence_samples(rows, samples);
    Ok(())
}

fn audit_coverage(
    session: &mut Session,
    scope: AuditScope,
    schema: AuditSchema,
) -> Result<AuditWindowCoverage, ConnectorError> {
    let table = session.audit_table.clone();
    audit_coverage_statement(&table, &scope, &schema).map_err(audit_schema_error)?;
    let statement_filter = scope.statement_filter;
    let rows = nullable_rows(
        session
            .remote
            .run(RemoteOperation::AuditCoverage {
                table,
                scope,
                schema,
            })
            .map_err(|error| error.connector("audit_coverage"))?,
        "audit_coverage",
    )?;
    decode_audit_coverage(rows, statement_filter)
}

fn decode_audit_coverage(
    rows: Vec<Vec<Option<String>>>,
    statement_filter: AuditStatementFilter,
) -> Result<AuditWindowCoverage, ConnectorError> {
    let [row] = rows.as_slice() else {
        return Err(audit_response_error("audit_coverage_row_count"));
    };
    const EXPECTED_COLUMNS: usize = 31;
    if row.len() != EXPECTED_COLUMNS {
        return Err(audit_response_error(AuditSqlBuildError::RowShape {
            expected: EXPECTED_COLUMNS,
            actual: row.len(),
        }));
    }
    let decimal = |index: usize| {
        row[index]
            .as_deref()
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .map(str::to_owned)
            .ok_or_else(|| audit_response_error("audit_coverage_decimal"))
    };
    let metric_inputs = [
        (AuditMetricId::QueryTime, AuditMetricAggregation::Sum),
        (AuditMetricId::ScanBytes, AuditMetricAggregation::Sum),
        (AuditMetricId::CpuCost, AuditMetricAggregation::Sum),
        (AuditMetricId::MemoryCost, AuditMetricAggregation::Max),
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, (metric_id, aggregation))| {
        let start = 8 + offset * 5;
        Ok(AuditMetricInputCoverage {
            metric_id,
            sql_null_count: decimal(start)?,
            text_present_count: decimal(start + 1)?,
            nonnegative_count: decimal(start + 2)?,
            negative_count: decimal(start + 3)?,
            aggregate_input: AuditMetricInput::from_sql_cell(row[start + 4].clone()),
            aggregation,
        })
    })
    .collect::<Result<Vec<_>, ConnectorError>>()?;
    Ok(AuditWindowCoverage {
        raw_row_count: decimal(0)?,
        invalid_query_id_count: decimal(1)?,
        exact_duplicate_row_count: decimal(2)?,
        conflicting_duplicate_row_count: decimal(3)?,
        missing_digest_count: decimal(4)?,
        missing_context_count: decimal(5)?,
        retained_event_count: decimal(6)?,
        retained_family_count: decimal(7)?,
        error_count: decimal(28)?,
        metric_inputs,
        // A requested filter must return both exact counts; `all` must return
        // two SQL NULLs. Anything else is a malformed source response.
        statement_filter: if statement_filter.is_applied() {
            AuditStatementFilterCoverage::Applied {
                hidden_non_query_row_count: decimal(29)?,
                hidden_doctor_row_count: decimal(30)?,
            }
        } else if row[29].is_none() && row[30].is_none() {
            AuditStatementFilterCoverage::NotApplied
        } else {
            return Err(audit_response_error("audit_coverage_statement_filter"));
        },
    })
}

fn next_audit_generation(
    session: &mut Session,
    scope: AuditScope,
    unique_query_ids: BTreeSet<String>,
) -> u32 {
    next_audit_generation_with_selector(
        session,
        scope,
        unique_query_ids,
        AuditDiscoverySelector::OccurrenceList,
    )
}

fn next_audit_generation_with_selector(
    session: &mut Session,
    scope: AuditScope,
    unique_query_ids: BTreeSet<String>,
    selector: AuditDiscoverySelector,
) -> u32 {
    session.plan_sql_by_ref.clear();
    session.observed_queries.clear();
    session.discovery_generation = session.discovery_generation.saturating_add(1).max(1);
    let generation = session.discovery_generation;
    session.audit_discovery = Some(AuditDiscovery {
        generation,
        scope,
        unique_query_ids,
        selector,
        families_by_key: BTreeMap::new(),
    });
    generation
}

fn register_audit_occurrence_discovery(
    session: &mut Session,
    scope: AuditScope,
    rows: &[AuditOccurrenceSourceRow],
) -> u32 {
    register_audit_occurrence_discovery_with_selector(
        session,
        scope,
        rows,
        AuditDiscoverySelector::OccurrenceList,
    )
}

fn register_audit_occurrence_discovery_with_selector(
    session: &mut Session,
    scope: AuditScope,
    rows: &[AuditOccurrenceSourceRow],
    selector: AuditDiscoverySelector,
) -> u32 {
    let mut query_id_counts = BTreeMap::<String, usize>::new();
    for row in rows {
        if let Some(query_id) = row.query_id.as_deref().and_then(canonical_query_id) {
            *query_id_counts.entry(query_id).or_default() += 1;
        }
    }
    let unique_query_ids = query_id_counts
        .iter()
        .filter_map(|(query_id, count)| (*count == 1).then_some(query_id.clone()))
        .collect();
    let generation =
        next_audit_generation_with_selector(session, scope, unique_query_ids, selector);
    for (row_index, row) in rows.iter().enumerate() {
        let Some(query_id) = row.query_id.as_deref().and_then(canonical_query_id) else {
            continue;
        };
        if query_id_counts.get(&query_id) != Some(&1) {
            continue;
        }
        session.observed_queries.insert(
            query_id,
            ObservedQuery {
                source: QuerySource::AuditLoader,
                discovery_generation: generation,
                row_index,
                row: audit_profile_context_row(row),
                profile_eligible: row
                    .state
                    .as_deref()
                    .is_some_and(|state| state.eq_ignore_ascii_case("EOF")),
                plan_sql_ref: None,
                sql: None,
            },
        );
    }
    generation
}

fn audit_profile_context_row(row: &AuditOccurrenceSourceRow) -> Vec<String> {
    let metric = |metric_id| {
        row.metrics
            .iter()
            .find_map(|(id, input)| (*id == metric_id).then_some(input))
            .and_then(|input| match input {
                AuditMetricInput::SqlNull => None,
                AuditMetricInput::Text { value } => Some(value.clone()),
                AuditMetricInput::Unavailable { .. } => None,
            })
            .unwrap_or_default()
    };
    // Keep the legacy `FetchedQueryContext` public row shape stable for
    // `fetch_profile`. The lossless 14-column audit workload source row stays
    // in `AuditOccurrenceSourceRow`; this compatibility metadata deliberately
    // omits V2-only `returnRows` and `catalog` rather than introducing a second
    // public row parser.
    vec![
        row.query_id.clone().unwrap_or_default(),
        row.event_time.clone().unwrap_or_default(),
        row.event_epoch_seconds.clone().unwrap_or_default(),
        row.state.clone().unwrap_or_default(),
        metric(AuditMetricId::QueryTime),
        metric(AuditMetricId::ScanBytes),
        metric(AuditMetricId::ScanRows),
        metric(AuditMetricId::CpuCost),
        metric(AuditMetricId::MemoryCost),
        row.database.clone().unwrap_or_default(),
        row.resource_group.clone().unwrap_or_default(),
        row.native_digest.clone().unwrap_or_default(),
    ]
}

fn audit_session_identity(session: &Session) -> AuditSessionIdentity {
    AuditSessionIdentity {
        connection_id: session.view.connection_id.clone(),
        detected_version: session.view.detected_version.clone(),
        cluster_identity: AuditClusterIdentity::SessionScoped,
        audit_timestamp_time_zone: session.view.time_zone.clone(),
    }
}

fn audit_sql_unavailable(limitation: &'static str) -> ConnectorError {
    ConnectorError::with_limitation(
        ConnectorErrorCode::QuerySqlUnavailable,
        "audit_sql",
        limitation,
    )
}

fn retain_plan_sql(
    session: &mut Session,
    rows: Vec<Vec<String>>,
    expected_columns: usize,
    source: QuerySource,
) -> Result<RetainedQueryRows, ConnectorError> {
    session.plan_sql_by_ref.clear();
    session.observed_queries.clear();
    session.audit_discovery = None;
    session.discovery_generation = session.discovery_generation.saturating_add(1).max(1);
    let discovery_generation = session.discovery_generation;
    let mut id_counts = BTreeMap::<String, usize>::new();
    for row in &rows {
        if let Some(query_id) = row.first().and_then(|value| canonical_query_id(value)) {
            *id_counts.entry(query_id).or_default() += 1;
        }
    }
    let ambiguous_query_ids = id_counts
        .iter()
        .filter_map(|(query_id, count)| (*count > 1).then_some(query_id.clone()))
        .collect::<BTreeSet<_>>();
    let mut sanitized_rows = Vec::with_capacity(rows.len());
    let mut public_references = BTreeMap::new();
    let mut rejected_row_count = 0;
    for mut row in rows {
        if row.len() != expected_columns {
            rejected_row_count += 1;
            continue;
        }
        // The public API enumerates only retained rows. Persist that same
        // index so a later fetch/compare points back to the identical locator
        // even when malformed source rows were rejected before this row.
        let row_index = sanitized_rows.len();
        let statement = row.pop().unwrap_or_default();
        let sql = bounded_retained_sql(&statement);
        let query_id = row.first().and_then(|value| canonical_query_id(value));
        if let Some(query_id) = query_id.as_ref() {
            row[0] = query_id.clone();
        }
        let public_normalizable = row_is_publicly_normalizable(source, &row);
        let unique_query_id = query_id
            .as_ref()
            .is_some_and(|query_id| !ambiguous_query_ids.contains(query_id));
        let profile_eligible = unique_query_id && row_is_profile_eligible(source, &row);
        let plan_sql_ref = if public_normalizable && allowed_candidate_sql(&statement).is_ok() {
            let Some(query_id) = query_id
                .as_ref()
                .filter(|query_id| !ambiguous_query_ids.contains(*query_id))
            else {
                sanitized_rows.push(row);
                continue;
            };
            let plan_sql_ref = format!("plan-{}", opaque_id()?);
            session.plan_sql_by_ref.insert(
                plan_sql_ref.clone(),
                StoredPlanSql {
                    query_id: query_id.clone(),
                    discovery_generation,
                    sql: statement,
                },
            );
            public_references.insert(query_id.clone(), plan_sql_ref.clone());
            Some(plan_sql_ref)
        } else {
            None
        };
        if unique_query_id
            && public_normalizable
            && let Some(query_id) = query_id
        {
            session.observed_queries.insert(
                query_id,
                ObservedQuery {
                    source,
                    discovery_generation,
                    row_index,
                    row: row.clone(),
                    profile_eligible,
                    plan_sql_ref,
                    sql,
                },
            );
        }
        sanitized_rows.push(row);
    }
    Ok(RetainedQueryRows {
        discovery_generation,
        rows: sanitized_rows,
        plan_sql_references: public_references,
        ambiguous_query_ids,
        rejected_row_count,
    })
}

fn bounded_retained_sql(statement: &str) -> Option<String> {
    let trimmed = statement.trim();
    (!trimmed.is_empty() && trimmed.len() <= MAX_PLAN_SQL_BYTES).then(|| trimmed.to_owned())
}

struct RetainedQueryRows {
    discovery_generation: u32,
    rows: Vec<Vec<String>>,
    plan_sql_references: BTreeMap<String, String>,
    ambiguous_query_ids: BTreeSet<String>,
    rejected_row_count: usize,
}

fn row_is_profile_eligible(source: QuerySource, row: &[String]) -> bool {
    row.get(3).is_some_and(|state| match source {
        QuerySource::AuditLoader => state.eq_ignore_ascii_case("EOF"),
        QuerySource::ProfileList => state.eq_ignore_ascii_case("Finished"),
        QuerySource::Auto => false,
    })
}

fn row_is_publicly_normalizable(source: QuerySource, row: &[String]) -> bool {
    match source {
        QuerySource::AuditLoader => {
            row.len() == 12 && bounded_public_text(&row[0], 36) && bounded_public_text(&row[1], 64)
        }
        QuerySource::ProfileList => {
            row.len() == 4 && bounded_public_text(&row[0], 36) && bounded_public_text(&row[3], 32)
        }
        QuerySource::Auto => false,
    }
}

fn bounded_public_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn validate_observed_query(
    session: &Session,
    query_id: &str,
    discovery_generation: u32,
) -> Result<ObservedQuery, ConnectorError> {
    if session.discovery_generation != discovery_generation {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_discovery_generation_expired",
        ));
    }
    let Some(query_id) = canonical_query_id(query_id) else {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_id_not_observed_in_current_session",
        ));
    };
    let Some(observed) = session.observed_queries.get(&query_id) else {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_id_not_observed_in_current_session",
        ));
    };
    if observed.discovery_generation != discovery_generation {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_discovery_generation_expired",
        ));
    }
    Ok(observed.clone())
}

fn validate_observed_profile(
    session: &Session,
    query_id: &str,
    discovery_generation: u32,
) -> Result<ObservedQuery, ConnectorError> {
    let observed = validate_observed_query(session, query_id, discovery_generation)?;
    if !observed.profile_eligible {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_not_finished",
        ));
    }
    Ok(observed)
}

fn validate_observed_running_profile(
    session: &Session,
    query_id: &str,
    discovery_generation: u32,
) -> Result<ObservedQuery, ConnectorError> {
    let observed = validate_observed_query(session, query_id, discovery_generation)?;
    if observed.source != QuerySource::ProfileList
        || !observed
            .row
            .get(3)
            .is_some_and(|state| state.eq_ignore_ascii_case("Running"))
    {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "query_source",
            "query_not_running",
        ));
    }
    Ok(observed)
}

fn validate_analyze_profile_node_ids(node_ids: &[u32]) -> Result<(), ConnectorError> {
    if node_ids.len() > ANALYZE_PROFILE_MAX_NODE_IDS
        || node_ids.iter().any(|id| *id > i32::MAX as u32)
        || node_ids.iter().copied().collect::<BTreeSet<_>>().len() != node_ids.len()
    {
        return Err(ConnectorError::new(
            ConnectorErrorCode::InvalidRequest,
            "request",
        ));
    }
    Ok(())
}

fn analyze_profile_statement(query_id: &str, node_ids: &[u32]) -> Result<String, ConnectorError> {
    let query_id = canonical_query_id(query_id)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
    validate_analyze_profile_node_ids(node_ids)?;
    let mut statement = format!("ANALYZE PROFILE FROM {}", quote_sql_literal(&query_id));
    for node_id in node_ids {
        statement.push_str(&format!(",{node_id}"));
    }
    Ok(statement)
}

fn validate_official_profile_rows(rows: &[String]) -> Result<(), ConnectorError> {
    let bytes = rows
        .iter()
        .try_fold(rows.len().saturating_sub(1), |size, row| {
            size.checked_add(row.len())
        });
    if rows.len() > mysql_text::MAX_RESULT_ROWS
        || bytes.is_none_or(|bytes| bytes > ANALYZE_PROFILE_MAX_OUTPUT_BYTES)
    {
        return Err(RemoteError::OutputBudgetExceeded.connector("analyze_profile"));
    }
    Ok(())
}

fn fetch_profile_session(
    session: &mut Session,
    query_id: &str,
    discovery_generation: u32,
) -> Result<FetchedProfile, ConnectorError> {
    let query_id = canonical_query_id(query_id)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
    let observed = validate_observed_profile(session, &query_id, discovery_generation)?;
    fetch_observed_profile_session(session, query_id, observed, discovery_generation)
}

fn fetch_running_profile_snapshot_session(
    session: &mut Session,
    query_id: &str,
    discovery_generation: u32,
) -> Result<FetchedProfile, ConnectorError> {
    let query_id = canonical_query_id(query_id)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request"))?;
    let observed = validate_observed_running_profile(session, &query_id, discovery_generation)?;
    fetch_observed_profile_session(session, query_id, observed, discovery_generation)
}

fn fetch_observed_profile_session(
    session: &mut Session,
    query_id: String,
    observed: ObservedQuery,
    discovery_generation: u32,
) -> Result<FetchedProfile, ConnectorError> {
    let profile = match session
        .remote
        .run(RemoteOperation::GetQueryProfile {
            query_id: query_id.clone(),
        })
        .map_err(|error| error.connector("query_source"))?
    {
        RemoteResponse::Profile(profile) if !profile.is_empty() => profile,
        RemoteResponse::Profile(_) => {
            return Err(ConnectorError::with_limitation(
                ConnectorErrorCode::ProfileUnavailable,
                "query_source",
                "not_found_or_expired_or_not_profiled",
            ));
        }
        _ => {
            return Err(ConnectorError::new(
                ConnectorErrorCode::ProfileUnavailable,
                "query_source",
            ));
        }
    };
    session.view.capabilities.get_query_profile = CapabilityState::Available;
    session.successful_profiles.record(query_id.clone());
    Ok(FetchedProfile {
        query_id,
        detected_version: session.view.detected_version.clone(),
        query_context: FetchedQueryContext {
            source: observed.source,
            discovery_generation,
            row_index: observed.row_index,
            row: observed.row,
            time_zone: session.view.time_zone.clone(),
            plan_sql_ref: observed.plan_sql_ref,
        },
        profile_text: profile.into_bytes(),
    })
}

fn filter_profile_list_rows(rows: Vec<Vec<String>>, status: Option<&str>) -> Vec<Vec<String>> {
    rows.into_iter()
        .filter(|row| {
            let Some(expected) = status else { return true };
            let Some(source_state) = row.get(3) else {
                return true;
            };
            match expected {
                "finished" => source_state.eq_ignore_ascii_case("finished"),
                "error" | "failed" => source_state.eq_ignore_ascii_case("error"),
                _ => false,
            }
        })
        .collect()
}

/// Fixed 3.5.14 SHOW PROFILELIST column 4 is the profiled statement text. The
/// isolated allin1 fixture showed it keeps this tool's leading marker (v0.7-08
/// receipt), so only an exact prefix match counts; a row without the column
/// is not hidden and is later rejected by the shape check.
fn profile_list_row_is_doctor_statement(row: &[String]) -> bool {
    row.get(4).is_some_and(|statement| {
        DOCTOR_STATEMENT_MARKERS
            .iter()
            .any(|marker| statement.starts_with(marker))
    })
}

fn decode_audit_search_hidden_counts(
    rows: Vec<Vec<String>>,
) -> Result<QueryListStatementFilterCoverage, ConnectorError> {
    let decimal = |value: &String| {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.clone())
            .ok_or_else(|| audit_response_error("audit_search_hidden_counts_decimal"))
    };
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [non_query, doctor] => Ok(QueryListStatementFilterCoverage::Applied {
                hidden_non_query_row_count: decimal(non_query)?,
                hidden_doctor_row_count: decimal(doctor)?,
            }),
            _ => Err(audit_response_error("audit_search_hidden_counts_shape")),
        },
        _ => Err(audit_response_error("audit_search_hidden_counts_row_count")),
    }
}

fn explain_session(
    session: &mut Session,
    candidate_sql: String,
    mode: ExplainMode,
) -> Result<ExplainResult, ConnectorError> {
    let rows = match session
        .remote
        .run(RemoteOperation::Explain {
            candidate_sql,
            mode: mode.clone(),
        })
        .map_err(|error| error.connector("explain"))?
    {
        RemoteResponse::Explain(rows) => rows,
        _ => {
            return Err(ConnectorError::new(
                ConnectorErrorCode::ExplainRejected,
                "explain",
            ));
        }
    };
    if mode == ExplainMode::Costs {
        session.view.capabilities.explain = CapabilityState::Available;
    }
    Ok(ExplainResult {
        rows,
        mode,
        detected_version: session.view.detected_version.clone(),
        default_database: session.default_database.clone(),
    })
}

fn valid_plan_sql_ref(value: &str) -> bool {
    value.len() == 53
        && value.starts_with("plan-")
        && value[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_search(search: &QuerySearch) -> Result<(), ConnectorError> {
    let limit = normalized_limit(search.limit);
    if search.start_seconds > search.end_seconds
        || search.end_seconds - search.start_seconds > MAX_QUERY_WINDOW.as_secs()
        || limit > MAX_QUERY_ROWS
    {
        return Err(ConnectorError::new(
            ConnectorErrorCode::InvalidRequest,
            "limits",
        ));
    }
    validate_query_parameters(&QueryParameters {
        start_seconds: search.start_seconds,
        end_seconds: search.end_seconds,
        state: search.state.clone(),
        digest: search.digest.clone(),
        limit,
        statement_filter: search.statement_filter,
    })
}

fn normalized_limit(limit: u16) -> u16 {
    if limit == 0 { 100 } else { limit }
}

fn audit_statement(table: &str, parameters: &QueryParameters) -> Result<String, ConnectorError> {
    let predicate = audit_search_field_predicate(parameters)?;
    // Same retained condition as the Audit history filter. The caller only
    // applies it after the session proved `isQuery` TINYINT and `stmt`.
    let statement_filter = if parameters.statement_filter.is_applied() {
        format!(
            " AND {}",
            audit_workload::statement_filter_retained_condition()
        )
    } else {
        String::new()
    };
    Ok(format!(
        "SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, UNIX_TIMESTAMP(`timestamp`) AS `event_epoch_seconds`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest`, `stmt` AS `statement` FROM {}{predicate}{statement_filter} ORDER BY `timestamp` DESC LIMIT {}",
        quote_identifier_path(table),
        parameters.limit,
    ))
}

/// Exact raw-row counts of the two hidden statement classes in the same window
/// and status/digest filters as the discovery list, independent of its limit.
/// One scan; the buckets are disjoint and use the Audit history conditions.
fn audit_search_hidden_counts_statement(
    table: &str,
    parameters: &QueryParameters,
) -> Result<String, ConnectorError> {
    if !parameters.statement_filter.is_applied() {
        return Err(ConnectorError::new(
            ConnectorErrorCode::InvalidRequest,
            "request",
        ));
    }
    let predicate = audit_search_field_predicate(parameters)?;
    Ok(format!(
        "SELECT COUNT(CASE WHEN {} THEN 1 END) AS `hidden_non_query_row_count`, COUNT(CASE WHEN {} THEN 1 END) AS `hidden_doctor_row_count` FROM {}{predicate}",
        audit_workload::statement_filter_hidden_non_query_condition(),
        audit_workload::statement_filter_hidden_doctor_condition(),
        quote_identifier_path(table),
    ))
}

/// Window plus status/digest filters shared by the discovery list and its
/// hidden-row counts.
fn audit_search_field_predicate(parameters: &QueryParameters) -> Result<String, ConnectorError> {
    validate_query_parameters(parameters)?;
    let state_filter = match parameters.state.as_deref() {
        Some(state) => {
            // API/DTO status is a product state. AuditLoader records 3.5.14
            // source state codes, so never forward an unrecognized value.
            let source_state = audit_source_state(state).ok_or_else(|| {
                ConnectorError::new(ConnectorErrorCode::InvalidRequest, "request")
            })?;
            format!(" AND `state` = {}", quote_sql_literal(source_state))
        }
        None => String::new(),
    };
    let digest_filter = parameters
        .digest
        .as_deref()
        .map(|digest| format!(" AND `digest` = {}", quote_sql_literal(digest)))
        .unwrap_or_default();
    Ok(format!(
        " WHERE `timestamp` >= FROM_UNIXTIME({}) AND `timestamp` <= FROM_UNIXTIME({}){state_filter}{digest_filter}",
        parameters.start_seconds, parameters.end_seconds,
    ))
}

fn validate_query_parameters(parameters: &QueryParameters) -> Result<(), ConnectorError> {
    if parameters.start_seconds > parameters.end_seconds
        || parameters.end_seconds - parameters.start_seconds > MAX_QUERY_WINDOW.as_secs()
        || !(1..=MAX_QUERY_ROWS).contains(&parameters.limit)
        || parameters
            .state
            .as_deref()
            .is_some_and(|state| audit_source_state(state).is_none())
        || parameters.digest.as_deref().is_some_and(|digest| {
            !(1..=128).contains(&digest.len())
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(ConnectorError::new(
            ConnectorErrorCode::InvalidRequest,
            "request",
        ));
    }
    Ok(())
}

/// Product-to-source mapping verified against the isolated 3.5.14
/// AuditLoader fixture. AuditLoader records terminal source codes (`EOF` and
/// `ERR`); it does not expose a running-query browse state in v0.1.
fn audit_source_state(value: &str) -> Option<&'static str> {
    match value {
        "finished" => Some("EOF"),
        "error" | "failed" => Some("ERR"),
        _ => None,
    }
}

fn quote_sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn allowed_candidate_sql(candidate_sql: &str) -> Result<String, ConnectorError> {
    if candidate_sql.len() > MAX_PLAN_SQL_BYTES || !plan_only_select(candidate_sql) {
        return Err(ConnectorError::new(
            ConnectorErrorCode::StatementNotAllowed,
            "security",
        ));
    }
    Ok(candidate_sql.to_owned())
}

fn valid_execution_attempt_id(value: &str) -> Result<String, ConnectorError> {
    canonical_query_id(value).ok_or_else(|| {
        ConnectorError::with_limitation(
            ConnectorErrorCode::InvalidRequest,
            "execution_attempt",
            "execution_attempt_id_invalid",
        )
    })
}

fn claim_execution_attempt(
    session: &mut Session,
    execution_attempt_id: String,
    method: ExecutionAttemptMethod,
) -> Result<(), ConnectorError> {
    if session
        .execution_attempts
        .contains_key(&execution_attempt_id)
    {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ExecutionAttemptSubmitted,
            "execution_attempt",
            "execution_attempt_already_submitted",
        ));
    }
    if session.execution_attempts.len() >= MAX_EXECUTION_ATTEMPTS_PER_SESSION {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::SessionLimit,
            "execution_attempt",
            "execution_attempt_limit_reached",
        ));
    }
    session.execution_attempts.insert(
        execution_attempt_id,
        ExecutionAttempt {
            method,
            completed_query_id: None,
        },
    );
    Ok(())
}

fn complete_standard_execution_attempt(
    session: &mut Session,
    execution_attempt_id: &str,
    query_id: &str,
) -> Result<(), ConnectorError> {
    let Some(attempt) = session.execution_attempts.get_mut(execution_attempt_id) else {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "execution_attempt",
            "execution_attempt_not_eligible_for_official_profile",
        ));
    };
    if attempt.method != ExecutionAttemptMethod::StandardProfile {
        return Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "execution_attempt",
            "execution_attempt_not_eligible_for_official_profile",
        ));
    }
    attempt.completed_query_id = Some(query_id.to_owned());
    Ok(())
}

fn validate_verified_standard_attempt(
    session: &Session,
    execution_attempt_id: &str,
    query_id: &str,
) -> Result<(), ConnectorError> {
    let eligible = session
        .execution_attempts
        .get(execution_attempt_id)
        .is_some_and(|attempt| {
            attempt.method == ExecutionAttemptMethod::StandardProfile
                && attempt.completed_query_id.as_deref() == Some(query_id)
        });
    if eligible {
        Ok(())
    } else {
        Err(ConnectorError::with_limitation(
            ConnectorErrorCode::ProfileUnavailable,
            "execution_attempt",
            "execution_attempt_not_eligible_for_official_profile",
        ))
    }
}

fn explain_statement(candidate_sql: &str, mode: &ExplainMode) -> Result<String, ConnectorError> {
    let candidate_sql = allowed_candidate_sql(candidate_sql)?;
    let prefix = match mode {
        ExplainMode::Logical => "EXPLAIN LOGICAL",
        ExplainMode::Verbose => "EXPLAIN VERBOSE",
        ExplainMode::Costs => "EXPLAIN COSTS",
    };
    Ok(format!("{prefix} {candidate_sql}"))
}

fn plan_only_select(sql: &str) -> bool {
    if sql.as_bytes().windows(3).any(|window| window == b"/*!") {
        return false;
    }
    let tokens = sql_tokens(sql);
    let Some(first) = tokens.first() else {
        return false;
    };
    if !matches!(first.as_str(), "select" | "with") {
        return false;
    }
    !tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "explain"
                | "analyze"
                | "set"
                | "insert"
                | "update"
                | "delete"
                | "create"
                | "alter"
                | "drop"
                | "truncate"
                | "load"
                | "admin"
                | "grant"
                | "revoke"
                | "use"
                | "call"
        )
    })
}

/// Only lexical security classification: quoted/ordinary-comment text never becomes a statement keyword.
/// MySQL executable comments are rejected before tokenization.
fn sql_tokens(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'\"' | b'`' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    if bytes[index - 1] == quote {
                        break;
                    }
                }
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b';' => return vec![";".into()],
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                tokens.push(String::from_utf8_lossy(&bytes[start..index]).to_ascii_lowercase());
            }
            _ => index += 1,
        }
    }
    tokens
}

/// The release behind `current_version()` ("3.4.10-965be75"), when the
/// multi-version audit covers it (3.1–4.1); anything else connects as
/// unsupported and never borrows a nearby release.
fn supported_release(value: &str) -> Option<doctor_core::ReleaseProfile> {
    let release = value.trim().split(['-', '+']).next()?;
    match doctor_core::resolve_release(release) {
        doctor_core::ReleaseResolution::Audited(profile) => Some(profile),
        _ => None,
    }
}

fn valid_host(value: &str) -> bool {
    value.parse::<IpAddr>().is_ok()
        || (!value.is_empty()
            && value.len() <= 253
            && !value.contains(['/', '\\', '@', '?', '#', ':'])
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
}

pub fn is_literal_loopback(host: &str) -> bool {
    matches!(host.parse::<IpAddr>(), Ok(IpAddr::V4(address)) if address.octets() == [127, 0, 0, 1])
        || matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(address)) if address.is_loopback())
}
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_'
                || byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphabetic())
        })
}
fn valid_identifier_path(value: &str) -> bool {
    (1..=3).contains(&value.split('.').count()) && value.split('.').all(valid_identifier)
}
fn quote_identifier_path(value: &str) -> String {
    value
        .split('.')
        .map(|part| format!("`{part}`"))
        .collect::<Vec<_>>()
        .join(".")
}
fn endpoint_label(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
/// Validates the opaque query ID shape before any remote Profile fetch.
///
/// This is deliberately shared with the API boundary so an invalid connected
/// comparison request is rejected before it can fetch either side.
pub fn is_valid_query_id(value: &str) -> bool {
    canonical_query_id(value).is_some()
}

pub fn canonical_query_id(value: &str) -> Option<String> {
    is_uuid(value).then(|| value.to_ascii_lowercase())
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                .then_some(byte == b'-')
                .unwrap_or_else(|| byte.is_ascii_hexdigit())
        })
}
fn opaque_id() -> Result<String, ConnectorError> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::ConnectionFailed, "connection"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::thread;

    use doctor_contracts::{
        AuditFamilySort, AuditOccurrenceSort, AuditOverviewFamilyTopSort, AuditOverviewPrimary,
        AuditOverviewRequest, AuditWindow,
    };

    use super::*;

    #[derive(Default)]
    struct TestClock(AtomicUsize);
    impl TestClock {
        fn advance(&self, seconds: usize) {
            self.0.store(seconds, Ordering::Relaxed);
        }
    }
    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.load(Ordering::Relaxed) as u64)
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        AuditFamilySamples,
        AuditOccurrenceSamples,
        AuditFingerprintEvents,
        AuditFingerprintSamples,
        AuditCollectionSlice,
        Version,
        TimeZone,
        AuditProbe,
        AuditSearch,
        AuditSearchHiddenCounts,
        AuditSchema,
        AuditOccurrences,
        AuditFamilies,
        AuditFamilyDetail,
        AuditFamilyDetailSqlLookup,
        AuditCoverage,
        AuditConcurrency,
        AuditPerformance,
        AuditOverviewFamilyTop,
        AuditSqlLookup,
        AuditActiveAt,
        AuditActiveAtSqlLookup,
        ProfileList,
        GetProfile,
        AnalyzeProfile,
        Advisor,
        ExplainLogical,
        ExplainVerbose,
        ExplainCosts,
        ProfiledSelect,
    }

    struct FakeRemote {
        operations: Arc<Mutex<Vec<Operation>>>,
        closes: Arc<AtomicUsize>,
        version: String,
        audit: Result<(), RemoteError>,
        audit_search: Result<(), RemoteError>,
        audit_schema_rows: Vec<Vec<Option<String>>>,
        audit_family_rows: Vec<Vec<Option<String>>>,
        audit_family_detail_rows: Option<Vec<Vec<Option<String>>>>,
        profile_list: Result<(), RemoteError>,
        profile_list_rows: Option<Vec<Vec<String>>>,
        advisor: Result<(), RemoteError>,
        profile: String,
        official_profile: Result<Vec<String>, RemoteError>,
    }
    impl Remote for FakeRemote {
        fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError> {
            let kind = match &operation {
                RemoteOperation::CurrentVersion => Operation::Version,
                RemoteOperation::SessionTimeZone => Operation::TimeZone,
                RemoteOperation::AuditProbe { .. } => Operation::AuditProbe,
                RemoteOperation::AuditSearch { .. } => Operation::AuditSearch,
                RemoteOperation::AuditSearchHiddenCounts { .. } => {
                    Operation::AuditSearchHiddenCounts
                }
                RemoteOperation::AuditSchemaProbe { .. } => Operation::AuditSchema,
                RemoteOperation::AuditOccurrences { .. } => Operation::AuditOccurrences,
                RemoteOperation::AuditFamilies { .. } => Operation::AuditFamilies,
                RemoteOperation::AuditFamilySamples { .. } => Operation::AuditFamilySamples,
                RemoteOperation::AuditOccurrenceSamples { .. } => Operation::AuditOccurrenceSamples,
                RemoteOperation::AuditFamilyDetail { .. } => Operation::AuditFamilyDetail,
                RemoteOperation::AuditFamilyDetailSqlLookup { .. } => {
                    Operation::AuditFamilyDetailSqlLookup
                }
                RemoteOperation::AuditCoverage { .. } => Operation::AuditCoverage,
                RemoteOperation::AuditFingerprintEvents { .. } => Operation::AuditFingerprintEvents,
                RemoteOperation::AuditCollectionSlice { .. } => Operation::AuditCollectionSlice,
                RemoteOperation::AuditFingerprintSamples { .. } => {
                    Operation::AuditFingerprintSamples
                }
                RemoteOperation::AuditConcurrency { .. } => Operation::AuditConcurrency,
                RemoteOperation::AuditPerformance { .. } => Operation::AuditPerformance,
                RemoteOperation::AuditOverviewFamilyTop { .. } => Operation::AuditOverviewFamilyTop,
                RemoteOperation::AuditSqlLookup { .. } => Operation::AuditSqlLookup,
                RemoteOperation::AuditActiveAt { .. } => Operation::AuditActiveAt,
                RemoteOperation::AuditActiveAtSqlLookup { .. } => Operation::AuditActiveAtSqlLookup,
                RemoteOperation::ShowProfileList { .. } => Operation::ProfileList,
                RemoteOperation::GetQueryProfile { .. } => Operation::GetProfile,
                RemoteOperation::AnalyzeProfile { .. } => Operation::AnalyzeProfile,
                RemoteOperation::ShowPlanAdvisor => Operation::Advisor,
                RemoteOperation::Explain { mode, .. } => match mode {
                    ExplainMode::Logical => Operation::ExplainLogical,
                    ExplainMode::Verbose => Operation::ExplainVerbose,
                    ExplainMode::Costs => Operation::ExplainCosts,
                },
                RemoteOperation::ProfiledSelect { .. } => Operation::ProfiledSelect,
            };
            self.operations.lock().unwrap().push(kind);
            match operation {
                RemoteOperation::CurrentVersion => {
                    Ok(RemoteResponse::Version(self.version.clone()))
                }
                RemoteOperation::SessionTimeZone => Ok(RemoteResponse::TimeZone(SessionTimeZone {
                    session_time_zone: "SYSTEM".into(),
                    system_time_zone: "Etc/UTC".into(),
                    effective_time_zone: "Etc/UTC".into(),
                    current_utc_offset_seconds: 0,
                })),
                RemoteOperation::AuditProbe { .. } => {
                    self.audit.map(|_| RemoteResponse::Rows(vec![]))
                }
                RemoteOperation::AuditSchemaProbe { .. } => self
                    .audit
                    .map(|_| RemoteResponse::NullableRows(self.audit_schema_rows.clone())),
                RemoteOperation::AuditCoverage { .. } => self
                    .audit_search
                    .map(|_| RemoteResponse::NullableRows(audit_coverage_rows())),
                RemoteOperation::AuditCollectionSlice {
                    table,
                    scope,
                    schema,
                } => {
                    self.audit_search?;
                    let sql = audit_collection_slice_statement(&table, &scope, &schema).unwrap();
                    assert!(sql.ends_with("ORDER BY `event_time` ASC, `query_id` ASC LIMIT 1000"));
                    let row = |id: &str, digest: Option<&str>, sql: &str| {
                        let mut row: Vec<Option<String>> = vec![Some("0".into()); 23];
                        row[0] = Some(id.into());
                        row[1] = Some("2026-08-30 00:00:00".into());
                        row[2] = Some("1788048000".into());
                        row[3] = Some("EOF".into());
                        (row[10], row[11], row[13]) = (
                            Some("default_catalog".into()),
                            Some("demo".into()),
                            digest.map(str::to_owned),
                        );
                        row[18] = Some(sql.into());
                        (row[19], row[20], row[21]) =
                            (Some("0".into()), Some("0".into()), Some("1".into()));
                        row[22] = Some("3".into());
                        row
                    };
                    Ok(RemoteResponse::NullableRows(vec![
                        row(
                            "00000000-0000-0000-0000-000000000001",
                            Some("ab12"),
                            "SELECT 1",
                        ),
                        row(
                            "00000000-0000-0000-0000-000000000002",
                            None,
                            "SELECT id FROM t WHERE id = 7",
                        ),
                    ]))
                }
                RemoteOperation::AuditFingerprintEvents {
                    table,
                    scope,
                    schema,
                } => {
                    self.audit_search?;
                    let sql = audit_fingerprint_event_statement(&table, &scope, &schema).unwrap();
                    assert!(sql.contains("COUNT(*) OVER () AS `eligible_count`"));
                    assert!(sql.ends_with("ORDER BY `event_time` DESC, `query_id` ASC LIMIT 500"));
                    let mut row: Vec<Option<String>> = vec![Some("0".into()); 19];
                    row[0] = Some("00000000-0000-0000-0000-000000000001".into());
                    row[1] = Some("2026-08-30 00:00:00".into());
                    row[3] = Some("EOF".into());
                    (row[10], row[11], row[13]) =
                        (Some("default_catalog".into()), Some("demo".into()), None);
                    row[18] = Some("1".into());
                    Ok(RemoteResponse::NullableRows(vec![row]))
                }
                RemoteOperation::AuditFingerprintSamples {
                    table,
                    scope,
                    query_ids,
                    schema,
                } => {
                    let sql =
                        audit_fingerprint_sample_statement(&table, &scope, &query_ids, &schema)
                            .unwrap();
                    assert!(sql.contains("LEFT(`stmt`, 8192)"));
                    Ok(RemoteResponse::NullableRows(
                        query_ids
                            .into_iter()
                            .map(|id| {
                                vec![
                                    Some(id),
                                    Some("SELECT 3".into()),
                                    Some("0".into()),
                                    Some("0".into()),
                                    Some("1".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditConcurrency { scope, .. } => self
                    .audit_search
                    .map(|_| RemoteResponse::NullableRows(audit_concurrency_rows(&scope))),
                RemoteOperation::AuditPerformance { .. } => Err(RemoteError::Unsupported),
                RemoteOperation::AuditOverviewFamilyTop { request, .. } => {
                    self.audit_search?;
                    assert_eq!(request.limit, AUDIT_OVERVIEW_FAMILY_TOP_LIMIT);
                    Ok(RemoteResponse::NullableRows(family_fixture_rows()))
                }
                RemoteOperation::AuditOccurrences { request, .. } => {
                    self.audit_search?;
                    assert!(request.limit <= 500);
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some("22222222-2222-2222-2222-222222222222".into()),
                        Some("2026-09-02 00:00:00".into()),
                        Some("1788278400".into()),
                        Some("EOF".into()),
                        Some("10".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("internal".into()),
                        Some("db".into()),
                        Some("default_wg".into()),
                        Some("a0b1".into()),
                        Some("".into()),
                        Some("1".into()),
                        Some("0".into()),
                        Some("1".into()),
                    ]]))
                }
                RemoteOperation::AuditFamilySamples {
                    table,
                    scope,
                    targets,
                    schema,
                    chars,
                } => {
                    let sql =
                        audit_family_sample_statement(&table, &scope, &targets, &schema, chars)
                            .unwrap();
                    assert!(sql.contains(&format!("LEFT(`stmt`, {chars})")));
                    Ok(RemoteResponse::NullableRows(
                        targets
                            .into_iter()
                            .map(|target| {
                                vec![
                                    Some(target.catalog),
                                    Some(target.database),
                                    Some(target.native_digest),
                                    Some("SELECT 1".into()),
                                    Some("0".into()),
                                    Some("0".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditOccurrenceSamples { query_ids, .. } => {
                    Ok(RemoteResponse::NullableRows(
                        query_ids
                            .into_iter()
                            .map(|query_id| {
                                vec![
                                    Some(query_id),
                                    Some("SELECT 2".into()),
                                    Some("0".into()),
                                    Some("0".into()),
                                    Some("1".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditFamilies { request, .. } => {
                    self.audit_search?;
                    assert!(request.limit <= 200);
                    Ok(RemoteResponse::NullableRows(self.audit_family_rows.clone()))
                }
                RemoteOperation::AuditFamilyDetail {
                    table,
                    parent,
                    target,
                    candidates,
                    schema,
                } => {
                    self.audit_search?;
                    let sql = audit_family_detail_statement(
                        &table,
                        &parent,
                        &target,
                        &candidates,
                        &schema,
                    )
                    .unwrap();
                    assert!(candidates.len() <= 500);
                    assert!(!sql.contains("AS `statement`"));
                    Ok(RemoteResponse::NullableRows(
                        self.audit_family_detail_rows
                            .clone()
                            .unwrap_or_else(|| family_detail_fixture_rows(&target, &candidates)),
                    ))
                }
                RemoteOperation::AuditFamilyDetailSqlLookup {
                    table,
                    parent,
                    target,
                    query_id,
                    schema,
                } => {
                    let sql = audit_family_detail_sql_lookup_statement(
                        &table, &parent, &target, &query_id, &schema,
                    )
                    .unwrap();
                    assert!(
                        parent.native_digest.is_none(),
                        "must retain parent, not narrowed family scope"
                    );
                    assert!(sql.contains("EXISTS (SELECT 1 FROM family_events"));
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(query_id),
                        Some("2026-09-02 00:00:00".into()),
                        Some("SELECT retained FROM t".into()),
                    ]]))
                }
                RemoteOperation::AuditSqlLookup { request, .. } => {
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(request.query_id),
                        Some("2026-09-02 00:00:00".into()),
                        Some("SELECT retained FROM t".into()),
                    ]]))
                }
                RemoteOperation::AuditActiveAt {
                    at_milliseconds, ..
                } => self
                    .audit_search
                    .map(|_| RemoteResponse::NullableRows(audit_active_at_rows(at_milliseconds))),
                RemoteOperation::AuditActiveAtSqlLookup { request, .. } => {
                    self.audit_search?;
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(request.query_id),
                        Some("2026-09-02 00:00:00".into()),
                        Some("SELECT retained FROM t".into()),
                    ]]))
                }
                RemoteOperation::ShowProfileList { limit } => {
                    assert!(limit == 1 || limit == MAX_QUERY_ROWS);
                    self.profile_list.map(|_| {
                        RemoteResponse::Rows(self.profile_list_rows.clone().unwrap_or_else(|| {
                            vec![vec![
                                "11111111-1111-1111-1111-111111111111".into(),
                                "2026-09-02 00:00:00".into(),
                                "1s".into(),
                                "Finished".into(),
                                "SELECT 1".into(),
                            ]]
                        }))
                    })
                }
                RemoteOperation::ShowPlanAdvisor => self
                    .advisor
                    .map(|_| RemoteResponse::Rows(vec![vec!["guide".into()]])),
                RemoteOperation::AuditSearch { table, parameters } => {
                    self.audit_search?;
                    let statement = audit_statement(&table, &parameters).unwrap();
                    assert!(statement.starts_with("SELECT `queryId` AS `query_id`"));
                    assert!(statement.contains("`timestamp` AS `event_time`"));
                    assert!(
                        statement.contains("UNIX_TIMESTAMP(`timestamp`) AS `event_epoch_seconds`")
                    );
                    assert!(!statement.contains("DATE_FORMAT"));
                    assert!(statement.contains("`starrocks_audit_db__`.`starrocks_audit_tbl__`"));
                    assert_eq!(parameters.limit, 100);
                    assert_eq!(
                        statement.contains("`isQuery` = 1 AND (`stmt` IS NULL OR (`stmt` NOT LIKE '/* sr-doctor */ %' AND `stmt` NOT LIKE '/* starrocks-doctor */ %'))"),
                        parameters.statement_filter.is_applied()
                    );
                    Ok(RemoteResponse::Rows(vec![vec![
                        "22222222-2222-2222-2222-222222222222".into(),
                        "2026-09-02 00:00:00".into(),
                        "1788278400".into(),
                        "EOF".into(),
                        "10".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "db".into(),
                        "default_wg".into(),
                        "a0b1".into(),
                        "SELECT secret_canary FROM t".into(),
                    ]]))
                }
                RemoteOperation::AuditSearchHiddenCounts { table, parameters } => {
                    self.audit_search?;
                    let statement =
                        audit_search_hidden_counts_statement(&table, &parameters).unwrap();
                    assert!(statement.starts_with("SELECT COUNT(CASE WHEN"));
                    Ok(RemoteResponse::Rows(vec![vec!["5".into(), "2".into()]]))
                }
                RemoteOperation::GetQueryProfile { .. } => {
                    Ok(RemoteResponse::Profile(self.profile.clone()))
                }
                RemoteOperation::AnalyzeProfile { query_id, node_ids } => {
                    assert!(analyze_profile_statement(&query_id, &node_ids).is_ok());
                    self.official_profile
                        .clone()
                        .map(RemoteResponse::AnalyzeProfile)
                }
                RemoteOperation::Explain { candidate_sql, .. } => {
                    assert!(
                        candidate_sql.starts_with("SELECT") || candidate_sql.starts_with("WITH")
                    );
                    Ok(RemoteResponse::Explain(vec!["PLAN FRAGMENT".into()]))
                }
                RemoteOperation::ProfiledSelect { candidate_sql } => {
                    assert!(
                        candidate_sql.starts_with("SELECT") || candidate_sql.starts_with("WITH")
                    );
                    Ok(RemoteResponse::ProfiledQuery {
                        query_id: "33333333-3333-3333-3333-333333333333".into(),
                        profile: self.profile.clone(),
                    })
                }
            }
        }
        fn close(&mut self) {
            self.closes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn audit_schema_rows() -> Vec<Vec<Option<String>>> {
        [
            "queryId",
            "timestamp",
            "state",
            "queryTime",
            "scanBytes",
            "scanRows",
            "cpuCostNs",
            "memCostBytes",
            "returnRows",
            "catalog",
            "db",
            "resourceGroup",
            "digest",
            "stmt",
            "errorCode",
            "pendingTimeMs",
        ]
        .into_iter()
        .map(|column| {
            let source_type = match column {
                "timestamp" => "DATETIME",
                "queryTime" | "scanBytes" | "scanRows" | "cpuCostNs" | "memCostBytes"
                | "returnRows" | "pendingTimeMs" => "BIGINT",
                _ => "VARCHAR",
            };
            vec![Some(column.into()), Some(source_type.into())]
        })
        .collect()
    }

    fn audit_schema_rows_with_user() -> Vec<Vec<Option<String>>> {
        let mut rows = audit_schema_rows();
        rows.push(vec![Some("user".into()), Some("VARCHAR".into())]);
        rows
    }
    fn audit_coverage_rows() -> Vec<Vec<Option<String>>> {
        let mut row = vec![
            Some("1".into()), // raw through retained family
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("1".into()),
            Some("1".into()),
        ];
        for aggregate in ["10", "0", "0", "0"] {
            row.extend([
                Some("0".into()),
                Some("1".into()),
                Some("1".into()),
                Some("0".into()),
                Some(aggregate.into()),
            ]);
        }
        row.push(Some("0".into())); // exact full-window ERR count
        row.extend([None, None]); // statement filter not applied: typed SQL NULLs
        vec![row]
    }

    fn family_fixture_rows() -> Vec<Vec<Option<String>>> {
        vec![vec![
            Some("internal".into()),
            Some("db".into()),
            Some("a0b1".into()),
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
            Some("10".into()),
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
            Some("0".into()),
            Some("10".into()),
            Some("1".into()), // population max for error-proportion sorting
        ]]
    }

    fn audit_concurrency_rows(scope: &AuditScope) -> Vec<Vec<Option<String>>> {
        let start = i128::from(scope.window.start_seconds) * 1_000;
        vec![
            vec![
                Some("0".into()),
                None,
                None,
                Some("2".into()),
                Some("1".into()),
                Some("1".into()),
                Some("1".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("00:00:00".into()),
                Some("00:15:00".into()),
                Some("00:30:00".into()),
                Some("00:45:00".into()),
                Some("01:00:00".into()),
                None,
            ],
            vec![
                Some("1".into()),
                Some((start + 1_000).to_string()),
                Some("-1".into()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("00:00:01".into()),
            ],
        ]
    }

    fn audit_active_at_rows(at_milliseconds: i128) -> Vec<Vec<Option<String>>> {
        let start = at_milliseconds - 1_000;
        vec![
            vec![
                Some("0".into()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("1".into()),
            ],
            vec![
                Some("1".into()),
                Some("22222222-2222-2222-2222-222222222222".into()),
                Some("2026-09-02 00:00:00".into()),
                Some((start / 1_000).to_string()),
                Some("EOF".into()),
                Some("2000".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("internal".into()),
                Some("db".into()),
                Some("default_wg".into()),
                Some("a0b1".into()),
                Some("".into()),
                Some("1".into()),
                Some("0".into()),
                Some("1".into()),
                None,
            ],
        ]
    }

    fn family_detail_fixture_rows(
        target: &AuditFamilyTarget,
        candidates: &[String],
    ) -> Vec<Vec<Option<String>>> {
        let id = "11111111-1111-1111-1111-111111111111";
        let mut summary = vec![Some("summary".into())];
        summary.extend(
            [
                "1", "1", "0", "0", "0", "1", "0", "0", "0", "0", "0", "0", "0",
            ]
            .map(|v| Some(v.into())),
        );
        summary.extend(vec![None; 18]);
        let mut rows = vec![summary];
        for role in ["recent", "slowest", "profile_known"] {
            if role == "profile_known" && !candidates.iter().any(|candidate| candidate == id) {
                continue;
            }
            let mut row = vec![Some(role.into())];
            row.extend(vec![None; 13]);
            row.extend([
                Some(id.into()),
                Some("2026-09-02 00:00:00".into()),
                Some("1788278400".into()),
                Some("EOF".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some(target.catalog.clone()),
                Some(target.database.clone()),
                None,
                Some(target.native_digest.clone()),
                Some("".into()),
                Some("1".into()),
                Some("0".into()),
                Some("1".into()),
            ]);
            rows.push(row);
        }
        rows
    }

    struct FakeFactory {
        opened: Arc<AtomicUsize>,
        operations: Arc<Mutex<Vec<Operation>>>,
        closes: Arc<AtomicUsize>,
        version: String,
        audit: Result<(), RemoteError>,
        audit_search: Result<(), RemoteError>,
        audit_schema_rows: Vec<Vec<Option<String>>>,
        audit_family_rows: Vec<Vec<Option<String>>>,
        audit_family_detail_rows: Option<Vec<Vec<Option<String>>>>,
        profile_list: Result<(), RemoteError>,
        profile_list_rows: Option<Vec<Vec<String>>>,
        advisor: Result<(), RemoteError>,
        profile: String,
        official_profile: Result<Vec<String>, RemoteError>,
    }
    impl FakeFactory {
        fn supported() -> Self {
            Self {
                opened: Arc::new(AtomicUsize::new(0)),
                operations: Arc::new(Mutex::new(vec![])),
                closes: Arc::new(AtomicUsize::new(0)),
                version: SUPPORTED_VERSION.into(),
                audit: Ok(()),
                audit_search: Ok(()),
                audit_schema_rows: audit_schema_rows(),
                audit_family_rows: family_fixture_rows(),
                audit_family_detail_rows: None,
                profile_list: Ok(()),
                profile_list_rows: None,
                advisor: Ok(()),
                profile: "Profile: Level 1\nOperatorTotalTime: 500ms".into(),
                official_profile: Ok(vec![
                    "Summary\n    QueryId: fixture\n    OperatorTotalTime: 1ms".into(),
                ]),
            }
        }
    }
    impl RemoteFactory for FakeFactory {
        fn open(&self, options: RemoteConnectOptions) -> Result<Box<dyn Remote>, RemoteError> {
            assert_eq!(options.password.as_bytes(), b"write-only");
            self.opened.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(FakeRemote {
                operations: Arc::clone(&self.operations),
                closes: Arc::clone(&self.closes),
                version: self.version.clone(),
                audit: self.audit,
                audit_search: self.audit_search,
                audit_schema_rows: self.audit_schema_rows.clone(),
                audit_family_rows: self.audit_family_rows.clone(),
                audit_family_detail_rows: self.audit_family_detail_rows.clone(),
                profile_list: self.profile_list,
                profile_list_rows: self.profile_list_rows.clone(),
                advisor: self.advisor,
                profile: self.profile.clone(),
                official_profile: self.official_profile.clone(),
            }))
        }
    }

    #[derive(Default)]
    struct OpenGate {
        opened: usize,
        released: bool,
    }

    struct BlockingFactory {
        gate: Arc<(Mutex<OpenGate>, Condvar)>,
    }

    struct BlockingRemote;

    impl Remote for BlockingRemote {
        fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError> {
            match operation {
                RemoteOperation::CurrentVersion => {
                    Ok(RemoteResponse::Version(SUPPORTED_VERSION.into()))
                }
                RemoteOperation::SessionTimeZone => Ok(RemoteResponse::TimeZone(SessionTimeZone {
                    session_time_zone: "Etc/UTC".into(),
                    system_time_zone: "Etc/UTC".into(),
                    effective_time_zone: "Etc/UTC".into(),
                    current_utc_offset_seconds: 0,
                })),
                RemoteOperation::AuditProbe { .. }
                | RemoteOperation::ShowProfileList { .. }
                | RemoteOperation::ShowPlanAdvisor => Ok(RemoteResponse::Rows(vec![])),
                RemoteOperation::AuditSchemaProbe { .. } => {
                    Ok(RemoteResponse::NullableRows(audit_schema_rows()))
                }
                RemoteOperation::AuditSearch { .. }
                | RemoteOperation::AuditSearchHiddenCounts { .. }
                | RemoteOperation::AuditOccurrences { .. }
                | RemoteOperation::AuditFamilies { .. }
                | RemoteOperation::AuditFamilySamples { .. }
                | RemoteOperation::AuditOccurrenceSamples { .. }
                | RemoteOperation::AuditFamilyDetail { .. }
                | RemoteOperation::AuditFamilyDetailSqlLookup { .. }
                | RemoteOperation::AuditCoverage { .. }
                | RemoteOperation::AuditFingerprintEvents { .. }
                | RemoteOperation::AuditCollectionSlice { .. }
                | RemoteOperation::AuditFingerprintSamples { .. }
                | RemoteOperation::AuditConcurrency { .. }
                | RemoteOperation::AuditPerformance { .. }
                | RemoteOperation::AuditOverviewFamilyTop { .. }
                | RemoteOperation::AuditSqlLookup { .. }
                | RemoteOperation::AuditActiveAt { .. }
                | RemoteOperation::AuditActiveAtSqlLookup { .. }
                | RemoteOperation::GetQueryProfile { .. }
                | RemoteOperation::AnalyzeProfile { .. }
                | RemoteOperation::Explain { .. }
                | RemoteOperation::ProfiledSelect { .. } => Err(RemoteError::Unsupported),
            }
        }
    }

    impl RemoteFactory for BlockingFactory {
        fn open(&self, _: RemoteConnectOptions) -> Result<Box<dyn Remote>, RemoteError> {
            let (lock, wake) = &*self.gate;
            let mut gate = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            gate.opened += 1;
            wake.notify_all();
            while !gate.released {
                gate = wake.wait(gate).unwrap_or_else(|poison| poison.into_inner());
            }
            Ok(Box::new(BlockingRemote))
        }
    }

    fn request() -> CreateConnectionRequest {
        CreateConnectionRequest {
            host: "starrocks.example.test".into(),
            port: 9030,
            username: "reader".into(),
            password: "write-only".into(),
            default_catalog: None,
            default_database: None,
            audit_table: None,
            tls_mode: TlsMode::VerifyFull,
        }
    }
    fn session_manager(factory: Arc<FakeFactory>, clock: Arc<TestClock>) -> SessionManager {
        SessionManager::with_seams(factory, clock, Duration::from_secs(10))
    }

    fn workload_scope() -> AuditScope {
        AuditScope {
            window: AuditWindow {
                start_seconds: 1,
                end_seconds: 86_401,
            },
            state: None,
            user: None,
            native_digest: None,
            catalog: Some("internal".into()),
            database: Some("db".into()),
            resource_group: None,
            statement_filter: AuditStatementFilter::All,
        }
    }

    fn workload_occurrences(scope: AuditScope) -> AuditOccurrenceRequest {
        AuditOccurrenceRequest {
            scope,
            sort: AuditOccurrenceSort::QueryTimeDesc,
            limit: 10,
        }
    }

    fn discover_detail(manager: &SessionManager) -> doctor_contracts::AuditFamilyDetailReadRequest {
        let view = manager.connect(request()).unwrap();
        let mut scope = workload_scope();
        scope.window = AuditWindow {
            start_seconds: 1788278390,
            end_seconds: 1788282000,
        };
        scope.catalog = None;
        scope.database = None;
        let workload = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(scope.clone()))
            .unwrap();
        let family_key = manager.lock_sessions().active[&view.connection_id]
            .audit_discovery
            .as_ref()
            .unwrap()
            .families_by_key
            .keys()
            .next()
            .unwrap()
            .clone();
        doctor_contracts::AuditFamilyDetailReadRequest {
            connection_id: view.connection_id,
            discovery_generation: workload.discovery_generation,
            scope,
            family_key,
        }
    }

    #[test]
    fn family_detail_stale_bindings_reject_before_any_remote_read() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(factory.clone(), Arc::new(TestClock::default()));
        let request = discover_detail(&manager);
        factory.operations.lock().unwrap().clear();
        for kind in 0..3 {
            let mut stale = request.clone();
            match kind {
                0 => stale.discovery_generation += 1,
                1 => stale.scope.window.start_seconds += 1,
                _ => stale.family_key = format!("audit-family-v1:{}", "0".repeat(64)),
            }
            assert_eq!(
                manager
                    .prepare_audit_family_detail(&request.connection_id, stale)
                    .err()
                    .expect("stale")
                    .code,
                ConnectorErrorCode::QueryDiscoveryStale
            );
        }
        assert!(factory.operations.lock().unwrap().is_empty());
        assert_eq!(
            manager.lock_sessions().active[&request.connection_id].discovery_generation,
            request.discovery_generation
        );
        manager
            .audit_query_sql(
                &request.connection_id,
                AuditSqlLookupRequest {
                    scope: request.scope,
                    query_id: "22222222-2222-2222-2222-222222222222".into(),
                },
                request.discovery_generation,
            )
            .unwrap();
    }

    #[test]
    fn family_detail_prepare_deduplicates_and_cas_publishes_once_with_original_parent_sql() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(factory.clone(), Arc::new(TestClock::default()));
        let request = discover_detail(&manager);
        factory.operations.lock().unwrap().clear();
        let first = manager
            .prepare_audit_family_detail(&request.connection_id, request.clone())
            .unwrap();
        assert_eq!(
            *factory.operations.lock().unwrap(),
            [Operation::ProfileList, Operation::AuditFamilyDetail]
        );
        assert_eq!(first.workload().representative_indices.len(), 1);
        assert_eq!(
            first.workload().selected_profile_evidence,
            Some(doctor_contracts::AuditProfileEvidence::ProfileList)
        );
        let scope = first.workload().scope.clone();
        assert_eq!(scope.native_digest.as_deref(), Some("a0b1"));
        {
            let sessions = manager.lock_sessions();
            let session = &sessions.active[&request.connection_id];
            assert_eq!(session.discovery_generation, request.discovery_generation);
            assert!(
                session
                    .audit_discovery
                    .as_ref()
                    .unwrap()
                    .unique_query_ids
                    .contains("22222222-2222-2222-2222-222222222222")
            );
        }
        let racing = manager
            .prepare_audit_family_detail(&request.connection_id, request.clone())
            .unwrap();
        factory.operations.lock().unwrap().clear();
        let generation = manager.commit_audit_family_detail(first).unwrap();
        assert_eq!(generation, request.discovery_generation + 1);
        assert_eq!(
            manager.commit_audit_family_detail(racing).unwrap_err().code,
            ConnectorErrorCode::QueryDiscoveryStale
        );
        assert!(factory.operations.lock().unwrap().is_empty());
        assert_eq!(
            manager.lock_sessions().active[&request.connection_id]
                .audit_discovery
                .as_ref()
                .unwrap()
                .unique_query_ids
                .len(),
            1
        );
        let id = "11111111-1111-1111-1111-111111111111";
        let sql = manager
            .audit_query_sql(
                &request.connection_id,
                AuditSqlLookupRequest {
                    scope: scope.clone(),
                    query_id: id.into(),
                },
                generation,
            )
            .unwrap();
        assert_eq!(sql.sql, "SELECT retained FROM t");
        assert_eq!(
            *factory.operations.lock().unwrap(),
            [Operation::AuditFamilyDetailSqlLookup]
        );
        let fetched = manager
            .fetch_profile(&request.connection_id, id, generation)
            .unwrap();
        assert_eq!(fetched.query_context.row_index, 0);
        let next = manager
            .prepare_audit_family_detail(
                &request.connection_id,
                doctor_contracts::AuditFamilyDetailReadRequest {
                    scope,
                    discovery_generation: generation,
                    ..request.clone()
                },
            )
            .unwrap();
        assert_eq!(
            next.workload().selected_profile_evidence,
            Some(doctor_contracts::AuditProfileEvidence::SuccessfulFetchInSession)
        );
        // Repeat drilldown keeps the original broad parent, not the narrowed response scope.
        manager.commit_audit_family_detail(next).unwrap();
    }

    #[test]
    fn family_detail_profile_evidence_cache_is_bounded_and_empty_fetch_is_not_evidence() {
        let mut fixture = FakeFactory::supported();
        fixture.profile = String::new();
        let factory = Arc::new(fixture);
        let manager = session_manager(factory, Arc::new(TestClock::default()));
        let request = discover_detail(&manager);
        assert!(
            manager
                .fetch_profile(
                    &request.connection_id,
                    "22222222-2222-2222-2222-222222222222",
                    request.discovery_generation
                )
                .is_err()
        );
        let detail = manager
            .prepare_audit_family_detail(&request.connection_id, request.clone())
            .unwrap();
        assert_eq!(
            detail
                .workload()
                .profile_evidence_coverage
                .successful_cache_retained,
            0
        );
        {
            let mut sessions = manager.lock_sessions();
            let cache = &mut sessions
                .active
                .get_mut(&request.connection_id)
                .unwrap()
                .successful_profiles;
            for i in 0..501 {
                cache.record(format!("00000000-0000-0000-0000-{i:012x}"));
            }
            cache.record("00000000-0000-0000-0000-000000000001".into());
        }
        let detail = manager
            .prepare_audit_family_detail(&request.connection_id, request.clone())
            .unwrap();
        let coverage = &detail.workload().profile_evidence_coverage;
        assert_eq!(coverage.successful_cache_retained, 500);
        assert!(coverage.successful_cache_evicted);
        assert_eq!(coverage.candidate_count, 500);
        assert!(coverage.candidate_truncated); // ProfileList Q1 does not displace successful evidence.
        assert!(detail.workload().source.profile_known.is_none());
        manager.commit_audit_family_detail(detail).unwrap();
    }

    #[test]
    fn family_detail_malformed_prepare_discards_fatal_session_without_publishing() {
        let mut fixture = FakeFactory::supported();
        fixture.audit_family_detail_rows = Some(vec![vec![Some("bad".into())]]);
        let factory = Arc::new(fixture);
        let manager = session_manager(factory.clone(), Arc::new(TestClock::default()));
        let request = discover_detail(&manager);
        assert_eq!(
            manager
                .prepare_audit_family_detail(&request.connection_id, request.clone())
                .err()
                .expect("fatal decode")
                .code,
            ConnectorErrorCode::ConnectionFailed
        );
        assert!(
            !manager
                .lock_sessions()
                .active
                .contains_key(&request.connection_id)
        );
        assert_eq!(
            manager
                .audit_query_sql(
                    &request.connection_id,
                    AuditSqlLookupRequest {
                        scope: request.scope,
                        query_id: "22222222-2222-2222-2222-222222222222".into()
                    },
                    request.discovery_generation
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn connects_with_ordered_probe_and_opaque_short_lived_session() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        assert_eq!(view.state, ConnectionState::Connected);
        assert_eq!(view.detected_version, SUPPORTED_VERSION);
        assert_eq!(
            view.time_zone
                .as_ref()
                .map(|value| value.effective_time_zone.as_str()),
            Some("Etc/UTC")
        );
        assert_eq!(view.capabilities.time_zone, CapabilityState::Available);
        assert_eq!(view.capabilities.audit_loader, CapabilityState::Available);
        assert_eq!(
            view.capabilities.audit_user_filter,
            CapabilityState::Unavailable
        );
        assert!(!format!("{view:?}").contains("write-only"));
        assert_eq!(
            *factory.operations.lock().unwrap(),
            vec![
                Operation::Version,
                Operation::TimeZone,
                Operation::AuditSchema,
                Operation::ProfileList,
                Operation::Advisor
            ]
        );
        assert_eq!(view.connection_id.len(), 48);
    }

    #[test]
    fn rejects_unsafe_connection_inputs_before_remote_open() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let mut unsafe_host = request();
        unsafe_host.host = "mysql://attacker.invalid/file".into();
        assert_eq!(
            manager.connect(unsafe_host).unwrap_err().code,
            ConnectorErrorCode::InvalidRequest
        );
        let mut plaintext_remote = request();
        plaintext_remote.tls_mode = TlsMode::DisabledLoopbackDocker;
        assert_eq!(
            manager.connect(plaintext_remote).unwrap_err().code,
            ConnectorErrorCode::TlsRequired
        );
        let mut catalog = request();
        catalog.default_catalog = Some("internal".into());
        let error = manager.connect(catalog).unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::InvalidRequest);
        assert_eq!(error.limitation, Some("default_catalog_not_supported_v0_1"));
        assert_eq!(factory.opened.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn loopback_docker_mode_stays_loopback_and_plaintext_mode_is_explicit() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        for host in ["127.0.0.1", "::1"] {
            let mut loopback = request();
            loopback.host = host.into();
            loopback.tls_mode = TlsMode::DisabledLoopbackDocker;
            assert_eq!(
                manager.connect(loopback).unwrap().tls_mode,
                TlsMode::DisabledLoopbackDocker
            );
        }
        for host in ["127.0.0.2", "localhost", "starrocks.example.test"] {
            let mut rejected = request();
            rejected.host = host.into();
            rejected.tls_mode = TlsMode::DisabledLoopbackDocker;
            assert_eq!(
                manager.connect(rejected).unwrap_err().code,
                ConnectorErrorCode::TlsRequired,
                "host={host}"
            );
        }
        // Rejected loopback-mode attempts never opened a connection.
        assert_eq!(factory.opened.load(Ordering::Relaxed), 2);
        // The explicit plaintext choice reaches any host, like a desktop SQL client.
        for host in ["10.0.0.5", "starrocks.example.test", "127.0.0.1"] {
            // A fresh manager each time: the session limit is not under test here.
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let mut plaintext = request();
            plaintext.host = host.into();
            plaintext.tls_mode = TlsMode::Plaintext;
            assert_eq!(
                manager.connect(plaintext).unwrap().tls_mode,
                TlsMode::Plaintext,
                "host={host}"
            );
        }
        assert_eq!(factory.opened.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn unsupported_version_never_runs_version_sensitive_probes() {
        let mut fake = FakeFactory::supported();
        // 4.2 is outside the audited lines (3.1–4.1).
        fake.version = "4.2.0".into();
        let factory = Arc::new(fake);
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        assert_eq!(view.state, ConnectionState::Unsupported);
        assert_eq!(
            *factory.operations.lock().unwrap(),
            vec![Operation::Version]
        );
        assert_eq!(
            manager
                .search(&view.connection_id, search(QuerySource::Auto))
                .unwrap_err()
                .code,
            ConnectorErrorCode::UnsupportedVersion
        );
    }

    #[test]
    fn releases_from_3_1_connect_and_earlier_lines_are_unsupported() {
        for (version, expected) in [
            ("3.1.17-67ae3b7", ConnectionState::Connected),
            ("3.4.10-965be75", ConnectionState::Connected),
            ("4.1.7-b81ff9c", ConnectionState::Connected),
            ("3.0.9-485df1a", ConnectionState::Unsupported),
            ("2.5.21-dc2bcdb", ConnectionState::Unsupported),
        ] {
            let mut fake = FakeFactory::supported();
            fake.version = version.into();
            let factory = Arc::new(fake);
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let view = manager.connect(request()).unwrap();
            assert_eq!(view.state, expected, "{version}");
        }
    }

    #[test]
    fn fatal_probe_failures_release_the_reserved_slot_without_creating_a_session() {
        for (remote_error, connector_code) in [
            (RemoteError::Tls, ConnectorErrorCode::TlsFailed),
            (
                RemoteError::Authentication,
                ConnectorErrorCode::AuthenticationFailed,
            ),
            (RemoteError::Timeout, ConnectorErrorCode::RemoteTimeout),
            (RemoteError::Protocol, ConnectorErrorCode::ConnectionFailed),
        ] {
            let mut fake = FakeFactory::supported();
            fake.audit = Err(remote_error);
            let factory = Arc::new(fake);
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            assert_eq!(manager.connect(request()).unwrap_err().code, connector_code);
            let sessions = manager.lock_sessions();
            assert!(sessions.active.is_empty());
            assert_eq!(sessions.pending_connects, 0);
            drop(sessions);
            assert_eq!(factory.opened.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn missing_optional_audit_object_keeps_the_session_and_later_probes() {
        let mut fake = FakeFactory::supported();
        fake.audit = Err(RemoteError::Unsupported);
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));

        let view = manager.connect(request()).unwrap();
        assert_eq!(view.capabilities.audit_loader, CapabilityState::Unavailable);
        assert_eq!(
            view.capabilities.audit_user_filter,
            CapabilityState::Unavailable
        );
        assert_eq!(view.capabilities.profile_list, CapabilityState::Available);
        assert_eq!(view.capabilities.plan_advisor, CapabilityState::Available);
        assert_eq!(
            *factory.operations.lock().unwrap(),
            vec![
                Operation::Version,
                Operation::TimeZone,
                Operation::AuditSchema,
                Operation::ProfileList,
                Operation::Advisor,
            ]
        );
        assert!(
            manager
                .sessions
                .lock()
                .unwrap()
                .active
                .contains_key(&view.connection_id)
        );
    }

    #[test]
    fn fixed_browse_paths_fallback_only_for_auto_and_keep_audit_explicit() {
        let mut fake = FakeFactory::supported();
        fake.audit = Err(RemoteError::PermissionDenied);
        let factory = Arc::new(fake);
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        let result = manager
            .search(&view.connection_id, search(QuerySource::Auto))
            .unwrap();
        assert_eq!(result.source, QuerySource::ProfileList);
        assert_eq!(
            result.degraded_reason.as_deref(),
            Some("audit_loader_unavailable")
        );
        assert_eq!(
            manager
                .search(&view.connection_id, search(QuerySource::AuditLoader))
                .unwrap_err()
                .code,
            ConnectorErrorCode::CapabilityUnavailable
        );
        assert!(
            !factory
                .operations
                .lock()
                .unwrap()
                .contains(&Operation::AuditSearch)
        );
    }

    fn audit_schema_rows_with_statement_class() -> Vec<Vec<Option<String>>> {
        let mut rows = audit_schema_rows();
        rows.push(vec![Some("isQuery".into()), Some("TINYINT".into())]);
        rows
    }

    fn excluding(source: QuerySource) -> QuerySearch {
        QuerySearch {
            statement_filter: AuditStatementFilter::ExcludeDoctorAndNonQuery,
            ..search(source)
        }
    }

    #[test]
    fn audit_discovery_statement_filter_reports_applied_not_applied_or_indistinguishable() {
        let mut fake = FakeFactory::supported();
        fake.audit_schema_rows = audit_schema_rows_with_statement_class();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();

        let before = factory.operations.lock().unwrap().len();
        let applied = manager
            .search(&view.connection_id, excluding(QuerySource::Auto))
            .unwrap();
        assert_eq!(applied.source, QuerySource::AuditLoader);
        assert_eq!(
            applied.statement_filter,
            QueryListStatementFilterCoverage::Applied {
                hidden_non_query_row_count: "5".into(),
                hidden_doctor_row_count: "2".into(),
            }
        );
        assert_eq!(applied.rows.len(), 1);
        assert_eq!(
            factory.operations.lock().unwrap()[before..],
            [Operation::AuditSearch, Operation::AuditSearchHiddenCounts]
        );

        // `all` sends the unfiltered template and no count statement.
        let before = factory.operations.lock().unwrap().len();
        let all = manager
            .search(&view.connection_id, search(QuerySource::Auto))
            .unwrap();
        assert_eq!(
            all.statement_filter,
            QueryListStatementFilterCoverage::NotApplied
        );
        assert_eq!(all.rows, applied.rows);
        assert_eq!(
            factory.operations.lock().unwrap()[before..],
            [Operation::AuditSearch]
        );

        // Without a proven `isQuery` column nothing is hidden or guessed.
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        assert_eq!(
            view.capabilities.audit_statement_filter,
            CapabilityState::Unavailable
        );
        let before = factory.operations.lock().unwrap().len();
        let unproven = manager
            .search(&view.connection_id, excluding(QuerySource::Auto))
            .unwrap();
        assert_eq!(
            unproven.statement_filter,
            QueryListStatementFilterCoverage::Indistinguishable {
                reason: "audit_statement_filter_unavailable".into()
            }
        );
        assert_eq!(unproven.rows.len(), 1);
        assert_eq!(
            factory.operations.lock().unwrap()[before..],
            [Operation::AuditSearch]
        );
    }

    #[test]
    fn audit_hidden_counts_must_be_one_row_of_two_exact_decimals() {
        assert_eq!(
            decode_audit_search_hidden_counts(vec![vec!["0".into(), "12".into()]]).unwrap(),
            QueryListStatementFilterCoverage::Applied {
                hidden_non_query_row_count: "0".into(),
                hidden_doctor_row_count: "12".into(),
            }
        );
        for rows in [
            vec![],
            vec![vec!["1".into()]],
            vec![vec!["1".into(), "".into()]],
            vec![vec!["1".into(), "-1".into()]],
            vec![vec!["1".into(), "2".into()], vec!["1".into(), "2".into()]],
        ] {
            assert_eq!(
                decode_audit_search_hidden_counts(rows).unwrap_err().code,
                ConnectorErrorCode::ConnectionFailed
            );
        }
    }

    #[test]
    fn profile_list_discovery_hides_only_marked_statements_before_the_limit() {
        let row = |id: char, state: &str, statement: &str| -> Vec<String> {
            vec![
                format!("{}-1111-1111-1111-111111111111", id.to_string().repeat(8)),
                "2026-09-02 00:00:00".into(),
                "1s".into(),
                state.into(),
                statement.into(),
            ]
        };
        let mut fake = FakeFactory::supported();
        fake.audit = Err(RemoteError::Unsupported);
        fake.profile_list_rows = Some(vec![
            row('a', "Finished", "/* sr-doctor */ SELECT last_query_id()"),
            row('b', "Finished", "SELECT user_query FROM t"),
            row('c', "Error", "/* sr-doctor */ SELECT current_version()"),
            // Marker not at the start, or with different spacing: not ours.
            row('d', "Finished", "SELECT 1 /* sr-doctor */"),
            row('e', "Finished", "/*  sr-doctor */ SELECT 2"),
            row('f', "Finished", "INSERT INTO t SELECT 3"),
            // Statements earlier releases sent carry the legacy marker: also ours.
            row('g', "Finished", "/* starrocks-doctor */ SELECT 4"),
        ]);
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();

        let finished = manager
            .search(
                &view.connection_id,
                QuerySearch {
                    state: Some("finished".into()),
                    limit: 2,
                    ..excluding(QuerySource::Auto)
                },
            )
            .unwrap();
        assert_eq!(finished.source, QuerySource::ProfileList);
        // Hidden count is over the status-filtered rows read, not the limit.
        assert_eq!(
            finished.statement_filter,
            QueryListStatementFilterCoverage::DoctorOnlyApplied {
                hidden_doctor_row_count: "2".into(),
            }
        );
        let ids = |result: &QueryBrowseResult| {
            result
                .rows
                .iter()
                .map(|row| row[0].chars().next().unwrap())
                .collect::<String>()
        };
        assert_eq!(ids(&finished), "bd");

        let all = manager
            .search(&view.connection_id, search(QuerySource::Auto))
            .unwrap();
        assert_eq!(
            all.statement_filter,
            QueryListStatementFilterCoverage::NotApplied
        );
        assert_eq!(ids(&all), "abcdefg");
        let excluded = manager
            .search(&view.connection_id, excluding(QuerySource::Auto))
            .unwrap();
        assert_eq!(ids(&excluded), "bdef");
        assert_eq!(
            excluded.statement_filter,
            QueryListStatementFilterCoverage::DoctorOnlyApplied {
                hidden_doctor_row_count: "3".into(),
            }
        );
        assert!(
            !factory
                .operations
                .lock()
                .unwrap()
                .contains(&Operation::AuditSearchHiddenCounts)
        );
    }

    #[test]
    fn running_candidates_hide_only_marked_statements_before_the_local_limit() {
        let row = |id: char, state: &str, statement: &str| -> Vec<String> {
            vec![
                format!("{}-1111-1111-1111-111111111111", id.to_string().repeat(8)),
                "2026-09-02 00:00:00".into(),
                "1s".into(),
                state.into(),
                statement.into(),
            ]
        };
        let mut fake = FakeFactory::supported();
        fake.profile_list_rows = Some(vec![
            row('a', "Running", "/* sr-doctor */ SELECT COUNT(*) FROM t"),
            row('b', "Running", "SELECT user_query FROM t"),
            // Finished rows are never Running candidates, marked or not.
            row('c', "Finished", "/* sr-doctor */ SELECT 1"),
            row('d', "Running", "SELECT 1 /* sr-doctor */"),
            row('e', "Running", "/* sr-doctor */ SELECT last_query_id()"),
        ]);
        let manager = session_manager(Arc::new(fake), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let ids = |result: &RunningProfileCandidates| {
            result
                .rows
                .iter()
                .map(|row| row[0].chars().next().unwrap())
                .collect::<String>()
        };

        let excluded = manager
            .list_running_profile_candidates(
                &view.connection_id,
                1,
                AuditStatementFilter::ExcludeDoctorAndNonQuery,
            )
            .unwrap();
        // Hidden before the local limit: the one slot goes to a user query.
        assert_eq!(ids(&excluded), "b");
        assert!(excluded.local_truncated);
        assert_eq!(
            excluded.statement_filter,
            QueryListStatementFilterCoverage::DoctorOnlyApplied {
                hidden_doctor_row_count: "2".into(),
            }
        );

        let all = manager
            .list_running_profile_candidates(&view.connection_id, 25, AuditStatementFilter::All)
            .unwrap();
        assert_eq!(ids(&all), "abde");
        assert_eq!(
            all.statement_filter,
            QueryListStatementFilterCoverage::NotApplied
        );
    }

    #[test]
    fn audit_browse_has_a_fixed_allowlisted_template_and_bound_parameters() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        let result = manager
            .search(&view.connection_id, search(QuerySource::AuditLoader))
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].len(), 12);
        assert_eq!(result.plan_sql_references.len(), 1);
        assert!(!format!("{result:?}").contains("secret_canary"));
        assert_eq!(
            factory.operations.lock().unwrap().last(),
            Some(&Operation::AuditSearch)
        );
    }

    #[test]
    fn audit_workload_reuses_profile_fetch_and_revokes_prior_discovery() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let scope = workload_scope();
        let occurrences = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(scope.clone()))
            .unwrap();
        assert_eq!(occurrences.rows.len(), 1);
        assert_eq!(occurrences.coverage.retained_event_count, "1");
        assert_eq!(occurrences.coverage.metric_inputs.len(), 4);
        assert_eq!(
            occurrences.coverage.metric_inputs[0].aggregation,
            AuditMetricAggregation::Sum
        );
        assert_eq!(
            occurrences.coverage.metric_inputs[3].aggregation,
            AuditMetricAggregation::Max
        );
        assert_eq!(
            occurrences.session_identity.cluster_identity,
            AuditClusterIdentity::SessionScoped
        );
        assert_eq!(
            occurrences
                .session_identity
                .audit_timestamp_time_zone
                .as_ref()
                .map(|time_zone| time_zone.effective_time_zone.as_str()),
            Some("Etc/UTC")
        );

        let query_id = "22222222-2222-2222-2222-222222222222";
        let profile = manager
            .fetch_profile(
                &view.connection_id,
                query_id,
                occurrences.discovery_generation,
            )
            .unwrap();
        assert_eq!(profile.query_context.plan_sql_ref, None);
        assert_eq!(profile.query_context.row.len(), 12);
        assert_eq!(
            profile.query_context.row,
            vec![
                "22222222-2222-2222-2222-222222222222",
                "2026-09-02 00:00:00",
                "1788278400",
                "EOF",
                "10",
                "0",
                "0",
                "0",
                "0",
                "db",
                "default_wg",
                "a0b1",
            ]
        );

        let sql = manager
            .audit_query_sql(
                &view.connection_id,
                AuditSqlLookupRequest {
                    scope: scope.clone(),
                    query_id: query_id.into(),
                },
                occurrences.discovery_generation,
            )
            .unwrap();
        assert_eq!(sql.sql, "SELECT retained FROM t");
        assert_eq!(
            factory.operations.lock().unwrap().last(),
            Some(&Operation::AuditSqlLookup)
        );

        let families = manager
            .audit_families(
                &view.connection_id,
                AuditFamilyRequest {
                    scope,
                    sort: AuditFamilySort::CumulativeQueryTimeDesc,
                    limit: 10,
                },
            )
            .unwrap();
        assert!(families.discovery_generation > occurrences.discovery_generation);
        assert_eq!(families.rows.len(), 1);
        let stale_profile = match manager.fetch_profile(
            &view.connection_id,
            query_id,
            occurrences.discovery_generation,
        ) {
            Ok(_) => panic!("stale audit generation unexpectedly fetched a Profile"),
            Err(error) => error,
        };
        assert_eq!(stale_profile.code, ConnectorErrorCode::ProfileUnavailable);
        assert_eq!(
            manager
                .audit_query_sql(
                    &view.connection_id,
                    AuditSqlLookupRequest {
                        scope: workload_scope(),
                        query_id: query_id.into(),
                    },
                    occurrences.discovery_generation,
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::QueryDiscoveryStale
        );
    }

    #[test]
    fn fingerprint_events_read_bounded_statements_without_publishing_discovery() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let request = AuditFamilyRequest {
            scope: workload_scope(),
            sort: AuditFamilySort::ExecutionCountDesc,
            limit: 10,
        };
        let workload = manager
            .audit_fingerprint_events_controlled(&view.connection_id, request, None)
            .unwrap();
        assert_eq!(workload.eligible_count, "1");
        assert_eq!(workload.rows.len(), 1);
        assert_eq!(workload.rows[0].native_digest, None);
        assert_eq!(
            workload.rows[0].statement_sample,
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: "SELECT 3".into(),
                truncated: false
            })
        );
        let operations = factory.operations.lock().unwrap();
        assert_eq!(
            operations[operations.len() - 2..],
            [
                Operation::AuditFingerprintEvents,
                Operation::AuditFingerprintSamples
            ]
        );
        // Neither read grants a SQL or Profile lookup of these executions.
        drop(operations);
        let Err(error) = manager.fetch_profile(
            &view.connection_id,
            "00000000-0000-0000-0000-000000000001",
            1,
        ) else {
            panic!("a fingerprint read unexpectedly granted a Profile fetch");
        };
        assert_ne!(error.code, ConnectorErrorCode::InvalidRequest);
    }

    #[test]
    fn collection_slices_are_bounded_oldest_first_and_carry_statement_prefixes() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let slice = manager
            .audit_collection_slice_controlled(&view.connection_id, 100, 400, None)
            .unwrap();
        assert_eq!(slice.slice_count, "3");
        assert_eq!(slice.rows.len(), 2);
        assert_eq!(slice.rows[0].native_digest.as_deref(), Some("ab12"));
        assert_eq!(
            slice.rows[1].statement_sample,
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: "SELECT id FROM t WHERE id = 7".into(),
                truncated: false
            })
        );
        assert_eq!(
            factory.operations.lock().unwrap().last(),
            Some(&Operation::AuditCollectionSlice)
        );
        assert!(
            manager
                .audit_collection_slice_controlled(&view.connection_id, 400, 100, None)
                .is_err()
        );
    }

    #[test]
    fn similar_sources_read_long_family_samples_and_fingerprints_without_discovery() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let workload = manager
            .audit_similar_sources_controlled(
                &view.connection_id,
                AuditFamilyRequest {
                    scope: workload_scope(),
                    sort: AuditFamilySort::CumulativeQueryTimeDesc,
                    limit: 10,
                },
                None,
            )
            .unwrap();
        assert!(!workload.families.is_empty());
        assert!(workload.families.iter().all(|row| matches!(
            row.statement_sample,
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text { .. })
        )));
        assert_eq!(workload.fingerprint.rows.len(), 1);
        let operations = factory.operations.lock().unwrap();
        assert_eq!(
            operations[operations.len() - 5..],
            [
                Operation::AuditCoverage,
                Operation::AuditFamilies,
                Operation::AuditFamilySamples,
                Operation::AuditFingerprintEvents,
                Operation::AuditFingerprintSamples
            ]
        );
    }

    #[test]
    fn user_scope_is_capability_gated_before_remote_reads_and_bound_across_workload_views() {
        let mut fake = FakeFactory::supported();
        fake.audit_schema_rows = audit_schema_rows_with_user();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        assert_eq!(
            view.capabilities.audit_user_filter,
            CapabilityState::Available
        );

        let mut scope = workload_scope();
        scope.user = Some("reader".into());
        let occurrences = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(scope.clone()))
            .unwrap();
        assert_eq!(occurrences.scope, scope);
        let sql = manager
            .audit_query_sql(
                &view.connection_id,
                AuditSqlLookupRequest {
                    scope: scope.clone(),
                    query_id: "22222222-2222-2222-2222-222222222222".into(),
                },
                occurrences.discovery_generation,
            )
            .unwrap();
        assert_eq!(sql.sql, "SELECT retained FROM t");
        let mut mismatched_scope = scope.clone();
        mismatched_scope.user = Some("other-reader".into());
        let before_mismatch = factory.operations.lock().unwrap().len();
        let error = manager
            .audit_query_sql(
                &view.connection_id,
                AuditSqlLookupRequest {
                    scope: mismatched_scope,
                    query_id: "22222222-2222-2222-2222-222222222222".into(),
                },
                occurrences.discovery_generation,
            )
            .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::QuerySqlUnavailable);
        assert_eq!(error.limitation, Some("audit_scope_mismatch"));
        assert_eq!(factory.operations.lock().unwrap().len(), before_mismatch);

        let families = manager
            .audit_families(
                &view.connection_id,
                AuditFamilyRequest {
                    scope: scope.clone(),
                    sort: AuditFamilySort::CumulativeQueryTimeDesc,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(families.scope, scope);
        let operations = factory.operations.lock().unwrap();
        assert!(operations.contains(&Operation::AuditCoverage));
        assert!(operations.contains(&Operation::AuditOccurrences));
        assert!(operations.contains(&Operation::AuditFamilies));
        assert!(operations.contains(&Operation::AuditSqlLookup));
    }

    #[test]
    fn occurrence_rows_carry_their_own_bounded_statement_sample() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let occurrences = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(workload_scope()))
            .unwrap();
        assert_eq!(
            occurrences.rows[0].statement_sample,
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: "SELECT 2".into(),
                truncated: false,
            })
        );
        let operations = factory.operations.lock().unwrap().clone();
        let read = operations
            .iter()
            .position(|operation| *operation == Operation::AuditOccurrences)
            .unwrap();
        assert_eq!(operations[read + 1], Operation::AuditOccurrenceSamples);

        let mut fake = FakeFactory::supported();
        fake.audit_schema_rows = audit_schema_rows()
            .into_iter()
            .filter(|row| row[0].as_deref() != Some("stmt"))
            .collect();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let occurrences = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(workload_scope()))
            .unwrap();
        assert_eq!(
            occurrences.rows[0].statement_sample,
            AuditSourceStatementSample::SourceColumnAbsent
        );
        assert!(
            !factory
                .operations
                .lock()
                .unwrap()
                .contains(&Operation::AuditOccurrenceSamples)
        );
    }

    #[test]
    fn user_scope_with_missing_column_fails_before_any_audit_read() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        assert_eq!(
            view.capabilities.audit_user_filter,
            CapabilityState::Unavailable
        );
        let mut scope = workload_scope();
        scope.user = Some("reader".into());
        let before = factory.operations.lock().unwrap().len();
        let error = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(scope))
            .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::CapabilityUnavailable);
        assert_eq!(error.limitation, Some("audit_user_filter_unavailable"));
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }
    #[test]
    fn coverage_decoder_keeps_large_decimal_and_nullable_aggregate_input() {
        let mut rows = audit_coverage_rows();
        rows[0][0] = Some("184467440737095516160".into());
        rows[0][12] = None; // query-time aggregate input after its four counts
        let coverage = decode_audit_coverage(rows, AuditStatementFilter::All).unwrap();
        assert_eq!(coverage.raw_row_count, "184467440737095516160");
        assert_eq!(
            coverage.statement_filter,
            AuditStatementFilterCoverage::NotApplied
        );
        assert_eq!(
            coverage.metric_inputs[0].aggregate_input,
            AuditMetricInput::SqlNull
        );
        assert_eq!(
            coverage.metric_inputs[0].aggregation,
            AuditMetricAggregation::Sum
        );
    }

    #[test]
    fn coverage_decoder_rejects_non_decimal_or_negative_count_cells() {
        for invalid_count in ["", "-1", "+1", " 1", "1.0", "not-a-count"] {
            let mut rows = audit_coverage_rows();
            rows[0][7] = Some(invalid_count.into()); // retained_family_count
            assert!(
                decode_audit_coverage(rows, AuditStatementFilter::All).is_err(),
                "{invalid_count:?}"
            );
        }
        let mut rows = audit_coverage_rows();
        rows[0][20] = Some("-1".into()); // cpu nonnegative_count
        assert!(decode_audit_coverage(rows, AuditStatementFilter::All).is_err());
    }

    #[test]
    fn coverage_decoder_keeps_statement_filter_hidden_counts_distinct_from_not_applied() {
        let mut rows = audit_coverage_rows();
        rows[0][29] = Some("12".into());
        rows[0][30] = Some("0".into());
        assert_eq!(
            decode_audit_coverage(rows.clone(), AuditStatementFilter::ExcludeDoctorAndNonQuery)
                .unwrap()
                .statement_filter,
            AuditStatementFilterCoverage::Applied {
                hidden_non_query_row_count: "12".into(),
                hidden_doctor_row_count: "0".into(),
            }
        );
        // Counts returned for `all`, or missing counts for an applied filter,
        // are malformed: never silently "not applied" and never zero.
        assert!(decode_audit_coverage(rows, AuditStatementFilter::All).is_err());
        assert!(
            decode_audit_coverage(
                audit_coverage_rows(),
                AuditStatementFilter::ExcludeDoctorAndNonQuery
            )
            .is_err()
        );
        let mut negative = audit_coverage_rows();
        negative[0][29] = Some("-1".into());
        negative[0][30] = Some("0".into());
        assert!(
            decode_audit_coverage(negative, AuditStatementFilter::ExcludeDoctorAndNonQuery)
                .is_err()
        );
    }

    #[test]
    fn audit_schema_optional_statement_never_unblocks_sql_or_missing_required_columns() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let without_statement = AuditSchema::from_schema_probe_rows(
            &audit_schema_rows()
                .into_iter()
                .filter(|row| row.first().and_then(|value| value.as_deref()) != Some("stmt"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(audit_schema_supports_workload(&without_statement));
        manager
            .lock_sessions()
            .active
            .get_mut(&view.connection_id)
            .unwrap()
            .audit_schema = Some(without_statement);
        let scope = workload_scope();
        let occurrences = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(scope.clone()))
            .unwrap();
        let before_sql_lookup = factory.operations.lock().unwrap().len();
        assert_eq!(
            manager
                .audit_query_sql(
                    &view.connection_id,
                    AuditSqlLookupRequest {
                        scope,
                        query_id: "22222222-2222-2222-2222-222222222222".into(),
                    },
                    occurrences.discovery_generation,
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::CapabilityUnavailable
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before_sql_lookup);

        let missing_required = AuditSchema::new([
            AuditColumn::QueryId,
            AuditColumn::Timestamp,
            AuditColumn::State,
        ]);
        assert!(!audit_schema_supports_workload(&missing_required));
        manager
            .lock_sessions()
            .active
            .get_mut(&view.connection_id)
            .unwrap()
            .audit_schema = Some(missing_required);
        let before_required_column = factory.operations.lock().unwrap().len();
        assert_eq!(
            manager
                .audit_occurrences(&view.connection_id, workload_occurrences(workload_scope()))
                .unwrap_err()
                .code,
            ConnectorErrorCode::CapabilityUnavailable
        );
        assert_eq!(
            factory.operations.lock().unwrap().len(),
            before_required_column
        );
    }

    #[test]
    fn audit_numeric_source_type_mismatch_is_typed_and_issues_no_remote_read() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let mut rows = audit_schema_rows();
        rows.iter_mut()
            .find(|row| row.first().and_then(|value| value.as_deref()) == Some("scanBytes"))
            .unwrap()[1] = Some("VARCHAR".into());
        let schema = AuditSchema::from_schema_probe_rows(&rows).unwrap();
        assert!(audit_schema_supports_workload(&schema));
        manager
            .lock_sessions()
            .active
            .get_mut(&view.connection_id)
            .unwrap()
            .audit_schema = Some(schema);
        let before = factory.operations.lock().unwrap().len();
        let error = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(workload_scope()))
            .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::CapabilityUnavailable);
        assert_eq!(error.stage, "audit_schema");
        assert_eq!(
            error.limitation,
            Some("audit_schema_required_column_type_unsupported")
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn error_proportion_population_guard_distinguishes_invalid_cap_and_non_proportion_sorts() {
        let sort = AuditFamilySort::ErrorProportionDesc;
        assert!(guard_error_proportion_population(&[], &sort, "audit_families").is_ok());

        let mut exact = decode_family_source_row(family_fixture_rows().remove(0)).unwrap();
        exact.error_proportion_population_max =
            Some(format!("000{ERROR_PROPORTION_EXACT_POPULATION_MAX}"));
        assert!(
            guard_error_proportion_population(&[exact.clone()], &sort, "audit_families").is_ok()
        );

        let mut inconsistent = exact.clone();
        inconsistent.error_proportion_population_max = Some("2".into());
        let error = guard_error_proportion_population(
            &[exact.clone(), inconsistent],
            &sort,
            "audit_families",
        )
        .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::ConnectionFailed);
        assert_eq!(error.stage, "audit_response");

        for invalid in [None, Some(""), Some("-1"), Some("1.0"), Some(" 1")] {
            let mut row = exact.clone();
            row.error_proportion_population_max = invalid.map(str::to_owned);
            assert_eq!(
                guard_error_proportion_population(&[row], &sort, "audit_families")
                    .unwrap_err()
                    .code,
                ConnectorErrorCode::ConnectionFailed
            );
        }

        for oversized in [
            (ERROR_PROPORTION_EXACT_POPULATION_MAX + 1).to_string(),
            u64::MAX.to_string(),
            "9".repeat(200),
        ] {
            let mut row = exact.clone();
            row.error_proportion_population_max = Some(oversized);
            let error =
                guard_error_proportion_population(&[row], &sort, "audit_families").unwrap_err();
            assert_eq!(error.code, ConnectorErrorCode::CapabilityUnavailable);
            assert_eq!(error.stage, "audit_families");
            assert_eq!(
                error.limitation,
                Some("audit_family_error_proportion_population_limit_exceeded")
            );
        }

        let mut non_proportion = exact;
        non_proportion.error_proportion_population_max = Some("9".repeat(200));
        assert!(
            guard_error_proportion_population(
                &[non_proportion],
                &AuditFamilySort::ErrorCountDesc,
                "audit_families"
            )
            .is_ok()
        );
    }

    #[test]
    fn error_proportion_cap_does_not_publish_family_or_overview_generation() {
        let mut fake = FakeFactory::supported();
        fake.audit_family_rows[0][14] =
            Some((ERROR_PROPORTION_EXACT_POPULATION_MAX + 1).to_string());
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let prior = manager
            .audit_occurrences(&view.connection_id, workload_occurrences(workload_scope()))
            .unwrap();

        let family_error = manager
            .audit_families(
                &view.connection_id,
                AuditFamilyRequest {
                    scope: workload_scope(),
                    sort: AuditFamilySort::ErrorProportionDesc,
                    limit: 10,
                },
            )
            .unwrap_err();
        assert_eq!(family_error.code, ConnectorErrorCode::CapabilityUnavailable);
        assert_eq!(family_error.stage, "audit_families");
        assert_eq!(
            family_error.limitation,
            Some("audit_family_error_proportion_population_limit_exceeded")
        );

        let overview_error = manager
            .audit_overview(
                &view.connection_id,
                AuditOverviewRequest {
                    scope: workload_scope(),
                    primary: AuditOverviewPrimary::Families {
                        sort: AuditFamilySort::ErrorProportionDesc,
                        limit: 10,
                    },
                    family_top_sort: AuditOverviewFamilyTopSort::ExecutionCountDesc,
                },
            )
            .unwrap_err();
        assert_eq!(
            overview_error.code,
            ConnectorErrorCode::CapabilityUnavailable
        );
        assert_eq!(overview_error.stage, "audit_overview_primary");
        assert_eq!(
            overview_error.limitation,
            Some("audit_family_error_proportion_population_limit_exceeded")
        );

        assert_eq!(
            manager
                .audit_query_sql(
                    &view.connection_id,
                    AuditSqlLookupRequest {
                        scope: workload_scope(),
                        query_id: "22222222-2222-2222-2222-222222222222".into(),
                    },
                    prior.discovery_generation,
                )
                .unwrap()
                .discovery_generation,
            prior.discovery_generation
        );
    }

    #[test]
    fn query_generation_binds_profile_and_plan_references_and_refresh_revokes_both() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let first_session = manager.connect(request()).unwrap();
        let second_session = manager.connect(request()).unwrap();
        let first = manager
            .search(
                &first_session.connection_id,
                search(QuerySource::AuditLoader),
            )
            .unwrap();
        let query_id = "22222222-2222-2222-2222-222222222222";
        let reference = first.plan_sql_references.get(query_id).unwrap().clone();
        let before_operations = factory.operations.lock().unwrap().len();
        let explained = manager
            .explain_reference(
                &first_session.connection_id,
                query_id,
                first.discovery_generation,
                &reference,
                ExplainMode::Verbose,
            )
            .unwrap();
        assert_eq!(explained.mode, ExplainMode::Verbose);
        assert_eq!(
            &factory.operations.lock().unwrap()[before_operations..],
            &[Operation::ExplainVerbose]
        );

        let cross_session = manager
            .explain_reference(
                &second_session.connection_id,
                query_id,
                first.discovery_generation,
                &reference,
                ExplainMode::Logical,
            )
            .unwrap_err();
        assert_eq!(
            cross_session.code,
            ConnectorErrorCode::CapabilityUnavailable
        );
        assert_eq!(
            factory.operations.lock().unwrap().len(),
            before_operations + 1
        );

        let refreshed = manager
            .search(
                &first_session.connection_id,
                search(QuerySource::AuditLoader),
            )
            .unwrap();
        assert_ne!(refreshed.discovery_generation, first.discovery_generation);
        let before_stale_references = factory.operations.lock().unwrap().len();
        let old_ref = manager
            .explain_reference(
                &first_session.connection_id,
                query_id,
                first.discovery_generation,
                &reference,
                ExplainMode::Costs,
            )
            .unwrap_err();
        assert_eq!(old_ref.code, ConnectorErrorCode::CapabilityUnavailable);
        let stale_profile = match manager.fetch_profile(
            &first_session.connection_id,
            query_id,
            first.discovery_generation,
        ) {
            Ok(_) => panic!("stale generation unexpectedly fetched a Profile"),
            Err(error) => error,
        };
        assert_eq!(stale_profile.code, ConnectorErrorCode::ProfileUnavailable);
        assert_eq!(
            factory.operations.lock().unwrap().len(),
            before_stale_references
        );
    }

    #[test]
    fn profile_list_digest_filter_is_rejected_instead_of_silently_ignored() {
        let mut fake = FakeFactory::supported();
        fake.audit = Err(RemoteError::Unsupported);
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        let error = manager
            .search(
                &view.connection_id,
                QuerySearch {
                    digest: Some("a0b1".into()),
                    ..search(QuerySource::ProfileList)
                },
            )
            .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::CapabilityUnavailable);
        assert_eq!(
            error.limitation,
            Some("profile_list_digest_filter_unavailable")
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn duplicate_query_ids_are_fail_closed_for_profile_and_plan_actions() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let query_id = "aaaaaaaa-2222-2222-2222-222222222222";
        let row = vec![
            query_id.into(),
            "2026-09-02 00:00:00".into(),
            "1788278400".into(),
            "EOF".into(),
            "10".into(),
            "0".into(),
            "0".into(),
            "0".into(),
            "0".into(),
            "db".into(),
            "default_wg".into(),
            "a0b1".into(),
            "SELECT secret_canary FROM t".into(),
        ];
        let malformed_case_variant = vec![query_id.to_ascii_uppercase()];
        let retained = {
            let mut sessions = manager.lock_sessions();
            let session = sessions.active.get_mut(&view.connection_id).unwrap();
            retain_plan_sql(
                session,
                vec![row, malformed_case_variant],
                13,
                QuerySource::AuditLoader,
            )
            .unwrap()
        };
        assert!(retained.ambiguous_query_ids.contains(query_id));
        assert!(retained.plan_sql_references.is_empty());
        let before = factory.operations.lock().unwrap().len();
        let error = match manager.fetch_profile(
            &view.connection_id,
            query_id,
            retained.discovery_generation,
        ) {
            Ok(_) => panic!("duplicate query ID unexpectedly fetched a Profile"),
            Err(error) => error,
        };
        assert_eq!(error.code, ConnectorErrorCode::ProfileUnavailable);
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn query_sql_is_current_session_bounded_and_never_executes_the_statement() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let query_id = "aaaaaaaa-3333-3333-3333-333333333333";
        let row = |statement: String| {
            vec![
                query_id.into(),
                "2026-09-02 00:00:00".into(),
                "1788278400".into(),
                "EOF".into(),
                "10".into(),
                "0".into(),
                "0".into(),
                "0".into(),
                "0".into(),
                "db".into(),
                "default_wg".into(),
                "a0b1".into(),
                statement,
            ]
        };
        let first = {
            let mut sessions = manager.lock_sessions();
            retain_plan_sql(
                sessions.active.get_mut(&view.connection_id).unwrap(),
                vec![row("  INSERT INTO t VALUES (1)  ".into())],
                13,
                QuerySource::AuditLoader,
            )
            .unwrap()
        };
        let before = factory.operations.lock().unwrap().len();
        let retained = manager
            .query_sql(&view.connection_id, query_id, first.discovery_generation)
            .unwrap();
        assert_eq!(retained.sql, "INSERT INTO t VALUES (1)");
        assert_eq!(retained.discovery_generation, first.discovery_generation);
        assert_eq!(factory.operations.lock().unwrap().len(), before);

        let current = {
            let mut sessions = manager.lock_sessions();
            retain_plan_sql(
                sessions.active.get_mut(&view.connection_id).unwrap(),
                vec![row("".into())],
                13,
                QuerySource::AuditLoader,
            )
            .unwrap()
        };
        let stale = manager
            .query_sql(&view.connection_id, query_id, first.discovery_generation)
            .unwrap_err();
        assert_eq!(stale.code, ConnectorErrorCode::QueryDiscoveryStale);
        let unavailable = manager
            .query_sql(&view.connection_id, query_id, current.discovery_generation)
            .unwrap_err();
        assert_eq!(unavailable.code, ConnectorErrorCode::QuerySqlUnavailable);

        let oversized = {
            let mut sessions = manager.lock_sessions();
            retain_plan_sql(
                sessions.active.get_mut(&view.connection_id).unwrap(),
                vec![row("x".repeat(MAX_PLAN_SQL_BYTES + 1))],
                13,
                QuerySource::AuditLoader,
            )
            .unwrap()
        };
        let unavailable = manager
            .query_sql(
                &view.connection_id,
                query_id,
                oversized.discovery_generation,
            )
            .unwrap_err();
        assert_eq!(unavailable.code, ConnectorErrorCode::QuerySqlUnavailable);
    }

    #[test]
    fn audit_template_uses_official_columns_and_closed_typed_parameters() {
        let parameters = QueryParameters {
            start_seconds: 1,
            end_seconds: 2,
            state: Some("finished".into()),
            digest: Some("A0b1".into()),
            limit: 7,
            statement_filter: AuditStatementFilter::All,
        };
        let statement = audit_statement(DEFAULT_AUDIT_TABLE, &parameters).unwrap();
        // `all` keeps the earlier discovery SQL byte for byte.
        assert_eq!(
            statement,
            "SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, UNIX_TIMESTAMP(`timestamp`) AS `event_epoch_seconds`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest`, `stmt` AS `statement` FROM `starrocks_audit_db__`.`starrocks_audit_tbl__` WHERE `timestamp` >= FROM_UNIXTIME(1) AND `timestamp` <= FROM_UNIXTIME(2) AND `state` = 'EOF' AND `digest` = 'A0b1' ORDER BY `timestamp` DESC LIMIT 7"
        );
        assert_eq!(
            audit_search_hidden_counts_statement(DEFAULT_AUDIT_TABLE, &parameters)
                .unwrap_err()
                .code,
            ConnectorErrorCode::InvalidRequest
        );
        let filtered = QueryParameters {
            statement_filter: AuditStatementFilter::ExcludeDoctorAndNonQuery,
            ..parameters.clone()
        };
        assert_eq!(
            audit_statement(DEFAULT_AUDIT_TABLE, &filtered).unwrap(),
            "SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, UNIX_TIMESTAMP(`timestamp`) AS `event_epoch_seconds`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest`, `stmt` AS `statement` FROM `starrocks_audit_db__`.`starrocks_audit_tbl__` WHERE `timestamp` >= FROM_UNIXTIME(1) AND `timestamp` <= FROM_UNIXTIME(2) AND `state` = 'EOF' AND `digest` = 'A0b1' AND `isQuery` = 1 AND (`stmt` IS NULL OR (`stmt` NOT LIKE '/* sr-doctor */ %' AND `stmt` NOT LIKE '/* starrocks-doctor */ %')) ORDER BY `timestamp` DESC LIMIT 7"
        );
        // Same window and field filters, no LIMIT, the two disjoint Audit
        // history buckets.
        assert_eq!(
            audit_search_hidden_counts_statement(DEFAULT_AUDIT_TABLE, &filtered).unwrap(),
            "SELECT COUNT(CASE WHEN (`isQuery` IS NULL OR `isQuery` <> 1) THEN 1 END) AS `hidden_non_query_row_count`, COUNT(CASE WHEN `isQuery` = 1 AND (`stmt` LIKE '/* sr-doctor */ %' OR `stmt` LIKE '/* starrocks-doctor */ %') THEN 1 END) AS `hidden_doctor_row_count` FROM `starrocks_audit_db__`.`starrocks_audit_tbl__` WHERE `timestamp` >= FROM_UNIXTIME(1) AND `timestamp` <= FROM_UNIXTIME(2) AND `state` = 'EOF' AND `digest` = 'A0b1'"
        );
        for column in [
            "`queryId`",
            "`timestamp`",
            "`queryTime`",
            "`scanBytes`",
            "`scanRows`",
            "`cpuCostNs`",
            "`memCostBytes`",
            "`db`",
            "`resourceGroup`",
            "`digest`",
        ] {
            assert!(statement.contains(column), "missing {column}: {statement}");
        }
        assert!(statement.contains("FROM_UNIXTIME(1)"));
        assert!(statement.contains("LIMIT 7"));
        assert!(statement.contains("`state` = 'EOF'"));
        assert!(statement.contains("`digest` = 'A0b1'"));
        assert_eq!(audit_source_state("error"), Some("ERR"));
        assert_eq!(audit_source_state("running"), None);
        assert_eq!(quote_sql_literal("a'b"), "'a''b'");

        for (state, digest, limit) in [
            (Some("EOF".into()), None, 7),
            (None, Some("not-a-hex-digest!".into()), 7),
            (None, None, MAX_QUERY_ROWS + 1),
        ] {
            assert_eq!(
                audit_statement(
                    DEFAULT_AUDIT_TABLE,
                    &QueryParameters {
                        state,
                        digest,
                        limit,
                        ..parameters.clone()
                    }
                )
                .unwrap_err()
                .code,
                ConnectorErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn source_state_codes_are_rejected_before_a_remote_browse_call() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        let error = manager
            .search(
                &view.connection_id,
                QuerySearch {
                    state: Some("EOF".into()),
                    ..search(QuerySource::AuditLoader)
                },
            )
            .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::InvalidRequest);
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn profile_is_uuid_bound_empty_is_ambiguous_and_only_enters_the_core_adapter() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        let invalid_id_error = match manager.fetch_profile(&view.connection_id, "not-a-uuid", 1) {
            Ok(_) => panic!("invalid profile ID unexpectedly returned a profile"),
            Err(error) => error,
        };
        assert_eq!(invalid_id_error.code, ConnectorErrorCode::InvalidRequest);
        let browse = manager
            .search(&view.connection_id, search(QuerySource::AuditLoader))
            .unwrap();
        let input = manager
            .fetch_profile(
                &view.connection_id,
                "22222222-2222-2222-2222-222222222222",
                browse.discovery_generation,
            )
            .unwrap()
            .into_adapter_input();
        assert_eq!(input.requested_kind, RequestedSourceKind::TextProfile);
        assert_eq!(input.version_hint, None);
        assert_eq!(
            input.acquisition_shape,
            RuntimeInventoryAcquisitionShape::GetQueryProfile
        );
        let mut empty = FakeFactory::supported();
        empty.profile.clear();
        let empty_factory = Arc::new(empty);
        let empty_manager =
            session_manager(Arc::clone(&empty_factory), Arc::new(TestClock::default()));
        let empty_view = empty_manager.connect(request()).unwrap();
        let empty_browse = empty_manager
            .search(&empty_view.connection_id, search(QuerySource::AuditLoader))
            .unwrap();
        let error = match empty_manager.fetch_profile(
            &empty_view.connection_id,
            "22222222-2222-2222-2222-222222222222",
            empty_browse.discovery_generation,
        ) {
            Ok(_) => panic!("empty profile unexpectedly returned a profile"),
            Err(error) => error,
        };
        assert_eq!(
            (error.code, error.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("not_found_or_expired_or_not_profiled")
            )
        );
    }

    trait ExpectConnectorError<T> {
        fn expect_error(self) -> ConnectorError;
    }

    impl<T> ExpectConnectorError<T> for Result<T, ConnectorError> {
        fn expect_error(self) -> ConnectorError {
            match self {
                Ok(_) => panic!("expected ConnectorError"),
                Err(error) => error,
            }
        }
    }
    fn profile_list_row(query_id: &str, state: &str) -> Vec<String> {
        vec![
            query_id.into(),
            "2026-09-02 00:00:00".into(),
            "1s".into(),
            state.into(),
            "SELECT 1".into(),
        ]
    }

    fn retain_profile_list_rows(
        manager: &SessionManager,
        connection_id: &str,
        rows: Vec<Vec<String>>,
    ) -> u32 {
        let mut sessions = manager.lock_sessions();
        let session = sessions.active.get_mut(connection_id).unwrap();
        retain_plan_sql(session, rows, 5, QuerySource::ProfileList)
            .unwrap()
            .discovery_generation
    }

    #[test]
    fn running_profile_snapshot_reads_one_opaque_profile_without_changing_finished_guards() {
        let profile_text = "Summary\nStatus: Finished\nProfile can remain opaque\n".to_owned();
        let mut fake = FakeFactory::supported();
        fake.profile = profile_text.clone();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let connection = manager.connect(request()).unwrap();
        let running_id = "aaaaaaaa-5555-5555-5555-555555555555";
        let generation = retain_profile_list_rows(
            &manager,
            &connection.connection_id,
            vec![profile_list_row(running_id, "rUnNiNg")],
        );

        let before_snapshot = factory.operations.lock().unwrap().len();
        let snapshot = manager
            .fetch_running_profile_snapshot(
                &connection.connection_id,
                &running_id.to_ascii_uppercase(),
                generation,
            )
            .unwrap();
        assert_eq!(snapshot.query_id, running_id);
        assert_eq!(snapshot.query_context.source, QuerySource::ProfileList);
        assert_eq!(snapshot.query_context.discovery_generation, generation);
        assert_eq!(snapshot.query_context.row[3], "rUnNiNg");
        assert_eq!(snapshot.profile_text, profile_text.as_bytes());
        assert_eq!(
            &factory.operations.lock().unwrap()[before_snapshot..],
            &[Operation::GetProfile]
        );

        let before_guards = factory.operations.lock().unwrap().len();
        let fetch_error = manager
            .fetch_profile(&connection.connection_id, running_id, generation)
            .expect_error();
        assert_eq!(
            (fetch_error.code, fetch_error.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_not_finished")
            )
        );
        let pair_error = manager
            .fetch_profile_pair(
                &connection.connection_id,
                running_id,
                "bbbbbbbb-5555-5555-5555-555555555555",
                generation,
            )
            .expect_error();
        assert_eq!(
            (pair_error.code, pair_error.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_not_finished")
            )
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before_guards);

        let baseline_id = "cccccccc-5555-5555-5555-555555555555";
        let candidate_id = "dddddddd-5555-5555-5555-555555555555";
        let finished_generation = retain_profile_list_rows(
            &manager,
            &connection.connection_id,
            vec![
                profile_list_row(baseline_id, "Finished"),
                profile_list_row(candidate_id, "Finished"),
            ],
        );
        let before_finished = factory.operations.lock().unwrap().len();
        assert!(
            manager
                .fetch_profile(&connection.connection_id, baseline_id, finished_generation)
                .is_ok()
        );
        assert!(
            manager
                .fetch_profile_pair(
                    &connection.connection_id,
                    baseline_id,
                    candidate_id,
                    finished_generation,
                )
                .is_ok()
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before_finished..],
            &[
                Operation::GetProfile,
                Operation::GetProfile,
                Operation::GetProfile,
            ]
        );
    }

    #[test]
    fn running_profile_snapshot_fails_closed_before_raw_io() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let first = manager.connect(request()).unwrap();
        let second = manager.connect(request()).unwrap();
        let running_id = "aaaaaaaa-6666-6666-6666-666666666666";
        let first_generation = retain_profile_list_rows(
            &manager,
            &first.connection_id,
            vec![profile_list_row(running_id, "Running")],
        );
        let second_generation = retain_profile_list_rows(
            &manager,
            &second.connection_id,
            vec![profile_list_row(
                "bbbbbbbb-6666-6666-6666-666666666666",
                "Running",
            )],
        );
        let before = factory.operations.lock().unwrap().len();

        let invalid = manager
            .fetch_running_profile_snapshot(&first.connection_id, "not-a-uuid", first_generation)
            .expect_error();
        assert_eq!(invalid.code, ConnectorErrorCode::InvalidRequest);
        let stale = manager
            .fetch_running_profile_snapshot(&first.connection_id, running_id, first_generation + 1)
            .expect_error();
        assert_eq!(
            (stale.code, stale.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_discovery_generation_expired")
            )
        );
        let unobserved = manager
            .fetch_running_profile_snapshot(
                &first.connection_id,
                "cccccccc-6666-6666-6666-666666666666",
                first_generation,
            )
            .expect_error();
        assert_eq!(
            (unobserved.code, unobserved.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_id_not_observed_in_current_session")
            )
        );
        let cross_session = manager
            .fetch_running_profile_snapshot(&second.connection_id, running_id, second_generation)
            .expect_error();
        assert_eq!(
            (cross_session.code, cross_session.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_id_not_observed_in_current_session")
            )
        );

        let non_running_generation = retain_profile_list_rows(
            &manager,
            &first.connection_id,
            vec![profile_list_row(running_id, "Finished")],
        );
        let non_running = manager
            .fetch_running_profile_snapshot(
                &first.connection_id,
                running_id,
                non_running_generation,
            )
            .expect_error();
        assert_eq!(
            (non_running.code, non_running.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_not_running")
            )
        );

        let audit_generation = {
            let mut sessions = manager.lock_sessions();
            let session = sessions.active.get_mut(&first.connection_id).unwrap();
            retain_plan_sql(
                session,
                vec![vec![
                    running_id.into(),
                    "2026-09-02 00:00:00".into(),
                    "1788278400".into(),
                    "EOF".into(),
                    "10".into(),
                    "0".into(),
                    "0".into(),
                    "0".into(),
                    "0".into(),
                    "db".into(),
                    "default_wg".into(),
                    "a0b1".into(),
                    "SELECT 1".into(),
                ]],
                13,
                QuerySource::AuditLoader,
            )
            .unwrap()
            .discovery_generation
        };
        let audit_source = manager
            .fetch_running_profile_snapshot(&first.connection_id, running_id, audit_generation)
            .expect_error();
        assert_eq!(
            (audit_source.code, audit_source.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_not_running")
            )
        );

        let duplicate_generation = retain_profile_list_rows(
            &manager,
            &first.connection_id,
            vec![
                profile_list_row(running_id, "Running"),
                vec![running_id.to_ascii_uppercase()],
            ],
        );
        let duplicate = manager
            .fetch_running_profile_snapshot(&first.connection_id, running_id, duplicate_generation)
            .expect_error();
        assert_eq!(
            (duplicate.code, duplicate.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("query_id_not_observed_in_current_session")
            )
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn running_profile_snapshot_empty_result_stays_typed_after_one_raw_read() {
        let mut fake = FakeFactory::supported();
        fake.profile.clear();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let connection = manager.connect(request()).unwrap();
        let query_id = "aaaaaaaa-7777-7777-7777-777777777777";
        let generation = retain_profile_list_rows(
            &manager,
            &connection.connection_id,
            vec![profile_list_row(query_id, "Running")],
        );
        let before = factory.operations.lock().unwrap().len();

        let error = manager
            .fetch_running_profile_snapshot(&connection.connection_id, query_id, generation)
            .expect_error();
        assert_eq!(
            (error.code, error.limitation),
            (
                ConnectorErrorCode::ProfileUnavailable,
                Some("not_found_or_expired_or_not_profiled")
            )
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::GetProfile]
        );
    }
    fn official_profile_request(generation: u32) -> AnalyzeProfileRequest {
        AnalyzeProfileRequest {
            query_id: "aaaaaaaa-1111-1111-1111-111111111111".into(),
            identity: OfficialProfileIdentity::Discovery {
                discovery_generation: generation,
            },
            node_ids: vec![27, 0],
        }
    }

    fn retain_official_profile_query(
        manager: &SessionManager,
        connection_id: &str,
        state: &str,
    ) -> u32 {
        let mut sessions = manager.lock_sessions();
        let session = sessions.active.get_mut(connection_id).unwrap();
        retain_plan_sql(
            session,
            vec![vec![
                official_profile_request(0).query_id,
                "2026-09-02 00:00:00".into(),
                "1s".into(),
                state.into(),
                "SELECT secret_canary".into(),
            ]],
            5,
            QuerySource::ProfileList,
        )
        .unwrap()
        .discovery_generation
    }

    #[test]
    fn official_profile_statement_is_uuid_and_bounded_integer_only() {
        let id = official_profile_request(0).query_id;
        assert_eq!(
            analyze_profile_statement(&id.to_ascii_uppercase(), &[]).unwrap(),
            format!("ANALYZE PROFILE FROM '{id}'")
        );
        assert_eq!(
            analyze_profile_statement(&id, &[27, 0, i32::MAX as u32]).unwrap(),
            format!("ANALYZE PROFILE FROM '{id}',27,0,2147483647")
        );
        assert!(analyze_profile_statement(&id, &(0..32).collect::<Vec<_>>()).is_ok());
        for invalid in [
            "not-a-uuid",
            "'; SELECT 1",
            "aaaaaaaa-1111-1111-1111-111111111111;SELECT 1",
        ] {
            assert_eq!(
                analyze_profile_statement(invalid, &[]).unwrap_err().code,
                ConnectorErrorCode::InvalidRequest
            );
        }
        for invalid in [vec![0, 0], vec![u32::MAX], (0..33).collect()] {
            assert_eq!(
                analyze_profile_statement(&id, &invalid).unwrap_err().code,
                ConnectorErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn official_profile_identity_and_node_validation_reject_before_any_remote_call() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let first = manager.connect(request()).unwrap();
        let second = manager.connect(request()).unwrap();
        let generation = retain_official_profile_query(&manager, &first.connection_id, "Finished");
        let valid = official_profile_request(generation);
        let before = factory.operations.lock().unwrap().len();
        for invalid in [
            AnalyzeProfileRequest {
                query_id: "bad'".into(),
                ..valid.clone()
            },
            AnalyzeProfileRequest {
                query_id: "bbbbbbbb-1111-1111-1111-111111111111".into(),
                ..valid.clone()
            },
            AnalyzeProfileRequest {
                identity: OfficialProfileIdentity::Discovery {
                    discovery_generation: generation + 1,
                },
                ..valid.clone()
            },
            AnalyzeProfileRequest {
                node_ids: vec![1, 1],
                ..valid.clone()
            },
            AnalyzeProfileRequest {
                node_ids: vec![u32::MAX],
                ..valid.clone()
            },
            AnalyzeProfileRequest {
                node_ids: (0..33).collect(),
                ..valid.clone()
            },
        ] {
            assert!(
                manager
                    .analyze_profile(&first.connection_id, invalid)
                    .is_err()
            );
        }
        assert!(
            manager
                .analyze_profile(&second.connection_id, valid.clone())
                .is_err()
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before);
        let refreshed = retain_official_profile_query(&manager, &first.connection_id, "Finished");
        assert_ne!(refreshed, generation);
        assert!(
            manager
                .analyze_profile(&first.connection_id, valid)
                .is_err()
        );
        {
            let mut sessions = manager.lock_sessions();
            let session = sessions.active.get_mut(&first.connection_id).unwrap();
            let id = official_profile_request(0).query_id;
            let retained = retain_plan_sql(
                session,
                vec![
                    vec![
                        id.clone(),
                        "2026-09-02 00:00:00".into(),
                        "1s".into(),
                        "Finished".into(),
                        "SELECT 1".into(),
                    ],
                    vec![id.to_ascii_uppercase()],
                ],
                5,
                QuerySource::ProfileList,
            )
            .unwrap();
            assert!(retained.ambiguous_query_ids.contains(&id));
        }
        let generation = manager.lock_sessions().active[&first.connection_id].discovery_generation;
        assert!(
            manager
                .analyze_profile(&first.connection_id, official_profile_request(generation))
                .is_err()
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn official_profile_finished_and_running_read_once_without_raw_fetch_or_execution() {
        for (state, expected) in [
            ("fINISHED", OfficialProfileLifecycle::Finished),
            ("rUNNING", OfficialProfileLifecycle::Running),
        ] {
            let mut fake = FakeFactory::supported();
            let rows = vec![
                "Summary\n  QueryId: query_canary\n".into(),
                "  Detail: <script>text_only</script>\r\n".into(),
            ];
            fake.official_profile = Ok(rows.clone());
            let factory = Arc::new(fake);
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let mut connection_request = request();
            connection_request.default_database = Some("db".into());
            let connection = manager.connect(connection_request).unwrap();
            let generation =
                retain_official_profile_query(&manager, &connection.connection_id, state);
            let before = factory.operations.lock().unwrap().len();
            let mut read_request = official_profile_request(generation);
            read_request.query_id = read_request.query_id.to_ascii_uppercase();
            let read = manager
                .analyze_profile(&connection.connection_id, read_request)
                .unwrap();
            assert_eq!(read.query_id, official_profile_request(0).query_id);
            assert_eq!(read.node_ids, vec![27, 0]);
            assert_eq!(read.observed_lifecycle, Some(expected));
            assert_eq!(read.detected_version, SUPPORTED_VERSION);
            assert_eq!(read.default_database.as_deref(), Some("db"));
            let query_context = read.query_context.as_ref().expect("discovery context");
            assert_eq!(query_context.discovery_generation, generation);
            assert_eq!(query_context.row_index, 0);
            assert_eq!(query_context.row[3], state);
            assert!(
                matches!(&read.content, OfficialProfileContent::Available { rows: actual } if actual == &rows)
            );
            assert!(!format!("{read:?}").contains("query_canary"));
            assert_eq!(
                &factory.operations.lock().unwrap()[before..],
                &[Operation::AnalyzeProfile]
            );
            assert_eq!(
                manager.lock_sessions().active[&connection.connection_id].discovery_generation,
                generation
            );
            if expected == OfficialProfileLifecycle::Running {
                assert!(
                    manager
                        .fetch_profile(&connection.connection_id, &read.query_id, generation)
                        .is_err()
                );
                assert!(
                    manager
                        .fetch_profile_pair(
                            &connection.connection_id,
                            &read.query_id,
                            "bbbbbbbb-1111-1111-1111-111111111111",
                            generation
                        )
                        .is_err()
                );
                assert_eq!(
                    &factory.operations.lock().unwrap()[before..],
                    &[Operation::AnalyzeProfile]
                );
            }
        }
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let connection = manager.connect(request()).unwrap();
        let browse = manager
            .search(&connection.connection_id, search(QuerySource::AuditLoader))
            .unwrap();
        let before = factory.operations.lock().unwrap().len();
        let read = manager
            .analyze_profile(
                &connection.connection_id,
                AnalyzeProfileRequest {
                    query_id: "22222222-2222-2222-2222-222222222222".into(),
                    ..official_profile_request(browse.discovery_generation)
                },
            )
            .unwrap();
        assert_eq!(
            read.observed_lifecycle,
            Some(OfficialProfileLifecycle::Finished)
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::AnalyzeProfile]
        );
    }

    #[test]
    fn official_profile_unknown_lifecycle_is_typed_unavailable_with_zero_io() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let connection = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        for state in ["Unknown", "Error", "Finished ", "", "Cancelled"] {
            let generation =
                retain_official_profile_query(&manager, &connection.connection_id, state);
            if state.is_empty() {
                assert!(
                    manager
                        .analyze_profile(
                            &connection.connection_id,
                            official_profile_request(generation)
                        )
                        .is_err()
                );
                continue;
            }
            let read = manager
                .analyze_profile(
                    &connection.connection_id,
                    official_profile_request(generation),
                )
                .unwrap();
            assert_eq!(read.observed_lifecycle, None);
            assert!(matches!(
                read.content,
                OfficialProfileContent::Unavailable {
                    reason: OfficialProfileUnavailableReason::LifecycleUnknown
                }
            ));
        }
        assert_eq!(factory.operations.lock().unwrap().len(), before);
    }

    #[test]
    fn official_profile_empty_permission_and_unsupported_remain_distinct_without_retry() {
        for (response, expected) in [
            (Ok(vec![]), OfficialProfileUnavailableReason::Empty),
            (
                Ok(vec![" \n\t".into()]),
                OfficialProfileUnavailableReason::Empty,
            ),
            (
                Err(RemoteError::PermissionDenied),
                OfficialProfileUnavailableReason::PermissionDenied,
            ),
            (
                Err(RemoteError::Unsupported),
                OfficialProfileUnavailableReason::RemoteUnsupported,
            ),
            // An evicted Profile keeps the session (found by the v0.6-10
            // footprint check: it used to discard the connection).
            (
                Err(RemoteError::QueryProfileNotFound),
                OfficialProfileUnavailableReason::NotRetained,
            ),
        ] {
            let mut fake = FakeFactory::supported();
            fake.official_profile = response;
            let factory = Arc::new(fake);
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let connection = manager.connect(request()).unwrap();
            let generation =
                retain_official_profile_query(&manager, &connection.connection_id, "Finished");
            let before = factory.operations.lock().unwrap().len();
            let read = manager
                .analyze_profile(
                    &connection.connection_id,
                    official_profile_request(generation),
                )
                .unwrap();
            assert!(
                matches!(read.content, OfficialProfileContent::Unavailable { reason } if reason == expected)
            );
            assert_eq!(
                &factory.operations.lock().unwrap()[before..],
                &[Operation::AnalyzeProfile]
            );
            assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
            assert_eq!(
                manager.lock_sessions().active[&connection.connection_id].discovery_generation,
                generation
            );
        }
    }

    #[test]
    fn official_profile_budget_and_transport_failures_discard_the_session() {
        for response in [
            Ok(vec!["x".repeat(ANALYZE_PROFILE_MAX_OUTPUT_BYTES + 1)]),
            Ok(vec![String::new(); mysql_text::MAX_RESULT_ROWS + 1]),
            Err(RemoteError::OutputBudgetExceeded),
            Err(RemoteError::Protocol),
            Err(RemoteError::Timeout),
            Err(RemoteError::Connection),
        ] {
            let mut fake = FakeFactory::supported();
            fake.official_profile = response;
            let factory = Arc::new(fake);
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let connection = manager.connect(request()).unwrap();
            let generation =
                retain_official_profile_query(&manager, &connection.connection_id, "Finished");
            let before = factory.operations.lock().unwrap().len();
            let error = manager
                .analyze_profile(
                    &connection.connection_id,
                    official_profile_request(generation),
                )
                .unwrap_err();
            assert!(session_must_be_discarded(&error));
            assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
            assert_eq!(
                &factory.operations.lock().unwrap()[before..],
                &[Operation::AnalyzeProfile]
            );
            assert_eq!(
                manager
                    .analyze_profile(
                        &connection.connection_id,
                        official_profile_request(generation)
                    )
                    .unwrap_err()
                    .code,
                ConnectorErrorCode::ConnectionExpired
            );
        }
        assert!(
            validate_official_profile_rows(&["x".repeat(ANALYZE_PROFILE_MAX_OUTPUT_BYTES)]).is_ok()
        );
        assert_eq!(
            validate_official_profile_rows(&[
                "x".repeat(ANALYZE_PROFILE_MAX_OUTPUT_BYTES),
                String::new()
            ])
            .unwrap_err()
            .limitation,
            Some("official_profile_output_budget_exceeded")
        );
    }

    #[test]
    fn explain_modes_validate_before_io_bind_the_response_and_only_costs_marks_capability() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), clock);
        let view = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        for mode in [
            ExplainMode::Logical,
            ExplainMode::Verbose,
            ExplainMode::Costs,
        ] {
            for sql in [
                "EXPLAIN ANALYZE SELECT 1",
                "SELECT 1; DELETE FROM t",
                "INSERT INTO t VALUES (1)",
                "SET query_timeout = 1",
                "ALTER PLAN ADVISOR DROP ALL",
                "SELECT 1 /*!50000 DELETE FROM t */",
            ] {
                assert_eq!(
                    manager
                        .explain(&view.connection_id, sql, mode.clone())
                        .unwrap_err()
                        .code,
                    ConnectorErrorCode::StatementNotAllowed
                );
            }
        }
        assert_eq!(factory.operations.lock().unwrap().len(), before);
        assert_eq!(
            manager.lock_sessions().active[&view.connection_id]
                .view
                .capabilities
                .explain,
            CapabilityState::Unknown
        );

        let logical = manager
            .explain(
                &view.connection_id,
                "WITH x AS (SELECT 1) SELECT * FROM x",
                ExplainMode::Logical,
            )
            .unwrap();
        assert_eq!(logical.mode, ExplainMode::Logical);
        assert_eq!(
            manager.lock_sessions().active[&view.connection_id]
                .view
                .capabilities
                .explain,
            CapabilityState::Unknown
        );
        let verbose = manager
            .explain(
                &view.connection_id,
                "WITH x AS (SELECT 1) SELECT * FROM x",
                ExplainMode::Verbose,
            )
            .unwrap();
        assert_eq!(verbose.mode, ExplainMode::Verbose);
        assert_eq!(
            manager.lock_sessions().active[&view.connection_id]
                .view
                .capabilities
                .explain,
            CapabilityState::Unknown
        );
        let costs = manager
            .explain_costs(&view.connection_id, "WITH x AS (SELECT 1) SELECT * FROM x")
            .unwrap();
        assert_eq!(costs.rows, vec!["PLAN FRAGMENT"]);
        assert_eq!(costs.mode, ExplainMode::Costs);
        assert_eq!(
            manager.lock_sessions().active[&view.connection_id]
                .view
                .capabilities
                .explain,
            CapabilityState::Available
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[
                Operation::ExplainLogical,
                Operation::ExplainVerbose,
                Operation::ExplainCosts,
            ]
        );
    }

    #[test]
    fn runtime_profile_uses_the_same_closed_statement_policy_and_returns_only_profile_input() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        for sql in [
            "SELECT 1; SELECT 2",
            "DELETE FROM t",
            "SELECT 1 /*!50000 SET x=1 */",
        ] {
            let error = match manager.execute_profiled_select(&view.connection_id, sql) {
                Ok(_) => panic!("rejected statement unexpectedly executed"),
                Err(error) => error,
            };
            assert_eq!(error.code, ConnectorErrorCode::StatementNotAllowed);
        }
        assert_eq!(factory.operations.lock().unwrap().len(), before);
        let executed = manager
            .execute_profiled_select(&view.connection_id, "WITH x AS (SELECT 1) SELECT * FROM x")
            .unwrap();
        assert_eq!(executed.query_id, "33333333-3333-3333-3333-333333333333");
        let input = executed.into_adapter_input();
        assert_eq!(input.requested_kind, RequestedSourceKind::TextProfile);
        assert_eq!(
            input.acquisition_shape,
            RuntimeInventoryAcquisitionShape::GetQueryProfile
        );
        assert_eq!(
            factory.operations.lock().unwrap().last(),
            Some(&Operation::ProfiledSelect)
        );
    }

    #[test]
    fn standard_execution_identity_allows_one_matching_same_session_official_read_only() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), Arc::clone(&clock));
        let first = manager.connect(request()).unwrap();
        let second = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        let attempt = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let executed = manager
            .execute_profiled_select_with_attempt(&first.connection_id, "SELECT 1", Some(attempt))
            .unwrap();
        let official_request = AnalyzeProfileRequest {
            query_id: executed.query_id.clone(),
            identity: OfficialProfileIdentity::VerifiedStandardAttempt {
                execution_attempt_id: attempt.into(),
            },
            node_ids: vec![],
        };
        let read = manager
            .analyze_profile(&first.connection_id, official_request.clone())
            .unwrap();
        assert_eq!(read.identity, official_request.identity);
        assert!(read.query_context.is_none());
        assert!(read.observed_lifecycle.is_none());
        assert!(matches!(
            read.content,
            OfficialProfileContent::Available { .. }
        ));
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::ProfiledSelect, Operation::AnalyzeProfile]
        );

        for invalid in [
            AnalyzeProfileRequest {
                query_id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into(),
                ..official_request.clone()
            },
            AnalyzeProfileRequest {
                identity: OfficialProfileIdentity::VerifiedStandardAttempt {
                    execution_attempt_id: "cccccccc-cccc-cccc-cccc-cccccccccccc".into(),
                },
                ..official_request.clone()
            },
        ] {
            assert!(
                manager
                    .analyze_profile(&first.connection_id, invalid)
                    .is_err()
            );
        }
        assert!(
            manager
                .analyze_profile(&second.connection_id, official_request.clone())
                .is_err()
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::ProfiledSelect, Operation::AnalyzeProfile]
        );

        manager.close(&first.connection_id);
        assert_eq!(
            manager
                .analyze_profile(&first.connection_id, official_request)
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::ProfiledSelect, Operation::AnalyzeProfile]
        );

        let expiring = second;
        let expiring_attempt = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let executed = manager
            .execute_profiled_select_with_attempt(
                &expiring.connection_id,
                "SELECT 1",
                Some(expiring_attempt),
            )
            .unwrap();
        clock.advance(11);
        assert_eq!(
            manager
                .analyze_profile(
                    &expiring.connection_id,
                    AnalyzeProfileRequest {
                        query_id: executed.query_id,
                        identity: OfficialProfileIdentity::VerifiedStandardAttempt {
                            execution_attempt_id: expiring_attempt.into(),
                        },
                        node_ids: vec![],
                    },
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[
                Operation::ProfiledSelect,
                Operation::AnalyzeProfile,
                Operation::ProfiledSelect,
            ]
        );
    }

    #[test]
    fn failed_standard_execution_stays_claimed_without_official_profile_eligibility() {
        let mut fake = FakeFactory::supported();
        fake.profile.clear();
        let factory = Arc::new(fake);
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let connection = manager.connect(request()).unwrap();
        let attempt = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        let before = factory.operations.lock().unwrap().len();
        let failed = match manager.execute_profiled_select_with_attempt(
            &connection.connection_id,
            "SELECT 1",
            Some(attempt),
        ) {
            Ok(_) => panic!("empty Profile unexpectedly established an execution identity"),
            Err(error) => error,
        };
        assert_eq!(failed.code, ConnectorErrorCode::ProfileUnavailable);
        let request = AnalyzeProfileRequest {
            query_id: "33333333-3333-3333-3333-333333333333".into(),
            identity: OfficialProfileIdentity::VerifiedStandardAttempt {
                execution_attempt_id: attempt.into(),
            },
            node_ids: vec![],
        };
        assert_eq!(
            manager
                .analyze_profile(&connection.connection_id, request)
                .unwrap_err()
                .limitation,
            Some("execution_attempt_not_eligible_for_official_profile")
        );
        let duplicate = match manager.execute_profiled_select_with_attempt(
            &connection.connection_id,
            "SELECT 1",
            Some(attempt),
        ) {
            Ok(_) => panic!("failed attempt unexpectedly ran twice"),
            Err(error) => error,
        };
        assert_eq!(
            duplicate.code,
            ConnectorErrorCode::ExecutionAttemptSubmitted
        );
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::ProfiledSelect]
        );
    }

    #[test]
    fn explain_context_is_session_bound_and_survives_close_without_a_second_lookup() {
        let mut fake = FakeFactory::supported();
        fake.version = "3.5.14-23a56ec".into();
        let factory = Arc::new(fake);
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), Arc::clone(&clock));
        let mut first_request = request();
        first_request.default_database = Some("private_database".into());
        let first = manager.connect(first_request).unwrap();
        let second = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        let captured = manager
            .explain_costs(&first.connection_id, "SELECT 1")
            .unwrap();
        let other = manager
            .explain_costs(&second.connection_id, "SELECT 1")
            .unwrap();
        assert_eq!(captured.detected_version, "3.5.14-23a56ec");
        assert_eq!(
            captured.default_database.as_deref(),
            Some("private_database")
        );
        assert_eq!(other.default_database, None);
        assert_eq!(other.detected_version, captured.detected_version);
        assert_eq!(captured.rows, vec!["PLAN FRAGMENT"]);
        assert_eq!(captured.mode, ExplainMode::Costs);
        assert_eq!(other.mode, ExplainMode::Costs);
        assert_eq!(
            &factory.operations.lock().unwrap()[before..],
            &[Operation::ExplainCosts, Operation::ExplainCosts]
        );
        manager.close(&first.connection_id);
        assert_eq!(
            manager
                .explain_costs(&first.connection_id, "SELECT 1")
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );
        clock.advance(11);
        assert_eq!(
            manager
                .explain_costs(&second.connection_id, "SELECT 1")
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );
        assert_eq!(factory.operations.lock().unwrap().len(), before + 2);
        assert_eq!(
            captured.default_database.as_deref(),
            Some("private_database")
        );
        assert_eq!(captured.detected_version, "3.5.14-23a56ec");
    }

    #[test]
    fn explain_result_debug_keeps_plan_and_database_opaque() {
        let result = ExplainResult {
            rows: vec!["TABLE: sensitive_table\nPREDICATES: secret_value".into()],
            mode: ExplainMode::Costs,
            detected_version: "3.5.14-sensitive_suffix".into(),
            default_database: Some("sensitive_database".into()),
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("row_count: 1"));
        assert!(debug.contains("mode: Costs"));
        assert!(debug.contains("database_selected: true"));
        for raw in [
            "sensitive_table",
            "secret_value",
            "sensitive_database",
            "sensitive_suffix",
            "PREDICATES",
        ] {
            assert!(!debug.contains(raw));
        }
    }

    #[test]
    fn concurrent_slow_connects_cannot_exceed_the_reserved_session_limit() {
        let gate = Arc::new((Mutex::new(OpenGate::default()), Condvar::new()));
        let factory: Arc<dyn RemoteFactory> = Arc::new(BlockingFactory {
            gate: Arc::clone(&gate),
        });
        let manager = Arc::new(SessionManager::with_seams(
            factory,
            Arc::new(TestClock::default()),
            Duration::from_secs(10),
        ));
        let handles = (0..(MAX_SESSIONS + 1))
            .map(|_| {
                let manager = Arc::clone(&manager);
                thread::spawn(move || manager.connect(request()))
            })
            .collect::<Vec<_>>();

        let (lock, wake) = &*gate;
        let mut gate_state = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        while gate_state.opened < MAX_SESSIONS {
            gate_state = wake
                .wait(gate_state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
        let observed_opened = gate_state.opened;
        gate_state.released = true;
        wake.notify_all();
        drop(gate_state);
        assert_eq!(observed_opened, MAX_SESSIONS);

        let mut views = Vec::new();
        let mut limit_errors = 0;
        for handle in handles {
            match handle.join().unwrap() {
                Ok(view) => views.push(view),
                Err(error) => {
                    assert_eq!(error.code, ConnectorErrorCode::SessionLimit);
                    limit_errors += 1;
                }
            }
        }
        assert_eq!(views.len(), MAX_SESSIONS);
        assert_eq!(limit_errors, 1);
        assert_eq!(manager.lock_sessions().active.len(), MAX_SESSIONS);
        for view in views {
            manager.close(&view.connection_id);
        }
    }

    #[test]
    fn unusable_transport_errors_evict_the_session_but_server_errors_do_not() {
        let mut timeout = FakeFactory::supported();
        timeout.audit_search = Err(RemoteError::Timeout);
        let timeout_factory = Arc::new(timeout);
        let timeout_manager =
            session_manager(Arc::clone(&timeout_factory), Arc::new(TestClock::default()));
        let timeout_view = timeout_manager.connect(request()).unwrap();
        assert_eq!(
            timeout_manager
                .search(
                    &timeout_view.connection_id,
                    search(QuerySource::AuditLoader)
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::RemoteTimeout
        );
        assert_eq!(timeout_factory.closes.load(Ordering::Relaxed), 1);
        assert_eq!(
            timeout_manager
                .search(
                    &timeout_view.connection_id,
                    search(QuerySource::AuditLoader)
                )
                .unwrap_err()
                .code,
            ConnectorErrorCode::ConnectionExpired
        );

        let mut denied = FakeFactory::supported();
        denied.audit_search = Err(RemoteError::PermissionDenied);
        let denied_factory = Arc::new(denied);
        let denied_manager =
            session_manager(Arc::clone(&denied_factory), Arc::new(TestClock::default()));
        let denied_view = denied_manager.connect(request()).unwrap();
        for _ in 0..2 {
            assert_eq!(
                denied_manager
                    .search(&denied_view.connection_id, search(QuerySource::AuditLoader))
                    .unwrap_err()
                    .code,
                ConnectorErrorCode::PermissionDenied
            );
        }
        assert_eq!(denied_factory.closes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn explicit_idle_expiry_and_close_drop_the_only_in_memory_session() {
        let factory = Arc::new(FakeFactory::supported());
        let clock = Arc::new(TestClock::default());
        let manager = session_manager(Arc::clone(&factory), Arc::clone(&clock));
        let view = manager.connect(request()).unwrap();
        assert!(manager.is_active(&view.connection_id));
        clock.advance(11);
        // Idle past the TTL reads as inactive before any sweep runs.
        assert!(!manager.is_active(&view.connection_id));
        assert_eq!(manager.expire_idle(), 1);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
        assert_eq!(
            manager.advisor(&view.connection_id).unwrap_err().code,
            ConnectorErrorCode::ConnectionExpired
        );
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
        manager.close(&view.connection_id);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
    }

    fn search(source: QuerySource) -> QuerySearch {
        QuerySearch {
            start_seconds: 0,
            end_seconds: 60,
            state: None,
            digest: None,
            limit: 0,
            source,
            statement_filter: AuditStatementFilter::All,
        }
    }
    #[test]
    fn audit_control_queued_expiry_returns_without_remote_operation_or_eviction() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = Arc::new(session_manager(
            Arc::clone(&factory),
            Arc::new(TestClock::default()),
        ));
        let view = manager.connect(request()).unwrap();
        let before = factory.operations.lock().unwrap().len();
        let lock = manager.lock_sessions();
        let control = OperationControl::new(Duration::from_secs(5));
        let worker_manager = Arc::clone(&manager);
        let worker_control = Arc::clone(&control);
        let id = view.connection_id.clone();
        let worker = thread::spawn(move || {
            worker_manager.with_supported_session_controlled(
                &id,
                Some(&worker_control),
                |_| -> Result<(), ConnectorError> { panic!("queued stopped work started") },
            )
        });
        control.stop();
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            ConnectorErrorCode::RemoteTimeout
        );
        drop(lock);
        assert_eq!(factory.operations.lock().unwrap().len(), before);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
        assert!(!control.acquired_session());
        manager
            .with_supported_session(&view.connection_id, |_| Ok(()))
            .unwrap();
    }

    #[test]
    fn audit_operation_ownership_fences_reuse_and_late_finish() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = Arc::new(session_manager(
            Arc::clone(&factory),
            Arc::new(TestClock::default()),
        ));
        let first = manager.connect(request()).unwrap();
        let other = manager.connect(request()).unwrap();
        let control = OperationControl::new(Duration::from_secs(5));
        manager
            .with_supported_session_controlled(&first.connection_id, Some(&control), |_| Ok(()))
            .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (ran_tx, ran_rx) = std::sync::mpsc::channel();
        let reuse_manager = Arc::clone(&manager);
        let id = first.connection_id.clone();
        let reuse = thread::spawn(move || {
            started_tx.send(()).unwrap();
            reuse_manager.with_supported_session(&id, |_| {
                ran_tx.send(()).unwrap();
                Ok(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(ran_rx.recv_timeout(Duration::from_millis(50)).is_err());
        // Waiting for first must release the global mutex for other sessions.
        manager
            .with_supported_session(&other.connection_id, |_| Ok(()))
            .unwrap();
        // An expired queued operation must not remove the first operation's session.
        let queued = OperationControl::new(Duration::ZERO);
        assert_eq!(
            manager
                .with_supported_session_controlled(&first.connection_id, Some(&queued), |_| Ok(()))
                .unwrap_err()
                .code,
            ConnectorErrorCode::RemoteTimeout
        );
        assert!(
            manager
                .finish_audit_operation(&first.connection_id, &queued)
                .is_err()
        );
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
        manager
            .finish_audit_operation(&first.connection_id, &control)
            .unwrap();
        ran_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        reuse.join().unwrap().unwrap();
        let next = OperationControl::new(Duration::from_secs(5));
        manager
            .with_supported_session_controlled(&first.connection_id, Some(&next), |_| Ok(()))
            .unwrap();
        control.stop(); // completed old request may never cancel this owner
        manager
            .finish_audit_operation(&first.connection_id, &control)
            .unwrap();
        assert!(!next.is_stopped());
        assert!(
            manager
                .lock_sessions()
                .active
                .get(&first.connection_id)
                .unwrap()
                .audit_operation
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &next))
        );
        manager
            .finish_audit_operation(&first.connection_id, &next)
            .unwrap();
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn audit_operation_cancelled_or_panicked_owner_is_evicted_before_reuse() {
        for panic_operation in [false, true] {
            let factory = Arc::new(FakeFactory::supported());
            let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
            let view = manager.connect(request()).unwrap();
            let control = OperationControl::new(Duration::from_secs(5));
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                manager
                    .with_supported_session_controlled(&view.connection_id, Some(&control), |_| {
                        if panic_operation {
                            panic!("synthetic session operation panic");
                        }
                        Ok(())
                    })
                    .unwrap();
            }));
            control.stop();
            assert!(
                manager
                    .finish_audit_operation(&view.connection_id, &control)
                    .is_err()
            );
            assert_eq!(
                manager
                    .with_supported_session(&view.connection_id, |_| Ok(()))
                    .unwrap_err()
                    .code,
                ConnectorErrorCode::ConnectionExpired
            );
            assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn audit_expired_before_available_lock_does_not_acquire_or_evict() {
        let factory = Arc::new(FakeFactory::supported());
        let manager = session_manager(Arc::clone(&factory), Arc::new(TestClock::default()));
        let view = manager.connect(request()).unwrap();
        let control = OperationControl::new(Duration::ZERO);
        let result = manager.with_supported_session_controlled(
            &view.connection_id,
            Some(&control),
            |_| -> Result<(), ConnectorError> { panic!("expired request ran") },
        );
        assert_eq!(result.unwrap_err().code, ConnectorErrorCode::RemoteTimeout);
        assert!(!control.acquired_session());
        assert!(
            manager
                .finish_audit_operation(&view.connection_id, &control)
                .is_err()
        );
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
        manager
            .with_supported_session(&view.connection_id, |_| Ok(()))
            .unwrap();
    }
}
