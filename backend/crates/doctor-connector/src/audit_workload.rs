//! Closed, bounded SQL builders for StarRocks 3.5.14 AuditLoader workload facts.
//!
//! These builders never accept a raw ORDER BY, GROUP BY, SQL fragment, or table
//! path. The session integration owns capability probing and converts the
//! nullable wire cells into the typed source rows below before core metric
//! normalization.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use doctor_contracts::{
    AUDIT_FINGERPRINT_READ_LIMIT, AUDIT_FINGERPRINT_STATEMENT_CHARS, AuditFamilyRequest,
    AuditFamilySort, AuditMetricId, AuditMetricInput, AuditOccurrenceRequest, AuditOccurrenceSort,
    AuditOptionalFactUnavailableReason, AuditScope, AuditStatementFilter, AuditTerminalState,
    AuditWindow, AuditWorkloadValidationError,
};

mod family_detail;
pub use family_detail::*;
mod performance;
pub use performance::*;

const OCCURRENCE_PROJECTION: &str = "`query_id`, `event_time`, UNIX_TIMESTAMP(`event_time`) AS `event_epoch_seconds`, `state`, `query_time`, `scan_bytes`, `scan_rows`, `cpu_cost_ns`, `mem_cost_bytes`, `return_rows`, `catalog_name`, `database_name`, `resource_group`, `digest`, `error_code`, `error_code_agrees`, `pending_time_ms`, `pending_time_agrees`";

pub const AUDIT_QUERY_TIMEOUT_SECONDS: u16 = 10;
pub const AUDIT_SQL_LOOKUP_LIMIT: u8 = 2;
pub const AUDIT_OVERVIEW_FAMILY_TOP_LIMIT: u16 = 10;
/// Characters of the audited `stmt` returned as one family's SQL sample.
pub const AUDIT_STATEMENT_SAMPLE_CHARS: u16 = 240;
/// The largest complete family population for which the fixed DECIMAL sort key
/// is provably an exact ordering of `error_count / execution_count`.
pub(crate) const ERROR_PROPORTION_EXACT_POPULATION_MAX: u64 = 3_000_000_000_000;
/// The wire carries the left-edge step plus at most 2047 source transitions.
/// A larger exact source is unavailable, never sampled.
pub const AUDIT_CONCURRENCY_MAX_TRANSITIONS: usize = 2047;
pub const AUDIT_CONCURRENCY_MAX_STEPS: usize = AUDIT_CONCURRENCY_MAX_TRANSITIONS + 1;
pub const AUDIT_ACTIVE_AT_PARTICIPANT_LIMIT: u16 = 500;
/// The intentionally incomplete historical interval lookback. It is applied
/// only to the concurrency/active-at cohort, never to public window cards or
/// Top tables.
pub const AUDIT_HISTORICAL_LOOKBACK_SECONDS: u32 = 60 * 60;
/// Product-owned prefix on every fixed statement this tool sends (never on a
/// user's Runtime Top SQL). Official 3.5.14 `ConnectProcessor.auditAfterExec`
/// stores the COM_QUERY text through `LogUtil.removeLineSeparator`, which keeps
/// `/* ... */` comments, and AuditLoader 5.0.0 only truncates `stmt` by bytes,
/// so the prefix stays at the start of the audited `stmt`. It contains no LIKE
/// wildcard, quote, or backslash, and only single spaces.
pub const DOCTOR_STATEMENT_MARKER: &str = "/* sr-doctor */ ";

/// The marker earlier releases put on their own statements. Audit history
/// keeps those rows, so they are recognised and hidden exactly like the
/// current marker; nothing is sent with it any more.
pub const LEGACY_DOCTOR_STATEMENT_MARKER: &str = "/* starrocks-doctor */ ";

/// Every marker that identifies this tool's own statements in audit history.
pub const DOCTOR_STATEMENT_MARKERS: [&str; 2] =
    [DOCTOR_STATEMENT_MARKER, LEGACY_DOCTOR_STATEMENT_MARKER];

/// The exact COM_QUERY text this tool sends for one of its own statements.
pub fn doctor_statement(statement: &str) -> String {
    format!("{DOCTOR_STATEMENT_MARKER}{statement}")
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuditColumn {
    QueryId,
    Timestamp,
    State,
    QueryTime,
    ScanBytes,
    ScanRows,
    CpuCostNs,
    MemCostBytes,
    ReturnRows,
    Catalog,
    Database,
    ResourceGroup,
    Digest,
    User,
    Statement,
    ErrorCode,
    PendingTimeMs,
    IsQuery,
}

impl AuditColumn {
    pub(crate) fn source_name(self) -> &'static str {
        match self {
            Self::QueryId => "queryId",
            Self::Timestamp => "timestamp",
            Self::State => "state",
            Self::QueryTime => "queryTime",
            Self::ScanBytes => "scanBytes",
            Self::ScanRows => "scanRows",
            Self::CpuCostNs => "cpuCostNs",
            Self::MemCostBytes => "memCostBytes",
            Self::ReturnRows => "returnRows",
            Self::Catalog => "catalog",
            Self::Database => "db",
            Self::ResourceGroup => "resourceGroup",
            Self::Digest => "digest",
            Self::User => "user",
            Self::Statement => "stmt",
            Self::ErrorCode => "errorCode",
            Self::PendingTimeMs => "pendingTimeMs",
            Self::IsQuery => "isQuery",
        }
    }

    fn from_source_name(value: &str) -> Option<Self> {
        [
            Self::QueryId,
            Self::Timestamp,
            Self::State,
            Self::QueryTime,
            Self::ScanBytes,
            Self::ScanRows,
            Self::CpuCostNs,
            Self::MemCostBytes,
            Self::ReturnRows,
            Self::Catalog,
            Self::Database,
            Self::ResourceGroup,
            Self::Digest,
            Self::User,
            Self::Statement,
            Self::ErrorCode,
            Self::PendingTimeMs,
            Self::IsQuery,
        ]
        .into_iter()
        .find(|column| column.source_name() == value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditSchema {
    columns: BTreeSet<AuditColumn>,
    /// `SHOW COLUMNS` field[1] is retained only for fixed source arithmetic
    /// guards. Legacy workload reads remain column-capability based.
    source_types: BTreeMap<AuditColumn, String>,
}

impl AuditSchema {
    pub fn new(columns: impl IntoIterator<Item = AuditColumn>) -> Self {
        Self {
            columns: columns.into_iter().collect(),
            source_types: BTreeMap::new(),
        }
    }

    /// Fixed official AuditLoader field set required by this first workload
    /// slice. A live session must prove these fields before executing a query.
    pub fn official_3_5_14() -> Self {
        let mut schema = Self::new([
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
            AuditColumn::Statement,
            AuditColumn::ErrorCode,
            AuditColumn::PendingTimeMs,
            AuditColumn::IsQuery,
        ]);
        for (column, source_type) in [
            (AuditColumn::Timestamp, "DATETIME"),
            (AuditColumn::QueryTime, "BIGINT"),
            (AuditColumn::ScanBytes, "BIGINT"),
            (AuditColumn::ScanRows, "BIGINT"),
            (AuditColumn::CpuCostNs, "BIGINT"),
            (AuditColumn::MemCostBytes, "BIGINT"),
            (AuditColumn::ReturnRows, "BIGINT"),
            (AuditColumn::PendingTimeMs, "BIGINT"),
            (AuditColumn::IsQuery, "TINYINT"),
        ] {
            schema.source_types.insert(column, source_type.into());
        }
        schema
    }

    /// Parses the first `SHOW COLUMNS` field for each source-table row. Unknown
    /// source fields remain deliberately irrelevant; operation-specific builder
    /// requirements decide whether a missing known field is fatal.
    pub fn from_schema_probe_rows(
        rows: &[Vec<Option<String>>],
    ) -> Result<Self, AuditSqlBuildError> {
        let mut columns = BTreeSet::new();
        let mut source_types = BTreeMap::new();
        for row in rows {
            let Some(Some(field)) = row.first() else {
                return Err(AuditSqlBuildError::SchemaProbeRow);
            };
            if let Some(column) = AuditColumn::from_source_name(field) {
                columns.insert(column);
                if let Some(Some(source_type)) = row.get(1) {
                    source_types.insert(column, source_type.trim().to_owned());
                }
            }
        }
        Ok(Self {
            columns,
            source_types,
        })
    }

    pub fn has(&self, column: AuditColumn) -> bool {
        self.columns.contains(&column)
    }

    fn has_source_type(&self, column: AuditColumn, expected: &str) -> bool {
        self.source_types
            .get(&column)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
    }

    /// Ordinary workload SQL compares, aggregates, or ranks exactly these
    /// source columns. Unknown/incompatible source types must fail before a
    /// remote statement can rely on StarRocks implicit numeric coercion.
    fn supports_base_metric_workload(&self) -> bool {
        self.has_source_type(AuditColumn::Timestamp, "DATETIME")
            && [
                AuditColumn::QueryTime,
                AuditColumn::ScanBytes,
                AuditColumn::ScanRows,
                AuditColumn::CpuCostNs,
                AuditColumn::MemCostBytes,
            ]
            .into_iter()
            .all(|column| self.has_source_type(column, "BIGINT"))
    }

    /// Fixed historical-in-flight SQL is deliberately stricter than the
    /// generic workload surface: its LARGEINT endpoint arithmetic is valid
    /// only for the exact official source types observed in this session.
    /// The explicit statement-class filter compares `isQuery = 1` and reads a
    /// `stmt` prefix. Both columns, and the official TINYINT source type, must
    /// be proven; the filter never silently degrades to an unfiltered read.
    pub fn supports_statement_filter(&self) -> bool {
        self.has(AuditColumn::Statement)
            && self.has(AuditColumn::IsQuery)
            && self.has_source_type(AuditColumn::IsQuery, "TINYINT")
    }

    pub fn supports_historical_concurrency(&self) -> bool {
        self.has_source_type(AuditColumn::Timestamp, "DATETIME")
            && self.has_source_type(AuditColumn::QueryTime, "BIGINT")
    }

    fn require(&self, required: &[AuditColumn]) -> Result<(), AuditSqlBuildError> {
        required
            .iter()
            .copied()
            .find(|column| !self.columns.contains(column))
            .map(|column| AuditSqlBuildError::SchemaMissing(column.source_name()))
            .map_or(Ok(()), Err)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditTableName(String);

impl AuditTableName {
    pub fn parse(value: impl Into<String>) -> Result<Self, AuditSqlBuildError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.len() <= 256
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            });
        valid
            .then_some(Self(value))
            .ok_or(AuditSqlBuildError::TableName)
    }

    pub(crate) fn quoted(&self) -> String {
        self.0
            .split('.')
            .map(|segment| format!("`{segment}`"))
            .collect::<Vec<_>>()
            .join(".")
    }

    pub(crate) fn as_source_name(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditSqlLookupRequest {
    pub scope: AuditScope,
    pub query_id: String,
}

impl AuditSqlLookupRequest {
    pub fn validate(&self) -> Result<(), AuditSqlBuildError> {
        self.scope.validate().map_err(AuditSqlBuildError::Request)?;
        valid_query_id(&self.query_id)
            .then_some(())
            .ok_or(AuditSqlBuildError::QueryId)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditSqlBuildError {
    Request(AuditWorkloadValidationError),
    TableName,
    QueryId,
    PointOutsideWindow,
    SchemaTypeUnsupported,
    /// The explicit statement-class filter was requested but `isQuery`
    /// (TINYINT) or `stmt` was not proven by the session schema probe.
    StatementFilterUnavailable,
    SchemaMissing(&'static str),
    SchemaProbeRow,
    FamilyTarget,
    FamilyCandidateBudget,
    StatementBudget,
    FamilyDetailResponse,
    /// Unsupported bucket width, or more buckets than one read may return.
    PerformanceBucket,
    PerformanceResponse,
    RowShape {
        expected: usize,
        actual: usize,
    },
}

impl fmt::Display for AuditSqlBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => write!(formatter, "{error}"),
            Self::TableName => formatter.write_str("audit_table_name"),
            Self::QueryId => formatter.write_str("audit_query_id"),
            Self::PointOutsideWindow => formatter.write_str("audit_point_outside_window"),
            Self::SchemaTypeUnsupported => formatter.write_str("audit_schema_type_unsupported"),
            Self::StatementFilterUnavailable => {
                formatter.write_str("audit_statement_filter_unavailable")
            }
            Self::SchemaMissing(column) => write!(formatter, "audit_schema_missing:{column}"),
            Self::SchemaProbeRow => formatter.write_str("audit_schema_probe_row"),
            Self::FamilyTarget => formatter.write_str("audit_family_target"),
            Self::FamilyCandidateBudget => formatter.write_str("audit_family_candidate_budget"),
            Self::StatementBudget => formatter.write_str("audit_statement_budget"),
            Self::FamilyDetailResponse => formatter.write_str("audit_family_detail_response"),
            Self::PerformanceBucket => formatter.write_str("audit_performance_bucket"),
            Self::PerformanceResponse => formatter.write_str("audit_performance_response"),
            Self::RowShape { expected, actual } => {
                write!(formatter, "audit_row_shape:{actual}_expected_{expected}")
            }
        }
    }
}

impl Error for AuditSqlBuildError {}

/// The newest canonical executions in scope with a complete context and no
/// valid native digest, for product fingerprints. One row more than the read
/// limit is never returned; `eligible_count` (the last column, the same on
/// every row) is the full-window count of such executions.
pub fn audit_fingerprint_event_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    validate_occurrence_scope(scope, schema)?;
    schema.require(&[AuditColumn::Statement])?;
    Ok(format!(
        "{} SELECT {OCCURRENCE_PROJECTION}, COUNT(*) OVER () AS `eligible_count` FROM occurrence_events WHERE `catalog_name` IS NOT NULL AND `catalog_name` <> {empty} AND LENGTH(`catalog_name`) <= 256 AND `database_name` IS NOT NULL AND `database_name` <> {empty} AND LENGTH(`database_name`) <= 256 AND (`digest` IS NULL OR `digest` NOT REGEXP '^[0-9A-Fa-f]{{1,128}}$') ORDER BY `event_time` DESC, `query_id` ASC LIMIT {limit}",
        canonical_occurrence_events_cte(table, scope, schema),
        empty = utf8_hex_literal(""),
        limit = AUDIT_FINGERPRINT_READ_LIMIT,
    ))
}

/// The most canonical executions one collection slice reads (stage B
/// development default; the production budget comes from G1).
pub const AUDIT_COLLECTION_SLICE_LIMIT: u16 = 1000;

/// One stage B collection slice: the canonical executions whose audit
/// timestamp falls in the scope window, oldest first, each with its
/// fingerprint-length `stmt` prefix and whether duplicate rows agree on it.
/// The last column is the full slice count, so a bounded read knows it is
/// partial. Duplicate audit rows of one execution are one event (by queryId).
pub fn audit_collection_slice_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    validate_occurrence_scope(scope, schema)?;
    schema.require(&[AuditColumn::Statement])?;
    Ok(format!(
        "{}, statement_facts AS (SELECT LOWER(`queryId`) AS `statement_query_id`, MAX(LEFT(`stmt`, {chars})) AS `statement_prefix`, MAX(CASE WHEN `stmt` IS NULL THEN 1 ELSE 0 END) AS `statement_null`, MAX(CASE WHEN CHAR_LENGTH(`stmt`) > {chars} THEN 1 ELSE 0 END) AS `statement_truncated`, CASE WHEN {agrees} THEN 1 ELSE 0 END AS `statement_agrees` FROM {table}{predicate} GROUP BY LOWER(`queryId`)) SELECT {OCCURRENCE_PROJECTION}, `statement_prefix`, `statement_null`, `statement_truncated`, `statement_agrees`, COUNT(*) OVER () AS `slice_count` FROM occurrence_events INNER JOIN statement_facts ON `occurrence_events`.`query_id` = `statement_facts`.`statement_query_id` ORDER BY `event_time` ASC, `query_id` ASC LIMIT {limit}",
        canonical_occurrence_events_cte(table, scope, schema),
        chars = AUDIT_FINGERPRINT_STATEMENT_CHARS,
        agrees = optional_rows_agree_having("stmt"),
        table = table.quoted(),
        predicate = scope_predicate(scope),
        limit = AUDIT_COLLECTION_SLICE_LIMIT,
    ))
}

pub fn audit_occurrence_statement(
    table: &AuditTableName,
    request: &AuditOccurrenceRequest,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    request.validate().map_err(AuditSqlBuildError::Request)?;
    validate_occurrence_scope(&request.scope, schema)?;
    Ok(format!(
        "{} SELECT {OCCURRENCE_PROJECTION} FROM occurrence_events ORDER BY {} LIMIT {}",
        canonical_occurrence_events_cte(table, &request.scope, schema),
        occurrence_order(&request.sort),
        request.limit,
    ))
}

fn validate_occurrence_scope(
    scope: &AuditScope,
    schema: &AuditSchema,
) -> Result<(), AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    schema.require(&[
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
    ])?;
    require_scope_columns(schema, scope)?;
    if !schema.supports_base_metric_workload() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    Ok(())
}

pub fn audit_family_statement(
    table: &AuditTableName,
    request: &AuditFamilyRequest,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    request.validate().map_err(AuditSqlBuildError::Request)?;
    schema.require(&[
        AuditColumn::QueryId,
        AuditColumn::Timestamp,
        AuditColumn::State,
        AuditColumn::QueryTime,
        AuditColumn::ScanBytes,
        AuditColumn::CpuCostNs,
        AuditColumn::MemCostBytes,
        AuditColumn::Catalog,
        AuditColumn::Database,
        AuditColumn::Digest,
    ])?;
    require_scope_columns(schema, &request.scope)?;
    if !schema.supports_base_metric_workload() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    Ok(format!(
        "{}, valid_family_events AS (SELECT * FROM canonical_events WHERE `catalog_name` IS NOT NULL AND `catalog_name` <> {} AND LENGTH(`catalog_name`) <= 256 AND `database_name` IS NOT NULL AND `database_name` <> {} AND LENGTH(`database_name`) <= 256 AND `digest` REGEXP '^[0-9A-Fa-f]{{1,128}}$'), family_aggregates AS (SELECT `catalog_name`, `database_name`, `digest`, COUNT(*) AS `canonical_execution_count`, SUM(CASE WHEN `state` = {} THEN 1 ELSE 0 END) AS `error_count`, SUM(CASE WHEN `query_time` >= 0 THEN 1 ELSE 0 END) AS `query_time_valid_count`, SUM(CASE WHEN `query_time` >= 0 THEN `query_time` ELSE NULL END) AS `cumulative_query_time`, SUM(CASE WHEN `scan_bytes` >= 0 THEN 1 ELSE 0 END) AS `scan_bytes_valid_count`, SUM(CASE WHEN `scan_bytes` >= 0 THEN `scan_bytes` ELSE NULL END) AS `cumulative_scan_bytes`, SUM(CASE WHEN `cpu_cost_ns` >= 0 THEN 1 ELSE 0 END) AS `cpu_cost_valid_count`, SUM(CASE WHEN `cpu_cost_ns` >= 0 THEN `cpu_cost_ns` ELSE NULL END) AS `cumulative_cpu_cost`, SUM(CASE WHEN `mem_cost_bytes` >= 0 THEN 1 ELSE 0 END) AS `memory_cost_valid_count`, MAX(CASE WHEN `mem_cost_bytes` >= 0 THEN `mem_cost_bytes` ELSE NULL END) AS `max_memory_cost`, MAX(CASE WHEN `query_time` >= 0 THEN `query_time` ELSE NULL END) AS `max_query_time` FROM valid_family_events GROUP BY `catalog_name`, `database_name`, `digest`), family_rankable AS (SELECT family_aggregates.*, MAX(`canonical_execution_count`) OVER () AS `error_proportion_population_max` FROM family_aggregates) SELECT `catalog_name`, `database_name`, `digest`, `canonical_execution_count`, `error_count`, `query_time_valid_count`, `cumulative_query_time`, `scan_bytes_valid_count`, `cumulative_scan_bytes`, `cpu_cost_valid_count`, `cumulative_cpu_cost`, `memory_cost_valid_count`, `max_memory_cost`, `max_query_time`, `error_proportion_population_max` FROM family_rankable ORDER BY {} LIMIT {}",
        canonical_events_cte(table, &request.scope),
        utf8_hex_literal(""),
        utf8_hex_literal(""),
        utf8_hex_literal("ERR"),
        family_order(&request.sort),
        request.limit,
    ))
}

/// A single full-window server aggregate. It deliberately has no `LIMIT` and
/// reports decimal SQL text so later JSON/TS boundaries never receive a lossy
/// counter number. Metric cells are source-presence counts only; the one core
/// typed metric parser owns invalid/overflow/precision classification.
pub fn audit_coverage_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    schema.require(&[
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
    ])?;
    require_scope_columns(schema, scope)?;
    if !schema.supports_base_metric_workload() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    Ok(format!(
        "WITH audit_window AS (SELECT LOWER(`queryId`) AS `query_id`, `timestamp` AS `event_time`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `returnRows` AS `return_rows`, `catalog` AS `catalog_name`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest` FROM {table}{predicate}), identity_groups AS (SELECT `query_id`, COUNT(*) AS `source_count`, CASE WHEN {rows_agree} THEN 1 ELSE 0 END AS `rows_agree`, MAX(`event_time`) AS `event_time`, MAX(`state`) AS `state`, MAX(`query_time`) AS `query_time`, MAX(`scan_bytes`) AS `scan_bytes`, MAX(`scan_rows`) AS `scan_rows`, MAX(`cpu_cost_ns`) AS `cpu_cost_ns`, MAX(`mem_cost_bytes`) AS `mem_cost_bytes`, MAX(`return_rows`) AS `return_rows`, MAX(`catalog_name`) AS `catalog_name`, MAX(`database_name`) AS `database_name`, MAX(`resource_group`) AS `resource_group`, MAX(`digest`) AS `digest` FROM audit_window WHERE `query_id` REGEXP '^[0-9A-Fa-f]{{8}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{12}}$' GROUP BY `query_id`), canonical_events AS (SELECT `query_id`, `event_time`, `state`, `query_time`, `scan_bytes`, `scan_rows`, `cpu_cost_ns`, `mem_cost_bytes`, `return_rows`, `catalog_name`, `database_name`, `resource_group`, `digest` FROM identity_groups WHERE `rows_agree` = 1), valid_family_events AS (SELECT * FROM canonical_events WHERE `catalog_name` IS NOT NULL AND `catalog_name` <> {empty} AND LENGTH(`catalog_name`) <= 256 AND `database_name` IS NOT NULL AND `database_name` <> {empty} AND LENGTH(`database_name`) <= 256 AND `digest` REGEXP '^[0-9A-Fa-f]{{1,128}}$') SELECT (SELECT COUNT(*) FROM audit_window) AS `raw_row_count`, COALESCE((SELECT SUM(CASE WHEN `query_id` REGEXP '^[0-9A-Fa-f]{{8}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{12}}$' THEN 0 ELSE 1 END) FROM audit_window), 0) AS `invalid_query_id_count`, COALESCE((SELECT SUM(CASE WHEN `rows_agree` = 1 THEN `source_count` - 1 ELSE 0 END) FROM identity_groups), 0) AS `exact_duplicate_row_count`, COALESCE((SELECT SUM(CASE WHEN `rows_agree` = 0 THEN `source_count` ELSE 0 END) FROM identity_groups), 0) AS `conflicting_duplicate_row_count`, COALESCE((SELECT SUM(CASE WHEN `rows_agree` = 1 AND (`digest` IS NULL OR `digest` = {empty} OR `digest` NOT REGEXP '^[0-9A-Fa-f]{{1,128}}$') THEN 1 ELSE 0 END) FROM identity_groups), 0) AS `missing_digest_count`, COALESCE((SELECT SUM(CASE WHEN `rows_agree` = 1 AND (`catalog_name` IS NULL OR `catalog_name` = {empty} OR LENGTH(`catalog_name`) > 256 OR `database_name` IS NULL OR `database_name` = {empty} OR LENGTH(`database_name`) > 256) THEN 1 ELSE 0 END) FROM identity_groups), 0) AS `missing_context_count`, (SELECT COUNT(*) FROM canonical_events) AS `retained_event_count`, (SELECT COUNT(*) FROM (SELECT `catalog_name`, `database_name`, `digest` FROM valid_family_events GROUP BY `catalog_name`, `database_name`, `digest`) AS retained_families) AS `retained_family_count`, COALESCE(SUM(CASE WHEN `query_time` IS NULL THEN 1 ELSE 0 END), 0) AS `query_time_sql_null_count`, COUNT(`query_time`) AS `query_time_text_present_count`, COALESCE(SUM(CASE WHEN `query_time` >= 0 THEN 1 ELSE 0 END), 0) AS `query_time_nonnegative_count`, COALESCE(SUM(CASE WHEN `query_time` < 0 THEN 1 ELSE 0 END), 0) AS `query_time_negative_count`, SUM(CASE WHEN `query_time` >= 0 THEN `query_time` ELSE NULL END) AS `cumulative_query_time`, COALESCE(SUM(CASE WHEN `scan_bytes` IS NULL THEN 1 ELSE 0 END), 0) AS `scan_bytes_sql_null_count`, COUNT(`scan_bytes`) AS `scan_bytes_text_present_count`, COALESCE(SUM(CASE WHEN `scan_bytes` >= 0 THEN 1 ELSE 0 END), 0) AS `scan_bytes_nonnegative_count`, COALESCE(SUM(CASE WHEN `scan_bytes` < 0 THEN 1 ELSE 0 END), 0) AS `scan_bytes_negative_count`, SUM(CASE WHEN `scan_bytes` >= 0 THEN `scan_bytes` ELSE NULL END) AS `cumulative_scan_bytes`, COALESCE(SUM(CASE WHEN `cpu_cost_ns` IS NULL THEN 1 ELSE 0 END), 0) AS `cpu_cost_sql_null_count`, COUNT(`cpu_cost_ns`) AS `cpu_cost_text_present_count`, COALESCE(SUM(CASE WHEN `cpu_cost_ns` >= 0 THEN 1 ELSE 0 END), 0) AS `cpu_cost_nonnegative_count`, COALESCE(SUM(CASE WHEN `cpu_cost_ns` < 0 THEN 1 ELSE 0 END), 0) AS `cpu_cost_negative_count`, SUM(CASE WHEN `cpu_cost_ns` >= 0 THEN `cpu_cost_ns` ELSE NULL END) AS `cumulative_cpu_cost`, COALESCE(SUM(CASE WHEN `mem_cost_bytes` IS NULL THEN 1 ELSE 0 END), 0) AS `memory_cost_sql_null_count`, COUNT(`mem_cost_bytes`) AS `memory_cost_text_present_count`, COALESCE(SUM(CASE WHEN `mem_cost_bytes` >= 0 THEN 1 ELSE 0 END), 0) AS `memory_cost_nonnegative_count`, COALESCE(SUM(CASE WHEN `mem_cost_bytes` < 0 THEN 1 ELSE 0 END), 0) AS `memory_cost_negative_count`, MAX(CASE WHEN `mem_cost_bytes` >= 0 THEN `mem_cost_bytes` ELSE NULL END) AS `max_memory_cost`, COALESCE(SUM(CASE WHEN `state` = {error_state} THEN 1 ELSE 0 END), 0) AS `error_count`, {statement_filter_counts} FROM canonical_events",
        table = table.quoted(),
        predicate = scope_predicate(scope),
        rows_agree = projected_rows_agree_having(),
        empty = utf8_hex_literal(""),
        error_state = utf8_hex_literal("ERR"),
        statement_filter_counts = statement_filter_hidden_counts(table, scope),
    ))
}

/// Hidden raw-row counts for the explicit statement-class filter over the same
/// window and field filters. The two buckets are disjoint and, with
/// `raw_row_count`, partition the unfiltered source rows. `all` emits typed
/// SQL NULL so "not applied" is never conflated with zero hidden rows.
fn statement_filter_hidden_counts(table: &AuditTableName, scope: &AuditScope) -> String {
    if !scope.statement_filter.is_applied() {
        return "CAST(NULL AS BIGINT) AS `hidden_non_query_row_count`, CAST(NULL AS BIGINT) AS `hidden_doctor_row_count`".into();
    }
    let base = scope_field_predicate_for_window(scope, scope.window.clone());
    let table = table.quoted();
    let non_query = statement_filter_hidden_non_query_condition();
    let doctor = statement_filter_hidden_doctor_condition();
    format!(
        "(SELECT COUNT(*) FROM {table}{base} AND {non_query}) AS `hidden_non_query_row_count`, (SELECT COUNT(*) FROM {table}{base} AND {doctor}) AS `hidden_doctor_row_count`"
    )
}

/// Historical terminal-audit concurrency is an interval projection, not a
/// live-query or blocking measurement. Its canonical cohort is deliberately
/// bounded by `audit_historical_source_window`; it never claims all history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditConcurrencyCoverageSource {
    pub canonical_bounded_event_count: String,
    pub interval_count: String,
    pub initial_active_count: String,
    pub transition_count: String,
    pub timestamp_missing_count: String,
    pub timestamp_invalid_count: String,
    pub non_terminal_count: String,
    pub query_time_sql_null_count: String,
    pub query_time_negative_count: String,
    pub query_time_overflow_count: String,
    pub zero_duration_count: String,
    pub endpoint_overflow_count: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditConcurrencyDeltaSource {
    pub at_milliseconds: i128,
    pub delta: i128,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditConcurrencySource {
    SchemaTypeUnsupported,
    Exact {
        coverage: AuditConcurrencyCoverageSource,
        axis_labels: Vec<String>,
        deltas: Vec<AuditConcurrencyDeltaSource>,
    },
    TransitionPointBudgetExceeded {
        coverage: AuditConcurrencyCoverageSource,
        axis_labels: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditActiveAtSource {
    pub at_milliseconds: i128,
    pub active_count: String,
    pub rows: Vec<AuditActiveAtParticipantSource>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditActiveAtParticipantSource {
    pub occurrence: AuditOccurrenceSourceRow,
    pub start_milliseconds: i128,
    pub end_milliseconds: i128,
    pub elapsed_milliseconds: i128,
}

/// One sealed summary plus at most 2047 exact grouped transitions. Source rows
/// are never capped: the summary proves the full transition count before the
/// transport budget decides whether a line is available.
pub fn audit_concurrency_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    if !schema.supports_historical_concurrency() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    schema.require(&[
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
    ])?;
    require_scope_columns(schema, scope)?;
    let start = i128::from(scope.window.start_seconds) * 1_000;
    let end = i128::from(scope.window.end_seconds) * 1_000;
    Ok(format!(
        "{}, classified_events AS (SELECT *, CASE WHEN `event_time` IS NULL THEN 'timestamp_missing' WHEN UNIX_TIMESTAMP(`event_time`) IS NULL THEN 'timestamp_invalid' WHEN `state` IS NULL OR (`state` <> {} AND `state` <> {}) THEN 'non_terminal' WHEN `query_time` IS NULL THEN 'query_time_sql_null' WHEN `query_time` < 0 THEN 'query_time_negative' WHEN `query_time` > {} THEN 'query_time_overflow' WHEN `query_time` = 0 THEN 'zero_duration' ELSE 'interval' END AS `interval_state` FROM canonical_events), valid_intervals AS (SELECT `query_id`, `event_time`, `state`, `query_time`, `scan_bytes`, `scan_rows`, `cpu_cost_ns`, `mem_cost_bytes`, `return_rows`, `catalog_name`, `database_name`, `resource_group`, `digest`, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 AS `start_ms`, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 + CAST(`query_time` AS LARGEINT) AS `end_ms` FROM classified_events WHERE `interval_state` = 'interval'), overlapping_intervals AS (SELECT * FROM valid_intervals WHERE `start_ms` < {} AND `end_ms` > {}), transition_events AS (SELECT `start_ms` AS `at_ms`, CAST(1 AS LARGEINT) AS `delta` FROM overlapping_intervals WHERE `start_ms` > {} AND `start_ms` < {} UNION ALL SELECT `end_ms` AS `at_ms`, CAST(-1 AS LARGEINT) AS `delta` FROM overlapping_intervals WHERE `end_ms` > {} AND `end_ms` < {}), grouped_transitions AS (SELECT `at_ms`, SUM(`delta`) AS `delta` FROM transition_events GROUP BY `at_ms` HAVING SUM(`delta`) <> 0), concurrency_coverage AS (SELECT (SELECT COUNT(*) FROM canonical_events) AS `canonical_bounded_event_count`, (SELECT COUNT(*) FROM overlapping_intervals) AS `interval_count`, (SELECT COUNT(*) FROM overlapping_intervals WHERE `start_ms` <= {} AND `end_ms` > {}) AS `initial_active_count`, (SELECT COUNT(*) FROM grouped_transitions) AS `transition_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'timestamp_missing' THEN 1 ELSE 0 END), 0) AS `timestamp_missing_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'timestamp_invalid' THEN 1 ELSE 0 END), 0) AS `timestamp_invalid_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'non_terminal' THEN 1 ELSE 0 END), 0) AS `non_terminal_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'query_time_sql_null' THEN 1 ELSE 0 END), 0) AS `query_time_sql_null_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'query_time_negative' THEN 1 ELSE 0 END), 0) AS `query_time_negative_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'query_time_overflow' THEN 1 ELSE 0 END), 0) AS `query_time_overflow_count`, COALESCE(SUM(CASE WHEN `interval_state` = 'zero_duration' THEN 1 ELSE 0 END), 0) AS `zero_duration_count`, CAST(0 AS BIGINT) AS `endpoint_overflow_count` FROM classified_events) SELECT * FROM (SELECT CAST(0 AS BIGINT) AS `row_kind`, CAST(NULL AS LARGEINT) AS `at_ms`, CAST(NULL AS LARGEINT) AS `delta`, `canonical_bounded_event_count`, `interval_count`, `initial_active_count`, `transition_count`, `timestamp_missing_count`, `timestamp_invalid_count`, `non_terminal_count`, `query_time_sql_null_count`, `query_time_negative_count`, `query_time_overflow_count`, `zero_duration_count`, `endpoint_overflow_count`, {}, CAST(NULL AS VARCHAR) AS `point_label` FROM concurrency_coverage UNION ALL SELECT CAST(1 AS BIGINT) AS `row_kind`, `at_ms`, `delta`, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, DATE_FORMAT(FROM_UNIXTIME(CAST(`at_ms` / 1000 AS BIGINT)), '{}') AS `point_label` FROM (SELECT * FROM grouped_transitions ORDER BY `at_ms` ASC LIMIT {}) AS bounded_transitions) AS concurrency_rows ORDER BY `row_kind` ASC, `at_ms` ASC",
        canonical_historical_events_cte(table, scope),
        utf8_hex_literal("EOF"),
        utf8_hex_literal("ERR"),
        doctor_core::MAX_AUDIT_QUERY_TIME_MS,
        end,
        start,
        start,
        end,
        start,
        end,
        start,
        start,
        time_tick_columns(scope),
        transition_point_label_format(scope),
        AUDIT_CONCURRENCY_MAX_TRANSITIONS,
    ))
}

pub fn decode_concurrency_source_rows(
    rows: Vec<Vec<Option<String>>>,
) -> Result<AuditConcurrencySource, AuditSqlBuildError> {
    const EXPECTED_COLUMNS: usize = 21;
    let mut summary = None;
    let mut deltas = Vec::new();
    for row in rows {
        if row.len() != EXPECTED_COLUMNS {
            return Err(row_shape(EXPECTED_COLUMNS, row.len()));
        }
        match required_unsigned(&row[0])?.as_str() {
            "0" if summary.is_none() => {
                let coverage = AuditConcurrencyCoverageSource {
                    canonical_bounded_event_count: required_unsigned(&row[3])?,
                    interval_count: required_unsigned(&row[4])?,
                    initial_active_count: required_unsigned(&row[5])?,
                    transition_count: required_unsigned(&row[6])?,
                    timestamp_missing_count: required_unsigned(&row[7])?,
                    timestamp_invalid_count: required_unsigned(&row[8])?,
                    non_terminal_count: required_unsigned(&row[9])?,
                    query_time_sql_null_count: required_unsigned(&row[10])?,
                    query_time_negative_count: required_unsigned(&row[11])?,
                    query_time_overflow_count: required_unsigned(&row[12])?,
                    zero_duration_count: required_unsigned(&row[13])?,
                    endpoint_overflow_count: required_unsigned(&row[14])?,
                };
                let axis_labels = row[15..20]
                    .iter()
                    .map(|value| {
                        value
                            .clone()
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                summary = Some((coverage, axis_labels));
            }
            "1" if summary.is_some() => {
                let at_milliseconds = required_signed(&row[1])?
                    .parse::<i128>()
                    .map_err(|_| row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS))?;
                let delta = required_signed(&row[2])?
                    .parse::<i128>()
                    .map_err(|_| row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS))?;
                let label = row[20]
                    .clone()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS))?;
                if at_milliseconds < 0
                    || delta == 0
                    || deltas
                        .last()
                        .is_some_and(|previous: &AuditConcurrencyDeltaSource| {
                            previous.at_milliseconds >= at_milliseconds
                        })
                {
                    return Err(row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS));
                }
                deltas.push(AuditConcurrencyDeltaSource {
                    at_milliseconds,
                    delta,
                    label,
                });
            }
            _ => return Err(row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS)),
        }
    }
    let (coverage, axis_labels) = summary.ok_or_else(|| row_shape(1, 0))?;
    let transition_count = coverage.transition_count.parse::<usize>().ok();
    if transition_count.is_none_or(|count| count > AUDIT_CONCURRENCY_MAX_TRANSITIONS) {
        return Ok(AuditConcurrencySource::TransitionPointBudgetExceeded {
            coverage,
            axis_labels,
        });
    }
    if transition_count != Some(deltas.len()) {
        return Err(row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS));
    }
    Ok(AuditConcurrencySource::Exact {
        coverage,
        axis_labels,
        deltas,
    })
}

/// A bounded drill uses the same canonical historical source and interval
/// eligibility as the line. It returns a source count plus deterministic IDs.
pub fn audit_active_at_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    at_milliseconds: i128,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    if !schema.supports_historical_concurrency() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    let start = i128::from(scope.window.start_seconds) * 1_000;
    let end = i128::from(scope.window.end_seconds) * 1_000;
    if at_milliseconds < start || at_milliseconds >= end {
        return Err(AuditSqlBuildError::PointOutsideWindow);
    }
    schema.require(&[
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
    ])?;
    require_scope_columns(schema, scope)?;
    Ok(format!(
        "{}, classified_events AS (SELECT *, CASE WHEN `event_time` IS NOT NULL AND UNIX_TIMESTAMP(`event_time`) IS NOT NULL AND (`state` = {} OR `state` = {}) AND `query_time` IS NOT NULL AND `query_time` > 0 AND `query_time` <= {} THEN 1 ELSE 0 END AS `is_interval` FROM occurrence_events), valid_intervals AS (SELECT *, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 AS `start_ms`, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 + CAST(`query_time` AS LARGEINT) AS `end_ms` FROM classified_events WHERE `is_interval` = 1), active_intervals AS (SELECT * FROM valid_intervals WHERE `start_ms` <= {} AND `end_ms` > {}) SELECT * FROM (SELECT CAST(0 AS BIGINT) AS `row_kind`, CAST(NULL AS VARCHAR) AS `query_id`, CAST(NULL AS VARCHAR) AS `event_time`, CAST(NULL AS VARCHAR) AS `event_epoch_seconds`, CAST(NULL AS VARCHAR) AS `state`, CAST(NULL AS VARCHAR) AS `query_time`, CAST(NULL AS VARCHAR) AS `scan_bytes`, CAST(NULL AS VARCHAR) AS `scan_rows`, CAST(NULL AS VARCHAR) AS `cpu_cost_ns`, CAST(NULL AS VARCHAR) AS `mem_cost_bytes`, CAST(NULL AS VARCHAR) AS `return_rows`, CAST(NULL AS VARCHAR) AS `catalog_name`, CAST(NULL AS VARCHAR) AS `database_name`, CAST(NULL AS VARCHAR) AS `resource_group`, CAST(NULL AS VARCHAR) AS `digest`, CAST(NULL AS VARCHAR) AS `error_code`, CAST(NULL AS BIGINT) AS `error_code_agrees`, CAST(NULL AS BIGINT) AS `pending_time_ms`, CAST(NULL AS BIGINT) AS `pending_time_agrees`, COUNT(*) AS `active_count` FROM active_intervals UNION ALL SELECT CAST(1 AS BIGINT) AS `row_kind`, `query_id`, `event_time`, CAST(UNIX_TIMESTAMP(`event_time`) AS VARCHAR) AS `event_epoch_seconds`, `state`, `query_time`, `scan_bytes`, `scan_rows`, `cpu_cost_ns`, `mem_cost_bytes`, `return_rows`, `catalog_name`, `database_name`, `resource_group`, `digest`, `error_code`, `error_code_agrees`, `pending_time_ms`, `pending_time_agrees`, CAST(NULL AS BIGINT) AS `active_count` FROM (SELECT * FROM active_intervals ORDER BY `query_id` ASC LIMIT {}) AS bounded_participants) AS active_rows ORDER BY `row_kind` ASC, `query_id` ASC",
        canonical_historical_occurrence_events_cte(table, scope, schema),
        utf8_hex_literal("EOF"),
        utf8_hex_literal("ERR"),
        doctor_core::MAX_AUDIT_QUERY_TIME_MS,
        at_milliseconds,
        at_milliseconds,
        AUDIT_ACTIVE_AT_PARTICIPANT_LIMIT,
    ))
}

pub fn decode_active_at_source_rows(
    rows: Vec<Vec<Option<String>>>,
    at_milliseconds: i128,
    schema: &AuditSchema,
) -> Result<AuditActiveAtSource, AuditSqlBuildError> {
    const EXPECTED_COLUMNS: usize = 20;
    let mut active_count = None;
    let mut participants = Vec::new();
    for row in rows {
        if row.len() != EXPECTED_COLUMNS {
            return Err(row_shape(EXPECTED_COLUMNS, row.len()));
        }
        match required_unsigned(&row[0])?.as_str() {
            "0" if active_count.is_none() => active_count = Some(required_unsigned(&row[19])?),
            "1" if active_count.is_some() => {
                let occurrence = decode_occurrence_source_row(row[1..19].to_vec(), schema)?;
                participants.push(active_at_participant(occurrence, at_milliseconds)?);
            }
            _ => return Err(row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS)),
        }
    }
    let active_count = active_count.ok_or_else(|| row_shape(1, 0))?;
    let source_count = active_count.parse::<usize>().ok();
    if source_count.is_some_and(|count| count < participants.len())
        || participants.len() > usize::from(AUDIT_ACTIVE_AT_PARTICIPANT_LIMIT)
    {
        return Err(row_shape(EXPECTED_COLUMNS, EXPECTED_COLUMNS));
    }
    Ok(AuditActiveAtSource {
        at_milliseconds,
        truncated: source_count.is_none_or(|count| count > participants.len()),
        active_count,
        rows: participants,
    })
}

fn active_at_participant(
    occurrence: AuditOccurrenceSourceRow,
    at_milliseconds: i128,
) -> Result<AuditActiveAtParticipantSource, AuditSqlBuildError> {
    let start_milliseconds = occurrence
        .event_epoch_seconds
        .as_deref()
        .and_then(|value| value.parse::<i128>().ok())
        .and_then(|value| value.checked_mul(1_000));
    let query_time = occurrence
        .metrics
        .iter()
        .find_map(|(metric_id, input)| {
            (*metric_id == AuditMetricId::QueryTime).then_some(input.clone())
        })
        .ok_or_else(|| row_shape(20, 20))?;
    let terminal = occurrence.state.as_deref().is_some_and(|state| {
        state.eq_ignore_ascii_case("EOF") || state.eq_ignore_ascii_case("ERR")
    });
    let interval = doctor_core::audit_interval(start_milliseconds, terminal, query_time)
        .map_err(|_| row_shape(20, 20))?;
    if !interval.active_at(at_milliseconds) {
        return Err(row_shape(20, 20));
    }
    Ok(AuditActiveAtParticipantSource {
        elapsed_milliseconds: at_milliseconds
            .checked_sub(interval.start_ms)
            .ok_or_else(|| row_shape(20, 20))?,
        start_milliseconds: interval.start_ms,
        end_milliseconds: interval.end_ms,
        occurrence,
    })
}

fn required_unsigned(value: &Option<String>) -> Result<String, AuditSqlBuildError> {
    let value = value
        .as_deref()
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| row_shape(21, 21))?;
    let normalized = value.trim_start_matches('0');
    Ok(if normalized.is_empty() {
        "0"
    } else {
        normalized
    }
    .to_owned())
}

fn required_signed(value: &Option<String>) -> Result<&str, AuditSqlBuildError> {
    value
        .as_deref()
        .filter(|value| !value.is_empty() && value.parse::<i128>().is_ok())
        .ok_or_else(|| row_shape(21, 21))
}

fn row_shape(expected: usize, actual: usize) -> AuditSqlBuildError {
    AuditSqlBuildError::RowShape { expected, actual }
}

/// Explicit lookup only. It deliberately reads at most two rows so a duplicate
/// query id is an observable ambiguity, never a silently selected SQL text.
pub fn audit_sql_lookup_statement(
    table: &AuditTableName,
    request: &AuditSqlLookupRequest,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    request.validate()?;
    schema.require(&[
        AuditColumn::QueryId,
        AuditColumn::Timestamp,
        AuditColumn::Statement,
    ])?;
    require_scope_columns(schema, &request.scope)?;
    Ok(format!(
        "SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, `stmt` AS `statement` FROM {}{} AND LOWER(`queryId`) = {} ORDER BY `timestamp` DESC LIMIT {}",
        table.quoted(),
        scope_predicate(&request.scope),
        utf8_hex_literal(&request.query_id.to_ascii_lowercase()),
        AUDIT_SQL_LOOKUP_LIMIT,
    ))
}

/// One bounded SQL sample per returned family: the `stmt` prefix of the
/// family's most recent valid-identity member inside the same scope. It is a
/// real member's audited text, not a normalized family statement. Only the
/// listed targets are read, so the result never widens the returned page.
pub fn audit_family_sample_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    targets: &[AuditFamilyTarget],
    schema: &AuditSchema,
    chars: u16,
) -> Result<String, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    schema.require(&[
        AuditColumn::QueryId,
        AuditColumn::Timestamp,
        AuditColumn::Catalog,
        AuditColumn::Database,
        AuditColumn::Digest,
        AuditColumn::Statement,
    ])?;
    require_scope_columns(schema, scope)?;
    if targets.is_empty() || targets.len() > usize::from(AUDIT_FAMILY_SAMPLE_TARGET_LIMIT) {
        return Err(AuditSqlBuildError::FamilyTarget);
    }
    let families = targets
        .iter()
        .map(|target| {
            format!(
                "(`catalog` = {} AND `db` = {} AND `digest` = {})",
                utf8_hex_literal(&target.catalog),
                utf8_hex_literal(&target.database),
                utf8_hex_literal(&target.native_digest),
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    Ok(format!(
        "WITH sample_source AS (SELECT `catalog` AS `catalog_name`, `db` AS `database_name`, `digest` AS `digest`, LOWER(`queryId`) AS `query_id`, `timestamp` AS `event_time`, LEFT(`stmt`, {chars}) AS `statement_prefix`, CASE WHEN `stmt` IS NULL THEN 1 ELSE 0 END AS `statement_null`, CASE WHEN CHAR_LENGTH(`stmt`) > {chars} THEN 1 ELSE 0 END AS `statement_truncated` FROM {table}{predicate} AND ({families}) AND LOWER(`queryId`) REGEXP '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$'), ranked_samples AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY `catalog_name`, `database_name`, `digest` ORDER BY `event_time` DESC, `query_id` ASC) AS `sample_rank` FROM sample_source) SELECT `catalog_name`, `database_name`, `digest`, `statement_prefix`, `statement_null`, `statement_truncated` FROM ranked_samples WHERE `sample_rank` = 1 ORDER BY `catalog_name` ASC, `database_name` ASC, `digest` ASC",
        table = table.quoted(),
        predicate = scope_predicate(scope),
    ))
}

/// The most families one sample read may name (the family page limit).
pub const AUDIT_FAMILY_SAMPLE_TARGET_LIMIT: u16 = 200;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditStatementSampleSource {
    Text { value: String, truncated: bool },
    SqlNull,
}

/// Whether a family or occurrence row carries a sample. Only the family page
/// and occurrence lists read one; other rows keep `NotRead`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AuditSourceStatementSample {
    #[default]
    NotRead,
    SourceColumnAbsent,
    /// Duplicate rows of one execution disagree on `stmt` (occurrences only).
    ConflictingSourceRows,
    Read(AuditStatementSampleSource),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilySampleRow {
    pub target: AuditFamilyTarget,
    pub sample: AuditStatementSampleSource,
}

pub fn decode_family_sample_row(
    row: Vec<Option<String>>,
) -> Result<AuditFamilySampleRow, AuditSqlBuildError> {
    let [catalog, database, digest, prefix, null, truncated]: [Option<String>; 6] = row
        .try_into()
        .map_err(|row: Vec<Option<String>>| row_shape(6, row.len()))?;
    let (Some(catalog), Some(database), Some(native_digest)) = (catalog, database, digest) else {
        return Err(AuditSqlBuildError::FamilyTarget);
    };
    let flag = |cell: Option<String>| match cell.as_deref() {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err(row_shape(6, 6)),
    };
    let sample = if flag(null)? {
        AuditStatementSampleSource::SqlNull
    } else {
        AuditStatementSampleSource::Text {
            value: prefix.ok_or_else(|| row_shape(6, 6))?,
            truncated: flag(truncated)?,
        }
    };
    Ok(AuditFamilySampleRow {
        target: AuditFamilyTarget {
            native_digest,
            catalog,
            database,
        },
        sample,
    })
}

/// Attach each returned family's sample by its exact (catalog, database,
/// digest) identity. A family with no sample row keeps `NotRead`.
pub fn attach_family_samples(
    rows: &mut [AuditFamilySourceRow],
    samples: Vec<AuditFamilySampleRow>,
) {
    for sample in samples {
        if let Some(row) = rows.iter_mut().find(|row| {
            row.catalog.as_deref() == Some(sample.target.catalog.as_str())
                && row.database.as_deref() == Some(sample.target.database.as_str())
                && row.native_digest.as_deref() == Some(sample.target.native_digest.as_str())
        }) {
            row.statement_sample = AuditSourceStatementSample::Read(sample.sample);
        }
    }
}

/// The families a sample read names: only rows with a complete identity.
pub fn family_sample_targets(rows: &[AuditFamilySourceRow]) -> Vec<AuditFamilyTarget> {
    rows.iter()
        .filter_map(|row| {
            Some(AuditFamilyTarget {
                native_digest: row.native_digest.clone()?,
                catalog: row.catalog.clone()?,
                database: row.database.clone()?,
            })
        })
        .collect()
}

/// The most executions one sample read may name: the occurrence page limit
/// and the active-at participant limit.
pub const AUDIT_OCCURRENCE_SAMPLE_TARGET_LIMIT: u16 = 500;

/// The source range an occurrence sample read uses. It is always the range
/// that produced the rows: the public scope for Top and member lists, the
/// fixed historical lookback for active-at participants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOccurrenceSampleWindow {
    Scope,
    HistoricalLookback,
}

/// One bounded SQL sample per returned execution: the `stmt` prefix of that
/// query id's audit rows inside the same source range. Duplicate rows must
/// agree on the full `stmt`; a disagreement is reported, never resolved by
/// picking one text. Only the listed ids are read.
pub fn audit_occurrence_sample_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    window: AuditOccurrenceSampleWindow,
    query_ids: &[String],
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    statement_prefix_statement(
        table,
        scope,
        window,
        query_ids,
        schema,
        AUDIT_STATEMENT_SAMPLE_CHARS,
    )
}

/// The same per-execution read with the longer fingerprint prefix: a
/// statement longer than it is reported truncated, never fingerprinted.
pub fn audit_fingerprint_sample_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    query_ids: &[String],
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    statement_prefix_statement(
        table,
        scope,
        AuditOccurrenceSampleWindow::Scope,
        query_ids,
        schema,
        AUDIT_FINGERPRINT_STATEMENT_CHARS,
    )
}

fn statement_prefix_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    window: AuditOccurrenceSampleWindow,
    query_ids: &[String],
    schema: &AuditSchema,
    chars: u16,
) -> Result<String, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    schema.require(&[
        AuditColumn::QueryId,
        AuditColumn::Timestamp,
        AuditColumn::Statement,
    ])?;
    require_scope_columns(schema, scope)?;
    if query_ids.is_empty() || query_ids.len() > usize::from(AUDIT_OCCURRENCE_SAMPLE_TARGET_LIMIT) {
        return Err(AuditSqlBuildError::QueryId);
    }
    let mut unique = BTreeSet::new();
    for id in query_ids {
        if crate::canonical_query_id(id).as_deref() != Some(id.as_str()) || !unique.insert(id) {
            return Err(AuditSqlBuildError::QueryId);
        }
    }
    let predicate = match window {
        AuditOccurrenceSampleWindow::Scope => scope_predicate(scope),
        AuditOccurrenceSampleWindow::HistoricalLookback => historical_scope_predicate(scope),
    };
    let statement = format!(
        "SELECT LOWER(`queryId`) AS `query_id`, MAX(LEFT(`stmt`, {chars})) AS `statement_prefix`, MAX(CASE WHEN `stmt` IS NULL THEN 1 ELSE 0 END) AS `statement_null`, MAX(CASE WHEN CHAR_LENGTH(`stmt`) > {chars} THEN 1 ELSE 0 END) AS `statement_truncated`, CASE WHEN {agrees} THEN 1 ELSE 0 END AS `statement_agrees` FROM {table}{predicate} AND LOWER(`queryId`) IN ({ids}) GROUP BY LOWER(`queryId`) ORDER BY `query_id` ASC",
        agrees = optional_rows_agree_having("stmt"),
        table = table.quoted(),
        ids = query_ids
            .iter()
            .map(|id| utf8_hex_literal(id))
            .collect::<Vec<_>>()
            .join(", "),
    );
    if statement.len() > crate::MAX_PLAN_SQL_BYTES {
        return Err(AuditSqlBuildError::StatementBudget);
    }
    Ok(statement)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditOccurrenceSampleRow {
    pub query_id: String,
    pub sample: AuditSourceStatementSample,
}

pub fn decode_occurrence_sample_row(
    row: Vec<Option<String>>,
) -> Result<AuditOccurrenceSampleRow, AuditSqlBuildError> {
    let [query_id, prefix, null, truncated, agrees]: [Option<String>; 5] = row
        .try_into()
        .map_err(|row: Vec<Option<String>>| row_shape(5, row.len()))?;
    let query_id = query_id
        .filter(|id| crate::canonical_query_id(id).as_deref() == Some(id.as_str()))
        .ok_or(AuditSqlBuildError::QueryId)?;
    let flag = |cell: Option<String>| match cell.as_deref() {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err(row_shape(5, 5)),
    };
    let sample = if !flag(agrees)? {
        AuditSourceStatementSample::ConflictingSourceRows
    } else if flag(null)? {
        AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull)
    } else {
        AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
            value: prefix.ok_or_else(|| row_shape(5, 5))?,
            truncated: flag(truncated)?,
        })
    };
    Ok(AuditOccurrenceSampleRow { query_id, sample })
}

/// Attach each sample to the execution with the same canonical query id. An
/// execution with no sample row keeps `NotRead`.
pub fn attach_occurrence_samples<'a>(
    rows: impl IntoIterator<Item = &'a mut AuditOccurrenceSourceRow>,
    samples: Vec<AuditOccurrenceSampleRow>,
) {
    let samples = samples
        .into_iter()
        .map(|sample| (sample.query_id, sample.sample))
        .collect::<BTreeMap<_, _>>();
    for row in rows {
        if let Some(sample) = row
            .query_id
            .as_deref()
            .and_then(crate::canonical_query_id)
            .and_then(|id| samples.get(&id))
        {
            row.statement_sample = sample.clone();
        }
    }
}

/// The executions a sample read names: each valid canonical id once.
pub fn occurrence_sample_targets<'a>(
    rows: impl IntoIterator<Item = &'a AuditOccurrenceSourceRow>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    rows.into_iter()
        .filter_map(|row| row.query_id.as_deref().and_then(crate::canonical_query_id))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// SQL lookup for an identity discovered by an explicit historical active-at
/// drill. Unlike ordinary occurrence lookup it retains the bounded historical predicate,
/// then proves the same interval remains active at the sealed point.
pub fn audit_active_at_sql_lookup_statement(
    table: &AuditTableName,
    request: &AuditSqlLookupRequest,
    at_milliseconds: i128,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    request.validate()?;
    if !schema.supports_historical_concurrency() {
        return Err(AuditSqlBuildError::SchemaTypeUnsupported);
    }
    schema.require(&[
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
        AuditColumn::Statement,
    ])?;
    require_scope_columns(schema, &request.scope)?;
    Ok(format!(
        "{}, valid_intervals AS (SELECT `query_id`, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 AS `start_ms`, CAST(UNIX_TIMESTAMP(`event_time`) AS LARGEINT) * 1000 + CAST(`query_time` AS LARGEINT) AS `end_ms` FROM canonical_events WHERE `event_time` IS NOT NULL AND UNIX_TIMESTAMP(`event_time`) IS NOT NULL AND (`state` = {} OR `state` = {}) AND `query_time` IS NOT NULL AND `query_time` > 0 AND `query_time` <= {}) SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, `stmt` AS `statement` FROM {}{} AND LOWER(`queryId`) = {} AND EXISTS (SELECT 1 FROM valid_intervals WHERE `query_id` = {} AND `start_ms` <= {} AND `end_ms` > {}) ORDER BY `timestamp` DESC LIMIT {}",
        canonical_historical_events_cte(table, &request.scope),
        utf8_hex_literal("EOF"),
        utf8_hex_literal("ERR"),
        doctor_core::MAX_AUDIT_QUERY_TIME_MS,
        table.quoted(),
        historical_scope_predicate(&request.scope),
        utf8_hex_literal(&request.query_id.to_ascii_lowercase()),
        utf8_hex_literal(&request.query_id.to_ascii_lowercase()),
        at_milliseconds,
        at_milliseconds,
        AUDIT_SQL_LOOKUP_LIMIT,
    ))
}

/// Lossless optional `errorCode` source fact. This stays out of the metric
/// parser because it is AuditLoader text, not a numeric measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditErrorCodeSource {
    Unavailable {
        reason: AuditOptionalFactUnavailableReason,
    },
    SqlNull,
    Text {
        value: String,
    },
}

/// Raw source row with nullable cells retained. No metric is parsed here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditOccurrenceSourceRow {
    pub query_id: Option<String>,
    /// Raw AuditLoader `timestamp` text. It carries no UTC assertion; consult
    /// `AuditSessionIdentity::audit_timestamp_time_zone` before interpreting it.
    pub event_time: Option<String>,
    /// `UNIX_TIMESTAMP(event_time)` evaluated by the same MySQL session that
    /// ran this fixed AuditLoader statement, not a claim about the raw column's
    /// original time-zone representation.
    pub event_epoch_seconds: Option<String>,
    pub state: Option<String>,
    pub metrics: Vec<(AuditMetricId, AuditMetricInput)>,
    pub error_code: AuditErrorCodeSource,
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub resource_group: Option<String>,
    pub native_digest: Option<String>,
    /// Filled only by a separate bounded sample read; decoding keeps `NotRead`.
    pub statement_sample: AuditSourceStatementSample,
}

pub fn decode_occurrence_source_row(
    row: Vec<Option<String>>,
    schema: &AuditSchema,
) -> Result<AuditOccurrenceSourceRow, AuditSqlBuildError> {
    const EXPECTED_COLUMNS: usize = 18;
    if row.len() != EXPECTED_COLUMNS {
        return Err(AuditSqlBuildError::RowShape {
            expected: EXPECTED_COLUMNS,
            actual: row.len(),
        });
    }
    Ok(AuditOccurrenceSourceRow {
        query_id: row[0].clone(),
        event_time: row[1].clone(),
        event_epoch_seconds: row[2].clone(),
        state: row[3].clone(),
        metrics: vec![
            (
                AuditMetricId::QueryTime,
                AuditMetricInput::from_sql_cell(row[4].clone()),
            ),
            (
                AuditMetricId::ScanBytes,
                AuditMetricInput::from_sql_cell(row[5].clone()),
            ),
            (
                AuditMetricId::ScanRows,
                AuditMetricInput::from_sql_cell(row[6].clone()),
            ),
            (
                AuditMetricId::CpuCost,
                AuditMetricInput::from_sql_cell(row[7].clone()),
            ),
            (
                AuditMetricId::MemoryCost,
                AuditMetricInput::from_sql_cell(row[8].clone()),
            ),
            (
                AuditMetricId::ReturnRows,
                AuditMetricInput::from_sql_cell(row[9].clone()),
            ),
            (
                AuditMetricId::PendingTime,
                optional_metric_input(
                    row[16].clone(),
                    row[17].clone(),
                    schema.has(AuditColumn::PendingTimeMs),
                )?,
            ),
        ],
        error_code: optional_error_code(
            row[14].clone(),
            row[15].clone(),
            schema.has(AuditColumn::ErrorCode),
        )?,
        catalog: row[10].clone(),
        database: row[11].clone(),
        resource_group: row[12].clone(),
        native_digest: row[13].clone(),
        statement_sample: AuditSourceStatementSample::NotRead,
    })
}

fn optional_metric_input(
    value: Option<String>,
    agreement: Option<String>,
    source_present: bool,
) -> Result<AuditMetricInput, AuditSqlBuildError> {
    if !source_present {
        return Ok(AuditMetricInput::Unavailable {
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        });
    }
    if !optional_fact_agrees(agreement)? {
        return Ok(AuditMetricInput::Unavailable {
            reason: AuditOptionalFactUnavailableReason::ConflictingSourceRows,
        });
    }
    Ok(AuditMetricInput::from_sql_cell(value))
}

fn optional_error_code(
    value: Option<String>,
    agreement: Option<String>,
    source_present: bool,
) -> Result<AuditErrorCodeSource, AuditSqlBuildError> {
    if !source_present {
        return Ok(AuditErrorCodeSource::Unavailable {
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        });
    }
    if !optional_fact_agrees(agreement)? {
        return Ok(AuditErrorCodeSource::Unavailable {
            reason: AuditOptionalFactUnavailableReason::ConflictingSourceRows,
        });
    }
    Ok(match value {
        Some(value) => AuditErrorCodeSource::Text { value },
        None => AuditErrorCodeSource::SqlNull,
    })
}

fn optional_fact_agrees(value: Option<String>) -> Result<bool, AuditSqlBuildError> {
    match value.as_deref() {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err(row_shape(18, 18)),
    }
}

/// Lossless family aggregate cells. Counters and aggregate values remain
/// decimal text until the root core normalizer gives them typed semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilySourceRow {
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub native_digest: Option<String>,
    pub canonical_execution_count: Option<String>,
    pub error_count: Option<String>,
    pub query_time_valid_count: Option<String>,
    pub cumulative_query_time: Option<String>,
    pub scan_bytes_valid_count: Option<String>,
    pub cumulative_scan_bytes: Option<String>,
    pub cpu_cost_valid_count: Option<String>,
    pub cumulative_cpu_cost: Option<String>,
    pub memory_cost_valid_count: Option<String>,
    pub max_memory_cost: Option<String>,
    pub max_query_time: Option<String>,
    /// Internal same-statement population guard for error-proportion ordering.
    /// It is not a public metric or family fact.
    pub error_proportion_population_max: Option<String>,
    pub statement_sample: AuditSourceStatementSample,
}

pub fn decode_family_source_row(
    row: Vec<Option<String>>,
) -> Result<AuditFamilySourceRow, AuditSqlBuildError> {
    const EXPECTED_COLUMNS: usize = 15;
    if row.len() != EXPECTED_COLUMNS {
        return Err(AuditSqlBuildError::RowShape {
            expected: EXPECTED_COLUMNS,
            actual: row.len(),
        });
    }
    Ok(AuditFamilySourceRow {
        catalog: row[0].clone(),
        database: row[1].clone(),
        native_digest: row[2].clone(),
        canonical_execution_count: row[3].clone(),
        error_count: row[4].clone(),
        query_time_valid_count: row[5].clone(),
        cumulative_query_time: row[6].clone(),
        scan_bytes_valid_count: row[7].clone(),
        cumulative_scan_bytes: row[8].clone(),
        cpu_cost_valid_count: row[9].clone(),
        cumulative_cpu_cost: row[10].clone(),
        memory_cost_valid_count: row[11].clone(),
        max_memory_cost: row[12].clone(),
        max_query_time: row[13].clone(),
        error_proportion_population_max: row[14].clone(),
        statement_sample: AuditSourceStatementSample::NotRead,
    })
}

/// Dedupe only a source event identity whose complete projection agrees. A
/// conflicting duplicate query id is excluded before either Top ordering or
/// family aggregation. The API integration must surface its coverage/exclusion
/// count; this closed builder deliberately does not silently select one row.
fn canonical_events_cte(table: &AuditTableName, scope: &AuditScope) -> String {
    canonical_events_cte_with_predicate(table, scope_predicate(scope))
}

/// Returns the fixed, disclosed source range `[B,W1)` for historical interval
/// reconstruction, where `B = max(0, W0 - LOOKBACK)`. The public scope remains
/// `[W0,W1)` and is used unchanged for coverage/cards/Top queries.
pub fn audit_historical_source_window(scope: &AuditScope) -> AuditWindow {
    AuditWindow {
        start_seconds: scope
            .window
            .start_seconds
            .saturating_sub(AUDIT_HISTORICAL_LOOKBACK_SECONDS),
        end_seconds: scope.window.end_seconds,
    }
}

/// Historical interval reads use the fixed bounded source range. This path
/// deliberately has a timestamp lower bound so it cannot regress to an
/// all-retained-prefix scan.
fn canonical_historical_events_cte(table: &AuditTableName, scope: &AuditScope) -> String {
    canonical_events_cte_with_predicate(table, historical_scope_predicate(scope))
}

/// Decorates the fixed base canonical cohort for occurrence-shaped reads only.
/// Optional facts are deliberately not part of `canonical_events` agreement:
/// conflicting optional rows become typed unavailable while the pre-existing
/// 14-column canonical event remains eligible for Top/coverage/family reads.
fn canonical_occurrence_events_cte(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> String {
    canonical_occurrence_events_cte_with_predicate(table, scope_predicate(scope), schema)
}

fn canonical_historical_occurrence_events_cte(
    table: &AuditTableName,
    scope: &AuditScope,
    schema: &AuditSchema,
) -> String {
    canonical_occurrence_events_cte_with_predicate(table, historical_scope_predicate(scope), schema)
}

fn canonical_occurrence_events_cte_with_predicate(
    table: &AuditTableName,
    predicate: String,
    schema: &AuditSchema,
) -> String {
    canonical_occurrence_events_cte_with_user(table, predicate, schema, false)
}

/// `with_user` adds the optional source user as one more decorated fact,
/// with the same agreement rule as `errorCode`; otherwise the CTE is the
/// unchanged occurrence cohort.
fn canonical_occurrence_events_cte_with_user(
    table: &AuditTableName,
    predicate: String,
    schema: &AuditSchema,
    with_user: bool,
) -> String {
    let (user_source, user_fact, user_column) = if with_user {
        (
            format!(
                ", {}",
                optional_source_projection(
                    schema,
                    AuditColumn::User,
                    "user",
                    "source_user",
                    "VARCHAR"
                )
            ),
            format!(
                ", CASE WHEN {agrees} THEN MAX(`source_user`) ELSE CAST(NULL AS VARCHAR) END AS `source_user`, CASE WHEN {agrees} THEN 1 ELSE 0 END AS `source_user_agrees`",
                agrees = optional_rows_agree_having("source_user")
            ),
            ", `source_user`, `source_user_agrees`".to_owned(),
        )
    } else {
        Default::default()
    };
    let error_code_source = optional_source_projection(
        schema,
        AuditColumn::ErrorCode,
        "errorCode",
        "error_code",
        "VARCHAR",
    );
    let pending_time_source = optional_source_projection(
        schema,
        AuditColumn::PendingTimeMs,
        "pendingTimeMs",
        "pending_time_ms",
        "BIGINT",
    );
    let error_code_agrees = optional_rows_agree_having("error_code");
    let pending_time_agrees = optional_rows_agree_having("pending_time_ms");
    format!(
        "WITH audit_window AS (SELECT LOWER(`queryId`) AS `query_id`, `timestamp` AS `event_time`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `returnRows` AS `return_rows`, `catalog` AS `catalog_name`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest`, {error_code_source}, {pending_time_source}{user_source} FROM {table}{predicate}), canonical_events AS (SELECT `query_id`, MAX(`event_time`) AS `event_time`, MAX(`state`) AS `state`, MAX(`query_time`) AS `query_time`, MAX(`scan_bytes`) AS `scan_bytes`, MAX(`scan_rows`) AS `scan_rows`, MAX(`cpu_cost_ns`) AS `cpu_cost_ns`, MAX(`mem_cost_bytes`) AS `mem_cost_bytes`, MAX(`return_rows`) AS `return_rows`, MAX(`catalog_name`) AS `catalog_name`, MAX(`database_name`) AS `database_name`, MAX(`resource_group`) AS `resource_group`, MAX(`digest`) AS `digest` FROM audit_window WHERE `query_id` REGEXP '^[0-9A-Fa-f]{{8}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{12}}$' GROUP BY `query_id` HAVING {base_agreement}), optional_facts AS (SELECT `query_id`, CASE WHEN {error_code_agrees} THEN MAX(`error_code`) ELSE CAST(NULL AS VARCHAR) END AS `error_code`, CASE WHEN {error_code_agrees} THEN 1 ELSE 0 END AS `error_code_agrees`, CASE WHEN {pending_time_agrees} THEN MAX(`pending_time_ms`) ELSE CAST(NULL AS BIGINT) END AS `pending_time_ms`, CASE WHEN {pending_time_agrees} THEN 1 ELSE 0 END AS `pending_time_agrees`{user_fact} FROM audit_window WHERE `query_id` REGEXP '^[0-9A-Fa-f]{{8}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{12}}$' GROUP BY `query_id`), occurrence_events AS (SELECT `canonical_events`.`query_id`, `event_time`, `state`, `query_time`, `scan_bytes`, `scan_rows`, `cpu_cost_ns`, `mem_cost_bytes`, `return_rows`, `catalog_name`, `database_name`, `resource_group`, `digest`, `error_code`, `error_code_agrees`, `pending_time_ms`, `pending_time_agrees`{user_column} FROM canonical_events INNER JOIN optional_facts ON `canonical_events`.`query_id` = `optional_facts`.`query_id`)",
        table = table.quoted(),
        base_agreement = projected_rows_agree_having(),
    )
}

fn optional_source_projection(
    schema: &AuditSchema,
    column: AuditColumn,
    source_name: &str,
    alias: &str,
    sql_type: &str,
) -> String {
    if schema.has(column) {
        format!("`{source_name}` AS `{alias}`")
    } else {
        format!("CAST(NULL AS {sql_type}) AS `{alias}`")
    }
}

fn optional_rows_agree_having(column: &str) -> String {
    format!(
        "COUNT(DISTINCT `{column}`) <= 1 AND (COUNT(`{column}`) = 0 OR COUNT(`{column}`) = COUNT(*))"
    )
}

fn canonical_events_cte_with_predicate(table: &AuditTableName, predicate: String) -> String {
    format!(
        "WITH audit_window AS (SELECT LOWER(`queryId`) AS `query_id`, `timestamp` AS `event_time`, `state` AS `state`, `queryTime` AS `query_time`, `scanBytes` AS `scan_bytes`, `scanRows` AS `scan_rows`, `cpuCostNs` AS `cpu_cost_ns`, `memCostBytes` AS `mem_cost_bytes`, `returnRows` AS `return_rows`, `catalog` AS `catalog_name`, `db` AS `database_name`, `resourceGroup` AS `resource_group`, `digest` AS `digest` FROM {}{}), canonical_events AS (SELECT `query_id`, MAX(`event_time`) AS `event_time`, MAX(`state`) AS `state`, MAX(`query_time`) AS `query_time`, MAX(`scan_bytes`) AS `scan_bytes`, MAX(`scan_rows`) AS `scan_rows`, MAX(`cpu_cost_ns`) AS `cpu_cost_ns`, MAX(`mem_cost_bytes`) AS `mem_cost_bytes`, MAX(`return_rows`) AS `return_rows`, MAX(`catalog_name`) AS `catalog_name`, MAX(`database_name`) AS `database_name`, MAX(`resource_group`) AS `resource_group`, MAX(`digest`) AS `digest` FROM audit_window WHERE `query_id` REGEXP '^[0-9A-Fa-f]{{8}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{4}}-[0-9A-Fa-f]{{12}}$' GROUP BY `query_id` HAVING {})",
        table.quoted(),
        predicate,
        projected_rows_agree_having(),
    )
}

fn projected_rows_agree_having() -> String {
    [
        "event_time",
        "state",
        "query_time",
        "scan_bytes",
        "scan_rows",
        "cpu_cost_ns",
        "mem_cost_bytes",
        "return_rows",
        "catalog_name",
        "database_name",
        "resource_group",
        "digest",
    ]
    .into_iter()
    .map(|column| {
        format!(
            "COUNT(DISTINCT `{column}`) <= 1 AND (COUNT(`{column}`) = 0 OR COUNT(`{column}`) = COUNT(*))"
        )
    })
    .collect::<Vec<_>>()
    .join(" AND ")
}

fn require_scope_columns(
    schema: &AuditSchema,
    scope: &AuditScope,
) -> Result<(), AuditSqlBuildError> {
    if scope.user.is_some() {
        schema.require(&[AuditColumn::User])?;
    }
    if scope.statement_filter.is_applied() && !schema.supports_statement_filter() {
        return Err(AuditSqlBuildError::StatementFilterUnavailable);
    }
    Ok(())
}

fn scope_predicate(scope: &AuditScope) -> String {
    scope_predicate_for_window(scope, scope.window.clone())
}

fn historical_scope_predicate(scope: &AuditScope) -> String {
    let source_window = audit_historical_source_window(scope);
    scope_predicate_for_window(scope, source_window)
}

fn scope_predicate_for_window(scope: &AuditScope, window: AuditWindow) -> String {
    let mut predicate = scope_field_predicate_for_window(scope, window);
    if scope.statement_filter == AuditStatementFilter::ExcludeDoctorAndNonQuery {
        predicate.push_str(" AND ");
        predicate.push_str(&statement_filter_retained_condition());
    }
    predicate
}

/// Rows kept by the explicit statement-class filter. A NULL `isQuery` is not a
/// proven query; a NULL `stmt` carries no marker and is therefore retained.
/// The connected discovery list reuses it so both surfaces hide the same rows.
pub(crate) fn statement_filter_retained_condition() -> String {
    format!(
        "`isQuery` = 1 AND (`stmt` IS NULL OR ({}))",
        DOCTOR_STATEMENT_MARKERS
            .map(|marker| format!("`stmt` NOT LIKE '{marker}%'"))
            .join(" AND ")
    )
}

/// Hidden bucket 1: not a proven query (SET/SHOW/DDL/DML or a NULL `isQuery`).
pub(crate) fn statement_filter_hidden_non_query_condition() -> &'static str {
    "(`isQuery` IS NULL OR `isQuery` <> 1)"
}

/// Hidden bucket 2: a proven query carrying this tool's statement marker.
/// Disjoint from bucket 1; together with the retained condition they cover
/// every row.
pub(crate) fn statement_filter_hidden_doctor_condition() -> String {
    format!(
        "`isQuery` = 1 AND ({})",
        DOCTOR_STATEMENT_MARKERS
            .map(|marker| format!("`stmt` LIKE '{marker}%'"))
            .join(" OR ")
    )
}

/// Window plus field filters, without the statement-class filter. It is the
/// base of the full scope and of the transparent hidden-row counts.
fn scope_field_predicate_for_window(scope: &AuditScope, window: AuditWindow) -> String {
    let mut predicate = format!(
        " WHERE `timestamp` >= FROM_UNIXTIME({}) AND `timestamp` < FROM_UNIXTIME({})",
        window.start_seconds, window.end_seconds
    );
    if let Some(state) = &scope.state {
        let source_state = match state {
            AuditTerminalState::Finished => "EOF",
            AuditTerminalState::Error => "ERR",
        };
        predicate.push_str(&format!(
            " AND `state` = {}",
            utf8_hex_literal(source_state)
        ));
    }
    if let Some(user) = &scope.user {
        predicate.push_str(&format!(" AND `user` = {}", utf8_hex_literal(user)));
    }
    if let Some(native_digest) = &scope.native_digest {
        predicate.push_str(&format!(
            " AND `digest` = {}",
            utf8_hex_literal(native_digest)
        ));
    }
    if let Some(catalog) = &scope.catalog {
        predicate.push_str(&format!(" AND `catalog` = {}", utf8_hex_literal(catalog)));
    }
    if let Some(database) = &scope.database {
        predicate.push_str(&format!(" AND `db` = {}", utf8_hex_literal(database)));
    }
    if let Some(resource_group) = &scope.resource_group {
        predicate.push_str(&format!(
            " AND `resourceGroup` = {}",
            utf8_hex_literal(resource_group)
        ));
    }
    predicate
}

fn time_tick_columns(scope: &AuditScope) -> String {
    let duration = scope.window.end_seconds - scope.window.start_seconds;
    let format = time_tick_label_format(scope);
    (0..=4)
        .map(|index| {
            let at = scope.window.start_seconds + duration * index / 4;
            format!("DATE_FORMAT(FROM_UNIXTIME({at}), '{format}') AS `tick_{index}`")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn time_tick_label_format(scope: &AuditScope) -> &'static str {
    if scope.window.end_seconds - scope.window.start_seconds >= 24 * 60 * 60 {
        "%m-%d %H:%i"
    } else {
        "%H:%i:%s"
    }
}

fn transition_point_label_format(scope: &AuditScope) -> &'static str {
    if scope.window.end_seconds - scope.window.start_seconds >= 24 * 60 * 60 {
        "%m-%d %H:%i:%s"
    } else {
        "%H:%i:%s"
    }
}

fn occurrence_order(sort: &AuditOccurrenceSort) -> &'static str {
    match sort {
        AuditOccurrenceSort::QueryTimeDesc => {
            "(`query_time` IS NULL) ASC, `query_time` DESC, `event_time` DESC, `query_id` ASC"
        }
        AuditOccurrenceSort::ScanBytesDesc => {
            "(`scan_bytes` IS NULL) ASC, `scan_bytes` DESC, `event_time` DESC, `query_id` ASC"
        }
        AuditOccurrenceSort::ScanRowsDesc => {
            "(`scan_rows` IS NULL) ASC, `scan_rows` DESC, `event_time` DESC, `query_id` ASC"
        }
        AuditOccurrenceSort::CpuCostDesc => {
            "(`cpu_cost_ns` IS NULL) ASC, `cpu_cost_ns` DESC, `event_time` DESC, `query_id` ASC"
        }
        AuditOccurrenceSort::MemoryCostDesc => {
            "(`mem_cost_bytes` IS NULL) ASC, `mem_cost_bytes` DESC, `event_time` DESC, `query_id` ASC"
        }
        AuditOccurrenceSort::NewestFirst => "`event_time` DESC, `query_id` ASC",
    }
}

fn family_order(sort: &AuditFamilySort) -> String {
    match sort {
        AuditFamilySort::CumulativeQueryTimeDesc => {
            "(`cumulative_query_time` IS NULL) ASC, `cumulative_query_time` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::CumulativeScanBytesDesc => {
            "(`cumulative_scan_bytes` IS NULL) ASC, `cumulative_scan_bytes` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::CumulativeCpuCostDesc => {
            "(`cumulative_cpu_cost` IS NULL) ASC, `cumulative_cpu_cost` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::ExecutionCountDesc => {
            "`canonical_execution_count` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::ErrorCountDesc => {
            "`error_count` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::ErrorProportionDesc => {
            format!(
                "CAST(LEAST(`error_count`, {ERROR_PROPORTION_EXACT_POPULATION_MAX}) AS DECIMAL(38,25)) / CAST(`canonical_execution_count` AS DECIMAL(38,0)) DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC"
            )
        }
        AuditFamilySort::MaxQueryTimeDesc => {
            "(`max_query_time` IS NULL) ASC, `max_query_time` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
        AuditFamilySort::MaxMemoryCostDesc => {
            "(`max_memory_cost` IS NULL) ASC, `max_memory_cost` DESC, `catalog_name` ASC, `database_name` ASC, `digest` ASC".into()
        }
    }
}

/// Uses a hex-only source literal, then a StarRocks-supported `UNHEX`/`VARCHAR`
/// conversion, so the emitted value never depends on a MySQL `sql_mode`
/// backslash-escape setting. `CONVERT(X'…' USING utf8)` is intentionally not
/// used: it is not accepted by the fixed 3.5.14 SQL grammar.
fn utf8_hex_literal(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0F) as usize] as char);
    }
    format!("CAST(UNHEX('{encoded}') AS VARCHAR)")
}

fn valid_query_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::{AuditWindow, MAX_AUDIT_FILTER_TEXT_BYTES, MAX_AUDIT_OCCURRENCE_LIMIT};

    fn scope() -> AuditScope {
        AuditScope {
            window: AuditWindow {
                start_seconds: 1,
                end_seconds: 86_401,
            },
            state: Some(AuditTerminalState::Finished),
            user: None,
            native_digest: None,
            catalog: None,
            database: None,
            resource_group: None,
            statement_filter: AuditStatementFilter::All,
        }
    }

    fn table() -> AuditTableName {
        AuditTableName::parse("starrocks_audit_db__.starrocks_audit_tbl__").unwrap()
    }

    #[test]
    fn oldest_slowest_oracle_orders_full_window_before_limit() {
        let statement = audit_occurrence_statement(
            &table(),
            &AuditOccurrenceRequest {
                scope: scope(),
                sort: AuditOccurrenceSort::QueryTimeDesc,
                limit: MAX_AUDIT_OCCURRENCE_LIMIT,
            },
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        let order = statement.find(" ORDER BY ").unwrap();
        let limit = statement.find(" LIMIT ").unwrap();
        assert!(
            order < limit,
            "full requested window must order before limit: {statement}"
        );
        assert!(statement.contains("FROM audit_window WHERE `query_id` REGEXP"));
        assert!(statement.contains("`query_time` DESC"));
        assert!(statement.contains("`timestamp` < FROM_UNIXTIME(86401)"));
        assert!(!statement.contains("LIMIT 500)"));
        assert!(statement.contains("GROUP BY `query_id` HAVING"));
    }

    #[test]
    fn family_sample_reads_one_bounded_prefix_per_named_family() {
        let target = AuditFamilyTarget {
            native_digest: "a0b1".into(),
            catalog: "default_catalog".into(),
            database: "demo".into(),
        };
        let schema = AuditSchema::official_3_5_14();
        let statement = audit_family_sample_statement(
            &table(),
            &scope(),
            std::slice::from_ref(&target),
            &schema,
            AUDIT_STATEMENT_SAMPLE_CHARS,
        )
        .unwrap();
        assert!(statement.contains("LEFT(`stmt`, 240) AS `statement_prefix`"));
        assert!(statement.contains("CHAR_LENGTH(`stmt`) > 240"));
        assert!(statement.contains("ROW_NUMBER() OVER (PARTITION BY `catalog_name`, `database_name`, `digest` ORDER BY `event_time` DESC, `query_id` ASC)"));
        assert!(statement.contains("WHERE `sample_rank` = 1"));
        assert!(statement.contains("`timestamp` < FROM_UNIXTIME("));
        // Identity values travel only as hex literals.
        assert!(!statement.contains("'demo'") && !statement.contains("a0b1'"));
        assert_eq!(
            audit_family_sample_statement(
                &table(),
                &scope(),
                &[],
                &schema,
                AUDIT_STATEMENT_SAMPLE_CHARS
            )
            .unwrap_err(),
            AuditSqlBuildError::FamilyTarget
        );
        let too_many = vec![target; usize::from(AUDIT_FAMILY_SAMPLE_TARGET_LIMIT) + 1];
        assert_eq!(
            audit_family_sample_statement(
                &table(),
                &scope(),
                &too_many,
                &schema,
                AUDIT_STATEMENT_SAMPLE_CHARS
            )
            .unwrap_err(),
            AuditSqlBuildError::FamilyTarget
        );
    }

    #[test]
    fn family_sample_rows_keep_null_and_truncation_distinct() {
        let row = |prefix: Option<&str>, null: &str, truncated: &str| {
            vec![
                Some("default_catalog".into()),
                Some("demo".into()),
                Some("a0b1".into()),
                prefix.map(str::to_owned),
                Some(null.into()),
                Some(truncated.into()),
            ]
        };
        assert_eq!(
            decode_family_sample_row(row(Some("SELECT 1"), "0", "1"))
                .unwrap()
                .sample,
            AuditStatementSampleSource::Text {
                value: "SELECT 1".into(),
                truncated: true
            }
        );
        assert_eq!(
            decode_family_sample_row(row(None, "1", "0"))
                .unwrap()
                .sample,
            AuditStatementSampleSource::SqlNull
        );
        // An empty audited statement is text, not SQL NULL.
        assert_eq!(
            decode_family_sample_row(row(Some(""), "0", "0"))
                .unwrap()
                .sample,
            AuditStatementSampleSource::Text {
                value: String::new(),
                truncated: false
            }
        );
        assert!(decode_family_sample_row(row(None, "0", "0")).is_err());
        assert!(decode_family_sample_row(row(Some("x"), "2", "0")).is_err());
        let mut rows = vec![
            decode_family_source_row(vec![
                Some("default_catalog".into()),
                Some("demo".into()),
                Some("a0b1".into()),
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
            ])
            .unwrap(),
        ];
        attach_family_samples(
            &mut rows,
            vec![decode_family_sample_row(row(Some("SELECT 1"), "0", "0")).unwrap()],
        );
        assert_eq!(
            rows[0].statement_sample,
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: "SELECT 1".into(),
                truncated: false
            })
        );
    }

    fn sample_id(index: usize) -> String {
        format!("00000000-0000-0000-0000-{index:012x}")
    }

    #[test]
    fn occurrence_sample_reads_only_named_ids_in_the_rows_own_range() {
        let schema = AuditSchema::official_3_5_14();
        let ids = vec![sample_id(1), sample_id(2)];
        let statement = audit_occurrence_sample_statement(
            &table(),
            &scope(),
            AuditOccurrenceSampleWindow::Scope,
            &ids,
            &schema,
        )
        .unwrap();
        assert!(statement.contains("MAX(LEFT(`stmt`, 240)) AS `statement_prefix`"));
        assert!(statement.contains("CHAR_LENGTH(`stmt`) > 240"));
        // Duplicate rows must agree on the full text, not just the prefix.
        assert!(statement.contains(
            "COUNT(DISTINCT `stmt`) <= 1 AND (COUNT(`stmt`) = 0 OR COUNT(`stmt`) = COUNT(*))"
        ));
        assert!(statement.contains("GROUP BY LOWER(`queryId`)"));
        assert!(statement.contains("`timestamp` >= FROM_UNIXTIME(1)"));
        assert!(statement.contains("`timestamp` < FROM_UNIXTIME(86401)"));
        assert!(statement.contains("`state` = "));
        assert!(!statement.contains("LIMIT") && !statement.contains(';'));
        // Ids travel only as hex literals.
        assert!(!statement.contains(&ids[0]));
        let historical = audit_occurrence_sample_statement(
            &table(),
            &scope(),
            AuditOccurrenceSampleWindow::HistoricalLookback,
            &ids,
            &schema,
        )
        .unwrap();
        assert!(historical.contains("`timestamp` >= FROM_UNIXTIME(0)"));
        assert!(historical.contains("`timestamp` < FROM_UNIXTIME(86401)"));
        let mut filtered = scope();
        filtered.statement_filter = AuditStatementFilter::ExcludeDoctorAndNonQuery;
        assert!(
            audit_occurrence_sample_statement(
                &table(),
                &filtered,
                AuditOccurrenceSampleWindow::Scope,
                &ids,
                &schema,
            )
            .unwrap()
            .contains(&statement_filter_retained_condition())
        );

        let build = |ids: &[String]| {
            audit_occurrence_sample_statement(
                &table(),
                &scope(),
                AuditOccurrenceSampleWindow::Scope,
                ids,
                &schema,
            )
        };
        let full = (0..usize::from(AUDIT_OCCURRENCE_SAMPLE_TARGET_LIMIT))
            .map(sample_id)
            .collect::<Vec<_>>();
        assert!(build(&full).unwrap().len() <= crate::MAX_PLAN_SQL_BYTES);
        let mut too_many = full;
        too_many.push(sample_id(9999));
        for invalid in [
            vec![],
            too_many,
            vec![sample_id(1), sample_id(1)],
            vec![sample_id(1).to_uppercase().replace('0', "A")],
            vec!["not-a-query-id".into()],
        ] {
            assert_eq!(build(&invalid).unwrap_err(), AuditSqlBuildError::QueryId);
        }
        let without_stmt = AuditSchema::new(
            AuditSchema::official_3_5_14()
                .columns
                .into_iter()
                .filter(|column| *column != AuditColumn::Statement),
        );
        assert!(
            audit_occurrence_sample_statement(
                &table(),
                &scope(),
                AuditOccurrenceSampleWindow::Scope,
                &ids,
                &without_stmt,
            )
            .is_err()
        );
    }

    #[test]
    fn occurrence_sample_rows_keep_null_conflict_and_truncation_distinct() {
        let row = |prefix: Option<&str>, null: &str, truncated: &str, agrees: &str| {
            vec![
                Some(sample_id(1)),
                prefix.map(str::to_owned),
                Some(null.into()),
                Some(truncated.into()),
                Some(agrees.into()),
            ]
        };
        let decode = |row| decode_occurrence_sample_row(row).map(|row| row.sample);
        assert_eq!(
            decode(row(Some("SELECT 1"), "0", "1", "1")).unwrap(),
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: "SELECT 1".into(),
                truncated: true
            })
        );
        assert_eq!(
            decode(row(None, "1", "0", "1")).unwrap(),
            AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull)
        );
        // An empty audited statement is text, not SQL NULL.
        assert_eq!(
            decode(row(Some(""), "0", "0", "1")).unwrap(),
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value: String::new(),
                truncated: false
            })
        );
        assert_eq!(
            decode(row(Some("SELECT 1"), "1", "0", "0")).unwrap(),
            AuditSourceStatementSample::ConflictingSourceRows
        );
        assert!(decode(row(None, "0", "0", "1")).is_err());
        assert!(decode(row(Some("x"), "0", "0", "2")).is_err());
        assert!(decode(row(Some("x"), "0", "0", "1")[..4].to_vec()).is_err());
        let mut upper = row(Some("x"), "0", "0", "1");
        upper[0] = Some("0000000A-0000-0000-0000-000000000001".into());
        assert!(decode(upper).is_err());
    }

    #[test]
    fn occurrence_samples_attach_by_canonical_id_and_leave_unmatched_rows_not_read() {
        let occurrence = |id: &str| {
            let mut cells = vec![Some(id.to_owned())];
            cells.extend(std::iter::repeat_n(None, 13));
            cells.extend([None, Some("1".into()), None, Some("1".into())]);
            decode_occurrence_source_row(cells, &AuditSchema::official_3_5_14()).unwrap()
        };
        let mut rows = vec![
            occurrence(&sample_id(1)),
            occurrence(&sample_id(2)),
            occurrence("bad"),
        ];
        assert!(
            rows.iter()
                .all(|row| row.statement_sample == AuditSourceStatementSample::NotRead)
        );
        assert_eq!(
            occurrence_sample_targets(&rows),
            vec![sample_id(1), sample_id(2)]
        );
        attach_occurrence_samples(
            &mut rows,
            vec![AuditOccurrenceSampleRow {
                query_id: sample_id(2),
                sample: AuditSourceStatementSample::ConflictingSourceRows,
            }],
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.statement_sample.clone())
                .collect::<Vec<_>>(),
            vec![
                AuditSourceStatementSample::NotRead,
                AuditSourceStatementSample::ConflictingSourceRows,
                AuditSourceStatementSample::NotRead,
            ]
        );
    }

    #[test]
    fn family_groups_and_sorts_full_window_before_limit() {
        let statement = audit_family_statement(
            &table(),
            &AuditFamilyRequest {
                scope: scope(),
                sort: AuditFamilySort::CumulativeQueryTimeDesc,
                limit: 20,
            },
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        let group = statement.rfind(" GROUP BY ").unwrap();
        let order = statement.find(" ORDER BY ").unwrap();
        let limit = statement.find(" LIMIT ").unwrap();
        assert!(group < order && order < limit, "{statement}");
        assert!(statement.contains("GROUP BY `catalog_name`, `database_name`, `digest`"));
        assert!(statement.contains("COUNT(*) AS `canonical_execution_count`"));
        assert!(statement.contains(
            "SUM(CASE WHEN `query_time` >= 0 THEN 1 ELSE 0 END) AS `query_time_valid_count`"
        ));
        assert!(statement.contains("SUM(CASE WHEN `query_time` >= 0 THEN `query_time` ELSE NULL END) AS `cumulative_query_time`"));
        assert!(statement.contains("`digest` REGEXP '^[0-9A-Fa-f]{1,128}$'"));
        assert!(statement.contains("family_aggregates AS"));
        assert!(statement.contains("family_rankable AS"));
        assert!(statement.contains(
            "MAX(`canonical_execution_count`) OVER () AS `error_proportion_population_max`"
        ));
    }

    #[test]
    fn error_proportion_order_is_decimal_bounded_and_population_first() {
        let statement = audit_family_statement(
            &table(),
            &AuditFamilyRequest {
                scope: scope(),
                sort: AuditFamilySort::ErrorProportionDesc,
                limit: 20,
            },
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        let aggregate = statement.find("family_aggregates AS").unwrap();
        let rankable = statement.find("family_rankable AS").unwrap();
        let order = statement.find(" ORDER BY ").unwrap();
        let limit = statement.find(" LIMIT ").unwrap();
        assert!(
            aggregate < rankable && rankable < order && order < limit,
            "{statement}"
        );
        assert!(statement.contains(
            "CAST(LEAST(`error_count`, 3000000000000) AS DECIMAL(38,25)) / CAST(`canonical_execution_count` AS DECIMAL(38,0)) DESC"
        ));
        assert!(
            statement.contains("`catalog_name` ASC, `database_name` ASC, `digest` ASC LIMIT 20")
        );
        assert!(!statement.contains("DOUBLE"));
        assert!(!statement.contains("JOIN family_aggregates"));
    }

    #[test]
    fn filters_use_utf8_hex_and_bounds_without_mysql_backslash_assumptions() {
        let mut filters = scope();
        filters.database = Some("warehouse\\' OR 1=1 --".into());
        filters.user = Some("reader\\' OR 1=1 --".into());
        let mut user_schema = AuditSchema::official_3_5_14();
        user_schema.columns.insert(AuditColumn::User);
        user_schema
            .source_types
            .insert(AuditColumn::User, "VARCHAR".into());
        let occurrence = AuditOccurrenceRequest {
            scope: filters.clone(),
            sort: AuditOccurrenceSort::NewestFirst,
            limit: 1,
        };
        let statement = audit_occurrence_statement(&table(), &occurrence, &user_schema).unwrap();
        assert!(
            statement.contains(
                "`user` = CAST(UNHEX('7265616465725C27204F5220313D31202D2D') AS VARCHAR)"
            )
        );
        assert!(statement.contains(
            "`db` = CAST(UNHEX('77617265686F7573655C27204F5220313D31202D2D') AS VARCHAR)"
        ));
        assert!(!statement.contains("reader\\' OR 1=1 --"));
        assert!(!statement.contains("warehouse\\' OR 1=1 --"));
        assert!(!statement.contains("'reader"));
        assert!(!statement.contains("'warehouse"));
        assert!(
            audit_family_statement(
                &table(),
                &AuditFamilyRequest {
                    scope: filters.clone(),
                    sort: AuditFamilySort::ExecutionCountDesc,
                    limit: 1,
                },
                &user_schema,
            )
            .unwrap()
            .contains("`user` = CAST(UNHEX('7265616465725C27204F5220313D31202D2D') AS VARCHAR)")
        );
        assert!(
            audit_coverage_statement(&table(), &filters, &user_schema)
                .unwrap()
                .contains(
                    "`user` = CAST(UNHEX('7265616465725C27204F5220313D31202D2D') AS VARCHAR)"
                )
        );
        assert!(
            audit_sql_lookup_statement(
                &table(),
                &AuditSqlLookupRequest {
                    scope: filters.clone(),
                    query_id: "01234567-89ab-4abc-8def-0123456789ab".into(),
                },
                &user_schema,
            )
            .unwrap()
            .contains("`user` = CAST(UNHEX('7265616465725C27204F5220313D31202D2D') AS VARCHAR)")
        );
        assert!(matches!(
            audit_occurrence_statement(&table(), &occurrence, &AuditSchema::official_3_5_14()),
            Err(AuditSqlBuildError::SchemaMissing("user"))
        ));
        assert!(matches!(
            AuditTableName::parse("audit; DROP TABLE audit"),
            Err(AuditSqlBuildError::TableName)
        ));
        assert!(matches!(
            audit_occurrence_statement(
                &table(),
                &AuditOccurrenceRequest {
                    scope: scope(),
                    sort: AuditOccurrenceSort::NewestFirst,
                    limit: MAX_AUDIT_OCCURRENCE_LIMIT + 1,
                },
                &AuditSchema::official_3_5_14(),
            ),
            Err(AuditSqlBuildError::Request(
                AuditWorkloadValidationError::OccurrenceLimit
            ))
        ));
        let mut oversized_filter_scope = scope();
        oversized_filter_scope.database = Some("a".repeat(MAX_AUDIT_FILTER_TEXT_BYTES + 1));
        assert!(matches!(
            audit_occurrence_statement(
                &table(),
                &AuditOccurrenceRequest {
                    scope: oversized_filter_scope,
                    sort: AuditOccurrenceSort::NewestFirst,
                    limit: 1,
                },
                &AuditSchema::official_3_5_14(),
            ),
            Err(AuditSqlBuildError::Request(
                AuditWorkloadValidationError::FilterText
            ))
        ));
        let mut invalid_digest_scope = scope();
        invalid_digest_scope.native_digest = Some("a0b1' OR 1=1 --".into());
        assert!(matches!(
            audit_occurrence_statement(
                &table(),
                &AuditOccurrenceRequest {
                    scope: invalid_digest_scope,
                    sort: AuditOccurrenceSort::NewestFirst,
                    limit: 1,
                },
                &AuditSchema::official_3_5_14(),
            ),
            Err(AuditSqlBuildError::Request(
                AuditWorkloadValidationError::NativeDigest
            ))
        ));
        let valid_query_id = "01234567-89ab-4abc-8def-0123456789ab";
        assert!(
            AuditSqlLookupRequest {
                scope: scope(),
                query_id: valid_query_id.into(),
            }
            .validate()
            .is_ok()
        );
        for position in [8, 13, 18, 23] {
            let mut malformed = valid_query_id.to_owned();
            malformed.replace_range(position..=position, "a");
            assert!(matches!(
                AuditSqlLookupRequest {
                    scope: scope(),
                    query_id: malformed,
                }
                .validate(),
                Err(AuditSqlBuildError::QueryId)
            ));
        }
    }

    #[test]
    fn statement_filter_is_one_shared_predicate_with_transparent_hidden_counts() {
        const RETAINED: &str = " AND `isQuery` = 1 AND (`stmt` IS NULL OR (`stmt` NOT LIKE '/* sr-doctor */ %' AND `stmt` NOT LIKE '/* starrocks-doctor */ %'))";
        const FIELDS: &str = " WHERE `timestamp` >= FROM_UNIXTIME(1) AND `timestamp` < FROM_UNIXTIME(86401) AND `state` = CAST(UNHEX('454F46') AS VARCHAR)";
        // The marker is a literal LIKE prefix: no wildcard, quote, escape, or
        // double space that `LogUtil.removeLineSeparator` would collapse.
        for marker in DOCTOR_STATEMENT_MARKERS {
            assert!(!marker.contains(['%', '_', '\'', '\\']) && !marker.contains("  "));
        }
        assert_eq!(doctor_statement("SELECT 1"), "/* sr-doctor */ SELECT 1");
        let mut filtered = scope();
        filtered.statement_filter = AuditStatementFilter::ExcludeDoctorAndNonQuery;
        let schema = AuditSchema::official_3_5_14();
        assert!(schema.supports_statement_filter());
        assert_eq!(scope_predicate(&filtered), format!("{FIELDS}{RETAINED}"));
        let occurrence = audit_occurrence_statement(
            &table(),
            &AuditOccurrenceRequest {
                scope: filtered.clone(),
                sort: AuditOccurrenceSort::QueryTimeDesc,
                limit: 50,
            },
            &schema,
        )
        .unwrap();
        assert!(occurrence.contains(&format!(
            "FROM `starrocks_audit_db__`.`starrocks_audit_tbl__`{FIELDS}{RETAINED})"
        )));
        let family = audit_family_statement(
            &table(),
            &AuditFamilyRequest {
                scope: filtered.clone(),
                sort: AuditFamilySort::ExecutionCountDesc,
                limit: 10,
            },
            &schema,
        )
        .unwrap();
        assert!(family.contains(RETAINED));
        assert!(
            audit_concurrency_statement(&table(), &filtered, &schema)
                .unwrap()
                .contains(RETAINED)
        );
        assert!(
            audit_sql_lookup_statement(
                &table(),
                &AuditSqlLookupRequest {
                    scope: filtered.clone(),
                    query_id: "01234567-89ab-4abc-8def-0123456789ab".into(),
                },
                &schema,
            )
            .unwrap()
            .contains(RETAINED)
        );
        let coverage = audit_coverage_statement(&table(), &filtered, &schema).unwrap();
        assert!(coverage.contains(&format!("{FIELDS}{RETAINED})")));
        // Hidden buckets use the same window and field filters, not the
        // statement filter, and are disjoint by `isQuery`.
        assert!(coverage.ends_with(&format!(
            "COALESCE(SUM(CASE WHEN `state` = CAST(UNHEX('455252') AS VARCHAR) THEN 1 ELSE 0 END), 0) AS `error_count`, (SELECT COUNT(*) FROM `starrocks_audit_db__`.`starrocks_audit_tbl__`{FIELDS} AND (`isQuery` IS NULL OR `isQuery` <> 1)) AS `hidden_non_query_row_count`, (SELECT COUNT(*) FROM `starrocks_audit_db__`.`starrocks_audit_tbl__`{FIELDS} AND `isQuery` = 1 AND (`stmt` LIKE '/* sr-doctor */ %' OR `stmt` LIKE '/* starrocks-doctor */ %')) AS `hidden_doctor_row_count` FROM canonical_events"
        )));

        // `all` is a real choice: no statement predicate and typed NULL counts.
        let all = audit_coverage_statement(&table(), &scope(), &schema).unwrap();
        assert!(!all.contains("`isQuery`") && !all.contains("doctor */"));
        assert!(all.ends_with("CAST(NULL AS BIGINT) AS `hidden_non_query_row_count`, CAST(NULL AS BIGINT) AS `hidden_doctor_row_count` FROM canonical_events"));

        // A requested filter never degrades to an unfiltered read.
        let mut without_is_query = schema.clone();
        without_is_query.columns.remove(&AuditColumn::IsQuery);
        let mut text_is_query = schema.clone();
        text_is_query
            .source_types
            .insert(AuditColumn::IsQuery, "VARCHAR(8)".into());
        let mut without_statement = schema.clone();
        without_statement.columns.remove(&AuditColumn::Statement);
        for unsupported in [without_is_query, text_is_query, without_statement] {
            assert!(!unsupported.supports_statement_filter());
            assert!(matches!(
                audit_coverage_statement(&table(), &filtered, &unsupported),
                Err(AuditSqlBuildError::StatementFilterUnavailable)
            ));
            assert!(audit_coverage_statement(&table(), &scope(), &unsupported).is_ok());
        }
        let probed = AuditSchema::from_schema_probe_rows(&[
            vec![Some("isQuery".into()), Some("tinyint".into())],
            vec![Some("stmt".into()), Some("varchar(1048576)".into())],
        ])
        .unwrap();
        assert!(probed.supports_statement_filter());
    }

    #[test]
    fn missing_schema_column_fails_before_sql_is_built() {
        let schema = AuditSchema::new([
            AuditColumn::QueryId,
            AuditColumn::Timestamp,
            AuditColumn::State,
        ]);
        assert_eq!(
            audit_occurrence_statement(
                &table(),
                &AuditOccurrenceRequest {
                    scope: scope(),
                    sort: AuditOccurrenceSort::NewestFirst,
                    limit: 1,
                },
                &schema,
            )
            .unwrap_err(),
            AuditSqlBuildError::SchemaMissing("queryTime")
        );

        let schema_without_catalog = AuditSchema::new([
            AuditColumn::QueryId,
            AuditColumn::Timestamp,
            AuditColumn::State,
            AuditColumn::QueryTime,
            AuditColumn::ScanBytes,
            AuditColumn::ScanRows,
            AuditColumn::CpuCostNs,
            AuditColumn::MemCostBytes,
            AuditColumn::ReturnRows,
            AuditColumn::Database,
            AuditColumn::ResourceGroup,
            AuditColumn::Digest,
            AuditColumn::Statement,
        ]);
        assert_eq!(
            audit_family_statement(
                &table(),
                &AuditFamilyRequest {
                    scope: scope(),
                    sort: AuditFamilySort::ExecutionCountDesc,
                    limit: 1,
                },
                &schema_without_catalog,
            )
            .unwrap_err(),
            AuditSqlBuildError::SchemaMissing("catalog")
        );
    }

    #[test]
    fn schema_probe_keeps_optional_statement_separate_from_required_columns() {
        let fields = [
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
            "future_optional_column",
        ]
        .into_iter()
        .map(|field| {
            let source_type = match field {
                "timestamp" => " DATETIME ",
                "queryTime" | "scanBytes" | "scanRows" | "cpuCostNs" | "memCostBytes"
                | "returnRows" => " bigint ",
                _ => "VARCHAR",
            };
            vec![Some(field.to_owned()), Some(source_type.into())]
        })
        .collect::<Vec<_>>();
        let schema = AuditSchema::from_schema_probe_rows(&fields).unwrap();
        assert!(schema.has(AuditColumn::QueryId));
        assert!(!schema.has(AuditColumn::Statement));
        let occurrence = audit_occurrence_statement(
            &table(),
            &AuditOccurrenceRequest {
                scope: scope(),
                sort: AuditOccurrenceSort::NewestFirst,
                limit: 1,
            },
            &schema,
        )
        .unwrap();
        assert!(occurrence.contains("CAST(NULL AS VARCHAR) AS `error_code`"));
        assert!(occurrence.contains("CAST(NULL AS BIGINT) AS `pending_time_ms`"));
        assert!(!occurrence.contains("`errorCode` AS `error_code`"));
        assert!(!occurrence.contains("`pendingTimeMs` AS `pending_time_ms`"));
        assert_eq!(
            audit_sql_lookup_statement(
                &table(),
                &AuditSqlLookupRequest {
                    scope: scope(),
                    query_id: "01234567-89ab-4abc-8def-0123456789ab".into(),
                },
                &schema,
            )
            .unwrap_err(),
            AuditSqlBuildError::SchemaMissing("stmt")
        );
    }

    #[test]
    fn ordinary_workload_requires_probed_temporal_and_numeric_source_types() {
        let occurrence = AuditOccurrenceRequest {
            scope: scope(),
            sort: AuditOccurrenceSort::NewestFirst,
            limit: 1,
        };
        let family = AuditFamilyRequest {
            scope: scope(),
            sort: AuditFamilySort::ExecutionCountDesc,
            limit: 1,
        };
        let expected = AuditSqlBuildError::SchemaTypeUnsupported;
        for (column, unsupported) in [
            (AuditColumn::Timestamp, "VARCHAR"),
            (AuditColumn::QueryTime, "VARCHAR"),
            (AuditColumn::ScanBytes, "DOUBLE"),
            (AuditColumn::ScanRows, "DECIMAL(20,0)"),
            (AuditColumn::CpuCostNs, "BIGINT UNSIGNED"),
            (AuditColumn::MemCostBytes, "unknown"),
        ] {
            let mut schema = AuditSchema::official_3_5_14();
            schema.source_types.insert(column, unsupported.into());
            assert!(!schema.supports_base_metric_workload(), "{column:?}");
            assert_eq!(
                audit_occurrence_statement(&table(), &occurrence, &schema).unwrap_err(),
                expected,
                "occurrence {column:?}"
            );
            assert_eq!(
                audit_family_statement(&table(), &family, &schema).unwrap_err(),
                expected,
                "family {column:?}"
            );
            assert_eq!(
                audit_coverage_statement(&table(), &scope(), &schema).unwrap_err(),
                expected,
                "coverage {column:?}"
            );
        }

        let mut normalized = AuditSchema::official_3_5_14();
        normalized
            .source_types
            .insert(AuditColumn::Timestamp, " datetime ".into());
        for column in [
            AuditColumn::QueryTime,
            AuditColumn::ScanBytes,
            AuditColumn::ScanRows,
            AuditColumn::CpuCostNs,
            AuditColumn::MemCostBytes,
        ] {
            normalized.source_types.insert(column, " bigint ".into());
        }
        assert!(normalized.supports_base_metric_workload());
        assert!(audit_occurrence_statement(&table(), &occurrence, &normalized).is_ok());

        let mut unknown = AuditSchema::official_3_5_14();
        unknown.source_types.remove(&AuditColumn::ScanRows);
        assert_eq!(
            audit_occurrence_statement(&table(), &occurrence, &unknown).unwrap_err(),
            AuditSqlBuildError::SchemaTypeUnsupported
        );

        let mut lossless = AuditSchema::official_3_5_14();
        for column in [
            AuditColumn::ReturnRows,
            AuditColumn::ErrorCode,
            AuditColumn::PendingTimeMs,
        ] {
            lossless.source_types.insert(column, "VARCHAR".into());
        }
        assert!(lossless.supports_base_metric_workload());
        assert!(audit_occurrence_statement(&table(), &occurrence, &lossless).is_ok());
    }

    #[test]
    fn coverage_is_full_window_canonical_and_keeps_aggregate_input_nullable() {
        let statement =
            audit_coverage_statement(&table(), &scope(), &AuditSchema::official_3_5_14()).unwrap();
        assert!(!statement.contains(" LIMIT "));
        assert!(statement.contains("canonical_events AS"));
        assert!(statement.contains("`retained_event_count`"));
        assert!(statement.contains("`retained_family_count`"));
        assert!(statement.contains(
            "SUM(CASE WHEN `query_time` >= 0 THEN `query_time` ELSE NULL END) AS `cumulative_query_time`"
        ));
        assert!(statement.contains(
            "MAX(CASE WHEN `mem_cost_bytes` >= 0 THEN `mem_cost_bytes` ELSE NULL END) AS `max_memory_cost`"
        ));
        assert!(statement.contains("`timestamp` < FROM_UNIXTIME(86401)"));
    }

    #[test]
    fn coverage_keeps_empty_context_sentinels_separate_from_error_state() {
        let statement =
            audit_coverage_statement(&table(), &scope(), &AuditSchema::official_3_5_14()).unwrap();
        assert!(statement.contains(
            "`catalog_name` IS NULL OR `catalog_name` = CAST(UNHEX('') AS VARCHAR) OR LENGTH(`catalog_name`) > 256"
        ));
        assert!(statement.contains(
            "SUM(CASE WHEN `state` = CAST(UNHEX('455252') AS VARCHAR) THEN 1 ELSE 0 END), 0) AS `error_count`"
        ));
    }

    #[test]
    fn concurrency_classifies_null_state_as_non_terminal() {
        let statement =
            audit_concurrency_statement(&table(), &scope(), &AuditSchema::official_3_5_14())
                .unwrap();
        assert!(statement.contains(
            "WHEN `state` IS NULL OR (`state` <> CAST(UNHEX('454F46') AS VARCHAR) AND `state` <> CAST(UNHEX('455252') AS VARCHAR)) THEN 'non_terminal'"
        ));
    }

    #[test]
    fn historical_source_is_sargable_bounded_and_shared_by_line_drill_and_sql_lookup() {
        let mut bounded_scope = scope();
        bounded_scope.window = AuditWindow {
            start_seconds: 7_200,
            end_seconds: 10_800,
        };
        let schema = AuditSchema::official_3_5_14();
        let public_time =
            "WHERE `timestamp` >= FROM_UNIXTIME(7200) AND `timestamp` < FROM_UNIXTIME(10800)";
        let historical_time =
            "WHERE `timestamp` >= FROM_UNIXTIME(3600) AND `timestamp` < FROM_UNIXTIME(10800)";
        let occurrence = audit_occurrence_statement(
            &table(),
            &AuditOccurrenceRequest {
                scope: bounded_scope.clone(),
                sort: AuditOccurrenceSort::NewestFirst,
                limit: 1,
            },
            &schema,
        )
        .unwrap();
        let family = audit_family_statement(
            &table(),
            &AuditFamilyRequest {
                scope: bounded_scope.clone(),
                sort: AuditFamilySort::ExecutionCountDesc,
                limit: 1,
            },
            &schema,
        )
        .unwrap();
        let coverage = audit_coverage_statement(&table(), &bounded_scope, &schema).unwrap();
        let lookup = audit_sql_lookup_statement(
            &table(),
            &AuditSqlLookupRequest {
                scope: bounded_scope.clone(),
                query_id: "01234567-89ab-4abc-8def-0123456789ab".into(),
            },
            &schema,
        )
        .unwrap();
        let concurrency = audit_concurrency_statement(&table(), &bounded_scope, &schema).unwrap();
        let active_at =
            audit_active_at_statement(&table(), &bounded_scope, 7_200_000, &schema).unwrap();
        let active_lookup = audit_active_at_sql_lookup_statement(
            &table(),
            &AuditSqlLookupRequest {
                scope: bounded_scope,
                query_id: "01234567-89ab-4abc-8def-0123456789ab".into(),
            },
            7_200_000,
            &schema,
        )
        .unwrap();

        for statement in [&occurrence, &family, &coverage, &lookup] {
            assert!(statement.contains(public_time), "{statement}");
        }
        for statement in [&concurrency, &active_at, &active_lookup] {
            assert!(statement.contains(historical_time), "{statement}");
            assert!(!statement.contains("WHERE `timestamp` < FROM_UNIXTIME"));
            assert!(!statement.contains("UNIX_TIMESTAMP(`timestamp`)"));
        }
    }

    #[test]
    fn historical_source_lookback_saturates_without_claiming_a_shorter_configuration() {
        let source = audit_historical_source_window(&scope());
        assert_eq!(source.start_seconds, 0);
        assert_eq!(source.end_seconds, 86_401);
        assert_eq!(AUDIT_HISTORICAL_LOOKBACK_SECONDS, 3_600);
    }

    #[test]
    fn historical_axis_labels_include_dates_for_one_day_and_thirty_day_windows() {
        let schema = AuditSchema::official_3_5_14();
        for duration in [24 * 60 * 60, 30 * 24 * 60 * 60] {
            let mut long_scope = scope();
            long_scope.window = AuditWindow {
                start_seconds: 1,
                end_seconds: 1 + duration,
            };
            let statement = audit_concurrency_statement(&table(), &long_scope, &schema).unwrap();
            assert!(statement.contains("'%m-%d %H:%i') AS `tick_0`"));
            assert!(statement.contains("'%m-%d %H:%i:%s') AS `point_label`"));
        }
        let mut short_scope = scope();
        short_scope.window = AuditWindow {
            start_seconds: 1,
            end_seconds: 3_601,
        };
        let statement = audit_concurrency_statement(&table(), &short_scope, &schema).unwrap();
        assert!(statement.contains("'%H:%i:%s') AS `tick_0`"));
        assert!(statement.contains("'%H:%i:%s') AS `point_label`"));
    }

    #[test]
    fn nullable_source_row_never_conflates_null_and_zero() {
        let decoded = decode_occurrence_source_row(
            vec![
                Some("a".into()),
                Some("2026-09-05 00:00:00".into()),
                Some("1".into()),
                Some("EOF".into()),
                None,
                Some("0".into()),
                Some("0".into()),
                Some("9007199254740992".into()),
                Some("-1".into()),
                Some("0".into()),
                Some("internal".into()),
                Some("db".into()),
                None,
                None,
                Some("".into()),
                Some("1".into()),
                Some("-1".into()),
                Some("1".into()),
            ],
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        assert_eq!(decoded.metrics[0].1, AuditMetricInput::SqlNull);
        assert_eq!(
            decoded.metrics[6].1,
            AuditMetricInput::Text { value: "-1".into() }
        );
        assert_eq!(
            decoded.metrics[1].1,
            AuditMetricInput::Text { value: "0".into() }
        );
        assert_eq!(
            decoded.error_code,
            AuditErrorCodeSource::Text { value: "".into() }
        );
        assert_eq!(decoded.native_digest, None);
    }

    #[test]
    fn optional_facts_preserve_base_cohort_and_typed_source_states() {
        let request = AuditOccurrenceRequest {
            scope: scope(),
            sort: AuditOccurrenceSort::NewestFirst,
            limit: 1,
        };
        let statement =
            audit_occurrence_statement(&table(), &request, &AuditSchema::official_3_5_14())
                .unwrap();
        let canonical_start = statement.find("canonical_events AS").unwrap();
        let optional_start = statement.find("optional_facts AS").unwrap();
        let canonical = &statement[canonical_start..optional_start];
        assert!(!canonical.contains("error_code"), "{canonical}");
        assert!(!canonical.contains("pending_time_ms"), "{canonical}");
        assert!(statement.contains("`errorCode` AS `error_code`"));
        assert!(statement.contains("`pendingTimeMs` AS `pending_time_ms`"));
        assert!(statement.contains(
            "CASE WHEN COUNT(DISTINCT `error_code`) <= 1 AND (COUNT(`error_code`) = 0 OR COUNT(`error_code`) = COUNT(*)) THEN 1 ELSE 0 END AS `error_code_agrees`"
        ));
        assert!(statement.contains(
            "CASE WHEN COUNT(DISTINCT `pending_time_ms`) <= 1 AND (COUNT(`pending_time_ms`) = 0 OR COUNT(`pending_time_ms`) = COUNT(*)) THEN 1 ELSE 0 END AS `pending_time_agrees`"
        ));

        let base = vec![
            Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into()),
            Some("2026-09-05 00:00:00".into()),
            Some("1".into()),
            Some("EOF".into()),
            Some("10".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("internal".into()),
            Some("db".into()),
            None,
            Some("a0b1".into()),
        ];
        let missing_schema = AuditSchema::new([
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
        ]);
        let mut missing = base.clone();
        missing.extend([None, Some("1".into()), None, Some("1".into())]);
        let missing = decode_occurrence_source_row(missing, &missing_schema).unwrap();
        assert_eq!(
            missing.error_code,
            AuditErrorCodeSource::Unavailable {
                reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
            }
        );
        assert_eq!(
            missing.metrics[6].1,
            AuditMetricInput::Unavailable {
                reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
            }
        );

        let mut sql_null = base.clone();
        sql_null.extend([None, Some("1".into()), None, Some("1".into())]);
        let sql_null =
            decode_occurrence_source_row(sql_null, &AuditSchema::official_3_5_14()).unwrap();
        assert_eq!(sql_null.error_code, AuditErrorCodeSource::SqlNull);
        assert_eq!(sql_null.metrics[6].1, AuditMetricInput::SqlNull);

        let mut zero = base.clone();
        zero.extend([
            Some("".into()),
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
        ]);
        let zero = decode_occurrence_source_row(zero, &AuditSchema::official_3_5_14()).unwrap();
        assert_eq!(
            zero.error_code,
            AuditErrorCodeSource::Text { value: "".into() }
        );
        assert_eq!(
            zero.metrics[6].1,
            AuditMetricInput::Text { value: "0".into() }
        );

        let mut conflicting = base;
        conflicting.extend([
            Some("ignored".into()),
            Some("0".into()),
            Some("99".into()),
            Some("0".into()),
        ]);
        let conflicting =
            decode_occurrence_source_row(conflicting, &AuditSchema::official_3_5_14()).unwrap();
        assert_eq!(
            conflicting.error_code,
            AuditErrorCodeSource::Unavailable {
                reason: AuditOptionalFactUnavailableReason::ConflictingSourceRows,
            }
        );
        assert_eq!(
            conflicting.metrics[6].1,
            AuditMetricInput::Unavailable {
                reason: AuditOptionalFactUnavailableReason::ConflictingSourceRows,
            }
        );
    }

    #[test]
    fn nullable_family_row_preserves_aggregate_absence() {
        let decoded = decode_family_source_row(vec![
            Some("internal".into()),
            Some("db".into()),
            Some("abc".into()),
            Some("1".into()),
            Some("0".into()),
            Some("0".into()),
            None,
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
            Some("0".into()),
            Some("0".into()),
            None,
            Some("0".into()),
            Some("1".into()),
        ])
        .unwrap();
        assert_eq!(decoded.cumulative_query_time, None);
        assert_eq!(decoded.max_memory_cost, None);
        assert_eq!(decoded.max_query_time.as_deref(), Some("0"));
        assert_eq!(
            decoded.error_proportion_population_max.as_deref(),
            Some("1")
        );
    }
}
