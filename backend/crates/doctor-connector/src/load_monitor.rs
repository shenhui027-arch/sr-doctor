//! Load workspace reads (task 51): fixed `SELECT`s over StarRocks
//! `information_schema.routine_load_jobs`, `loads` and `load_tracking_logs`,
//! and their decoding into contract views. The statements take no user text
//! except a numeric job id. Data source properties are read only for the
//! topic: broker lists and client properties may hold credentials.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    BatchLoadDetail, LOAD_NEAR_TIMEOUT_PERMYRIAD, LOAD_OVERVIEW_HOURS, LOAD_OVERVIEW_LAGS,
    LOAD_OVERVIEW_MAX_HOURS, LOAD_OVERVIEW_PAUSED, LOAD_OVERVIEW_SLOWEST, LOAD_OVERVIEW_TABLES,
    LOAD_RECORDS_MAX, LoadDurationBin, LoadDurationDistribution, LoadFailureReason,
    LoadGroupSummary, LoadGroupTable, LoadHourBucket, LoadHourCoverage, LoadHourSegment,
    LoadKindCount, LoadKindFilter, LoadKindGroup, LoadOutcomeFilter, LoadOverviewRequest,
    LoadOverviewResponse, LoadPhaseBin, LoadPhaseDistribution, LoadPhaseQuantiles, LoadRecord,
    LoadRecordsRequest, LoadRecordsResponse, LoadRecordsSummary, LoadSlowRecord, LoadSource,
    LoadStateClass, LoadStateCount, LoadTableBar, LoadTableSummary, LoadViewSupport,
    LoadVolumeUnit, RoutineJobHealth, RoutineJobIssue, RoutineLagBar, RoutineLoadDetail,
    RoutineLoadJob, RoutineLoadJobsResponse, RoutineLoadOverview, RoutineLoadPartition,
    RoutineLoadStateClass, RoutineLoadStatistics, RoutinePausedJob, StreamLoadDetail,
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
        "SELECT ID, LABEL, PROFILE_ID, DB_NAME, TABLE_NAME, USER, STATE, TYPE, FILTERED_ROWS, UNSELECTED_ROWS, SINK_ROWS, CAST(RUNTIME_DETAILS AS VARCHAR), CAST(CREATE_TIME AS VARCHAR), CAST(LOAD_FINISH_TIME AS VARCHAR), ERROR_MSG, TRACKING_SQL, SCAN_ROWS, SCAN_BYTES, CAST(LOAD_COMMIT_TIME AS VARCHAR), CAST(PROPERTIES AS VARCHAR), PRIORITY FROM information_schema.loads ORDER BY ID DESC LIMIT {LOAD_RECORDS_READ_LIMIT}"
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
                desired_tasks: number_text(properties.get("desireTaskConcurrentNum")),
                max_batch_interval_seconds: number_text(properties.get("maxBatchIntervalS")),
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

/// The RUNTIME_DETAILS keys of a Stream Load's millisecond phases.
const PHASE_KEYS: [&str; 3] = ["begin_txn_time_ms", "plan_time_ms", "receive_data_time_ms"];

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

fn kind_group(kind: &str) -> LoadKindGroup {
    match kind {
        _ if kind.contains("STREAM_LOAD") => LoadKindGroup::StreamLoad,
        "ROUTINE_LOAD" => LoadKindGroup::RoutineLoad,
        "BROKER" => LoadKindGroup::Broker,
        "INSERT" => LoadKindGroup::Insert,
        _ => LoadKindGroup::Other,
    }
}

fn filter_group(filter: LoadKindFilter) -> LoadKindGroup {
    match filter {
        LoadKindFilter::StreamLoad => LoadKindGroup::StreamLoad,
        LoadKindFilter::RoutineLoad => LoadKindGroup::RoutineLoad,
        LoadKindFilter::Broker => LoadKindGroup::Broker,
        LoadKindFilter::Insert => LoadKindGroup::Insert,
    }
}

fn kind_matches(kind: &str, filter: LoadKindFilter) -> bool {
    kind_group(kind) == filter_group(filter)
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
            // `StreamLoadTask.setLoadState` fills the phase timings only from a
            // Stream Load commit attachment and writes 0 until then; a Routine
            // Load task never has one. So a 0 is "not measured", not 0 ms,
            // unless the load finished, or was cancelled with an attachment.
            let raw = |key: &str| number_text(details.get(key));
            let measured = kind != "ROUTINE_LOAD"
                && match load_state_class(&state) {
                    LoadStateClass::Finished => true,
                    LoadStateClass::Cancelled => PHASE_KEYS
                        .iter()
                        .any(|key| raw(key).is_some_and(|value| value != "0")),
                    _ => false,
                };
            let phase = |key: &str| measured.then(|| raw(key)).flatten();
            let create_time = text(&row, 12);
            let finish_time = text(&row, 13);
            if let Some(created) = &create_time
                && earliest.as_ref().is_none_or(|current| created < current)
            {
                earliest = Some(created.clone());
            }
            let commit_time = text(&row, 18);
            let span = |from: Option<&String>, to: Option<&String>| match (
                from.and_then(|value| civil_seconds(value)),
                to.and_then(|value| civil_seconds(value)),
            ) {
                (Some(start), Some(end)) if end >= start => Some((end - start).to_string()),
                _ => None,
            };
            let total_seconds = span(create_time.as_ref(), finish_time.as_ref());
            let execute_seconds = span(create_time.as_ref(), commit_time.as_ref());
            let publish_seconds = span(commit_time.as_ref(), finish_time.as_ref());
            let timeout_seconds = number_text(
                json(text(&row, 19).as_deref())
                    .unwrap_or_default()
                    .get("timeout"),
            )
            .filter(|value| value.parse::<u64>().is_ok_and(|seconds| seconds > 0));
            let timeout_share_permyriad = match (
                total_seconds
                    .as_deref()
                    .and_then(|v| v.parse::<u128>().ok()),
                timeout_seconds
                    .as_deref()
                    .and_then(|v| v.parse::<u128>().ok()),
            ) {
                (Some(total), Some(timeout)) if timeout > 0 => {
                    Some(u32::try_from(total.saturating_mul(10_000) / timeout).unwrap_or(u32::MAX))
                }
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
                begin_txn_ms: phase("begin_txn_time_ms"),
                plan_ms: phase("plan_time_ms"),
                receive_data_ms: phase("receive_data_time_ms"),
                create_time,
                finish_time,
                total_seconds,
                scan_rows: count(&row, 16),
                scan_bytes: count(&row, 17),
                commit_time,
                execute_seconds,
                publish_seconds,
                timeout_seconds,
                timeout_share_permyriad,
                priority: text(&row, 20),
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
    let mut receive: Vec<u64> = records
        .iter()
        .filter_map(|record| parse_ms(record.receive_data_ms.as_deref()))
        .collect();
    begin.sort_unstable();
    plan.sort_unstable();
    receive.sort_unstable();
    let bin_of = |value: u64| {
        PHASE_EDGES_MS
            .iter()
            .rposition(|edge| value >= *edge)
            .unwrap_or(0)
    };
    let mut counts = [(0u128, 0u128, 0u128); PHASE_EDGES_MS.len()];
    for value in &begin {
        counts[bin_of(*value)].0 += 1;
    }
    for value in &plan {
        counts[bin_of(*value)].1 += 1;
    }
    for value in &receive {
        counts[bin_of(*value)].2 += 1;
    }
    let top = axis_top(
        counts
            .iter()
            .map(|(b, p, r)| (*b).max(*p).max(*r))
            .max()
            .unwrap_or(0),
    );
    let quantiles = |sorted: &[u64]| LoadPhaseQuantiles {
        samples: sorted.len().to_string(),
        p50_ms: nearest_rank(sorted, 50),
        p95_ms: nearest_rank(sorted, 95),
    };
    LoadPhaseDistribution {
        bins: counts
            .iter()
            .enumerate()
            .map(|(index, (b, p, r))| LoadPhaseBin {
                lower_ms: PHASE_EDGES_MS[index].to_string(),
                upper_ms: PHASE_EDGES_MS.get(index + 1).map(u64::to_string),
                begin_txn: b.to_string(),
                plan: p.to_string(),
                receive_data: r.to_string(),
                begin_txn_permyriad: permyriad(*b, top),
                plan_permyriad: permyriad(*p, top),
                receive_data_permyriad: permyriad(*r, top),
            })
            .collect(),
        axis_ticks: axis_ticks(top),
        begin_txn: quantiles(&begin),
        plan: quantiles(&plan),
        receive_data: quantiles(&receive),
    }
}

#[derive(Default)]
struct HourTally {
    loads: u128,
    failed: u128,
    filtered: u128,
    filtered_unknown: u128,
    failed_tables: BTreeMap<String, u128>,
    groups: BTreeMap<LoadKindGroup, u128>,
    volume: u128,
    volume_unknown: u128,
}

struct HourChart {
    buckets: Vec<LoadHourBucket>,
    ticks: Vec<String>,
    volume_ticks: Vec<String>,
    outside: usize,
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
    volume: fn(&LoadRecord) -> Option<&str>,
) -> HourChart {
    let mut tallies: BTreeMap<i64, HourTally> = BTreeMap::new();
    let mut unplaced = 0usize;
    for record in records {
        let Some(index) = record_hour(record) else {
            unplaced += 1;
            continue;
        };
        let tally = tallies.entry(index).or_default();
        tally.loads += 1;
        *tally.groups.entry(kind_group(&record.kind)).or_default() += 1;
        add(&mut tally.volume, &mut tally.volume_unknown, volume(record));
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
        return HourChart {
            buckets: Vec::new(),
            ticks: axis_ticks(axis_top(0)),
            volume_ticks: axis_ticks(axis_top(0)),
            outside: unplaced,
        };
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
    let volume_top = axis_top(
        tallies
            .range(first..=last)
            .map(|(_, tally)| tally.volume)
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
            let volume_known = tally.loads == 0 || tally.volume_unknown < tally.loads;
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
                segments: tally
                    .groups
                    .iter()
                    .map(|(group, loads)| LoadHourSegment {
                        group: *group,
                        loads: loads.to_string(),
                        permyriad: permyriad(*loads, top),
                    })
                    .collect(),
                volume: volume_known.then(|| tally.volume.to_string()),
                volume_unknown: tally.volume_unknown.to_string(),
                volume_permyriad: volume_known.then(|| permyriad(tally.volume, volume_top)),
            }
        })
        .collect();
    HourChart {
        buckets,
        ticks: axis_ticks(top),
        volume_ticks: axis_ticks(volume_top),
        outside: unplaced + usize::try_from(outside).unwrap_or(usize::MAX),
    }
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
        jobs: jobs.len().to_string(),
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

fn number(value: Option<&str>) -> Option<u128> {
    value.and_then(|value| value.parse::<u128>().ok())
}

fn record_bytes(record: &LoadRecord) -> Option<&str> {
    record.scan_bytes.as_deref()
}

fn record_rows(record: &LoadRecord) -> Option<&str> {
    record.loaded_rows.as_deref()
}

fn record_total(record: &LoadRecord) -> Option<&str> {
    record.total_seconds.as_deref()
}

fn record_publish(record: &LoadRecord) -> Option<&str> {
    record.publish_seconds.as_deref()
}

/// Second-precision timings of finished records only: a running or
/// cancelled load has no comparable end.
fn finished_seconds(records: &[&LoadRecord], pick: fn(&LoadRecord) -> Option<&str>) -> Vec<u64> {
    let mut values: Vec<u64> = records
        .iter()
        .filter(|record| record.state_class == LoadStateClass::Finished)
        .filter_map(|record| parse_ms(pick(record)))
        .collect();
    values.sort_unstable();
    values
}

/// Total-time bins for Stream Load, INSERT and Broker Load (seconds).
const TOTAL_EDGES_S: [u64; 6] = [0, 1, 10, 60, 300, 1_800];
/// Routine Load tasks run for about `max_batch_interval` (default 10 s).
const TASK_EDGES_S: [u64; 6] = [0, 5, 10, 15, 30, 60];

fn duration_distribution(sorted: &[u64], edges: &[u64]) -> LoadDurationDistribution {
    let mut counts = vec![0u128; edges.len()];
    for value in sorted {
        counts[edges.iter().rposition(|edge| value >= edge).unwrap_or(0)] += 1;
    }
    let top = counts.iter().copied().max().unwrap_or(0);
    LoadDurationDistribution {
        bins: counts
            .iter()
            .enumerate()
            .map(|(index, loads)| LoadDurationBin {
                lower_seconds: edges[index].to_string(),
                upper_seconds: edges.get(index + 1).map(u64::to_string),
                loads: loads.to_string(),
                permyriad: permyriad(*loads, top),
            })
            .collect(),
        samples: sorted.len().to_string(),
        p50_seconds: nearest_rank(sorted, 50),
        p95_seconds: nearest_rank(sorted, 95),
        max_seconds: sorted.last().map(u64::to_string),
    }
}

#[derive(Default)]
struct GroupTally {
    loads: u128,
    failed: u128,
    sink: u128,
    sink_unknown: u128,
    filtered: u128,
    filtered_unknown: u128,
}

/// Every group among `records`, in `LoadKindGroup` order.
fn group_summaries(records: &[&LoadRecord]) -> Vec<LoadGroupSummary> {
    let mut tallies: BTreeMap<LoadKindGroup, (GroupTally, Vec<&LoadRecord>)> = BTreeMap::new();
    for record in records {
        let (tally, members) = tallies.entry(kind_group(&record.kind)).or_default();
        tally.loads += 1;
        tally.failed += u128::from(record.state_class == LoadStateClass::Cancelled);
        add(
            &mut tally.sink,
            &mut tally.sink_unknown,
            record.loaded_rows.as_deref(),
        );
        add(
            &mut tally.filtered,
            &mut tally.filtered_unknown,
            record.filtered_rows.as_deref(),
        );
        members.push(record);
    }
    tallies
        .into_iter()
        .map(|(group, (tally, members))| LoadGroupSummary {
            group,
            loads: tally.loads.to_string(),
            failed: tally.failed.to_string(),
            sink_rows: tally.sink.to_string(),
            sink_rows_unknown: tally.sink_unknown.to_string(),
            filtered_rows: tally.filtered.to_string(),
            filtered_unknown: tally.filtered_unknown.to_string(),
            total_p95_seconds: nearest_rank(&finished_seconds(&members, record_total), 95),
        })
        .collect()
}

/// Stream Load errors are free text from a few fixed sources; INSERT and
/// Broker Load write `type:<CancelType>; msg:<text>`.
fn failure_reason(record: &LoadRecord) -> String {
    let message = record.error_message.as_deref().unwrap_or_default();
    match kind_group(&record.kind) {
        LoadKindGroup::StreamLoad | LoadKindGroup::RoutineLoad => {
            // StarRocks quotes the label as `[label]`; it is user text and may
            // hold any of these words.
            let lower = message
                .replace(&format!("[{}]", record.label), "")
                .to_ascii_lowercase();
            // `LabelAlreadyUsedException`: "Label [..] has already been used."
            if lower.contains("has already been used") || lower.contains("already exist") {
                "label_exists"
            } else if lower.contains("too many filtered rows") {
                "filtered_ratio"
            } else if lower.contains("timeout") || lower.contains("timed out") {
                "timeout"
            } else {
                "other"
            }
            .to_owned()
        }
        _ => message
            .strip_prefix("type:")
            .and_then(|rest| rest.split(';').next())
            .map(str::trim)
            .filter(|kind| {
                (1..=40).contains(&kind.len())
                    && kind
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            })
            .map_or_else(|| "other".to_owned(), str::to_owned),
    }
}

/// Failures of one reason, and the finish time and table of its newest.
type ReasonTally = (u128, Option<String>, String);
/// `(client ip, user)` and their `(loads, failed)`.
type SourceTally = ((Option<String>, Option<String>), (u128, u128));

/// The newest of a reason is the one that finished last; both times are
/// `YYYY-MM-DD HH:MM:SS`, so they compare as text.
fn failure_reasons(records: &[&LoadRecord]) -> Vec<LoadFailureReason> {
    let mut reasons: BTreeMap<String, ReasonTally> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.state_class == LoadStateClass::Cancelled)
    {
        let time = record
            .finish_time
            .clone()
            .or_else(|| record.create_time.clone());
        let entry = reasons
            .entry(failure_reason(record))
            .or_insert_with(|| (0, None, String::new()));
        entry.0 += 1;
        if entry.0 == 1 || time > entry.1 {
            entry.1 = time;
            entry.2 = format!("{}.{}", record.db, record.table);
        }
    }
    let mut rows: Vec<(String, ReasonTally)> = reasons.into_iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let top = rows.first().map_or(0, |(_, (loads, _, _))| *loads);
    rows.into_iter()
        .map(|(reason, (loads, time, table))| LoadFailureReason {
            reason,
            loads: loads.to_string(),
            latest_time: time,
            latest_table: Some(table),
            width_permyriad: permyriad(loads, top),
        })
        .collect()
}

fn slow_record(record: &LoadRecord, longest: u128) -> LoadSlowRecord {
    LoadSlowRecord {
        job_id: record.job_id.clone(),
        label: record.label.clone(),
        table: format!("{}.{}", record.db, record.table),
        total_seconds: record.total_seconds.clone().unwrap_or_default(),
        execute_seconds: record.execute_seconds.clone(),
        publish_seconds: record.publish_seconds.clone(),
        timeout_seconds: record.timeout_seconds.clone(),
        timeout_share_permyriad: record.timeout_share_permyriad,
        execute_permyriad: permyriad(count_of(record.execute_seconds.as_deref()), longest),
        publish_permyriad: permyriad(count_of(record.publish_seconds.as_deref()), longest),
    }
}

/// The slowest finished records, longest first.
fn slowest(records: &[&LoadRecord]) -> Vec<LoadSlowRecord> {
    let mut finished: Vec<(u128, &LoadRecord)> = records
        .iter()
        .filter(|record| record.state_class == LoadStateClass::Finished)
        .filter_map(|record| Some((number(record.total_seconds.as_deref())?, *record)))
        .collect();
    finished.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.job_id.cmp(&b.1.job_id)));
    let longest = finished.first().map_or(0, |(total, _)| *total);
    finished
        .into_iter()
        .take(LOAD_OVERVIEW_SLOWEST)
        .map(|(_, record)| slow_record(record, longest))
        .collect()
}

/// Records that used at least `LOAD_NEAR_TIMEOUT_PERMYRIAD` of their
/// timeout, highest share first, and how many there are.
fn near_timeout(records: &[&LoadRecord]) -> (usize, Vec<LoadSlowRecord>) {
    let mut near: Vec<&LoadRecord> = records
        .iter()
        .copied()
        .filter(|record| {
            record
                .timeout_share_permyriad
                .is_some_and(|share| share >= LOAD_NEAR_TIMEOUT_PERMYRIAD)
        })
        .collect();
    near.sort_by(|a, b| {
        b.timeout_share_permyriad
            .cmp(&a.timeout_share_permyriad)
            .then(a.job_id.cmp(&b.job_id))
    });
    let longest = near
        .iter()
        .filter_map(|record| number(record.total_seconds.as_deref()))
        .max()
        .unwrap_or(0);
    (
        near.len(),
        near.into_iter()
            .take(LOAD_OVERVIEW_SLOWEST)
            .map(|record| slow_record(record, longest))
            .collect(),
    )
}

#[derive(Default)]
struct GroupTableTally<'a> {
    loads: u128,
    failed: u128,
    scan_rows: u128,
    scan_bytes: u128,
    sink_rows: u128,
    filtered: u128,
    unknown: u128,
    members: Vec<&'a LoadRecord>,
    priorities: BTreeMap<String, u128>,
}

/// Per-table counts of one group. `stream` decides which counts the group
/// reports (Stream Load has no scanned rows) and the order: most loads for
/// Stream Load, most rows written for INSERT and Broker Load.
fn group_tables(records: &[&LoadRecord], stream: bool) -> Vec<LoadGroupTable> {
    let mut tables: BTreeMap<String, GroupTableTally> = BTreeMap::new();
    for record in records {
        let tally = tables
            .entry(format!("{}.{}", record.db, record.table))
            .or_default();
        tally.loads += 1;
        tally.failed += u128::from(record.state_class == LoadStateClass::Cancelled);
        let mut missing = false;
        let mut sum = |total: &mut u128, value: Option<&str>| match number(value) {
            Some(value) => *total += value,
            None => missing = true,
        };
        if !stream {
            sum(&mut tally.scan_rows, record.scan_rows.as_deref());
        }
        sum(&mut tally.scan_bytes, record.scan_bytes.as_deref());
        sum(&mut tally.sink_rows, record.loaded_rows.as_deref());
        sum(&mut tally.filtered, record.filtered_rows.as_deref());
        tally.unknown += u128::from(missing);
        if let Some(priority) = &record.priority {
            *tally.priorities.entry(priority.clone()).or_default() += 1;
        }
        tally.members.push(record);
    }
    let mut rows: Vec<(String, GroupTableTally)> = tables.into_iter().collect();
    rows.sort_by(|a, b| {
        let key = |tally: &GroupTableTally| if stream { tally.loads } else { tally.sink_rows };
        key(&b.1).cmp(&key(&a.1)).then(a.0.cmp(&b.0))
    });
    rows.into_iter()
        .take(LOAD_OVERVIEW_TABLES)
        .map(|(table, tally)| {
            let mut plans: Vec<u64> = tally
                .members
                .iter()
                .filter_map(|record| parse_ms(record.plan_ms.as_deref()))
                .collect();
            plans.sort_unstable();
            LoadGroupTable {
                table,
                loads: tally.loads.to_string(),
                failed: tally.failed.to_string(),
                scan_rows: tally.scan_rows.to_string(),
                scan_bytes: tally.scan_bytes.to_string(),
                sink_rows: tally.sink_rows.to_string(),
                filtered_rows: tally.filtered.to_string(),
                counts_unknown: tally.unknown.to_string(),
                total_p95_seconds: nearest_rank(
                    &finished_seconds(&tally.members, record_total),
                    95,
                ),
                plan_p95_ms: stream.then(|| nearest_rank(&plans, 95)).flatten(),
                priority: (!stream)
                    .then(|| {
                        tally
                            .priorities
                            .iter()
                            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                            .map(|(priority, _)| priority.clone())
                    })
                    .flatten(),
            }
        })
        .collect()
}

fn stream_detail(records: &[&LoadRecord]) -> StreamLoadDetail {
    let (mut bytes, mut bytes_unknown, mut unselected, mut unselected_unknown) =
        (0u128, 0u128, 0u128, 0u128);
    let mut sources: BTreeMap<(Option<String>, Option<String>), (u128, u128)> = BTreeMap::new();
    for record in records {
        add(&mut bytes, &mut bytes_unknown, record.scan_bytes.as_deref());
        add(
            &mut unselected,
            &mut unselected_unknown,
            record.unselected_rows.as_deref(),
        );
        let source = sources
            .entry((record.client_ip.clone(), record.user.clone()))
            .or_default();
        source.0 += 1;
        source.1 += u128::from(record.state_class == LoadStateClass::Cancelled);
    }
    let mut sources: Vec<SourceTally> = sources.into_iter().collect();
    sources.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let (near_count, near) = near_timeout(records);
    StreamLoadDetail {
        received_bytes: bytes.to_string(),
        received_bytes_unknown: bytes_unknown.to_string(),
        unselected_rows: unselected.to_string(),
        unselected_unknown: unselected_unknown.to_string(),
        total: duration_distribution(&finished_seconds(records, record_total), &TOTAL_EDGES_S),
        publish_p95_seconds: nearest_rank(&finished_seconds(records, record_publish), 95),
        near_timeout_count: near_count.to_string(),
        near_timeout: near,
        failure_reasons: failure_reasons(records),
        tables: group_tables(records, true),
        sources: sources
            .into_iter()
            .take(LOAD_OVERVIEW_TABLES)
            .map(|((client_ip, user), (loads, failed))| LoadSource {
                client_ip,
                user,
                loads: loads.to_string(),
                failed: failed.to_string(),
            })
            .collect(),
    }
}

fn batch_detail(records: &[&LoadRecord]) -> BatchLoadDetail {
    let (mut scan_rows, mut scan_bytes, mut unknown) = (0u128, 0u128, 0u128);
    let mut running: BTreeMap<String, u128> = BTreeMap::new();
    for record in records {
        let rows = number(record.scan_rows.as_deref());
        let bytes = number(record.scan_bytes.as_deref());
        scan_rows += rows.unwrap_or(0);
        scan_bytes += bytes.unwrap_or(0);
        unknown += u128::from(rows.is_none() || bytes.is_none());
        if record.state_class == LoadStateClass::Running {
            *running.entry(record.state.clone()).or_default() += 1;
        }
    }
    let (near_count, _) = near_timeout(records);
    BatchLoadDetail {
        scan_rows: scan_rows.to_string(),
        scan_bytes: scan_bytes.to_string(),
        counts_unknown: unknown.to_string(),
        duration: duration_distribution(&finished_seconds(records, record_total), &TOTAL_EDGES_S),
        slowest: slowest(records),
        near_timeout_count: near_count.to_string(),
        failure_reasons: failure_reasons(records),
        running_states: running
            .into_iter()
            .map(|(state, loads)| LoadStateCount {
                state,
                loads: loads.to_string(),
            })
            .collect(),
        tables: group_tables(records, false),
    }
}

fn job_health(job: &RoutineLoadJob) -> RoutineJobHealth {
    let statistics = &job.statistics;
    let committed = number(statistics.committed_tasks.as_deref());
    let aborted = number(statistics.aborted_tasks.as_deref());
    // StarRocks starts the execution time at 1 ms and divides by it, so its
    // rates mean nothing before the first committed task.
    let measured = committed.is_some_and(|tasks| tasks > 0);
    RoutineJobHealth {
        job_id: job.job_id.clone(),
        name: job.name.clone(),
        db: job.db.clone(),
        table: job.table.clone(),
        state: job.state.clone(),
        state_class: job.state_class,
        current_tasks: job.current_tasks.clone(),
        desired_tasks: job.desired_tasks.clone(),
        load_rows_rate: statistics.load_rows_rate.clone().filter(|_| measured),
        received_bytes_rate: statistics.received_bytes_rate.clone().filter(|_| measured),
        loaded_rows: statistics.loaded_rows.clone(),
        total_rows: statistics.total_rows.clone(),
        error_rows: statistics.error_rows.clone(),
        error_share_permyriad: match (
            number(statistics.error_rows.as_deref()),
            number(statistics.total_rows.as_deref()),
        ) {
            (Some(errors), Some(total)) => share(errors, total),
            _ => None,
        },
        committed_tasks: statistics.committed_tasks.clone(),
        aborted_tasks: statistics.aborted_tasks.clone(),
        abort_share_permyriad: match (committed, aborted) {
            (Some(committed), Some(aborted)) => share(aborted, committed + aborted),
            _ => None,
        },
        average_task_ms: match (
            committed.filter(|tasks| *tasks > 0),
            number(statistics.task_execute_time_ms.as_deref()),
        ) {
            (Some(tasks), Some(total)) => Some((total / tasks).to_string()),
            _ => None,
        },
        max_batch_interval_seconds: job.max_batch_interval_seconds.clone(),
        max_lag: job.max_lag.clone(),
        max_lag_partition: job.max_lag_partition.clone(),
        newest_consumed_ms: job
            .partitions
            .iter()
            .filter_map(|partition| {
                partition
                    .consumed_message_ms
                    .as_deref()?
                    .parse::<i64>()
                    .ok()
            })
            .max()
            .map(|ms| ms.to_string()),
    }
}

fn routine_detail(jobs: &[RoutineLoadJob], tasks: &[&LoadRecord]) -> RoutineLoadDetail {
    let (mut errors, mut total, mut committed, mut aborted) = (0u128, 0u128, 0u128, 0u128);
    let (mut executed, mut executed_tasks) = (0u128, 0u128);
    let (mut rows_unknown, mut tasks_unknown) = (0usize, 0usize);
    // The average covers every job or none: a job whose tasks or task time
    // are unknown would leave it silently partial.
    let mut executed_complete = true;
    for job in jobs {
        let statistics = &job.statistics;
        if let (Some(e), Some(t)) = (
            number(statistics.error_rows.as_deref()),
            number(statistics.total_rows.as_deref()),
        ) {
            errors += e;
            total += t;
        } else {
            rows_unknown += 1;
        }
        if let (Some(c), Some(a)) = (
            number(statistics.committed_tasks.as_deref()),
            number(statistics.aborted_tasks.as_deref()),
        ) {
            committed += c;
            aborted += a;
            if c > 0 {
                match number(statistics.task_execute_time_ms.as_deref()) {
                    Some(ms) => {
                        executed += ms;
                        executed_tasks += c;
                    }
                    None => executed_complete = false,
                }
            }
        } else {
            tasks_unknown += 1;
            executed_complete = false;
        }
    }
    let rows_known = rows_unknown < jobs.len();
    let tasks_known = tasks_unknown < jobs.len();
    let mut paused: Vec<&RoutineLoadJob> = jobs
        .iter()
        .filter(|job| job.state_class == RoutineLoadStateClass::Paused)
        .collect();
    paused.sort_by(|a, b| b.pause_time.cmp(&a.pause_time).then(a.name.cmp(&b.name)));
    let mut ended: Vec<&RoutineLoadJob> = jobs
        .iter()
        .filter(|job| {
            matches!(
                job.state_class,
                RoutineLoadStateClass::Stopped | RoutineLoadStateClass::Cancelled
            )
        })
        .collect();
    ended.sort_by(|a, b| b.end_time.cmp(&a.end_time).then(a.name.cmp(&b.name)));
    let issue = |job: &RoutineLoadJob, time: Option<String>| {
        let health = job_health(job);
        RoutineJobIssue {
            job_id: job.job_id.clone(),
            name: job.name.clone(),
            state: job.state.clone(),
            state_class: job.state_class,
            time,
            reason: job.reason.clone(),
            error_share_permyriad: health.error_share_permyriad,
            aborted_tasks: health.aborted_tasks,
        }
    };
    RoutineLoadDetail {
        jobs: jobs.iter().map(job_health).collect(),
        error_rows: rows_known.then(|| errors.to_string()),
        total_rows: rows_known.then(|| total.to_string()),
        rows_unknown: rows_unknown.to_string(),
        error_share_permyriad: share(errors, total),
        committed_tasks: tasks_known.then(|| committed.to_string()),
        aborted_tasks: tasks_known.then(|| aborted.to_string()),
        tasks_unknown: tasks_unknown.to_string(),
        abort_share_permyriad: share(aborted, committed + aborted),
        average_task_ms: (executed_complete && executed_tasks > 0)
            .then(|| (executed / executed_tasks).to_string()),
        held_tasks: tasks.len().to_string(),
        tasks: duration_distribution(&finished_seconds(tasks, record_total), &TASK_EDGES_S),
        issues: paused
            .into_iter()
            .map(|job| issue(job, job.pause_time.clone()))
            .chain(
                ended
                    .into_iter()
                    .map(|job| issue(job, job.end_time.clone())),
            )
            .take(LOAD_OVERVIEW_SLOWEST)
            .collect(),
    }
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
    let visible: Vec<&LoadRecord> = all
        .iter()
        .filter(|record| request.include_system || !record.system)
        .collect();
    let scope: Vec<&LoadRecord> = visible
        .iter()
        .copied()
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
    let group = request.kind.map(filter_group);
    let (volume_unit, volume): (LoadVolumeUnit, fn(&LoadRecord) -> Option<&str>) =
        if group == Some(LoadKindGroup::StreamLoad) {
            (LoadVolumeUnit::Bytes, record_bytes)
        } else {
            (LoadVolumeUnit::Rows, record_rows)
        };
    let chart = hour_buckets(
        &scope,
        known.as_ref().map(|(hour, _)| *hour),
        complete_hour,
        volume,
    );
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
        hours: chart.buckets,
        records_outside_hours: chart.outside.to_string(),
        hour_axis_ticks: chart.ticks,
        groups: group_summaries(&visible),
        all_loads: visible.len().to_string(),
        volume_unit,
        volume_axis_ticks: chart.volume_ticks,
        stream: (group == Some(LoadKindGroup::StreamLoad)).then(|| stream_detail(&scope)),
        batch: matches!(group, Some(LoadKindGroup::Insert | LoadKindGroup::Broker))
            .then(|| batch_detail(&scope)),
        routine_detail: (group == Some(LoadKindGroup::RoutineLoad))
            .then(|| routine_detail(&routine.jobs, &scope)),
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
        hidden_system: (all.len() - visible.len()).to_string(),
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

    /// A `load_row` with the task 56 columns: scan rows and bytes, commit
    /// time, properties and priority.
    fn wide_row(
        mut row: Vec<Option<String>>,
        scan: (&str, &str),
        commit: Option<&str>,
        timeout: &str,
        priority: &str,
    ) -> Vec<Option<String>> {
        row.extend([
            s(scan.0),
            s(scan.1),
            commit.map(str::to_owned),
            Some(format!(
                r#"{{"timeout": {timeout}, "max_filter_ratio": 0.0}}"#
            )),
            s(priority),
        ]);
        row
    }

    #[test]
    fn ld09_overview_splits_groups_and_details_each_type() {
        let stream = |id, details: &str, state, finished, commit| {
            wide_row(
                load_row(
                    id,
                    "s",
                    "ods",
                    "STREAM_LOAD",
                    state,
                    "0",
                    details,
                    "2026-10-30 08:00:00",
                    finished,
                    false,
                ),
                ("0", "1000"),
                commit,
                "10",
                "NORMAL",
            )
        };
        let mut insert_failed = wide_row(
            load_row(
                "4",
                "i_bad",
                "dwd",
                "INSERT",
                "CANCELLED",
                "0",
                "{}",
                "2026-10-30 08:20:00",
                Some("2026-10-30 08:30:00"),
                false,
            ),
            ("50", "500"),
            None,
            "3600",
            "HIGH",
        );
        insert_failed[14] = s("type:TIMEOUT; msg:timeout by txn manager");
        let rows = vec![
            stream(
                "1",
                r#"{"client_ip": "10.0.0.7", "begin_txn_time_ms": 2, "plan_time_ms": 3, "receive_data_time_ms": 40}"#,
                "FINISHED",
                Some("2026-10-30 08:00:09"),
                Some("2026-10-30 08:00:08"),
            ),
            stream(
                "2",
                r#"{"client_ip": "10.0.0.7"}"#,
                "CANCELLED",
                Some("2026-10-30 08:00:01"),
                None,
            ),
            wide_row(
                load_row(
                    "3",
                    "i_ok",
                    "dwd",
                    "INSERT",
                    "FINISHED",
                    "0",
                    "{}",
                    "2026-10-30 08:10:00",
                    Some("2026-10-30 08:12:00"),
                    false,
                ),
                ("900", "-1"),
                Some("2026-10-30 08:11:50"),
                "3600",
                "HIGH",
            ),
            insert_failed,
            wide_row(
                load_row(
                    "5",
                    "i_run",
                    "dwd",
                    "INSERT",
                    "LOADING",
                    "-1",
                    "{}",
                    "2026-10-30 08:40:00",
                    None,
                    false,
                ),
                ("-1", "-1"),
                None,
                "3600",
                "NORMAL",
            ),
            wide_row(
                load_row(
                    "6",
                    "ld_kafka-1",
                    "lr",
                    "ROUTINE_LOAD",
                    "FINISHED",
                    "1",
                    "{}",
                    "2026-10-30 08:50:00",
                    Some("2026-10-30 08:50:07"),
                    false,
                ),
                ("0", "2393"),
                Some("2026-10-30 08:50:07"),
                "60",
                "NORMAL",
            ),
        ];
        let routine = decode_routine_load_jobs(vec![routine_row()], LoadViewSupport::Full, None);
        let request = |kind| LoadOverviewRequest {
            connection_id: "c".into(),
            kind,
            include_system: false,
        };
        let all = build_load_overview(
            rows.clone(),
            &request(None),
            LoadViewSupport::Full,
            None,
            None,
            &routine,
        );
        let groups: Vec<(LoadKindGroup, &str, &str)> = all
            .groups
            .iter()
            .map(|g| (g.group, g.loads.as_str(), g.failed.as_str()))
            .collect();
        assert_eq!(
            groups,
            [
                (LoadKindGroup::StreamLoad, "2", "1"),
                (LoadKindGroup::Insert, "3", "1"),
                (LoadKindGroup::RoutineLoad, "1", "0"),
            ]
        );
        let last = all.hours.last().unwrap();
        assert_eq!(last.segments.len(), 3);
        assert_eq!(all.volume_unit, LoadVolumeUnit::Rows);
        assert!(all.stream.is_none() && all.batch.is_none() && all.routine_detail.is_none());

        // Stream Load: bytes, the receive phase, timeout share and sources.
        let overview = build_load_overview(
            rows.clone(),
            &request(Some(LoadKindFilter::StreamLoad)),
            LoadViewSupport::Full,
            None,
            None,
            &routine,
        );
        assert_eq!(overview.volume_unit, LoadVolumeUnit::Bytes);
        assert_eq!(
            overview.hours.last().unwrap().volume.as_deref(),
            Some("2000")
        );
        assert_eq!(overview.phases.receive_data.samples, "1");
        let detail = overview.stream.unwrap();
        assert_eq!(detail.received_bytes, "2000");
        assert_eq!(detail.near_timeout_count, "1");
        assert_eq!(detail.near_timeout[0].timeout_share_permyriad, Some(9_000));
        assert_eq!(detail.near_timeout[0].publish_seconds.as_deref(), Some("1"));
        assert_eq!(detail.failure_reasons[0].reason, "filtered_ratio");
        assert_eq!(detail.sources[0].client_ip.as_deref(), Some("10.0.0.7"));
        assert_eq!(
            (
                detail.sources[0].loads.as_str(),
                detail.sources[0].failed.as_str()
            ),
            ("2", "1")
        );
        assert_eq!(detail.tables[0].scan_rows, "0");

        // INSERT: seconds, the slowest with its two segments, failure types,
        // running states; an unknown byte count is counted apart.
        let overview = build_load_overview(
            rows.clone(),
            &request(Some(LoadKindFilter::Insert)),
            LoadViewSupport::Full,
            None,
            None,
            &routine,
        );
        let detail = overview.batch.unwrap();
        assert_eq!(detail.duration.samples, "1");
        assert_eq!(detail.duration.max_seconds.as_deref(), Some("120"));
        let slow = &detail.slowest[0];
        assert_eq!(
            (
                slow.execute_seconds.as_deref(),
                slow.publish_seconds.as_deref()
            ),
            (Some("110"), Some("10"))
        );
        assert_eq!(
            (slow.execute_permyriad, slow.publish_permyriad),
            (9_166, 833)
        );
        assert_eq!(detail.failure_reasons[0].reason, "TIMEOUT");
        assert_eq!(detail.running_states[0].state, "LOADING");
        assert_eq!(
            (detail.scan_rows.as_str(), detail.counts_unknown.as_str()),
            ("950", "2")
        );
        assert_eq!(detail.tables[0].priority.as_deref(), Some("HIGH"));

        // Routine Load: lifetime counters per job and the held tasks.
        let overview = build_load_overview(
            rows,
            &request(Some(LoadKindFilter::RoutineLoad)),
            LoadViewSupport::Full,
            None,
            None,
            &routine,
        );
        let detail = overview.routine_detail.unwrap();
        let job = &detail.jobs[0];
        assert_eq!(job.average_task_ms.as_deref(), Some("543"));
        assert_eq!(job.error_share_permyriad, Some(49));
        assert_eq!(job.abort_share_permyriad, Some(0));
        assert_eq!(job.desired_tasks.as_deref(), Some("1"));
        assert_eq!(job.newest_consumed_ms.as_deref(), Some("1793348096695"));
        assert_eq!(detail.held_tasks, "1");
        assert_eq!(detail.tasks.samples, "1");
        assert_eq!(detail.tasks.bins[1].loads, "1");
        assert!(detail.issues.is_empty());
    }

    #[test]
    fn routine_rates_wait_for_a_committed_task() {
        let mut row = routine_row();
        row[12] = s(
            r#"{"loadedRows":0,"loadRowsRate":0,"receivedBytesRate":0,"committedTaskNum":0,"abortedTaskNum":2,"taskExecuteTimeMs":1,"errorRows":0,"totalRows":0}"#,
        );
        row[7] = s("PAUSED");
        row[3] = s("2026-10-30 09:00:00");
        row[14] =
            s("ErrorReason{code=errCode = 4, msg='Job failed to fetch all current partition'}");
        let routine = decode_routine_load_jobs(vec![row], LoadViewSupport::Full, None);
        let detail = routine_detail(&routine.jobs, &[]);
        let job = &detail.jobs[0];
        assert_eq!(
            (
                job.load_rows_rate.as_deref(),
                job.average_task_ms.as_deref()
            ),
            (None, None)
        );
        assert_eq!(job.error_share_permyriad, None);
        assert_eq!(job.abort_share_permyriad, Some(10_000));
        assert_eq!(detail.average_task_ms, None);
        assert_eq!(
            detail.issues[0].time.as_deref(),
            Some("2026-10-30 09:00:00")
        );
        assert_eq!(detail.tasks.samples, "0");
    }

    #[test]
    fn unmeasured_and_partial_values_stay_unknown() {
        // A running Stream Load has no commit attachment yet, so its 0s are
        // not measurements; nor are a cancelled one's all-zero phases.
        let zeros = r#"{"begin_txn_time_ms": 0, "plan_time_ms": 0, "receive_data_time_ms": 0}"#;
        let plan = |state, finished| {
            let row = load_row(
                "1",
                "s",
                "ods",
                "STREAM_LOAD",
                state,
                "0",
                zeros,
                "2026-10-30 08:00:00",
                finished,
                false,
            );
            parse_load_rows(vec![row], None).all[0].plan_ms.clone()
        };
        assert_eq!(plan("LOADING", None), None);
        assert_eq!(plan("CANCELLED", Some("2026-10-30 08:00:01")), None);
        assert_eq!(
            plan("FINISHED", Some("2026-10-30 08:00:01")).as_deref(),
            Some("0")
        );

        // The official duplicate-label message, with "timeout" in the label;
        // and an unknown unselected count.
        let mut duplicate = load_row(
            "2",
            "orders_timeout",
            "ods",
            "STREAM_LOAD",
            "CANCELLED",
            "0",
            "{}",
            "2026-10-30 08:00:00",
            Some("2026-10-30 08:00:01"),
            false,
        );
        duplicate[9] = s("-1");
        duplicate[14] = s("Label [orders_timeout] has already been used.");
        // Past its timeout (150%), then near it (90%).
        let timed = |id, finished| {
            wide_row(
                load_row(
                    id,
                    id,
                    "ods",
                    "STREAM_LOAD",
                    "FINISHED",
                    "0",
                    "{}",
                    "2026-10-30 07:00:00",
                    Some(finished),
                    false,
                ),
                ("0", "10"),
                Some(finished),
                "60",
                "NORMAL",
            )
        };
        let parsed = parse_load_rows(
            vec![
                duplicate,
                timed("3", "2026-10-30 07:00:54"),
                timed("4", "2026-10-30 07:01:30"),
            ],
            None,
        );
        let records: Vec<&LoadRecord> = parsed.all.iter().collect();
        let detail = stream_detail(&records);
        assert_eq!(detail.failure_reasons[0].reason, "label_exists");
        assert_eq!(detail.unselected_unknown, "1");
        let near: Vec<(&str, Option<u32>)> = detail
            .near_timeout
            .iter()
            .map(|record| (record.job_id.as_str(), record.timeout_share_permyriad))
            .collect();
        assert_eq!(near, [("4", Some(15_000)), ("3", Some(9_000))]);

        // The latest failure is the one that finished last, not the newest id.
        let failed = |id, db, created, finished| {
            load_row(
                id,
                id,
                db,
                "INSERT",
                "CANCELLED",
                "0",
                "{}",
                created,
                Some(finished),
                false,
            )
        };
        let parsed = parse_load_rows(
            vec![
                failed("101", "b", "2026-10-30 11:00:00", "2026-10-30 11:01:00"),
                failed("100", "a", "2026-10-30 10:00:00", "2026-10-30 12:00:00"),
            ],
            None,
        );
        let records: Vec<&LoadRecord> = parsed.all.iter().collect();
        let reasons = failure_reasons(&records);
        assert_eq!(
            (
                reasons[0].latest_time.as_deref(),
                reasons[0].latest_table.as_deref()
            ),
            (Some("2026-10-30 12:00:00"), Some("a.t"))
        );

        // A job without its counters is left out and counted, never as 0.
        let job = |id: &str, statistics: &str| {
            let mut row = routine_row();
            row[0] = s(id);
            row[12] = s(statistics);
            row
        };
        let known = job(
            "1",
            r#"{"errorRows":0,"totalRows":100,"committedTaskNum":1,"abortedTaskNum":0,"taskExecuteTimeMs":40}"#,
        );
        let unknown = job("2", r#"{"errorRows":10}"#);
        let routine =
            decode_routine_load_jobs(vec![known, unknown.clone()], LoadViewSupport::Full, None);
        let detail = routine_detail(&routine.jobs, &[]);
        assert_eq!(
            (
                detail.error_rows.as_deref(),
                detail.rows_unknown.as_str(),
                detail.tasks_unknown.as_str()
            ),
            (Some("0"), "1", "1")
        );
        // Partial task counts or a missing task time give no average at all.
        assert_eq!(detail.average_task_ms, None);
        let untimed = job(
            "3",
            r#"{"errorRows":0,"totalRows":10,"committedTaskNum":9,"abortedTaskNum":0}"#,
        );
        let routine = decode_routine_load_jobs(
            vec![
                job(
                    "1",
                    r#"{"errorRows":0,"totalRows":100,"committedTaskNum":1,"abortedTaskNum":0,"taskExecuteTimeMs":100}"#,
                ),
                untimed,
            ],
            LoadViewSupport::Full,
            None,
        );
        let detail = routine_detail(&routine.jobs, &[]);
        assert_eq!(
            (detail.tasks_unknown.as_str(), detail.average_task_ms),
            ("0", None)
        );
        let routine = decode_routine_load_jobs(vec![unknown], LoadViewSupport::Full, None);
        let detail = routine_detail(&routine.jobs, &[]);
        assert_eq!(
            (
                detail.error_rows.as_deref(),
                detail.committed_tasks.as_deref(),
                detail.error_share_permyriad
            ),
            (None, None, None)
        );
    }

    #[test]
    fn routine_load_tasks_have_no_phase_timings() {
        // StarRocks writes 0 for a Routine Load task's phases it never measured;
        // a Stream Load's 0 ms is a real measurement and stays.
        let zeros = r#"{"begin_txn_time_ms": 0, "plan_time_ms": 0, "receive_data_time_ms": 0}"#;
        let rows = vec![
            load_row(
                "2",
                "ld_kafka-1",
                "lr",
                "ROUTINE_LOAD",
                "FINISHED",
                "1",
                zeros,
                "2026-10-30 08:10:00",
                Some("2026-10-30 08:10:05"),
                false,
            ),
            load_row(
                "1",
                "ld_ok",
                "lr",
                "STREAM_LOAD",
                "FINISHED",
                "0",
                zeros,
                "2026-10-30 08:09:00",
                Some("2026-10-30 08:09:01"),
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
            None,
            None,
            &routine,
        );
        assert_eq!(overview.summary.loads, "2");
        assert_eq!(overview.phases.begin_txn.samples, "1");
        assert_eq!(overview.phases.plan.samples, "1");
        let records = decode_load_records(
            rows,
            &LoadRecordsRequest {
                connection_id: "c".into(),
                kind: None,
                outcome: None,
                table: None,
                include_system: false,
                created_hour: None,
            },
            LoadViewSupport::Full,
            None,
            None,
        );
        let kafka = &records.records[0];
        assert_eq!(kafka.kind, "ROUTINE_LOAD");
        assert_eq!(
            (
                kafka.begin_txn_ms.as_deref(),
                kafka.plan_ms.as_deref(),
                kafka.receive_data_ms.as_deref()
            ),
            (None, None, None)
        );
        assert_eq!(kafka.total_seconds.as_deref(), Some("5"));
        assert_eq!(records.records[1].begin_txn_ms.as_deref(), Some("0"));
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
