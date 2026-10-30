//! Load workspace reads (task 51): fixed `SELECT`s over StarRocks
//! `information_schema.routine_load_jobs`, `loads` and `load_tracking_logs`,
//! and their decoding into contract views. The statements take no user text
//! except a numeric job id. Data source properties are read only for the
//! topic: broker lists and client properties may hold credentials.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    LOAD_OVERVIEW_HOURS, LOAD_OVERVIEW_LAGS, LOAD_OVERVIEW_MAX_HOURS, LOAD_OVERVIEW_PAUSED,
    LOAD_OVERVIEW_TABLES, LOAD_RECORDS_MAX, LoadHourBucket, LoadHourCoverage, LoadKindCount,
    LoadKindFilter, LoadOutcomeFilter, LoadOverviewRequest, LoadOverviewResponse, LoadPhaseBin,
    LoadPhaseDistribution, LoadPhaseQuantiles, LoadRecord, LoadRecordsRequest, LoadRecordsResponse,
    LoadRecordsSummary, LoadStateClass, LoadTableBar, LoadTableSummary, LoadViewSupport,
    RoutineLagBar, RoutineLoadJob, RoutineLoadJobsResponse, RoutineLoadOverview,
    RoutineLoadPartition, RoutineLoadStateClass, RoutineLoadStatistics, RoutinePausedJob,
};
use serde_json::Value;

/// Routine Load jobs read per request (newest first).
pub const ROUTINE_LOAD_JOBS_LIMIT: usize = 500;
/// Two FE managers keep about 1000 records each by default; the transport
/// returns at most 2048 rows.
pub const LOAD_RECORDS_READ_LIMIT: usize = 2000;
/// A manager this full is probably dropping older records.
const RETENTION_NEAR_LIMIT: usize = 900;
const P95_MIN_SAMPLES: usize = 20;
/// Databases whose loads StarRocks or AuditLoader make on their own.
const SYSTEM_DATABASES: [&str; 1] = ["_statistics_"];

/// `(routine load jobs, load records)` for a detected version text such as
/// `3.5.14-23a56ec`. 3.2 lacks the lag columns and has an older `loads`
/// shape without runtime details; 3.1 has neither view.
pub fn load_support(version: Option<&str>) -> (LoadViewSupport, LoadViewSupport) {
    let parsed = version.and_then(|text| {
        let mut parts = text.split(['.', '-']);
        Some((
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
        ))
    });
    match parsed {
        Some((major, minor)) if (major, minor) >= (3, 3) => {
            (LoadViewSupport::Full, LoadViewSupport::Full)
        }
        Some((3, 2)) => (
            LoadViewSupport::WithoutLag,
            LoadViewSupport::UnsupportedVersion,
        ),
        _ => (
            LoadViewSupport::UnsupportedVersion,
            LoadViewSupport::UnsupportedVersion,
        ),
    }
}

pub fn routine_load_jobs_statement(with_lag: bool) -> String {
    let lag = if with_lag {
        ", CAST(LATEST_SOURCE_POSITION AS VARCHAR), CAST(OFFSET_LAG AS VARCHAR), CAST(TIMESTAMP_PROGRESS AS VARCHAR)"
    } else {
        ""
    };
    format!(
        "SELECT ID, NAME, CAST(CREATE_TIME AS VARCHAR), CAST(PAUSE_TIME AS VARCHAR), CAST(END_TIME AS VARCHAR), DB_NAME, TABLE_NAME, STATE, DATA_SOURCE_TYPE, CURRENT_TASK_NUM, JOB_PROPERTIES, DATA_SOURCE_PROPERTIES, STATISTICS, PROGRESS, REASONS_OF_STATE_CHANGED, ERROR_LOG_URLS, OTHER_MSG{lag} FROM information_schema.routine_load_jobs ORDER BY ID DESC LIMIT {}",
        ROUTINE_LOAD_JOBS_LIMIT + 1
    )
}

pub fn load_records_statement() -> String {
    format!(
        "SELECT ID, LABEL, PROFILE_ID, DB_NAME, TABLE_NAME, USER, STATE, TYPE, FILTERED_ROWS, UNSELECTED_ROWS, SINK_ROWS, CAST(RUNTIME_DETAILS AS VARCHAR), CAST(CREATE_TIME AS VARCHAR), CAST(LOAD_FINISH_TIME AS VARCHAR), ERROR_MSG, TRACKING_SQL FROM information_schema.loads ORDER BY ID DESC LIMIT {LOAD_RECORDS_READ_LIMIT}"
    )
}

/// One job at a time: the BE scanner fetches every listed URL and joins
/// rows of a multi-row result into each other.
pub fn load_tracking_log_statement(job_id: u64) -> String {
    format!(
        "SELECT TRACKING_LOG FROM information_schema.load_tracking_logs WHERE JOB_ID = {job_id}"
    )
}

fn text(row: &[Option<String>], index: usize) -> Option<String> {
    row.get(index)
        .cloned()
        .flatten()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// StarRocks writes -1 for counts it does not know.
fn count(row: &[Option<String>], index: usize) -> Option<String> {
    text(row, index).filter(|value| value.parse::<i64>().is_ok_and(|number| number >= 0))
}

fn json(value: Option<&str>) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(value?).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A JSON number or numeric string as text; anything else is missing.
fn number_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) if text.parse::<f64>().is_ok() => Some(text.clone()),
        _ => None,
    }
}

fn string_map(value: Option<&str>) -> BTreeMap<String, String> {
    json(value)
        .map(|map| {
            map.into_iter()
                .filter_map(|(key, value)| match value {
                    Value::String(text) => Some((key, text)),
                    Value::Number(number) => Some((key, number.to_string())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn routine_state_class(state: &str) -> RoutineLoadStateClass {
    match state {
        "RUNNING" => RoutineLoadStateClass::Running,
        "NEED_SCHEDULE" => RoutineLoadStateClass::Scheduling,
        "PAUSED" => RoutineLoadStateClass::Paused,
        "STOPPED" => RoutineLoadStateClass::Stopped,
        "CANCELLED" => RoutineLoadStateClass::Cancelled,
        _ => RoutineLoadStateClass::Unknown,
    }
}

fn partition_order(partition: &str) -> (u64, String) {
    (partition.parse().unwrap_or(u64::MAX), partition.to_owned())
}

pub fn decode_routine_load_jobs(
    rows: Vec<Vec<Option<String>>>,
    support: LoadViewSupport,
    fe_time_zone: Option<String>,
) -> RoutineLoadJobsResponse {
    let with_lag = support == LoadViewSupport::Full;
    let truncated = rows.len() > ROUTINE_LOAD_JOBS_LIMIT;
    let jobs = rows
        .into_iter()
        .take(ROUTINE_LOAD_JOBS_LIMIT)
        .filter_map(|row| {
            let job_id = text(&row, 0)?;
            let state = text(&row, 7).unwrap_or_default();
            let statistics = json(text(&row, 12).as_deref()).unwrap_or_default();
            let stat = |key: &str| number_text(statistics.get(key));
            let properties = json(text(&row, 10).as_deref()).unwrap_or_default();
            let topic = json(text(&row, 11).as_deref())
                .and_then(|map| map.get("topic").and_then(Value::as_str).map(str::to_owned));
            let progress = string_map(text(&row, 13).as_deref());
            let (latest, lag, timestamps) = if with_lag {
                (
                    string_map(text(&row, 17).as_deref()),
                    string_map(text(&row, 18).as_deref()),
                    string_map(text(&row, 19).as_deref()),
                )
            } else {
                Default::default()
            };
            let keys: BTreeSet<(u64, String)> = progress
                .keys()
                .chain(latest.keys())
                .chain(lag.keys())
                .map(|key| partition_order(key))
                .collect();
            let partitions: Vec<RoutineLoadPartition> = keys
                .into_iter()
                .map(|(_, key)| RoutineLoadPartition {
                    consumed: progress.get(&key).cloned(),
                    latest: latest.get(&key).cloned(),
                    lag: lag
                        .get(&key)
                        .cloned()
                        .filter(|value| value.parse::<i64>().is_ok()),
                    consumed_message_ms: timestamps
                        .get(&key)
                        .cloned()
                        .filter(|value| value.parse::<i64>().is_ok()),
                    partition: key,
                })
                .collect();
            let max = partitions
                .iter()
                .filter_map(|partition| {
                    Some((partition.lag.as_deref()?.parse::<i64>().ok()?, partition))
                })
                .max_by_key(|(lag, _)| *lag);
            Some(RoutineLoadJob {
                job_id,
                name: text(&row, 1).unwrap_or_default(),
                db: text(&row, 5).unwrap_or_default(),
                table: text(&row, 6).unwrap_or_default(),
                state_class: routine_state_class(&state),
                state,
                create_time: text(&row, 2),
                pause_time: text(&row, 3),
                end_time: text(&row, 4),
                source_type: text(&row, 8).unwrap_or_default(),
                topic,
                current_tasks: count(&row, 9),
                statistics: RoutineLoadStatistics {
                    received_bytes: stat("receivedBytes"),
                    received_bytes_rate: stat("receivedBytesRate"),
                    loaded_rows: stat("loadedRows"),
                    load_rows_rate: stat("loadRowsRate"),
                    error_rows: stat("errorRows"),
                    total_rows: stat("totalRows"),
                    unselected_rows: stat("unselectedRows"),
                    committed_tasks: stat("committedTaskNum"),
                    aborted_tasks: stat("abortedTaskNum"),
                    task_execute_time_ms: stat("taskExecuteTimeMs"),
                },
                max_error_number: number_text(
                    properties
                        .get("maxErrorNum")
                        .or_else(|| properties.get("max_error_number")),
                ),
                max_lag: max.map(|(lag, _)| lag.to_string()),
                max_lag_partition: max.map(|(_, partition)| partition.partition.clone()),
                partitions,
                reason: text(&row, 14),
                other_message: text(&row, 16),
                // Error logs are read through `load_tracking_logs`, checked
                // on 3.3+ only; 3.2 does not offer the entry.
                has_error_log: with_lag && text(&row, 15).is_some(),
            })
        })
        .collect::<Vec<_>>();
    let mut limitations = vec![
        "live_read_not_stored".to_owned(),
        "times_in_fe_time_zone".to_owned(),
    ];
    if !with_lag {
        limitations.push("offset_lag_requires_3_3".to_owned());
        limitations.push("error_log_requires_3_3".to_owned());
    }
    RoutineLoadJobsResponse {
        summary: routine_overview(&jobs, support, truncated),
        support,
        fe_time_zone,
        jobs,
        truncated,
        limitations,
    }
}

fn load_state_class(state: &str) -> LoadStateClass {
    match state {
        "FINISHED" => LoadStateClass::Finished,
        "CANCELLED" => LoadStateClass::Cancelled,
        "PENDING" | "ETL" | "LOADING" | "PREPARING" | "PREPARED" | "COMMITTED" | "COMMITED"
        | "QUEUEING" | "BEGIN" | "BEFORE_LOAD" => LoadStateClass::Running,
        _ => LoadStateClass::Unknown,
    }
}

/// Seconds since the epoch of a `YYYY-MM-DD HH:MM:SS` string, only to
/// subtract two of them; the zone cancels out.
fn civil_seconds(value: &str) -> Option<i64> {
    let (date, time) = value.trim().split_once(' ')?;
    let mut d = date.split('-').map(str::parse::<i64>);
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time
        .split(':')
        .map(|part| part.split('.').next().unwrap_or(part).parse::<i64>());
    let (h, min, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + day;
    Some(days * 86_400 + h * 3_600 + min * 60 + s)
}

fn kind_matches(kind: &str, filter: LoadKindFilter) -> bool {
    match filter {
        LoadKindFilter::StreamLoad => kind.contains("STREAM_LOAD"),
        LoadKindFilter::RoutineLoad => kind == "ROUTINE_LOAD",
        LoadKindFilter::Broker => kind == "BROKER",
        LoadKindFilter::Insert => kind == "INSERT",
    }
}

/// Adds a known count; an unknown one is counted apart, never as zero.
fn add(total: &mut u128, unknown: &mut u128, value: Option<&str>) {
    match value.and_then(|value| value.parse::<u128>().ok()) {
        Some(count) => *total += count,
        None => *unknown += 1,
    }
}

#[derive(Default)]
struct TableTally {
    loads: u128,
    failed: u128,
    filtered: u128,
    filtered_unknown: u128,
}

impl TableTally {
    fn add(&mut self, record: &LoadRecord) {
        self.loads += 1;
        self.failed += u128::from(record.state_class == LoadStateClass::Cancelled);
        add(
            &mut self.filtered,
            &mut self.filtered_unknown,
            record.filtered_rows.as_deref(),
        );
    }
}

/// Every decoded record StarRocks returned, with the retention signals
/// both the records tab and the overview report.
struct ParsedRecords {
    all: Vec<LoadRecord>,
    earliest: Option<String>,
    at_retention_limit: bool,
}

fn parse_load_rows(rows: Vec<Vec<Option<String>>>, audit_database: Option<&str>) -> ParsedRecords {
    let read = rows.len();
    let mut stream_like = 0usize;
    let mut earliest: Option<String> = None;
    let all: Vec<LoadRecord> = rows
        .into_iter()
        .filter_map(|row| {
            let job_id = text(&row, 0)?;
            let kind = text(&row, 7).unwrap_or_default();
            if kind.contains("STREAM_LOAD") || kind == "ROUTINE_LOAD" {
                stream_like += 1;
            }
            let db = text(&row, 3).unwrap_or_default();
            let state = text(&row, 6).unwrap_or_default();
            let details = json(text(&row, 11).as_deref()).unwrap_or_default();
            let create_time = text(&row, 12);
            let finish_time = text(&row, 13);
            if let Some(created) = &create_time
                && earliest.as_ref().is_none_or(|current| created < current)
            {
                earliest = Some(created.clone());
            }
            let total_seconds = match (
                create_time.as_deref().and_then(civil_seconds),
                finish_time.as_deref().and_then(civil_seconds),
            ) {
                (Some(start), Some(end)) if end >= start => Some((end - start).to_string()),
                _ => None,
            };
            let system = SYSTEM_DATABASES.contains(&db.as_str())
                || audit_database.is_some_and(|audit| audit == db);
            Some(LoadRecord {
                job_id,
                label: text(&row, 1).unwrap_or_default(),
                profile_id: text(&row, 2),
                table: text(&row, 4).unwrap_or_default(),
                user: text(&row, 5),
                state_class: load_state_class(&state),
                state,
                kind,
                filtered_share_permyriad: match (
                    count(&row, 8).and_then(|v| v.parse::<u128>().ok()),
                    count(&row, 10).and_then(|v| v.parse::<u128>().ok()),
                ) {
                    (Some(filtered), Some(loaded)) => share(filtered, filtered + loaded),
                    _ => None,
                },
                filtered_rows: count(&row, 8),
                unselected_rows: count(&row, 9),
                loaded_rows: count(&row, 10),
                client_ip: details
                    .get("client_ip")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                begin_txn_ms: number_text(details.get("begin_txn_time_ms")),
                plan_ms: number_text(details.get("plan_time_ms")),
                receive_data_ms: number_text(details.get("receive_data_time_ms")),
                create_time,
                finish_time,
                total_seconds,
                error_message: text(&row, 14),
                has_error_log: text(&row, 15).is_some(),
                system,
                db,
            })
        })
        .collect();
    let others = read - stream_like;
    ParsedRecords {
        all,
        earliest,
        at_retention_limit: read >= LOAD_RECORDS_READ_LIMIT
            || stream_like >= RETENTION_NEAR_LIMIT
            || others >= RETENTION_NEAR_LIMIT,
    }
}

fn parse_ms(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.parse::<u64>().ok())
}

/// Nearest-rank percentile of sorted samples; `None` below the sample floor.
fn nearest_rank(sorted: &[u64], percent: usize) -> Option<String> {
    (sorted.len() >= P95_MIN_SAMPLES)
        .then(|| sorted[(sorted.len() * percent).div_ceil(100) - 1].to_string())
}

fn summary_of(records: &[&LoadRecord]) -> LoadRecordsSummary {
    let (mut loaded, mut filtered) = (0u128, 0u128);
    let (mut loaded_unknown, mut filtered_unknown) = (0u128, 0u128);
    let mut begin_plan: Vec<u64> = Vec::new();
    for record in records {
        add(
            &mut loaded,
            &mut loaded_unknown,
            record.loaded_rows.as_deref(),
        );
        add(
            &mut filtered,
            &mut filtered_unknown,
            record.filtered_rows.as_deref(),
        );
        if let (Some(begin), Some(plan)) = (
            parse_ms(record.begin_txn_ms.as_deref()),
            parse_ms(record.plan_ms.as_deref()),
        ) {
            begin_plan.push(begin + plan);
        }
    }
    begin_plan.sort_unstable();
    LoadRecordsSummary {
        loads: records.len().to_string(),
        failed: records
            .iter()
            .filter(|record| record.state_class == LoadStateClass::Cancelled)
            .count()
            .to_string(),
        loaded_rows: loaded.to_string(),
        filtered_rows: filtered.to_string(),
        loaded_rows_unknown: loaded_unknown.to_string(),
        filtered_rows_unknown: filtered_unknown.to_string(),
        filtered_share_permyriad: if loaded_unknown == 0 && filtered_unknown == 0 {
            share(filtered, filtered + loaded)
        } else {
            None
        },
        begin_plan_p95_ms: nearest_rank(&begin_plan, 95),
    }
}

const RECORD_LIMITATIONS: [&str; 5] = [
    "live_read_not_stored",
    "times_in_fe_time_zone",
    "total_seconds_second_precision",
    "write_and_publish_phases_not_recorded",
    "fe_retention_3_days_or_1000",
];

pub fn decode_load_records(
    rows: Vec<Vec<Option<String>>>,
    request: &LoadRecordsRequest,
    support: LoadViewSupport,
    fe_time_zone: Option<String>,
    audit_database: Option<&str>,
) -> LoadRecordsResponse {
    let ParsedRecords {
        all,
        earliest,
        at_retention_limit,
        ..
    } = parse_load_rows(rows, audit_database);
    let visible: Vec<&LoadRecord> = all
        .iter()
        .filter(|record| request.include_system || !record.system)
        .collect();
    let hidden_system = all.len() - visible.len();
    let needle = request
        .table
        .as_deref()
        .map(|table| table.trim().to_lowercase())
        .filter(|table| !table.is_empty());
    let matched: Vec<&LoadRecord> = visible
        .iter()
        .copied()
        .filter(|record| {
            request
                .kind
                .is_none_or(|kind| kind_matches(&record.kind, kind))
        })
        .filter(|record| match request.outcome {
            None => true,
            Some(LoadOutcomeFilter::Failed) => record.state_class == LoadStateClass::Cancelled,
            Some(LoadOutcomeFilter::Running) => record.state_class == LoadStateClass::Running,
            Some(LoadOutcomeFilter::Filtered) => record
                .filtered_rows
                .as_deref()
                .is_some_and(|rows| rows != "0"),
        })
        .filter(|record| {
            needle.as_deref().is_none_or(|needle| {
                format!("{}.{}", record.db, record.table)
                    .to_lowercase()
                    .contains(needle)
            })
        })
        .filter(|record| {
            request.created_hour.as_deref().is_none_or(|hour| {
                record
                    .create_time
                    .as_deref()
                    .is_some_and(|created| created.get(..13) == Some(hour))
            })
        })
        .collect();
    let mut tables: BTreeMap<String, TableTally> = BTreeMap::new();
    for record in &visible {
        tables
            .entry(format!("{}.{}", record.db, record.table))
            .or_default()
            .add(record);
    }
    let mut table_rows: Vec<(String, TableTally)> = tables.into_iter().collect();
    table_rows.sort_by(|a, b| b.1.loads.cmp(&a.1.loads).then(a.0.cmp(&b.0)));
    LoadRecordsResponse {
        support,
        fe_time_zone,
        matched: matched.len().to_string(),
        records: matched
            .into_iter()
            .take(LOAD_RECORDS_MAX)
            .cloned()
            .collect(),
        summary: summary_of(&visible),
        tables: table_rows
            .into_iter()
            .take(10)
            .map(|(table, tally)| LoadTableSummary {
                table,
                loads: tally.loads.to_string(),
                failed: tally.failed.to_string(),
                filtered_rows: tally.filtered.to_string(),
                filtered_unknown: tally.filtered_unknown.to_string(),
            })
            .collect(),
        earliest_create_time: earliest,
        at_retention_limit,
        hidden_system: hidden_system.to_string(),
        limitations: RECORD_LIMITATIONS.into_iter().map(str::to_owned).collect(),
    }
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Hours since the epoch of a `YYYY-MM-DD HH` key; the zone is the FE's
/// and is never converted, only counted along.
fn hour_index(key: &str) -> Option<i64> {
    if !doctor_contracts::valid_load_hour(key) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| key.get(range)?.parse::<i64>().ok();
    let (year, month, day, hour) = (
        number(0..4)?,
        number(5..7)?,
        number(8..10)?,
        number(11..13)?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 24 + hour)
}

fn hour_key(index: i64) -> String {
    let (year, month, day) = civil_from_days(index.div_euclid(24));
    format!("{year:04}-{month:02}-{day:02} {:02}", index.rem_euclid(24))
}

/// The smallest of 2, 4, 10, 20, 40, 100, ... at or above `max`, so the
/// half tick stays a whole number.
fn axis_top(max: u128) -> u128 {
    let mut scale = 1u128;
    loop {
        for step in [2u128, 4, 10] {
            if step * scale >= max {
                return step * scale;
            }
        }
        scale = scale.saturating_mul(10);
    }
}

fn axis_ticks(top: u128) -> Vec<String> {
    vec![top.to_string(), (top / 2).to_string(), "0".to_owned()]
}

fn permyriad(value: u128, top: u128) -> u16 {
    if top == 0 {
        return 0;
    }
    u16::try_from((value.saturating_mul(10_000) / top).min(10_000)).unwrap_or(10_000)
}

/// Log-scaled bar length, so tens and hundreds of thousands share an axis.
fn log_permyriad(value: u128, max: u128) -> u16 {
    if value == 0 || max == 0 {
        return 0;
    }
    let share = ((value as f64) + 1.0).ln() / ((max as f64) + 1.0).ln();
    (share * 10_000.0).round().clamp(1.0, 10_000.0) as u16
}

fn count_of(value: Option<&str>) -> u128 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

/// Phase histogram edges in milliseconds; the last bin is open.
const PHASE_EDGES_MS: [u64; 7] = [0, 10, 50, 100, 500, 1_000, 5_000];

fn phase_distribution(records: &[&LoadRecord]) -> LoadPhaseDistribution {
    let mut begin: Vec<u64> = records
        .iter()
        .filter_map(|record| parse_ms(record.begin_txn_ms.as_deref()))
        .collect();
    let mut plan: Vec<u64> = records
        .iter()
        .filter_map(|record| parse_ms(record.plan_ms.as_deref()))
        .collect();
    begin.sort_unstable();
    plan.sort_unstable();
    let bin_of = |value: u64| {
        PHASE_EDGES_MS
            .iter()
            .rposition(|edge| value >= *edge)
            .unwrap_or(0)
    };
    let mut counts = [(0u128, 0u128); PHASE_EDGES_MS.len()];
    for value in &begin {
        counts[bin_of(*value)].0 += 1;
    }
    for value in &plan {
        counts[bin_of(*value)].1 += 1;
    }
    let top = axis_top(counts.iter().map(|(b, p)| (*b).max(*p)).max().unwrap_or(0));
    let quantiles = |sorted: &[u64]| LoadPhaseQuantiles {
        samples: sorted.len().to_string(),
        p50_ms: nearest_rank(sorted, 50),
        p95_ms: nearest_rank(sorted, 95),
    };
    LoadPhaseDistribution {
        bins: counts
            .iter()
            .enumerate()
            .map(|(index, (b, p))| LoadPhaseBin {
                lower_ms: PHASE_EDGES_MS[index].to_string(),
                upper_ms: PHASE_EDGES_MS.get(index + 1).map(u64::to_string),
                begin_txn: b.to_string(),
                plan: p.to_string(),
                begin_txn_permyriad: permyriad(*b, top),
                plan_permyriad: permyriad(*p, top),
            })
            .collect(),
        axis_ticks: axis_ticks(top),
        begin_txn: quantiles(&begin),
        plan: quantiles(&plan),
    }
}

#[derive(Default)]
struct HourTally {
    loads: u128,
    failed: u128,
    filtered: u128,
    filtered_unknown: u128,
    failed_tables: BTreeMap<String, u128>,
}

fn record_hour(record: &LoadRecord) -> Option<i64> {
    record
        .create_time
        .as_deref()
        .and_then(|created| created.get(..13))
        .and_then(hour_index)
}

/// Hourly buckets, oldest first: 72 ending at the newest record's hour,
/// widened (up to 96) so no held record in scope falls before the first
/// bucket. Hours before `known_from` are unknown, not zero; hours before
/// `complete_from` keep their held counts but are partial.
fn hour_buckets(
    records: &[&LoadRecord],
    known_from: Option<i64>,
    complete_from: Option<i64>,
) -> (Vec<LoadHourBucket>, Vec<String>, usize) {
    let mut tallies: BTreeMap<i64, HourTally> = BTreeMap::new();
    let mut unplaced = 0usize;
    for record in records {
        let Some(index) = record_hour(record) else {
            unplaced += 1;
            continue;
        };
        let tally = tallies.entry(index).or_default();
        tally.loads += 1;
        match record
            .filtered_rows
            .as_deref()
            .and_then(|v| v.parse::<u128>().ok())
        {
            Some(rows) => tally.filtered += rows,
            None => tally.filtered_unknown += 1,
        }
        if record.state_class == LoadStateClass::Cancelled {
            tally.failed += 1;
            *tally
                .failed_tables
                .entry(format!("{}.{}", record.db, record.table))
                .or_default() += 1;
        }
    }
    let (Some(&oldest), Some(&last)) = (tallies.keys().next(), tallies.keys().next_back()) else {
        return (Vec::new(), axis_ticks(axis_top(0)), unplaced);
    };
    let first = (last - LOAD_OVERVIEW_HOURS as i64 + 1)
        .min(oldest.max(last - LOAD_OVERVIEW_MAX_HOURS as i64 + 1));
    let outside: u128 = tallies.range(..first).map(|(_, tally)| tally.loads).sum();
    let top = axis_top(
        tallies
            .range(first..=last)
            .map(|(_, tally)| tally.loads)
            .max()
            .unwrap_or(0),
    );
    let empty = HourTally::default();
    let buckets = (first..=last)
        .map(|index| {
            let tally = tallies.get(&index).unwrap_or(&empty);
            let worst = tally
                .failed_tables
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)));
            LoadHourBucket {
                hour: hour_key(index),
                coverage: if known_from.is_some_and(|from| index < from) {
                    LoadHourCoverage::Unknown
                } else if complete_from.is_some_and(|from| index < from) {
                    LoadHourCoverage::Partial
                } else {
                    LoadHourCoverage::Complete
                },
                loads: tally.loads.to_string(),
                failed: tally.failed.to_string(),
                filtered_rows: tally.filtered.to_string(),
                filtered_unknown: tally.filtered_unknown.to_string(),
                loads_permyriad: permyriad(tally.loads, top),
                failed_permyriad: permyriad(tally.failed, top),
                top_failed_table: worst.map(|(table, _)| table.clone()),
                top_failed_count: worst.map(|(_, count)| count.to_string()),
            }
        })
        .collect();
    (
        buckets,
        axis_ticks(top),
        unplaced + usize::try_from(outside).unwrap_or(usize::MAX),
    )
}

fn lag_bar(job: &RoutineLoadJob, partition: &RoutineLoadPartition, lag: u128) -> RoutineLagBar {
    RoutineLagBar {
        job_id: job.job_id.clone(),
        job_name: job.name.clone(),
        db: job.db.clone(),
        table: job.table.clone(),
        partition: partition.partition.clone(),
        lag: lag.to_string(),
        state_class: job.state_class,
        consumed_message_ms: partition.consumed_message_ms.clone(),
        width_permyriad: 0,
    }
}

/// Routine Load counts, error rows, the largest partition lags of jobs
/// still alive and the newest paused jobs, from one decoded read.
pub fn routine_overview(
    jobs: &[RoutineLoadJob],
    support: LoadViewSupport,
    truncated: bool,
) -> RoutineLoadOverview {
    let tally = |class: RoutineLoadStateClass| {
        jobs.iter()
            .filter(|job| job.state_class == class)
            .count()
            .to_string()
    };
    let alive = |job: &&RoutineLoadJob| {
        !matches!(
            job.state_class,
            RoutineLoadStateClass::Stopped | RoutineLoadStateClass::Cancelled
        )
    };
    let (mut error_rows, mut error_rows_unknown) = (0u128, 0usize);
    for job in jobs {
        match job
            .statistics
            .error_rows
            .as_deref()
            .and_then(|rows| rows.parse::<u128>().ok())
        {
            Some(rows) => error_rows += rows,
            None => error_rows_unknown += 1,
        }
    }
    let mut bars: Vec<(u128, RoutineLagBar)> = jobs
        .iter()
        .filter(alive)
        .flat_map(|job| {
            job.partitions.iter().filter_map(move |partition| {
                let lag = partition.lag.as_deref()?.parse::<u128>().ok()?;
                Some((lag, lag_bar(job, partition, lag)))
            })
        })
        .collect();
    bars.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.job_name.cmp(&b.1.job_name))
            .then(partition_order(&a.1.partition).cmp(&partition_order(&b.1.partition)))
    });
    let max = bars.first().map_or(0, |(lag, _)| *lag);
    let max_running_lag = bars
        .iter()
        .find(|(_, bar)| bar.state_class == RoutineLoadStateClass::Running)
        .map(|(lag, bar)| RoutineLagBar {
            width_permyriad: log_permyriad(*lag, max),
            ..bar.clone()
        });
    let lags = bars
        .into_iter()
        .take(LOAD_OVERVIEW_LAGS)
        .map(|(lag, mut bar)| {
            bar.width_permyriad = log_permyriad(lag, max);
            bar
        })
        .collect();
    let mut paused: Vec<&RoutineLoadJob> = jobs
        .iter()
        .filter(|job| job.state_class == RoutineLoadStateClass::Paused)
        .collect();
    paused.sort_by(|a, b| b.pause_time.cmp(&a.pause_time).then(a.name.cmp(&b.name)));
    RoutineLoadOverview {
        support,
        running: tally(RoutineLoadStateClass::Running),
        scheduling: tally(RoutineLoadStateClass::Scheduling),
        paused: tally(RoutineLoadStateClass::Paused),
        ended: jobs.iter().filter(|job| !alive(job)).count().to_string(),
        error_rows: error_rows.to_string(),
        error_rows_unknown: error_rows_unknown.to_string(),
        lags,
        max_running_lag,
        paused_jobs: paused
            .into_iter()
            .take(LOAD_OVERVIEW_PAUSED)
            .map(|job| RoutinePausedJob {
                job_id: job.job_id.clone(),
                name: job.name.clone(),
                db: job.db.clone(),
                table: job.table.clone(),
                max_lag: job.max_lag.clone(),
                pause_time: job.pause_time.clone(),
                reason: job.reason.clone(),
            })
            .collect(),
        truncated,
    }
}

fn share(part: u128, whole: u128) -> Option<u16> {
    (whole > 0).then(|| permyriad(part, whole))
}

/// Stream Load and Routine Load tasks share one FE manager, every other
/// kind another; each drops its own oldest records when full.
fn stream_manager(kind: &str) -> bool {
    kind.contains("STREAM_LOAD") || kind == "ROUTINE_LOAD"
}

/// The oldest held record of a set, as `(hour, create_time)`.
fn oldest<'a>(records: impl Iterator<Item = &'a LoadRecord>) -> Option<(i64, String)> {
    records
        .filter_map(|record| Some((record_hour(record)?, record.create_time.clone()?)))
        .min()
}

/// The hour from which every FE manager behind the scope still holds its
/// records: the latest of their oldest records. A manager drops finished
/// records in bulk when busy, so a later oldest record cannot prove the
/// earlier hours had none of its loads.
fn complete_from(all: &[LoadRecord], scope: &[&LoadRecord]) -> Option<(i64, String)> {
    [true, false]
        .into_iter()
        .filter(|stream| {
            scope
                .iter()
                .any(|record| stream_manager(&record.kind) == *stream)
        })
        .filter_map(|stream| {
            oldest(
                all.iter()
                    .filter(|record| stream_manager(&record.kind) == stream),
            )
        })
        .max()
}

/// Task 52: the overview of everything one `loads` read and one
/// `routine_load_jobs` read hold. Every count, bucket and bar length is
/// decided here; the page only lays them out.
pub fn build_load_overview(
    rows: Vec<Vec<Option<String>>>,
    request: &LoadOverviewRequest,
    support: LoadViewSupport,
    fe_time_zone: Option<String>,
    audit_database: Option<&str>,
    routine: &RoutineLoadJobsResponse,
) -> LoadOverviewResponse {
    let ParsedRecords {
        all,
        at_retention_limit,
        ..
    } = parse_load_rows(rows, audit_database);
    let visible = all
        .iter()
        .filter(|record| request.include_system || !record.system)
        .count();
    let scope: Vec<&LoadRecord> = all
        .iter()
        .filter(|record| request.include_system || !record.system)
        .filter(|record| {
            request
                .kind
                .is_none_or(|kind| kind_matches(&record.kind, kind))
        })
        .collect();
    let summary = summary_of(&scope);
    let mut kinds: BTreeMap<String, (u128, u128)> = BTreeMap::new();
    let mut tables: BTreeMap<String, TableTally> = BTreeMap::new();
    for record in &scope {
        let failed = u128::from(record.state_class == LoadStateClass::Cancelled);
        let kind = kinds.entry(record.kind.clone()).or_default();
        kind.0 += 1;
        kind.1 += failed;
        tables
            .entry(format!("{}.{}", record.db, record.table))
            .or_default()
            .add(record);
    }
    let mut by_kind: Vec<(String, (u128, u128))> = kinds.into_iter().collect();
    by_kind.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let mut filtered_tables: Vec<(String, TableTally)> = tables
        .into_iter()
        .filter(|(_, tally)| tally.filtered > 0 || tally.filtered_unknown > 0)
        .collect();
    filtered_tables.sort_by(|a, b| {
        b.1.filtered
            .cmp(&a.1.filtered)
            .then(b.1.loads.cmp(&a.1.loads))
            .then(a.0.cmp(&b.0))
    });
    let widest = filtered_tables
        .first()
        .map_or(0, |(_, tally)| tally.filtered);
    let known = oldest(scope.iter().copied());
    let complete = complete_from(&all, &scope);
    // A boundary inside an hour leaves that hour partial: the manager may
    // have dropped records from its first minutes.
    let complete_hour = complete.as_ref().map(|(hour, created)| {
        if created.get(14..19) == Some("00:00") {
            *hour
        } else {
            hour + 1
        }
    });
    let (hours, hour_axis_ticks, outside) =
        hour_buckets(&scope, known.as_ref().map(|(hour, _)| *hour), complete_hour);
    let mut limitations: Vec<String> = RECORD_LIMITATIONS.into_iter().map(str::to_owned).collect();
    limitations.extend(
        ["hours_by_create_time", "routine_lag_is_current"]
            .into_iter()
            .map(str::to_owned),
    );
    LoadOverviewResponse {
        support,
        fe_time_zone,
        failed_share_permyriad: share(
            count_of(Some(&summary.failed)),
            count_of(Some(&summary.loads)),
        ),
        summary,
        by_kind: by_kind
            .into_iter()
            .map(|(kind, (loads, failed))| LoadKindCount {
                kind,
                loads: loads.to_string(),
                failed: failed.to_string(),
            })
            .collect(),
        hours,
        records_outside_hours: outside.to_string(),
        hour_axis_ticks,
        tables: filtered_tables
            .into_iter()
            .take(LOAD_OVERVIEW_TABLES)
            .map(|(table, tally)| LoadTableBar {
                table,
                loads: tally.loads.to_string(),
                failed: tally.failed.to_string(),
                filtered_rows: tally.filtered.to_string(),
                filtered_unknown: tally.filtered_unknown.to_string(),
                width_permyriad: log_permyriad(tally.filtered, widest),
            })
            .collect(),
        phases: phase_distribution(&scope),
        routine: routine_overview(&routine.jobs, routine.support, routine.truncated),
        earliest_create_time: known.map(|(_, created)| created),
        complete_from: complete.map(|(_, created)| created),
        at_retention_limit,
        hidden_system: (all.len() - visible).to_string(),
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(value: &str) -> Option<String> {
        Some(value.to_owned())
    }

    #[test]
    fn ld04_versions_decide_what_can_be_read() {
        assert_eq!(
            load_support(Some("3.5.14-23a56ec")),
            (LoadViewSupport::Full, LoadViewSupport::Full)
        );
        assert_eq!(
            load_support(Some("4.1.7")),
            (LoadViewSupport::Full, LoadViewSupport::Full)
        );
        assert_eq!(
            load_support(Some("3.2.16")),
            (
                LoadViewSupport::WithoutLag,
                LoadViewSupport::UnsupportedVersion
            )
        );
        assert_eq!(
            load_support(Some("3.1.17")).0,
            LoadViewSupport::UnsupportedVersion
        );
        assert_eq!(load_support(None).1, LoadViewSupport::UnsupportedVersion);
        assert!(!routine_load_jobs_statement(false).contains("OFFSET_LAG"));
        assert!(routine_load_jobs_statement(true).contains("CAST(OFFSET_LAG AS VARCHAR)"));
        assert_eq!(
            load_tracking_log_statement(10256),
            "SELECT TRACKING_LOG FROM information_schema.load_tracking_logs WHERE JOB_ID = 10256"
        );
    }

    /// The row shape measured on official 3.5.14 (task 50).
    fn routine_row() -> Vec<Option<String>> {
        vec![
            s("10276"),
            s("rl_kafka"),
            s("2026-10-30 08:15:05"),
            None,
            None,
            s("lr"),
            s("t"),
            s("RUNNING"),
            s("KAFKA"),
            s("1"),
            s(r#"{"maxErrorNum":"10","desireTaskConcurrentNum":"1"}"#),
            s(
                r#"{"topic":"demo","brokerList":"127.0.0.1:9092","property.sasl.password":"secret"}"#,
            ),
            s(
                r#"{"receivedBytes":2393,"errorRows":1,"committedTaskNum":1,"loadedRows":200,"loadRowsRate":368,"abortedTaskNum":0,"totalRows":201,"unselectedRows":0,"receivedBytesRate":4406,"taskExecuteTimeMs":543}"#,
            ),
            s(r#"{"0":"200","1":"OFFSET_ZERO"}"#),
            s(""),
            s("http://127.0.0.1:8040/api/_load_error_log?file=error_log_13c"),
            s(""),
            s(r#"{"0": "201", "1": "0"}"#),
            s(r#"{"0": "1"}"#),
            s(r#"{"0": "1793348096695"}"#),
        ]
    }

    #[test]
    fn routine_jobs_decode_partitions_lag_and_counters_without_credentials() {
        let response =
            decode_routine_load_jobs(vec![routine_row()], LoadViewSupport::Full, s("UTC"));
        let job = &response.jobs[0];
        assert_eq!(
            (job.name.as_str(), job.state_class),
            ("rl_kafka", RoutineLoadStateClass::Running)
        );
        assert_eq!(job.topic.as_deref(), Some("demo"));
        assert_eq!(job.statistics.loaded_rows.as_deref(), Some("200"));
        assert_eq!(job.statistics.error_rows.as_deref(), Some("1"));
        assert_eq!(job.max_error_number.as_deref(), Some("10"));
        assert_eq!(job.partitions.len(), 2);
        assert_eq!(job.partitions[0].lag.as_deref(), Some("1"));
        assert_eq!(job.partitions[1].consumed.as_deref(), Some("OFFSET_ZERO"));
        assert_eq!(job.partitions[1].lag, None);
        assert_eq!(
            (job.max_lag.as_deref(), job.max_lag_partition.as_deref()),
            (Some("1"), Some("0"))
        );
        assert!(job.has_error_log && job.reason.is_none());
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("secret") && !serialized.contains("9092"));
        // 3.2: no lag columns are read or shown.
        let mut row = routine_row();
        row.truncate(17);
        let response = decode_routine_load_jobs(vec![row], LoadViewSupport::WithoutLag, None);
        assert!(response.jobs[0].partitions.iter().all(|p| p.lag.is_none()));
        assert!(
            response
                .limitations
                .contains(&"offset_lag_requires_3_3".to_owned())
        );
    }

    #[test]
    fn routine_overview_counts_states_ranks_alive_lags_and_lists_paused() {
        let mut paused = routine_row();
        paused[0] = s("10300");
        paused[1] = s("events_rl");
        paused[3] = s("2026-10-30 08:57:04");
        paused[7] = s("PAUSED");
        paused[14] = s("ErrorReason{errCode = 102, msg='too many errors'}");
        paused[18] = s(r#"{"0": "240784"}"#);
        let mut cancelled = routine_row();
        cancelled[0] = s("10200");
        cancelled[1] = s("old_rl");
        cancelled[7] = s("CANCELLED");
        cancelled[18] = s(r#"{"0": "999999"}"#);
        let jobs = decode_routine_load_jobs(
            vec![paused, routine_row(), cancelled],
            LoadViewSupport::Full,
            None,
        );
        // The decoder attaches the summary; the page never sums jobs itself.
        let overview = &jobs.summary;
        assert_eq!(
            (
                overview.error_rows.as_str(),
                overview.error_rows_unknown.as_str()
            ),
            ("3", "0")
        );
        assert_eq!(
            (
                overview.running.as_str(),
                overview.paused.as_str(),
                overview.ended.as_str()
            ),
            ("1", "1", "1")
        );
        // Ended jobs keep their last lag but do not rank.
        assert_eq!(overview.lags.len(), 2);
        assert_eq!(
            (
                overview.lags[0].job_name.as_str(),
                overview.lags[0].lag.as_str()
            ),
            ("events_rl", "240784")
        );
        assert_eq!(overview.lags[0].width_permyriad, 10_000);
        assert!(overview.lags[1].width_permyriad < 1_000);
        let running = overview.max_running_lag.as_ref().unwrap();
        assert_eq!(
            (running.job_name.as_str(), running.lag.as_str()),
            ("rl_kafka", "1")
        );
        assert_eq!(overview.paused_jobs.len(), 1);
        assert_eq!(overview.paused_jobs[0].max_lag.as_deref(), Some("240784"));
        assert!(
            overview.paused_jobs[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("too many errors")
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn load_row(
        id: &str,
        label: &str,
        db: &str,
        kind: &str,
        state: &str,
        filtered: &str,
        details: &str,
        created: &str,
        finished: Option<&str>,
        tracking: bool,
    ) -> Vec<Option<String>> {
        vec![
            s(id),
            s(label),
            None,
            s(db),
            s("t"),
            s("root"),
            s(state),
            s(kind),
            s(filtered),
            s("0"),
            s("4"),
            s(details),
            s(created),
            finished.map(str::to_owned),
            (state == "CANCELLED").then(|| "too many filtered rows".to_owned()),
            tracking.then(|| {
                "select tracking_log from information_schema.load_tracking_logs where job_id=1"
                    .to_owned()
            }),
        ]
    }

    #[test]
    fn ld02_records_keep_unknowns_missing_and_hide_system_loads() {
        let details = r#"{"begin_txn_time_ms": 1, "client_ip": "127.0.0.1", "plan_time_ms": 2, "receive_data_time_ms": 0, "txn_id": 15}"#;
        let rows = vec![
            load_row(
                "12",
                "l_bad2",
                "lr",
                "STREAM_LOAD",
                "CANCELLED",
                "2",
                details,
                "2026-10-30 08:10:32",
                Some("2026-10-30 08:10:35"),
                true,
            ),
            load_row(
                "11",
                "l_ok",
                "lr",
                "STREAM_LOAD",
                "CANCELLED",
                "-1",
                r#"{"txn_id": -1}"#,
                "2026-10-30 08:10:23",
                None,
                true,
            ),
            load_row(
                "10",
                "insert_x",
                "_statistics_",
                "INSERT",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 08:07:46",
                Some("2026-10-30 08:07:46"),
                false,
            ),
            load_row(
                "9",
                "audit_batch",
                "starrocks_audit_db__",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 08:00:00",
                Some("2026-10-30 08:00:01"),
                false,
            ),
        ];
        let request = LoadRecordsRequest {
            connection_id: "c".into(),
            kind: None,
            outcome: None,
            table: None,
            include_system: false,
            created_hour: None,
        };
        let response = decode_load_records(
            rows.clone(),
            &request,
            LoadViewSupport::Full,
            s("UTC"),
            Some("starrocks_audit_db__"),
        );
        assert_eq!(response.records.len(), 2);
        assert_eq!(response.hidden_system, "2");
        let first = &response.records[0];
        assert_eq!(
            (
                first.begin_txn_ms.as_deref(),
                first.plan_ms.as_deref(),
                first.receive_data_ms.as_deref()
            ),
            (Some("1"), Some("2"), Some("0"))
        );
        assert_eq!(first.client_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(first.total_seconds.as_deref(), Some("3"));
        assert!(first.has_error_log && first.state_class == LoadStateClass::Cancelled);
        // -1 is unknown, never zero.
        assert_eq!(response.records[1].filtered_rows, None);
        assert_eq!(response.records[1].total_seconds, None);
        assert_eq!(response.summary.failed, "2");
        assert_eq!(response.summary.filtered_rows, "2");
        assert_eq!(response.summary.begin_plan_p95_ms, None);
        assert_eq!(
            response.earliest_create_time.as_deref(),
            Some("2026-10-30 08:00:00")
        );
        assert!(!response.at_retention_limit);
        let with_system = decode_load_records(
            rows.clone(),
            &LoadRecordsRequest {
                include_system: true,
                ..request.clone()
            },
            LoadViewSupport::Full,
            None,
            Some("starrocks_audit_db__"),
        );
        assert_eq!(with_system.records.len(), 4);
        let filtered = decode_load_records(
            rows.clone(),
            &LoadRecordsRequest {
                outcome: Some(LoadOutcomeFilter::Filtered),
                ..request.clone()
            },
            LoadViewSupport::Full,
            None,
            None,
        );
        assert_eq!(
            filtered
                .records
                .iter()
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>(),
            ["l_bad2"]
        );
        let insert = decode_load_records(
            rows,
            &LoadRecordsRequest {
                kind: Some(LoadKindFilter::Insert),
                include_system: true,
                ..request
            },
            LoadViewSupport::Full,
            None,
            None,
        );
        assert_eq!(insert.matched, "1");
        let hour = decode_load_records(
            rows_for_hour(),
            &LoadRecordsRequest {
                created_hour: Some("2026-10-30 07".into()),
                ..request_all()
            },
            LoadViewSupport::Full,
            None,
            None,
        );
        assert_eq!(hour.matched, "1");
        assert_eq!(hour.records[0].label, "early");
    }

    fn request_all() -> LoadRecordsRequest {
        LoadRecordsRequest {
            connection_id: "c".into(),
            kind: None,
            outcome: None,
            table: None,
            include_system: false,
            created_hour: None,
        }
    }

    fn rows_for_hour() -> Vec<Vec<Option<String>>> {
        vec![
            load_row(
                "2",
                "late",
                "lr",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 08:00:00",
                None,
                false,
            ),
            load_row(
                "1",
                "early",
                "lr",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 07:59:59",
                None,
                false,
            ),
        ]
    }

    #[test]
    fn ld06_overview_buckets_hours_ranks_tables_and_keeps_missing_unknown() {
        let phase = |begin: u64, plan: u64| {
            format!(r#"{{"begin_txn_time_ms": {begin}, "plan_time_ms": {plan}}}"#)
        };
        let rows = vec![
            load_row(
                "5",
                "a",
                "ods",
                "STREAM_LOAD",
                "CANCELLED",
                "40",
                &phase(3, 120),
                "2026-10-31 01:10:00",
                None,
                true,
            ),
            load_row(
                "4",
                "b",
                "ods",
                "STREAM_LOAD",
                "FINISHED",
                "2",
                &phase(8, 6),
                "2026-10-31 01:05:00",
                Some("2026-10-31 01:05:01"),
                false,
            ),
            load_row(
                "3",
                "c",
                "dwd",
                "INSERT",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 23:20:00",
                Some("2026-10-30 23:20:00"),
                false,
            ),
            load_row(
                "2",
                "d",
                "_statistics_",
                "INSERT",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 22:00:00",
                Some("2026-10-30 22:00:00"),
                false,
            ),
            load_row(
                "1",
                "e",
                "ods",
                "STREAM_LOAD",
                "FINISHED",
                "-1",
                &phase(70, 9000),
                "2026-10-30 22:30:00",
                None,
                false,
            ),
        ];
        let request = LoadOverviewRequest {
            connection_id: "c".into(),
            kind: None,
            include_system: false,
        };
        let routine = decode_routine_load_jobs(Vec::new(), LoadViewSupport::Full, None);
        let overview = build_load_overview(
            rows.clone(),
            &request,
            LoadViewSupport::Full,
            s("UTC"),
            None,
            &routine,
        );
        assert_eq!(overview.summary.loads, "4");
        assert_eq!(overview.hidden_system, "1");
        assert_eq!(overview.failed_share_permyriad, Some(2_500));
        // 72 hours ending at the newest record's hour, across midnight.
        assert_eq!(overview.hours.len(), LOAD_OVERVIEW_HOURS);
        let last = overview.hours.last().unwrap();
        assert_eq!(last.hour, "2026-10-31 01");
        assert_eq!((last.loads.as_str(), last.failed.as_str()), ("2", "1"));
        assert_eq!(last.top_failed_table.as_deref(), Some("ods.t"));
        assert_eq!(
            overview.hours[overview.hours.len() - 3].hour,
            "2026-10-30 23"
        );
        assert_eq!(overview.hour_axis_ticks, ["2", "1", "0"]);
        assert_eq!(last.loads_permyriad, 10_000);
        // Before the oldest record in scope (the hidden system load does not
        // count) the hours are unknown, not zero loads.
        let earliest = overview.earliest_create_time.as_deref().unwrap();
        assert_eq!(earliest, "2026-10-30 22:30:00");
        assert_eq!(overview.hours[0].coverage, LoadHourCoverage::Unknown);
        // Both managers hold records from 22:30 on (the hidden statistics
        // INSERT counts for its manager): hour 22 is partial, 23 complete.
        let hour22 = overview
            .hours
            .iter()
            .find(|h| h.hour == "2026-10-30 22")
            .unwrap();
        assert_eq!(hour22.coverage, LoadHourCoverage::Partial);
        let hour23 = overview
            .hours
            .iter()
            .find(|h| h.hour == "2026-10-30 23")
            .unwrap();
        assert_eq!(hour23.coverage, LoadHourCoverage::Complete);
        assert_eq!(
            overview.complete_from.as_deref(),
            Some("2026-10-30 22:30:00")
        );
        // A -1 filtered count stays out of the table bars and totals.
        assert_eq!(overview.tables.len(), 1);
        assert_eq!(overview.tables[0].filtered_rows, "42");
        // The -1 is counted apart, and no share is claimed over it.
        assert_eq!(overview.tables[0].filtered_unknown, "1");
        assert_eq!(overview.summary.filtered_rows_unknown, "1");
        assert_eq!(overview.summary.filtered_share_permyriad, None);
        assert_eq!(overview.records_outside_hours, "0");
        assert_eq!(overview.tables[0].width_permyriad, 10_000);
        assert_eq!(overview.by_kind[0].kind, "STREAM_LOAD");
        let bins = &overview.phases.bins;
        assert_eq!(bins.len(), PHASE_EDGES_MS.len());
        assert_eq!(
            (bins[0].begin_txn.as_str(), bins[0].plan.as_str()),
            ("2", "1")
        );
        assert_eq!(bins.last().unwrap().plan, "1");
        assert_eq!(bins.last().unwrap().upper_ms, None);
        assert_eq!(overview.phases.begin_txn.samples, "3");
        assert_eq!(overview.phases.begin_txn.p95_ms, None);
        let inserts = build_load_overview(
            rows,
            &LoadOverviewRequest {
                kind: Some(LoadKindFilter::Insert),
                ..request
            },
            LoadViewSupport::Full,
            None,
            None,
            &routine,
        );
        assert_eq!(inserts.summary.loads, "1");
        assert_eq!(inserts.failed_share_permyriad, Some(0));
        assert_eq!(inserts.summary.filtered_share_permyriad, Some(0));
        assert!(inserts.tables.is_empty());
    }

    #[test]
    fn overview_hours_cover_every_held_record_and_respect_full_managers() {
        let request = LoadOverviewRequest {
            connection_id: "c".into(),
            kind: None,
            include_system: false,
        };
        let routine = decode_routine_load_jobs(Vec::new(), LoadViewSupport::Full, None);
        // Under 72 hours apart, but 73 natural hours: the first hour is drawn.
        let rows = vec![
            load_row(
                "2",
                "new",
                "lr",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                "{}",
                "2026-10-30 10:30:00",
                None,
                false,
            ),
            load_row(
                "1",
                "old",
                "lr",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                "{}",
                "2026-10-27 10:45:00",
                None,
                false,
            ),
        ];
        let overview =
            build_load_overview(rows, &request, LoadViewSupport::Full, None, None, &routine);
        assert_eq!(overview.hours.len(), 73);
        assert_eq!(overview.hours[0].hour, "2026-10-27 10");
        assert_eq!(overview.records_outside_hours, "0");
        // The oldest record is at 10:45, so its hour may have lost earlier
        // records; every later hour is complete.
        assert_eq!(overview.hours[0].coverage, LoadHourCoverage::Partial);
        assert!(
            overview.hours[1..]
                .iter()
                .all(|hour| hour.coverage == LoadHourCoverage::Complete)
        );
        // The Stream Load manager kept only the last hour; an old INSERT
        // must not make its earlier hours look complete.
        let mut rows: Vec<_> = (0..RETENTION_NEAR_LIMIT)
            .map(|i| {
                load_row(
                    &(i + 10).to_string(),
                    "s",
                    "lr",
                    "STREAM_LOAD",
                    "FINISHED",
                    "0",
                    "{}",
                    "2026-10-30 10:05:00",
                    None,
                    false,
                )
            })
            .collect();
        rows.push(load_row(
            "1",
            "ins",
            "lr",
            "INSERT",
            "FINISHED",
            "0",
            "{}",
            "2026-10-29 08:00:00",
            None,
            false,
        ));
        let overview =
            build_load_overview(rows, &request, LoadViewSupport::Full, None, None, &routine);
        assert_eq!(
            overview.earliest_create_time.as_deref(),
            Some("2026-10-29 08:00:00")
        );
        assert_eq!(
            overview.complete_from.as_deref(),
            Some("2026-10-30 10:05:00")
        );
        let hour = |key: &str| overview.hours.iter().find(|h| h.hour == key).unwrap();
        // The old INSERT stays drawn, as a partial hour; it never proves the
        // Stream Load hours before 10:00 were empty.
        assert_eq!(hour("2026-10-29 08").coverage, LoadHourCoverage::Partial);
        assert_eq!(hour("2026-10-29 08").loads, "1");
        assert_eq!(hour("2026-10-30 09").coverage, LoadHourCoverage::Partial);
        // 10:05 is inside the hour: 10:00-10:05 may be gone too.
        assert_eq!(hour("2026-10-30 10").coverage, LoadHourCoverage::Partial);
        assert_eq!(overview.hours[0].coverage, LoadHourCoverage::Unknown);
    }

    #[test]
    fn overview_axis_and_hour_keys_stay_whole() {
        assert_eq!(axis_top(0), 2);
        assert_eq!(axis_top(3), 4);
        assert_eq!(axis_top(37), 40);
        assert_eq!(axis_top(41), 100);
        assert_eq!(
            hour_key(hour_index("2024-02-29 23").unwrap() + 1),
            "2024-03-01 00"
        );
        assert_eq!(
            hour_key(hour_index("2026-12-31 23").unwrap() + 1),
            "2027-01-01 00"
        );
        assert_eq!(hour_index("2026-13-01 00"), None);
        assert_eq!(log_permyriad(0, 10), 0);
        assert_eq!(log_permyriad(240_784, 240_784), 10_000);
        assert!(log_permyriad(8, 240_784) < 2_000);
    }

    #[test]
    fn begin_plan_p95_needs_twenty_samples() {
        let rows: Vec<_> = (0..40)
            .map(|i| {
                load_row(
                    &i.to_string(),
                    "l",
                    "lr",
                    "STREAM_LOAD",
                    "FINISHED",
                    "0",
                    &format!(r#"{{"begin_txn_time_ms": {i}, "plan_time_ms": 1}}"#),
                    "2026-10-30 08:00:00",
                    Some("2026-10-30 08:00:00"),
                    false,
                )
            })
            .collect();
        let request = LoadRecordsRequest {
            connection_id: "c".into(),
            kind: None,
            outcome: None,
            table: Some("LR.T".into()),
            include_system: false,
            created_hour: None,
        };
        let response = decode_load_records(rows, &request, LoadViewSupport::Full, None, None);
        assert_eq!(response.summary.begin_plan_p95_ms.as_deref(), Some("38"));
        assert_eq!(response.matched, "40");
        assert_eq!(
            civil_seconds("2026-03-01 00:00:00").unwrap()
                - civil_seconds("2026-02-28 23:59:59").unwrap(),
            1
        );
    }
}
