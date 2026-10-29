//! One bounded window read for the performance overview: time buckets, exact
//! nearest-rank quantiles, the fixed latency histogram, failure codes and
//! Top-N dimension values, all over the same canonical occurrence cohort as
//! the cards and Top tables. Cells stay lossless decimal text; the core typed
//! normalizer gives them metric semantics later.
use super::*;
use doctor_contracts::{
    AUDIT_PERFORMANCE_BUCKET_SECONDS, AUDIT_PERFORMANCE_MAX_BUCKETS, AuditPerformanceDimensionId,
};
use doctor_core::{AUDIT_FAMILY_HISTOGRAM_BINS, MAX_AUDIT_QUERY_TIME_MS};

pub const AUDIT_PERFORMANCE_TOP_LIMIT: u16 = 10;
pub const AUDIT_PERFORMANCE_SUMMARY_COLUMNS: usize = 22;
const HEADER_COLUMNS: usize = 4;
const WIRE_COLUMNS: usize = HEADER_COLUMNS + AUDIT_PERFORMANCE_SUMMARY_COLUMNS;
const HISTOGRAM_COLUMNS: usize = 12;
const DIMENSIONS_WITHOUT_USER: [(&str, &str); 3] = [
    ("database", "`database_name`"),
    ("resource_group", "`resource_group`"),
    ("catalog", "`catalog_name`"),
];

/// Summary cell positions, shared by the window row and every bucket row.
pub mod summary_cell {
    pub const EXECUTIONS: usize = 0;
    pub const ERRORS: usize = 1;
    pub const QUERY_TIME_NONNEGATIVE: usize = 2;
    pub const QUERY_TIME_ELIGIBLE: usize = 3;
    pub const QUERY_TIME_P50: usize = 4;
    pub const QUERY_TIME_P95: usize = 5;
    pub const QUERY_TIME_P99: usize = 6;
    pub const QUERY_TIME_SUM: usize = 7;
    pub const SCAN_BYTES_NONNEGATIVE: usize = 8;
    pub const SCAN_BYTES_SUM: usize = 9;
    pub const CPU_COST_NONNEGATIVE: usize = 10;
    pub const CPU_COST_SUM: usize = 11;
    pub const MEMORY_COST_NONNEGATIVE: usize = 12;
    pub const MEMORY_COST_MAX: usize = 13;
    pub const PENDING_TIME_ELIGIBLE: usize = 14;
    pub const PENDING_TIME_P50: usize = 15;
    pub const PENDING_TIME_P95: usize = 16;
    pub const PENDING_TIME_P99: usize = 17;
    pub const PENDING_TIME_CONFLICTING: usize = 18;
    pub const QUERY_TIME_SQL_NULL: usize = 19;
    pub const QUERY_TIME_NEGATIVE: usize = 20;
    pub const QUERY_TIME_OVERFLOW: usize = 21;
    /// Cells that must be present exact counters.
    pub const COUNTERS: [usize; 12] = [
        EXECUTIONS,
        ERRORS,
        QUERY_TIME_NONNEGATIVE,
        QUERY_TIME_ELIGIBLE,
        SCAN_BYTES_NONNEGATIVE,
        CPU_COST_NONNEGATIVE,
        MEMORY_COST_NONNEGATIVE,
        PENDING_TIME_ELIGIBLE,
        PENDING_TIME_CONFLICTING,
        QUERY_TIME_SQL_NULL,
        QUERY_TIME_NEGATIVE,
        QUERY_TIME_OVERFLOW,
    ];
}

/// Number of buckets for a validated window and a supported width.
pub fn audit_performance_bucket_count(
    scope: &AuditScope,
    bucket_seconds: u32,
) -> Result<u32, AuditSqlBuildError> {
    scope.validate().map_err(AuditSqlBuildError::Request)?;
    if !AUDIT_PERFORMANCE_BUCKET_SECONDS.contains(&bucket_seconds) {
        return Err(AuditSqlBuildError::PerformanceBucket);
    }
    let count = (scope.window.end_seconds - scope.window.start_seconds).div_ceil(bucket_seconds);
    if count > AUDIT_PERFORMANCE_MAX_BUCKETS {
        return Err(AuditSqlBuildError::PerformanceBucket);
    }
    Ok(count)
}

fn summary_columns() -> String {
    let error = utf8_hex_literal("ERR");
    let max = MAX_AUDIT_QUERY_TIME_MS;
    let qt_eligible = format!("`query_time` >= 0 AND `query_time` <= {max}");
    let pending_eligible = format!(
        "`pending_time_agrees` = 1 AND `pending_time_ms` >= 0 AND `pending_time_ms` <= {max}"
    );
    let count_if = |predicate: &str| format!("COUNT(CASE WHEN {predicate} THEN 1 END)");
    let quantile = |predicate: &str, column: &str, q: &str| {
        format!("PERCENTILE_DISC(CASE WHEN {predicate} THEN `{column}` END, {q})")
    };
    let sum_nonnegative =
        |column: &str| format!("SUM(CASE WHEN `{column}` >= 0 THEN `{column}` END)");
    [
        "COUNT(*)".to_owned(),
        count_if(&format!("`state` = {error}")),
        count_if("`query_time` >= 0"),
        count_if(&qt_eligible),
        quantile(&qt_eligible, "query_time", "0.5"),
        quantile(&qt_eligible, "query_time", "0.95"),
        quantile(&qt_eligible, "query_time", "0.99"),
        sum_nonnegative("query_time"),
        count_if("`scan_bytes` >= 0"),
        sum_nonnegative("scan_bytes"),
        count_if("`cpu_cost_ns` >= 0"),
        sum_nonnegative("cpu_cost_ns"),
        count_if("`mem_cost_bytes` >= 0"),
        "MAX(CASE WHEN `mem_cost_bytes` >= 0 THEN `mem_cost_bytes` END)".to_owned(),
        count_if(&pending_eligible),
        quantile(&pending_eligible, "pending_time_ms", "0.5"),
        quantile(&pending_eligible, "pending_time_ms", "0.95"),
        quantile(&pending_eligible, "pending_time_ms", "0.99"),
        count_if("`pending_time_agrees` = 0"),
        count_if("`query_time` IS NULL"),
        count_if("`query_time` < 0"),
        count_if(&format!("`query_time` > {max}")),
    ]
    .into_iter()
    .map(|cell| format!("CAST({cell} AS VARCHAR)"))
    .collect::<Vec<_>>()
    .join(", ")
}

fn null_cells(count: usize) -> String {
    vec!["CAST(NULL AS VARCHAR)"; count].join(", ")
}

/// Every cell is cast to text so `UNION ALL` never widens an exact integer
/// into a floating type. Row kinds: `window` (one), `bucket` (one per
/// non-empty bucket), `hist` (one), `error` (Top-N, only when `errorCode` is
/// proven) and `dim` (Top-N per key per dimension).
pub fn audit_performance_statement(
    table: &AuditTableName,
    scope: &AuditScope,
    bucket_seconds: u32,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    validate_occurrence_scope(scope, schema)?;
    audit_performance_bucket_count(scope, bucket_seconds)?;
    let with_user = schema.has(AuditColumn::User);
    let cte =
        canonical_occurrence_events_cte_with_user(table, scope_predicate(scope), schema, with_user);
    let start = scope.window.start_seconds;
    let error = utf8_hex_literal("ERR");
    let max = MAX_AUDIT_QUERY_TIME_MS;
    let summary = summary_columns();
    let top = AUDIT_PERFORMANCE_TOP_LIMIT;
    let mut histogram = AUDIT_FAMILY_HISTOGRAM_BINS
        .iter()
        .map(|bin| {
            format!(
                "COUNT(CASE WHEN `query_time` >= {} AND `query_time` < {} THEN 1 END)",
                bin.lower_ms, bin.upper_ms
            )
        })
        .collect::<Vec<_>>();
    histogram.extend([
        "COUNT(CASE WHEN `query_time` IS NULL THEN 1 END)".to_owned(),
        "COUNT(CASE WHEN `query_time` < 0 THEN 1 END)".to_owned(),
        format!("COUNT(CASE WHEN `query_time` > {max} THEN 1 END)"),
        format!("COUNT(CASE WHEN `query_time` >= 0 AND `query_time` <= {max} THEN 1 END)"),
    ]);
    let histogram = histogram
        .into_iter()
        .map(|cell| format!("CAST({cell} AS VARCHAR)"))
        .collect::<Vec<_>>()
        .join(", ");
    let dimension_cells = format!(
        "COUNT(*) AS `c`, COUNT(CASE WHEN `state` = {error} THEN 1 END) AS `e`, COUNT(CASE WHEN `query_time` >= 0 THEN 1 END) AS `tv`, SUM(CASE WHEN `query_time` >= 0 THEN `query_time` END) AS `t`"
    );
    let mut dimension_groups = DIMENSIONS_WITHOUT_USER
        .iter()
        .map(|(name, column)| {
            format!(
                "SELECT '{name}' AS `dim`, 1 AS `agrees`, {column} AS `value`, {dimension_cells} FROM perf_events GROUP BY {column}"
            )
        })
        .collect::<Vec<_>>();
    if with_user {
        dimension_groups.push(format!(
            "SELECT 'user' AS `dim`, `source_user_agrees` AS `agrees`, `source_user` AS `value`, {dimension_cells} FROM perf_events GROUP BY `source_user_agrees`, `source_user`"
        ));
    }
    let tie = "`agrees` DESC, (`value` IS NULL) ASC, `value` ASC";
    let mut sections = vec![
        format!(
            "SELECT 'window', {}, {summary} FROM perf_events",
            null_cells(3)
        ),
        format!(
            "SELECT 'bucket', CAST(`bucket_index` AS VARCHAR), {}, {summary} FROM perf_events GROUP BY `bucket_index`",
            null_cells(2)
        ),
        format!(
            "SELECT 'hist', {}, {histogram}, {} FROM perf_events",
            null_cells(3),
            null_cells(AUDIT_PERFORMANCE_SUMMARY_COLUMNS - HISTOGRAM_COLUMNS)
        ),
        format!(
            "SELECT 'dim', `dim`, CAST(`agrees` AS VARCHAR), `value`, CAST(`c` AS VARCHAR), CAST(`e` AS VARCHAR), CAST(`tv` AS VARCHAR), CAST(`t` AS VARCHAR), CAST(`rc` AS VARCHAR), CAST(`rt` AS VARCHAR), CAST(`re` AS VARCHAR), CAST(`n` AS VARCHAR), {} FROM dim_ranked WHERE `rc` <= {top} OR `rt` <= {top} OR `re` <= {top}",
            null_cells(AUDIT_PERFORMANCE_SUMMARY_COLUMNS - 8)
        ),
    ];
    let mut ctes = vec![
        format!(
            "perf_events AS (SELECT *, (UNIX_TIMESTAMP(`event_time`) - {start}) DIV {bucket_seconds} AS `bucket_index` FROM occurrence_events)"
        ),
        format!("dim_groups AS ({})", dimension_groups.join(" UNION ALL ")),
        format!(
            "dim_ranked AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY `dim` ORDER BY `c` DESC, {tie}) AS `rc`, ROW_NUMBER() OVER (PARTITION BY `dim` ORDER BY (`t` IS NULL) ASC, `t` DESC, {tie}) AS `rt`, ROW_NUMBER() OVER (PARTITION BY `dim` ORDER BY `e` DESC, {tie}) AS `re`, COUNT(*) OVER (PARTITION BY `dim`) AS `n` FROM dim_groups)"
        ),
    ];
    if schema.has(AuditColumn::ErrorCode) {
        ctes.push(format!(
            "error_groups AS (SELECT `error_code_agrees`, `error_code`, COUNT(*) AS `c`, UNIX_TIMESTAMP(MAX(`event_time`)) AS `last_seconds` FROM perf_events WHERE `state` = {error} GROUP BY `error_code_agrees`, `error_code`)"
        ));
        ctes.push("error_ranked AS (SELECT *, ROW_NUMBER() OVER (ORDER BY `c` DESC, `error_code_agrees` DESC, (`error_code` IS NULL) ASC, `error_code` ASC) AS `r`, COUNT(*) OVER () AS `n` FROM error_groups)".to_owned());
        sections.push(format!(
            "SELECT 'error', {}, CAST(`error_code_agrees` AS VARCHAR), `error_code`, CAST(`c` AS VARCHAR), CAST(`last_seconds` AS VARCHAR), CAST(`n` AS VARCHAR), {} FROM error_ranked WHERE `r` <= {top}",
            null_cells(1),
            null_cells(AUDIT_PERFORMANCE_SUMMARY_COLUMNS - 3)
        ));
    }
    let statement = format!(
        "{cte}, {} {}",
        ctes.join(", "),
        sections.join(" UNION ALL ")
    );
    if statement.len() > crate::MAX_PLAN_SQL_BYTES {
        return Err(AuditSqlBuildError::StatementBudget);
    }
    Ok(statement)
}

/// Lossless summary cells. Counters are proven canonical decimals; quantile,
/// sum and maximum cells keep SQL NULL distinct from "0".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPerformanceSummarySource {
    pub cells: Vec<Option<String>>,
}

impl AuditPerformanceSummarySource {
    pub fn counter(&self, index: usize) -> &str {
        self.cells[index].as_deref().unwrap_or("0")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPerformanceErrorSource {
    pub agrees: bool,
    pub code: Option<String>,
    pub count: String,
    pub last_event_seconds: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPerformanceDimensionSource {
    pub dimension: AuditPerformanceDimensionId,
    pub agrees: bool,
    pub value: Option<String>,
    pub execution_count: String,
    pub error_count: String,
    pub query_time_nonnegative_count: String,
    pub query_time_sum: Option<String>,
    pub rank_by_count: u16,
    pub rank_by_query_time: u16,
    pub rank_by_errors: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPerformanceSource {
    pub window: AuditPerformanceSummarySource,
    /// Non-empty buckets by index, ascending.
    pub buckets: BTreeMap<u32, AuditPerformanceSummarySource>,
    /// Eight bins, then SQL NULL, negative, overflow and eligible counts.
    pub histogram: Vec<String>,
    /// `None` when `errorCode` is not a proven source column.
    pub errors: Option<Vec<AuditPerformanceErrorSource>>,
    pub error_code_count: String,
    pub dimensions: Vec<AuditPerformanceDimensionSource>,
    pub dimension_value_counts: BTreeMap<AuditPerformanceDimensionId, String>,
}

fn counter_text(value: &Option<String>) -> Result<String, AuditSqlBuildError> {
    match value.as_deref() {
        Some(text)
            if !text.is_empty()
                && text.bytes().all(|byte| byte.is_ascii_digit())
                && (text == "0" || !text.starts_with('0')) =>
        {
            Ok(text.to_owned())
        }
        _ => Err(AuditSqlBuildError::PerformanceResponse),
    }
}

fn rank(value: &Option<String>) -> Result<u16, AuditSqlBuildError> {
    counter_text(value)?
        .parse::<u16>()
        .ok()
        .filter(|rank| *rank >= 1)
        .ok_or(AuditSqlBuildError::PerformanceResponse)
}

fn agreement(value: &Option<String>) -> Result<bool, AuditSqlBuildError> {
    match value.as_deref() {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err(AuditSqlBuildError::PerformanceResponse),
    }
}

fn summary(row: &[Option<String>]) -> Result<AuditPerformanceSummarySource, AuditSqlBuildError> {
    let cells = row[HEADER_COLUMNS..].to_vec();
    for index in summary_cell::COUNTERS {
        counter_text(&cells[index])?;
    }
    Ok(AuditPerformanceSummarySource { cells })
}

fn dimension_id(value: &Option<String>) -> Result<AuditPerformanceDimensionId, AuditSqlBuildError> {
    Ok(match value.as_deref() {
        Some("user") => AuditPerformanceDimensionId::User,
        Some("database") => AuditPerformanceDimensionId::Database,
        Some("resource_group") => AuditPerformanceDimensionId::ResourceGroup,
        Some("catalog") => AuditPerformanceDimensionId::Catalog,
        _ => return Err(AuditSqlBuildError::PerformanceResponse),
    })
}

/// Reject any malformed envelope before a response is built: unknown row
/// kinds, duplicates, out-of-range buckets, wrong widths or non-canonical
/// counters all fail the whole read rather than drawing a partial picture.
pub fn decode_performance_source_rows(
    rows: Vec<Vec<Option<String>>>,
    scope: &AuditScope,
    bucket_seconds: u32,
    schema: &AuditSchema,
) -> Result<AuditPerformanceSource, AuditSqlBuildError> {
    let bucket_count = audit_performance_bucket_count(scope, bucket_seconds)?;
    let dimension_count = if schema.has(AuditColumn::User) { 4 } else { 3 };
    let budget = 2
        + bucket_count as usize
        + usize::from(AUDIT_PERFORMANCE_TOP_LIMIT)
        + dimension_count * 3 * usize::from(AUDIT_PERFORMANCE_TOP_LIMIT);
    if rows.len() > budget {
        return Err(AuditSqlBuildError::PerformanceResponse);
    }
    let bad = || AuditSqlBuildError::PerformanceResponse;
    let mut window = None;
    let mut histogram = None;
    let mut buckets = BTreeMap::new();
    let mut errors = schema.has(AuditColumn::ErrorCode).then(Vec::new);
    let mut error_code_count = None;
    let mut dimensions = Vec::new();
    let mut dimension_value_counts = BTreeMap::new();
    for row in rows {
        if row.len() != WIRE_COLUMNS {
            return Err(row_shape(WIRE_COLUMNS, row.len()));
        }
        match row[0].as_deref() {
            Some("window")
                if window.is_none() && row[1..HEADER_COLUMNS].iter().all(Option::is_none) =>
            {
                window = Some(summary(&row)?);
            }
            Some("bucket") if row[2].is_none() && row[3].is_none() => {
                let index = counter_text(&row[1])?
                    .parse::<u32>()
                    .ok()
                    .filter(|index| *index < bucket_count)
                    .ok_or_else(bad)?;
                if buckets.insert(index, summary(&row)?).is_some() {
                    return Err(bad());
                }
            }
            Some("hist")
                if histogram.is_none()
                    && row[1..HEADER_COLUMNS].iter().all(Option::is_none)
                    && row[HEADER_COLUMNS + HISTOGRAM_COLUMNS..]
                        .iter()
                        .all(Option::is_none) =>
            {
                histogram = Some(
                    row[HEADER_COLUMNS..HEADER_COLUMNS + HISTOGRAM_COLUMNS]
                        .iter()
                        .map(counter_text)
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            Some("error")
                if row[1].is_none() && row[HEADER_COLUMNS + 3..].iter().all(Option::is_none) =>
            {
                let list = errors.as_mut().ok_or_else(bad)?;
                let agrees = agreement(&row[2])?;
                if !agrees && row[3].is_some() {
                    return Err(bad());
                }
                let count = counter_text(&row[HEADER_COLUMNS + 2])?;
                if error_code_count
                    .replace(count.clone())
                    .is_some_and(|prior| prior != count)
                {
                    return Err(bad());
                }
                list.push(AuditPerformanceErrorSource {
                    agrees,
                    code: row[3].clone(),
                    count: counter_text(&row[HEADER_COLUMNS])?,
                    last_event_seconds: counter_text(&row[HEADER_COLUMNS + 1])?,
                });
            }
            Some("dim") if row[HEADER_COLUMNS + 8..].iter().all(Option::is_none) => {
                let dimension = dimension_id(&row[1])?;
                let agrees = agreement(&row[2])?;
                if (!agrees && (row[3].is_some() || dimension != AuditPerformanceDimensionId::User))
                    || (dimension == AuditPerformanceDimensionId::User && dimension_count == 3)
                {
                    return Err(bad());
                }
                let cell = |offset: usize| &row[HEADER_COLUMNS + offset];
                let distinct = counter_text(cell(7))?;
                if dimension_value_counts
                    .insert(dimension, distinct.clone())
                    .is_some_and(|prior| prior != distinct)
                {
                    return Err(bad());
                }
                let sum = cell(3).clone();
                if sum
                    .as_deref()
                    .is_some_and(|text| counter_text(&Some(text.to_owned())).is_err())
                {
                    return Err(bad());
                }
                dimensions.push(AuditPerformanceDimensionSource {
                    dimension,
                    agrees,
                    value: row[3].clone(),
                    execution_count: counter_text(cell(0))?,
                    error_count: counter_text(cell(1))?,
                    query_time_nonnegative_count: counter_text(cell(2))?,
                    query_time_sum: sum,
                    rank_by_count: rank(cell(4))?,
                    rank_by_query_time: rank(cell(5))?,
                    rank_by_errors: rank(cell(6))?,
                });
            }
            _ => return Err(bad()),
        }
    }
    let window = window.ok_or_else(bad)?;
    if errors
        .as_ref()
        .is_some_and(|list| list.is_empty() != (window.counter(summary_cell::ERRORS) == "0"))
    {
        return Err(bad());
    }
    Ok(AuditPerformanceSource {
        window,
        buckets,
        histogram: histogram.ok_or_else(bad)?,
        errors,
        error_code_count: error_code_count.unwrap_or_else(|| "0".into()),
        dimensions,
        dimension_value_counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> AuditScope {
        AuditScope {
            window: AuditWindow {
                start_seconds: 1_800_000_000,
                end_seconds: 1_800_086_400,
            },
            state: None,
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

    fn schema_with_user() -> AuditSchema {
        let mut schema = AuditSchema::official_3_5_14();
        schema.columns.insert(AuditColumn::User);
        schema
    }

    fn summary_row(
        kind: &str,
        key: Option<&str>,
        executions: &str,
        errors: &str,
    ) -> Vec<Option<String>> {
        let mut row = vec![Some(kind.to_owned()), key.map(str::to_owned), None, None];
        let mut cells = vec![Some("0".to_owned()); AUDIT_PERFORMANCE_SUMMARY_COLUMNS];
        cells[summary_cell::EXECUTIONS] = Some(executions.into());
        cells[summary_cell::ERRORS] = Some(errors.into());
        for index in [
            summary_cell::QUERY_TIME_P50,
            summary_cell::QUERY_TIME_P95,
            summary_cell::QUERY_TIME_P99,
            summary_cell::QUERY_TIME_SUM,
            summary_cell::PENDING_TIME_P50,
            summary_cell::PENDING_TIME_P95,
            summary_cell::PENDING_TIME_P99,
        ] {
            cells[index] = None;
        }
        row.extend(cells);
        row
    }

    fn histogram_row() -> Vec<Option<String>> {
        let mut row = vec![Some("hist".to_owned()), None, None, None];
        row.extend((0..HISTOGRAM_COLUMNS).map(|_| Some("0".to_owned())));
        row.extend((HISTOGRAM_COLUMNS..AUDIT_PERFORMANCE_SUMMARY_COLUMNS).map(|_| None));
        row
    }

    #[test]
    fn statement_buckets_from_the_window_start_and_keeps_every_cell_text() {
        let statement =
            audit_performance_statement(&table(), &scope(), 900, &schema_with_user()).unwrap();
        assert!(
            statement
                .contains("(UNIX_TIMESTAMP(`event_time`) - 1800000000) DIV 900 AS `bucket_index`")
        );
        assert!(
            statement.contains("PERCENTILE_DISC(CASE WHEN `query_time` >= 0 AND `query_time` <= ")
        );
        assert!(statement.contains("GROUP BY `source_user_agrees`, `source_user`"));
        assert!(statement.contains("FROM error_ranked WHERE `r` <= 10"));
        assert!(statement.contains("WHERE `rc` <= 10 OR `rt` <= 10 OR `re` <= 10"));
        assert!(!statement.contains("`stmt`"));
        // The occurrence cohort is the shared one: the same dedup agreement.
        assert!(statement.contains(&projected_rows_agree_having()));
    }

    #[test]
    fn statement_omits_optional_sections_the_schema_did_not_prove() {
        let schema = AuditSchema::new([
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
        let mut schema = schema;
        for (column, kind) in [
            (AuditColumn::Timestamp, "DATETIME"),
            (AuditColumn::QueryTime, "BIGINT"),
            (AuditColumn::ScanBytes, "BIGINT"),
            (AuditColumn::ScanRows, "BIGINT"),
            (AuditColumn::CpuCostNs, "BIGINT"),
            (AuditColumn::MemCostBytes, "BIGINT"),
        ] {
            schema.source_types.insert(column, kind.into());
        }
        let statement = audit_performance_statement(&table(), &scope(), 900, &schema).unwrap();
        assert!(!statement.contains("error_ranked"));
        assert!(!statement.contains("source_user"));
        assert!(statement.contains("CAST(NULL AS BIGINT) AS `pending_time_ms`"));
    }

    #[test]
    fn unsupported_or_oversized_buckets_fail_before_sql() {
        assert_eq!(
            audit_performance_statement(&table(), &scope(), 120, &schema_with_user()),
            Err(AuditSqlBuildError::PerformanceBucket)
        );
        let mut wide = scope();
        wide.window.end_seconds = wide.window.start_seconds + 2 * 86_400;
        assert_eq!(
            audit_performance_statement(&table(), &wide, 60, &schema_with_user()),
            Err(AuditSqlBuildError::PerformanceBucket)
        );
    }

    #[test]
    fn decoder_keeps_empty_buckets_absent_and_null_quantiles_null() {
        let source = decode_performance_source_rows(
            vec![
                summary_row("window", None, "3", "0"),
                summary_row("bucket", Some("4"), "3", "0"),
                histogram_row(),
            ],
            &scope(),
            900,
            &schema_with_user(),
        )
        .unwrap();
        assert_eq!(source.buckets.len(), 1);
        assert_eq!(source.buckets[&4].cells[summary_cell::QUERY_TIME_P95], None);
        assert_eq!(source.errors, Some(Vec::new()));
    }

    #[test]
    fn decoder_rejects_out_of_range_buckets_duplicates_and_bad_counters() {
        let decode =
            |rows| decode_performance_source_rows(rows, &scope(), 900, &schema_with_user());
        assert!(
            decode(vec![
                summary_row("window", None, "1", "0"),
                summary_row("bucket", Some("96"), "1", "0"),
                histogram_row()
            ])
            .is_err()
        );
        assert!(
            decode(vec![
                summary_row("window", None, "1", "0"),
                summary_row("window", None, "1", "0"),
                histogram_row()
            ])
            .is_err()
        );
        assert!(
            decode(vec![
                summary_row("window", None, "01", "0"),
                histogram_row()
            ])
            .is_err()
        );
        assert!(decode(vec![summary_row("window", None, "1", "0")]).is_err());
        // A failed window must come with its error-code rows when the column exists.
        assert!(decode(vec![summary_row("window", None, "2", "1"), histogram_row()]).is_err());
    }

    #[test]
    fn decoder_keeps_conflicting_users_distinct_from_null_users() {
        let mut dim = vec![
            Some("dim".to_owned()),
            Some("user".to_owned()),
            Some("0".to_owned()),
            None,
        ];
        dim.extend(["2", "0", "2", "10", "1", "1", "1", "3"].map(|cell| Some(cell.to_owned())));
        dim.extend((8..AUDIT_PERFORMANCE_SUMMARY_COLUMNS).map(|_| None));
        let source = decode_performance_source_rows(
            vec![
                summary_row("window", None, "2", "0"),
                histogram_row(),
                dim.clone(),
            ],
            &scope(),
            900,
            &schema_with_user(),
        )
        .unwrap();
        assert!(!source.dimensions[0].agrees);
        assert_eq!(
            source.dimension_value_counts[&AuditPerformanceDimensionId::User],
            "3"
        );
        dim[3] = Some("someone".into());
        assert!(
            decode_performance_source_rows(
                vec![summary_row("window", None, "2", "0"), histogram_row(), dim],
                &scope(),
                900,
                &schema_with_user(),
            )
            .is_err()
        );
    }
}
