//! Task 58 (L1): load collection inside the one bounded collector, and the
//! load history views over the local history store. Finished loads are read
//! per settled slice by finish time; Routine Load jobs are snapshotted each
//! cycle; at most a few error logs are read per cycle, one job at a time.
//! A slice is partial when it was cut at the row limit or when an FE manager
//! cleared records into it before it was read; nothing is interpolated.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    CoverageHourState, HistoryLoadCoverage, HistoryLoadSegment, LOAD_HISTORY_ERROR_LOGS_PAGE,
    LOAD_HISTORY_REASONS, LOAD_HISTORY_TABLES, LoadErrorCategory, LoadErrorLogState,
    LoadErrorReasonSummary, LoadErrorStorageView, LoadHistoryCollection, LoadHistoryDay,
    LoadHistoryErrorLogRequest, LoadHistoryErrorLogResponse, LoadHistoryErrorLogsRequest,
    LoadHistoryErrorLogsResponse, LoadHistoryKindDay, LoadHistoryKindTotal, LoadHistoryReason,
    LoadHistoryRequest, LoadHistoryResponse, LoadHistoryRoutineRequest, LoadHistoryRoutineResponse,
    LoadHistoryTable, LoadHistoryTotals, LoadKindGroup, RoutineHistoryCell, RoutineHistoryDay,
    RoutineHistoryDetail, RoutineHistoryEvent, RoutineHistoryJob, RoutineHistoryState,
    RoutineLagPoint, StoredLoadErrorLog, StoredLoadErrorLogState,
};
use doctor_core::{
    HISTORY_BUCKET_SECONDS, LoadBucketDelta, LoadHistoryKind, aggregate_loads,
    history_rule_version, parse_load_error_log,
};
use doctor_history::{
    ERROR_LOG_MAX_TOTAL_BYTES, ErrorLogCandidate, ErrorLogOutcome, ErrorLogState, HistoryStore,
    RoutineSnapshotJob, RunRecord, RunState, StoredErrorLog,
};

use super::*;
use crate::audit::no_store;
use crate::audit::performance::{linear_axis, permyriad};
use crate::history::{HistoryConfig, history_error, plan_slices, unix_now};
use crate::history_trend::{coverage_map, seconds};

const DAY: i64 = 86_400;
/// Error logs read per cycle, one `job_id` each.
const ERROR_LOGS_PER_CYCLE: u32 = 5;
/// The BE deletes error logs after `load_error_log_reserve_hours` (48).
const BE_ERROR_LOG_SECONDS: i64 = 48 * 3_600;
/// A reason is new when first seen within this span after at least this
/// span of saved logs without it.
const NEW_REASON_SECONDS: i64 = 3 * DAY;

fn group(kind: LoadHistoryKind) -> LoadKindGroup {
    match kind {
        LoadHistoryKind::StreamLoad => LoadKindGroup::StreamLoad,
        LoadHistoryKind::Insert => LoadKindGroup::Insert,
        LoadHistoryKind::RoutineLoad => LoadKindGroup::RoutineLoad,
        LoadHistoryKind::Broker => LoadKindGroup::Broker,
        LoadHistoryKind::Other => LoadKindGroup::Other,
    }
}

/// A count for the views; far beyond any real number of loads.
fn small(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn ppm(part: u128, whole: u128) -> Option<u32> {
    (whole > 0).then(|| u32::try_from(part.saturating_mul(1_000_000) / whole).unwrap_or(u32::MAX))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Clearing {
    None,
    /// The earliest finished record moved past the slice start since the
    /// last look, or every record went: records were cleared before reading.
    Cleared,
    /// First look, and nothing older than the slice start is held: neither
    /// a clearing nor completeness can be shown.
    Unproven,
}

/// How far a seen clearing reaches into what was not read yet. A manager's
/// earliest finished record moved forward since the last look and now lies
/// past the watermark: records up to it were dropped before they were read.
/// The reach is kept, so a catch-up over several rounds marks every slice
/// before it. (Every record gone at once cannot be told from plain expiry on
/// a quiet cluster; that case is not claimed.)
pub(crate) fn clearing_reach(
    previous: Option<i64>,
    earliest_now: Option<i64>,
    earliest_before: Option<i64>,
    observed: bool,
    watermark: Option<i64>,
) -> Option<i64> {
    let seen = earliest_now.filter(|now| {
        observed
            && earliest_before.is_some_and(|before| before < *now)
            && watermark.is_none_or(|watermark| *now > watermark)
    });
    previous.max(seen)
}

/// Is a slice starting at `slice_start` missing records a manager cleared,
/// or (before the first look's bound) not provably complete?
pub(crate) fn clearing(
    slice_start: i64,
    reach: Option<i64>,
    unproven_until: Option<i64>,
) -> Clearing {
    if reach.is_some_and(|reach| reach > slice_start) {
        Clearing::Cleared
    } else if unproven_until.is_some_and(|until| until > slice_start) {
        Clearing::Unproven
    } else {
        Clearing::None
    }
}

fn routine_job(job: &doctor_contracts::RoutineLoadJob) -> RoutineSnapshotJob {
    let count = |value: &Option<String>| value.as_deref().and_then(|v| v.parse::<u128>().ok());
    RoutineSnapshotJob {
        job_id: job.job_id.clone(),
        name: job.name.clone(),
        database: job.db.clone(),
        table: job.table.clone(),
        state: job.state.clone(),
        reason: job.reason.clone(),
        max_lag: count(&job.max_lag),
        loaded_rows: count(&job.statistics.loaded_rows),
        error_rows: count(&job.statistics.error_rows),
        committed_tasks: count(&job.statistics.committed_tasks),
        aborted_tasks: count(&job.statistics.aborted_tasks),
    }
}

fn expired(error: &ConnectorError) -> bool {
    error.code == ConnectorErrorCode::ConnectionExpired
}

/// The load part of one collector cycle. A read failure is recorded and
/// returned; the load watermark stays at the last good slice.
pub(crate) fn run_load_cycle(
    sessions: &SessionManager,
    store: &HistoryStore,
    config: &HistoryConfig,
    connection_id: &str,
    target: &str,
    now: i64,
) -> Result<(), ConnectorError> {
    let store_error = |_| history_error("history_store");
    let state = store.load_target_state(target).map_err(store_error)?;
    let plan = plan_slices(state.watermark, now, config);
    let mut looked: Option<doctor_history::LoadTargetState> = None;
    let mut snapshot_failure: Option<ConnectorError> = None;
    let mut supported = true;
    for (index, (start, end)) in plan.slices.iter().copied().enumerate() {
        let began = Instant::now();
        let failed = |error: ConnectorError| {
            if !expired(&error) {
                let run = RunRecord {
                    started_at: now,
                    slice_start: start,
                    slice_end: end,
                    state: RunState::Failed,
                    reason: Some(format!("read_failed:{}", error.stage)),
                    rows_read: 0,
                    attributed: 0,
                    partial_statements: 0,
                    unavailable_statements: 0,
                    duration_ms: began.elapsed().as_millis() as u64,
                };
                let _ = store.record_load_run(target, &run, None);
            }
            error
        };
        let read = sessions
            .load_history_slice_controlled(connection_id, seconds(start), seconds(end), None)
            .map_err(failed)?;
        let Some((slice, engine)) = read else {
            // Before 3.3 there is no `loads` view to collect from.
            supported = false;
            break;
        };
        if index == 0 {
            if let Some((gap_start, gap_end)) = plan.gap {
                let run = RunRecord {
                    started_at: now,
                    slice_start: gap_start,
                    slice_end: gap_end,
                    state: RunState::Gap,
                    reason: Some("beyond_catch_up".into()),
                    rows_read: 0,
                    attributed: 0,
                    partial_statements: 0,
                    unavailable_statements: 0,
                    duration_ms: 0,
                };
                store
                    .record_load_run(target, &run, Some(gap_end))
                    .map_err(store_error)?;
            }
            let held = sessions
                .load_retention_controlled(connection_id, None)
                .map_err(failed)?;
            looked = Some(doctor_history::LoadTargetState {
                watermark: state.watermark,
                earliest_stream: held.stream_earliest_finish,
                earliest_other: held.other_earliest_finish,
                observed: true,
                cleared_stream: clearing_reach(
                    state.cleared_stream,
                    held.stream_earliest_finish,
                    state.earliest_stream,
                    state.observed,
                    state.watermark,
                ),
                cleared_other: clearing_reach(
                    state.cleared_other,
                    held.other_earliest_finish,
                    state.earliest_other,
                    state.observed,
                    state.watermark,
                ),
                // Kept from the first look on; only the first look sets it.
                unproven_until: if state.observed {
                    state.unproven_until
                } else {
                    held.stream_earliest_finish.max(held.other_earliest_finish)
                },
            });
        }
        let held = looked.clone().unwrap_or_default();
        let mut reasons = Vec::new();
        if slice.cut {
            reasons.push("row_limit");
        }
        let managers = [
            (held.cleared_stream, "stream_records_cleared"),
            (held.cleared_other, "load_records_cleared"),
        ];
        for (reach, cleared) in managers {
            match clearing(start, reach, held.unproven_until) {
                Clearing::Cleared => reasons.push(cleared),
                Clearing::Unproven if !reasons.contains(&"first_look_unproven") => {
                    reasons.push("first_look_unproven")
                }
                _ => {}
            }
        }
        let buckets = aggregate_loads(&slice.loads);
        let candidates: Vec<ErrorLogCandidate> = slice
            .loads
            .iter()
            .filter(|load| load.wants_error_log())
            .map(|load| ErrorLogCandidate {
                job_id: load.job_id,
                label: load.label.clone(),
                database: load.database.clone(),
                table: load.table.clone(),
                kind: load.kind,
                finish_time: load.finish_epoch_seconds,
                filtered_rows: load.filtered_rows,
            })
            .collect();
        let run = RunRecord {
            started_at: now,
            slice_start: start,
            slice_end: end,
            state: if reasons.is_empty() {
                RunState::Complete
            } else {
                RunState::Partial
            },
            reason: (!reasons.is_empty()).then(|| reasons.join(",")),
            rows_read: slice.read,
            attributed: slice.loads.len() as u64,
            partial_statements: 0,
            unavailable_statements: slice.unusable,
            duration_ms: began.elapsed().as_millis() as u64,
        };
        let recorded = store
            .record_load_slice(
                target,
                &engine,
                &history_rule_version(),
                &run,
                end,
                &buckets,
                &candidates,
            )
            .map_err(store_error)?;
        if !recorded {
            break;
        }
    }
    if let Some(held) = looked {
        store
            .set_load_retention(target, &held, now)
            .map_err(store_error)?;
    }
    // Routine Load jobs exist from 3.2; the decoder reports what it can read.
    match sessions.routine_load_jobs_controlled(connection_id, None) {
        Ok(jobs) if jobs.support != doctor_contracts::LoadViewSupport::UnsupportedVersion => {
            let snapshot: Vec<_> = jobs.jobs.iter().map(routine_job).collect();
            // Task 59: the version and rule set, so lag is never compared
            // across them.
            let engine = sessions.detected_version(connection_id);
            let rules = history_rule_version();
            store
                .record_routine_snapshot(
                    target,
                    now,
                    i64::from(config.period_seconds) * 3,
                    &snapshot,
                    jobs.truncated,
                    engine.as_deref().map(|engine| (engine, rules.as_str())),
                )
                .map_err(store_error)?;
        }
        Ok(_) => {}
        Err(error) if expired(&error) => return Err(error),
        // Reported after the error logs, so this source backs off.
        Err(error) => snapshot_failure = Some(error),
    }
    if !supported {
        return snapshot_failure.map_or(Ok(()), Err);
    }
    let due = store
        .due_error_logs(
            target,
            now,
            now - BE_ERROR_LOG_SECONDS,
            ERROR_LOGS_PER_CYCLE,
        )
        .map_err(store_error)?;
    for log in due {
        let Ok(job_id) = log.job_id.parse::<u64>() else {
            continue;
        };
        let outcome = match sessions.load_tracking_log_controlled(connection_id, job_id, None) {
            Ok(text) => {
                let parsed = parse_load_error_log(text.as_deref());
                match parsed.state {
                    LoadErrorLogState::Available => ErrorLogOutcome::Saved {
                        entries: parsed.entries,
                        categories: parsed.categories,
                        truncated: parsed.truncated,
                    },
                    LoadErrorLogState::Purged => ErrorLogOutcome::Purged,
                    LoadErrorLogState::None => ErrorLogOutcome::NoLog,
                    LoadErrorLogState::Unavailable => ErrorLogOutcome::ReadFailed,
                }
            }
            Err(error) if expired(&error) => return Err(error),
            Err(_) => ErrorLogOutcome::ReadFailed,
        };
        store
            .save_error_log(target, &log.job_id, outcome, now)
            .map_err(store_error)?;
    }
    // A failed Routine Load snapshot fails this source even when slices read.
    snapshot_failure.map_or(Ok(()), Err)
}

fn invalid(state: &ApiState, code: &'static str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::BAD_REQUEST,
        code,
        "history",
        code,
        "change_input",
    )
}

fn history_store(state: &ApiState) -> Result<Arc<HistoryStore>, ApiFailure> {
    state.history.store().cloned().ok_or_else(|| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::CONFLICT,
            "history_disabled",
            "history",
            "history_disabled",
            "change_input",
        )
    })
}

/// The connected session's target, otherwise the most recently collected.
fn pick_target(
    store: &HistoryStore,
    requested: Option<String>,
    connected: Option<String>,
) -> doctor_history::Result<Option<String>> {
    let known = store.load_targets()?;
    if let Some(target) = requested.filter(|target| known.contains(target)) {
        return Ok(Some(target));
    }
    Ok(connected.or_else(|| known.into_iter().next()))
}

fn range(days: u16, now: i64) -> (i64, i64, i64) {
    let today = now - now.rem_euclid(DAY);
    let start = today - (i64::from(days) - 1) * DAY;
    let end = today + DAY;
    let elapsed_end =
        (now - now.rem_euclid(HISTORY_BUCKET_SECONDS) + HISTORY_BUCKET_SECONDS).min(end);
    (start, end, elapsed_end)
}

fn stored_state(state: ErrorLogState) -> StoredLoadErrorLogState {
    match state {
        ErrorLogState::Pending => StoredLoadErrorLogState::Pending,
        ErrorLogState::Saved => StoredLoadErrorLogState::Saved,
        ErrorLogState::Purged => StoredLoadErrorLogState::Purged,
        ErrorLogState::NoLog => StoredLoadErrorLogState::NoLog,
        ErrorLogState::Failed => StoredLoadErrorLogState::Failed,
        ErrorLogState::Expired => StoredLoadErrorLogState::Expired,
        ErrorLogState::Trimmed => StoredLoadErrorLogState::Trimmed,
    }
}

fn stored_log(log: &StoredErrorLog) -> StoredLoadErrorLog {
    StoredLoadErrorLog {
        job_id: log.job_id.clone(),
        label: log.label.clone(),
        database: log.database.clone(),
        table: log.table.clone(),
        kind: group(log.kind),
        finish_seconds: seconds(log.finish_time),
        filtered_rows: log.filtered_rows.clone(),
        state: stored_state(log.state),
        attempts: log.attempts,
        saved_seconds: log.saved_at.map(seconds),
        sampled: log.sampled,
        truncated: log.truncated,
        categories: log.categories.clone(),
    }
}

fn has_samples(log: &StoredErrorLog) -> bool {
    matches!(log.state, ErrorLogState::Saved | ErrorLogState::Trimmed)
}

fn storage_view(
    store: &HistoryStore,
    logs: &[StoredErrorLog],
) -> doctor_history::Result<LoadErrorStorageView> {
    let storage = store.error_log_storage()?;
    Ok(LoadErrorStorageView {
        logs: small(storage.logs),
        saved: small(storage.saved),
        raw_bytes: storage.raw_bytes.to_string(),
        limit_bytes: ERROR_LOG_MAX_TOTAL_BYTES.to_string(),
        used_permyriad: permyriad(
            u128::from(storage.raw_bytes),
            u128::from(ERROR_LOG_MAX_TOTAL_BYTES),
        ),
        earliest_saved_seconds: storage.earliest_saved.map(seconds),
        missed: small(
            logs.iter()
                .filter(|log| matches!(log.state, ErrorLogState::Purged | ErrorLogState::Expired))
                .count() as u64,
        ),
    })
}

type ReasonKey = (
    String,
    String,
    LoadErrorCategory,
    Option<String>,
    Option<String>,
);

#[derive(Default)]
struct ReasonTally {
    sampled: u32,
    loads: u32,
    first: i64,
    last: i64,
}

/// Sampled reasons per table over saved logs, with when each first showed.
fn reason_tallies(logs: &[StoredErrorLog]) -> BTreeMap<ReasonKey, ReasonTally> {
    let mut tallies: BTreeMap<ReasonKey, ReasonTally> = BTreeMap::new();
    for log in logs.iter().filter(|log| has_samples(log)) {
        for category in &log.categories {
            let tally = tallies
                .entry((
                    log.database.clone(),
                    log.table.clone(),
                    category.category,
                    category.column.clone(),
                    category.target_type.clone(),
                ))
                .or_insert_with(|| ReasonTally {
                    first: log.finish_time,
                    last: log.finish_time,
                    ..ReasonTally::default()
                });
            tally.sampled += u32::from(category.count);
            tally.loads += 1;
            tally.first = tally.first.min(log.finish_time);
            tally.last = tally.last.max(log.finish_time);
        }
    }
    tallies
}

fn is_new(first: i64, earliest_saved: Option<i64>, now: i64) -> bool {
    first >= now - NEW_REASON_SECONDS
        && earliest_saved.is_some_and(|earliest| earliest <= first - NEW_REASON_SECONDS)
}

fn segments_of(
    versions: &[BTreeSet<(String, String)>],
    starts: &[i64],
) -> (Vec<HistoryLoadSegment>, Vec<u16>) {
    let mut segments: Vec<HistoryLoadSegment> = Vec::new();
    let mut segment_of = Vec::with_capacity(versions.len());
    let mut current: Option<&BTreeSet<(String, String)>> = None;
    for (pairs, start) in versions.iter().zip(starts) {
        if !pairs.is_empty() && current != Some(pairs) {
            segments.push(HistoryLoadSegment {
                engine_versions: pairs
                    .iter()
                    .map(|pair| pair.0.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                rule_versions: pairs
                    .iter()
                    .map(|pair| pair.1.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                first_day_seconds: seconds(*start),
            });
            current = Some(pairs);
        }
        segment_of.push(segments.len().saturating_sub(1) as u16);
    }
    (segments, segment_of)
}

const KIND_ORDER: [LoadKindGroup; 5] = [
    LoadKindGroup::StreamLoad,
    LoadKindGroup::Insert,
    LoadKindGroup::RoutineLoad,
    LoadKindGroup::Broker,
    LoadKindGroup::Other,
];

#[derive(Default)]
struct Sums {
    loads: u128,
    failed: u128,
    loaded: u128,
    loaded_unknown: u64,
    filtered: u128,
    filtered_unknown: u64,
    by_kind: BTreeMap<LoadKindGroup, (u128, u128)>,
}

impl Sums {
    fn add(&mut self, delta: &LoadBucketDelta) {
        let kind = group(delta.kind.unwrap_or(LoadHistoryKind::Other));
        self.loads += u128::from(delta.loads);
        self.failed += u128::from(delta.failed);
        self.loaded += delta.loaded_rows.sum;
        self.loaded_unknown += delta.loaded_rows.unknown;
        self.filtered += delta.filtered_rows.sum;
        self.filtered_unknown += delta.filtered_rows.unknown;
        let entry = self.by_kind.entry(kind).or_default();
        entry.0 += u128::from(delta.loads);
        entry.1 += u128::from(delta.failed);
    }

    /// Only when every load reported both counts: otherwise the numerator
    /// and denominator could come from different loads.
    fn filtered_ppm(&self) -> Option<u32> {
        (self.loaded_unknown == 0 && self.filtered_unknown == 0)
            .then(|| ppm(self.filtered, self.loaded + self.filtered))
            .flatten()
    }
}

pub(crate) fn build_history(
    store: &HistoryStore,
    target: Option<String>,
    request: &LoadHistoryRequest,
    now: i64,
) -> doctor_history::Result<LoadHistoryResponse> {
    let (start, end, elapsed_end) = range(request.days, now);
    let (runs, buckets, logs, load_state) = match target.as_deref() {
        Some(target) => (
            store.load_runs(target, start, end)?,
            store.load_buckets(target, start, end)?,
            store.error_logs(target, start, end)?,
            store.load_target_state(target)?,
        ),
        None => Default::default(),
    };
    let routine_jobs = match target.as_deref() {
        Some(target) => store
            .routine_hours(target, start, end)?
            .into_iter()
            .map(|hour| hour.job_id)
            .collect::<BTreeSet<_>>()
            .len() as u32,
        None => 0,
    };
    let coverage = coverage_map(&runs, start, elapsed_end);
    let visible: Vec<_> = buckets
        .iter()
        .filter(|bucket| request.include_system || !bucket.delta.system)
        .collect();
    let table_options: Vec<String> = visible
        .iter()
        .map(|bucket| format!("{}.{}", bucket.delta.database, bucket.delta.table))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let in_table = |delta: &LoadBucketDelta| {
        request
            .table
            .as_deref()
            .is_none_or(|table| table == format!("{}.{}", delta.database, delta.table))
    };
    let scoped: Vec<_> = visible
        .iter()
        .filter(|bucket| in_table(&bucket.delta))
        .copied()
        .collect();
    let mut kind_sums = Sums::default();
    for bucket in &scoped {
        kind_sums.add(&bucket.delta);
    }
    let selected: Vec<_> = scoped
        .iter()
        .filter(|bucket| {
            request.kind.is_none_or(|kind| {
                group(bucket.delta.kind.unwrap_or(LoadHistoryKind::Other)) == kind
            })
        })
        .copied()
        .collect();

    let day_count = usize::from(request.days);
    let starts: Vec<i64> = (0..day_count as i64).map(|i| start + i * DAY).collect();
    let mut day_sums: Vec<Sums> = (0..day_count).map(|_| Sums::default()).collect();
    let mut day_versions: Vec<BTreeSet<(String, String)>> = vec![BTreeSet::new(); day_count];
    for bucket in &selected {
        let index = ((bucket.delta.bucket_start - start) / DAY) as usize;
        if index < day_count {
            day_sums[index].add(&bucket.delta);
            day_versions[index]
                .insert((bucket.engine_version.clone(), bucket.rule_version.clone()));
        }
    }
    let (segments, segment_of) = segments_of(&day_versions, &starts);
    let mut partial_hours_total = 0u32;
    let mut missing_hours_total = 0u32;
    struct DayFacts {
        coverage: HistoryLoadCoverage,
        complete: u16,
        partial: u16,
        missing: u16,
        reasons: Vec<String>,
    }
    let facts: Vec<DayFacts> = starts
        .iter()
        .enumerate()
        .map(|(index, day_start)| {
            let (mut complete, mut partial, mut missing) = (0u16, 0u16, 0u16);
            for hour in (*day_start..day_start + DAY).step_by(HISTORY_BUCKET_SECONDS as usize) {
                match coverage.get(&hour) {
                    Some(CoverageHourState::Complete) => complete += 1,
                    Some(CoverageHourState::Partial) => partial += 1,
                    Some(CoverageHourState::NotCollected) => missing += 1,
                    None => {}
                }
            }
            partial_hours_total += u32::from(partial);
            missing_hours_total += u32::from(missing);
            let reasons: BTreeSet<String> = runs
                .iter()
                .filter(|run| {
                    run.state == RunState::Partial
                        && run.slice_end > *day_start
                        && run.slice_start < day_start + DAY
                })
                .flat_map(|run| {
                    run.reason
                        .as_deref()
                        .unwrap_or_default()
                        .split(',')
                        .filter(|reason| !reason.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .collect();
            let read = complete + partial;
            let finished = day_start + DAY <= now;
            let state = if read == 0 && day_versions[index].is_empty() {
                HistoryLoadCoverage::NotCollected
            } else if partial == 0 && missing == 0 && finished {
                HistoryLoadCoverage::Complete
            } else {
                HistoryLoadCoverage::Partial
            };
            DayFacts {
                coverage: state,
                complete,
                partial,
                missing,
                reasons: reasons.into_iter().collect(),
            }
        })
        .collect();
    let max_loads = facts
        .iter()
        .zip(&day_sums)
        .filter(|(day, _)| day.coverage != HistoryLoadCoverage::NotCollected)
        .map(|(_, sums)| sums.loads)
        .max()
        .unwrap_or(0);
    let (count_top, count_ticks) = linear_axis(max_loads);
    let day_rates: Vec<(Option<u32>, Option<u32>)> = facts
        .iter()
        .zip(&day_sums)
        .map(|(day, sums)| {
            if day.coverage == HistoryLoadCoverage::NotCollected {
                (None, None)
            } else {
                (ppm(sums.failed, sums.loads), sums.filtered_ppm())
            }
        })
        .collect();
    let max_rate = day_rates
        .iter()
        .flat_map(|(failed, filtered)| [*failed, *filtered])
        .flatten()
        .max()
        .unwrap_or(0);
    let (rate_top, rate_ticks) = linear_axis(u128::from(max_rate));
    let days = facts
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let sums = &day_sums[index];
            let read = day.coverage != HistoryLoadCoverage::NotCollected;
            let (failed_rate, filtered_rate) = day_rates[index];
            LoadHistoryDay {
                start_seconds: seconds(starts[index]),
                coverage: day.coverage,
                complete_hours: day.complete,
                partial_hours: day.partial,
                not_collected_hours: day.missing,
                partial_reasons: day.reasons.clone(),
                segment: segment_of[index],
                loads: read.then(|| sums.loads.to_string()),
                failed: read.then(|| sums.failed.to_string()),
                loaded_rows: read.then(|| sums.loaded.to_string()),
                filtered_rows: read.then(|| sums.filtered.to_string()),
                unknown_counts: small(sums.loaded_unknown + sums.filtered_unknown),
                by_kind: if read {
                    KIND_ORDER
                        .iter()
                        .filter_map(|kind| {
                            sums.by_kind.get(kind).map(|(loads, _)| LoadHistoryKindDay {
                                kind: *kind,
                                loads: loads.to_string(),
                                height_permyriad: permyriad(*loads, count_top),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                failed_rate_ppm: failed_rate,
                failed_rate_y_permyriad: failed_rate
                    .map(|rate| permyriad(u128::from(rate), rate_top)),
                filtered_rate_ppm: filtered_rate,
                filtered_rate_y_permyriad: filtered_rate
                    .map(|rate| permyriad(u128::from(rate), rate_top)),
            }
        })
        .collect();

    let mut totals = Sums::default();
    let mut tables: BTreeMap<(String, String), (Sums, BTreeSet<LoadKindGroup>)> = BTreeMap::new();
    for bucket in &selected {
        totals.add(&bucket.delta);
        let entry = tables
            .entry((bucket.delta.database.clone(), bucket.delta.table.clone()))
            .or_default();
        entry.0.add(&bucket.delta);
        entry
            .1
            .insert(group(bucket.delta.kind.unwrap_or(LoadHistoryKind::Other)));
    }
    let scoped_logs: Vec<StoredErrorLog> = logs
        .iter()
        .filter(|log| request.kind.is_none_or(|kind| group(log.kind) == kind))
        .cloned()
        .collect();
    let tallies = reason_tallies(&scoped_logs);
    let earliest_saved = logs
        .iter()
        .filter(|log| has_samples(log))
        .map(|log| log.finish_time)
        .min();
    let mut table_rows: Vec<_> = tables.into_iter().collect();
    table_rows.sort_by(|(a_key, (a, _)), (b_key, (b, _))| {
        b.filtered
            .cmp(&a.filtered)
            .then(b.failed.cmp(&a.failed))
            .then(b.loads.cmp(&a.loads))
            .then(a_key.cmp(b_key))
    });
    let tables_with_filtered = table_rows
        .iter()
        .filter(|(_, (sums, _))| sums.filtered > 0)
        .count() as u32;
    let tables_out = table_rows
        .into_iter()
        .take(LOAD_HISTORY_TABLES)
        .map(|((database, table), (sums, kinds))| {
            let mine: Vec<_> = tallies
                .iter()
                .filter(|(key, _)| key.0 == database && key.1 == table)
                .collect();
            let sampled: u32 = mine.iter().map(|(_, tally)| tally.sampled).sum();
            let top_reason = mine
                .iter()
                .max_by(|a, b| a.1.sampled.cmp(&b.1.sampled).then(b.0.cmp(a.0)))
                .map(|(key, tally)| LoadHistoryReason {
                    category: key.2,
                    column: key.3.clone(),
                    target_type: key.4.clone(),
                    sample_share_permyriad: permyriad(
                        u128::from(tally.sampled),
                        u128::from(sampled),
                    ),
                    sampled_rows: tally.sampled,
                    new_since_seconds: is_new(tally.first, earliest_saved, now)
                        .then(|| seconds(tally.first)),
                });
            let saved_logs = scoped_logs
                .iter()
                .filter(|log| has_samples(log) && log.database == database && log.table == table)
                .count() as u32;
            LoadHistoryTable {
                kinds: KIND_ORDER
                    .iter()
                    .copied()
                    .filter(|kind| kinds.contains(kind))
                    .collect(),
                loads: sums.loads.to_string(),
                failed: sums.failed.to_string(),
                loaded_rows: sums.loaded.to_string(),
                filtered_rows: sums.filtered.to_string(),
                unknown_counts: small(sums.loaded_unknown + sums.filtered_unknown),
                top_reason,
                saved_logs,
                database,
                table,
            }
        })
        .collect();
    let last_run = runs.iter().max_by_key(|run| run.started_at);
    let mut limitations = vec![
        "days_are_utc".to_owned(),
        "finished_loads_by_finish_time".to_owned(),
        "reasons_are_samples".to_owned(),
    ];
    if segments.len() > 1 {
        limitations.push("version_segments_not_compared".to_owned());
    }
    Ok(LoadHistoryResponse {
        target,
        collection: LoadHistoryCollection {
            watermark_seconds: load_state.watermark.map(seconds),
            last_run_seconds: last_run.map(|run| seconds(run.started_at)),
            last_rows_read: last_run.map(|run| small(run.rows_read)),
        },
        days,
        segments,
        kinds: KIND_ORDER
            .iter()
            .filter_map(|kind| {
                kind_sums
                    .by_kind
                    .get(kind)
                    .map(|(loads, failed)| LoadHistoryKindTotal {
                        kind: *kind,
                        loads: loads.to_string(),
                        failed: failed.to_string(),
                    })
            })
            .collect(),
        all_loads: kind_sums.loads.to_string(),
        routine_jobs,
        totals: LoadHistoryTotals {
            loads: totals.loads.to_string(),
            failed: totals.failed.to_string(),
            failed_ppm: ppm(totals.failed, totals.loads),
            loaded_rows: totals.loaded.to_string(),
            loaded_unknown: small(totals.loaded_unknown),
            filtered_rows: totals.filtered.to_string(),
            filtered_unknown: small(totals.filtered_unknown),
            filtered_ppm: totals.filtered_ppm(),
            tables_with_filtered,
            partial_hours: partial_hours_total,
            not_collected_hours: missing_hours_total,
        },
        count_axis: count_ticks.iter().map(u128::to_string).collect(),
        rate_axis: rate_ticks.iter().map(u128::to_string).collect(),
        tables: tables_out,
        table_options,
        error_logs: storage_view(store, &logs)?,
        limitations,
    })
}

fn routine_state(state: &str) -> RoutineHistoryState {
    match state {
        "RUNNING" => RoutineHistoryState::Running,
        "NEED_SCHEDULE" => RoutineHistoryState::Scheduling,
        "PAUSED" => RoutineHistoryState::Paused,
        "STOPPED" => RoutineHistoryState::Stopped,
        "CANCELLED" => RoutineHistoryState::Cancelled,
        _ => RoutineHistoryState::Unknown,
    }
}

fn state_rank(state: RoutineHistoryState) -> u8 {
    match state {
        RoutineHistoryState::Cancelled => 7,
        RoutineHistoryState::Stopped => 6,
        RoutineHistoryState::Paused => 5,
        RoutineHistoryState::Scheduling => 4,
        RoutineHistoryState::Running => 3,
        RoutineHistoryState::Unknown => 2,
        RoutineHistoryState::Absent => 1,
        RoutineHistoryState::NotCollected => 0,
    }
}

pub(crate) fn build_routine(
    store: &HistoryStore,
    target: Option<String>,
    request: &LoadHistoryRoutineRequest,
    now: i64,
) -> doctor_history::Result<LoadHistoryRoutineResponse> {
    let (start, end, elapsed_end) = range(request.days, now);
    let cell_seconds: i64 = if request.days <= 7 { 3_600 } else { 6 * 3_600 };
    let (hours, events, snapshots) = match target.as_deref() {
        Some(target) => (
            store.routine_hours(target, start, end)?,
            store.routine_events(target, start, end)?,
            store.routine_snapshot_hours(target, start, end)?,
        ),
        None => Default::default(),
    };
    // Hours with any snapshot, and those with a complete one (only a complete
    // snapshot can show that a job was not there).
    let collected: BTreeSet<i64> = snapshots.iter().map(|hour| hour.bucket_start).collect();
    let complete: BTreeSet<i64> = snapshots
        .iter()
        .filter(|hour| hour.complete())
        .map(|hour| hour.bucket_start)
        .collect();
    let mut by_job: BTreeMap<String, Vec<&doctor_history::StoredRoutineHour>> = BTreeMap::new();
    for hour in &hours {
        by_job.entry(hour.job_id.clone()).or_default().push(hour);
    }
    let mut jobs: Vec<RoutineHistoryJob> = by_job
        .iter()
        .map(|(job_id, hours)| {
            let last = hours.last().copied().expect("non-empty");
            let cells = (start..elapsed_end)
                .step_by(cell_seconds as usize)
                .map(|cell| {
                    let state = hours
                        .iter()
                        .filter(|hour| {
                            hour.bucket_start >= cell && hour.bucket_start < cell + cell_seconds
                        })
                        .map(|hour| routine_state(&hour.worst_state))
                        .max_by_key(|state| state_rank(*state))
                        .unwrap_or_else(|| {
                            if complete.range(cell..cell + cell_seconds).next().is_some() {
                                RoutineHistoryState::Absent
                            } else if collected.range(cell..cell + cell_seconds).next().is_some() {
                                RoutineHistoryState::Unknown
                            } else {
                                RoutineHistoryState::NotCollected
                            }
                        });
                    RoutineHistoryCell {
                        start_seconds: seconds(cell),
                        state,
                    }
                })
                .collect();
            // No known interval at all: the increase is unknown, not zero.
            let known = hours.iter().any(|hour| hour.known_intervals > 0);
            let sum = |pick: fn(&doctor_history::StoredRoutineHour) -> u128| {
                known.then(|| {
                    hours
                        .iter()
                        .map(|hour| pick(hour))
                        .sum::<u128>()
                        .to_string()
                })
            };
            RoutineHistoryJob {
                job_id: job_id.clone(),
                name: last.name.clone(),
                database: last.database.clone(),
                table: last.table.clone(),
                current_state: last.last_state.clone(),
                cells,
                pauses: events
                    .iter()
                    .filter(|event| {
                        &event.job_id == job_id
                            && event.to_state == "PAUSED"
                            && event.from_state.is_some()
                    })
                    .count() as u32,
                loaded_rows: sum(|hour| hour.loaded_delta),
                error_rows: sum(|hour| hour.error_delta),
                committed_tasks: sum(|hour| hour.committed_delta),
                aborted_tasks: sum(|hour| hour.aborted_delta),
                unknown_intervals: hours.iter().map(|hour| hour.unknown_intervals).sum(),
                max_lag: hours
                    .iter()
                    .filter_map(|hour| hour.max_lag)
                    .max()
                    .map(|lag| lag.to_string()),
            }
        })
        .collect();
    jobs.sort_by(|a, b| a.name.cmp(&b.name).then(a.job_id.cmp(&b.job_id)));
    let chosen = request
        .job_id
        .as_ref()
        .filter(|id| by_job.contains_key(*id))
        .cloned()
        .or_else(|| jobs.first().map(|job| job.job_id.clone()));
    let selected = chosen.map(|job_id| {
        let mine = &by_job[&job_id];
        let of_hour: BTreeMap<i64, &&doctor_history::StoredRoutineHour> =
            mine.iter().map(|hour| (hour.bucket_start, hour)).collect();
        let max_lag = mine
            .iter()
            .filter_map(|hour| hour.max_lag)
            .max()
            .unwrap_or(0);
        let (lag_top, lag_ticks) = linear_axis(max_lag);
        let lag = (start..elapsed_end)
            .step_by(HISTORY_BUCKET_SECONDS as usize)
            .map(|hour| {
                let found = of_hour.get(&hour);
                RoutineLagPoint {
                    start_seconds: seconds(hour),
                    max_lag: found.and_then(|h| h.max_lag).map(|lag| lag.to_string()),
                    y_permyriad: found
                        .and_then(|h| h.max_lag)
                        .map(|lag| permyriad(lag, lag_top)),
                    paused: found.is_some_and(|h| {
                        matches!(
                            routine_state(&h.worst_state),
                            RoutineHistoryState::Paused
                                | RoutineHistoryState::Stopped
                                | RoutineHistoryState::Cancelled
                        )
                    }),
                    collected: collected.contains(&hour),
                }
            })
            .collect();
        let day_count = i64::from(request.days);
        let day_sums: Vec<(Option<u128>, Option<u128>, u32, bool)> = (0..day_count)
            .map(|index| {
                let day_start = start + index * DAY;
                let in_day: Vec<_> = mine
                    .iter()
                    .filter(|hour| {
                        hour.bucket_start >= day_start && hour.bucket_start < day_start + DAY
                    })
                    .collect();
                let known = in_day.iter().any(|hour| hour.known_intervals > 0);
                (
                    known.then(|| in_day.iter().map(|hour| hour.loaded_delta).sum()),
                    known.then(|| in_day.iter().map(|hour| hour.error_delta).sum()),
                    in_day.iter().map(|hour| hour.unknown_intervals).sum(),
                    collected.range(day_start..day_start + DAY).next().is_some(),
                )
            })
            .collect();
        let rows_max = day_sums.iter().filter_map(|day| day.0).max().unwrap_or(0);
        let (rows_top, rows_ticks) = linear_axis(rows_max);
        let job = mine.last().expect("non-empty");
        RoutineHistoryDetail {
            job_id: job_id.clone(),
            name: job.name.clone(),
            lag,
            lag_axis: lag_ticks.iter().map(u128::to_string).collect(),
            lag_known: mine.iter().any(|hour| hour.lag_snapshots > 0),
            events: events
                .iter()
                .filter(|event| event.job_id == job_id)
                .take(50)
                .map(|event| RoutineHistoryEvent {
                    at_seconds: seconds(event.observed_at),
                    from_state: event.from_state.clone(),
                    to_state: event.to_state.clone(),
                    reason: event.reason.clone(),
                })
                .collect(),
            days: day_sums
                .iter()
                .enumerate()
                .map(
                    |(index, (loaded, error, unknown, collected))| RoutineHistoryDay {
                        start_seconds: seconds(start + index as i64 * DAY),
                        loaded_rows: loaded.map(|v| v.to_string()),
                        error_rows: error.map(|v| v.to_string()),
                        loaded_height_permyriad: loaded.map_or(0, |v| permyriad(v, rows_top)),
                        error_height_permyriad: error.map_or(0, |v| permyriad(v, rows_top)),
                        unknown_intervals: *unknown,
                        collected: *collected,
                    },
                )
                .collect(),
            rows_axis: rows_ticks.iter().map(u128::to_string).collect(),
        }
    });
    Ok(LoadHistoryRoutineResponse {
        target,
        cell_seconds: seconds(cell_seconds),
        range_start_seconds: seconds(start),
        jobs,
        selected,
        limitations: vec![
            "snapshot_every_cycle".to_owned(),
            "increase_between_close_snapshots_only".to_owned(),
        ],
    })
}

pub(crate) fn build_error_logs(
    store: &HistoryStore,
    target: Option<String>,
    request: &LoadHistoryErrorLogsRequest,
    now: i64,
) -> doctor_history::Result<LoadHistoryErrorLogsResponse> {
    let (start, end, _) = range(request.days, now);
    let logs = match target.as_deref() {
        Some(target) => store.error_logs(target, start, end)?,
        None => Vec::new(),
    };
    let table_options: Vec<String> = logs
        .iter()
        .map(|log| format!("{}.{}", log.database, log.table))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let in_table = |log: &&StoredErrorLog| {
        request
            .table
            .as_deref()
            .is_none_or(|table| table == format!("{}.{}", log.database, log.table))
    };
    let scoped: Vec<StoredErrorLog> = logs.iter().filter(in_table).cloned().collect();
    let earliest_saved = logs
        .iter()
        .filter(|log| has_samples(log))
        .map(|log| log.finish_time)
        .min();
    let mut reasons: Vec<_> = reason_tallies(&scoped).into_iter().collect();
    reasons.sort_by(|a, b| b.1.sampled.cmp(&a.1.sampled).then(a.0.cmp(&b.0)));
    let matching: Vec<_> = scoped
        .iter()
        .filter(|log| {
            request.category.is_none_or(|category| {
                log.categories
                    .iter()
                    .any(|count| count.category == category)
            })
        })
        .collect();
    let offset = request.offset as usize;
    Ok(LoadHistoryErrorLogsResponse {
        storage: storage_view(store, &logs)?,
        reasons: reasons
            .into_iter()
            .take(LOAD_HISTORY_REASONS)
            .map(
                |((database, table, category, column, target_type), tally)| {
                    LoadErrorReasonSummary {
                        new: is_new(tally.first, earliest_saved, now),
                        database,
                        table,
                        category,
                        column,
                        target_type,
                        sampled_rows: tally.sampled,
                        loads: tally.loads,
                        first_seconds: seconds(tally.first),
                        last_seconds: seconds(tally.last),
                    }
                },
            )
            .collect(),
        total: matching.len() as u32,
        logs: matching
            .iter()
            .skip(offset)
            .take(usize::from(LOAD_HISTORY_ERROR_LOGS_PAGE))
            .map(|log| stored_log(log))
            .collect(),
        table_options,
        target,
        limitations: vec![
            "sample_of_about_50_rows_per_file".to_owned(),
            "stored_plain_text_on_this_host".to_owned(),
        ],
    })
}

async fn blocking<T: Send + 'static>(
    state: &ApiState,
    work: impl FnOnce() -> doctor_history::Result<T> + Send + 'static,
) -> Result<T, ApiFailure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| internal_failure(state))?
        .map_err(|_| internal_failure(state))
}

pub(super) async fn history(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadHistoryRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_history_request"));
    }
    let store = history_store(&state)?;
    let connected = request
        .connection_id
        .as_deref()
        .and_then(|id| state.history.entry_target(id));
    let response = blocking(&state, move || {
        let target = pick_target(&store, request.target.clone(), connected)?;
        build_history(&store, target, &request, unix_now())
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn routine(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadHistoryRoutineRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_history_request"));
    }
    let store = history_store(&state)?;
    let connected = request
        .connection_id
        .as_deref()
        .and_then(|id| state.history.entry_target(id));
    let response = blocking(&state, move || {
        let target = pick_target(&store, request.target.clone(), connected)?;
        build_routine(&store, target, &request, unix_now())
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn error_logs(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadHistoryErrorLogsRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_history_request"));
    }
    let store = history_store(&state)?;
    let connected = request
        .connection_id
        .as_deref()
        .and_then(|id| state.history.entry_target(id));
    let response = blocking(&state, move || {
        let target = pick_target(&store, request.target.clone(), connected)?;
        build_error_logs(&store, target, &request, unix_now())
    })
    .await?;
    Ok(no_store(response))
}

/// The saved text of one load's error log, only when asked for.
pub(super) async fn error_log(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadHistoryErrorLogRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_history_request"));
    }
    let store = history_store(&state)?;
    let connected = request
        .connection_id
        .as_deref()
        .and_then(|id| state.history.entry_target(id));
    let cutoff = unix_now() - i64::from(state.history.config.retention_days) * DAY;
    let found = blocking(&state, move || {
        let Some(target) = pick_target(&store, request.target.clone(), connected)? else {
            return Ok(None);
        };
        // Past retention the text is gone, even before the next purge runs.
        let Some(log) = store
            .error_log(&target, &request.job_id)?
            .filter(|log| log.finish_time >= cutoff)
        else {
            return Ok(None);
        };
        let entries = store.error_log_entries(&target, &request.job_id)?;
        Ok(Some((log, entries)))
    })
    .await?;
    let Some((log, entries)) = found else {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::NOT_FOUND,
            "load_error_log_not_saved",
            "history",
            "load_error_log_not_saved",
            "change_input",
        ));
    };
    let mut limitations = vec![
        "sample_of_about_50_rows_per_file".to_owned(),
        "first_error_per_row_only".to_owned(),
        "stored_plain_text_on_this_host".to_owned(),
    ];
    if log.state == ErrorLogState::Trimmed {
        limitations.push("text_dropped_under_50_mb_limit".to_owned());
    }
    Ok(no_store(LoadHistoryErrorLogResponse {
        log: stored_log(&log),
        entries,
        limitations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::{LoadErrorCategoryCount, LoadErrorEntry};
    use doctor_core::{CollectedLoad, LOAD_PHASES};

    #[test]
    fn audit_lh_03_cleared_records_mark_a_slice_partial_only_with_evidence() {
        // Steady state: the earliest held record is old, nothing reaches the slice.
        assert_eq!(
            clearing_reach(None, Some(500), Some(400), true, Some(900)),
            None
        );
        assert_eq!(clearing(1_000, None, None), Clearing::None);
        // The earliest moved forward past the watermark since the last look:
        // records up to it were cleared before they were read.
        let reach = clearing_reach(None, Some(1_400), Some(400), true, Some(900));
        assert_eq!(reach, Some(1_400));
        assert_eq!(clearing(1_000, reach, None), Clearing::Cleared);
        // A later catch-up round sees the same earliest record; the kept reach
        // still marks every slice before it, and none after.
        let later = clearing_reach(reach, Some(1_400), Some(1_400), true, Some(1_200));
        assert_eq!(clearing(1_200, later, None), Clearing::Cleared);
        assert_eq!(clearing(1_500, later, None), Clearing::None);
        // Expiry of already-read records does not reach unread slices.
        assert_eq!(
            clearing_reach(None, Some(800), Some(400), true, Some(900)),
            None
        );
        // Records appearing where there were none is not a clearing.
        assert_eq!(
            clearing_reach(None, Some(1_200), None, true, Some(900)),
            None
        );
        // Before the first look's bound: unproven, not claimed as a clearing,
        // in every round until the watermark passes it.
        assert_eq!(clearing(1_000, None, Some(1_200)), Clearing::Unproven);
        assert_eq!(clearing(1_100, None, Some(1_200)), Clearing::Unproven);
        assert_eq!(clearing(1_200, None, Some(1_200)), Clearing::None);
        // A seen clearing wins over the first-look bound.
        assert_eq!(clearing(1_000, Some(1_100), Some(1_200)), Clearing::Cleared);
    }

    fn load(kind: LoadHistoryKind, finish: i64, failed: bool, filtered: u128) -> CollectedLoad {
        CollectedLoad {
            job_id: finish as u64,
            label: format!("l{finish}"),
            database: "ods".into(),
            table: if kind == LoadHistoryKind::Insert {
                "gmv".into()
            } else {
                "orders".into()
            },
            kind,
            cancelled: failed,
            system: false,
            finish_epoch_seconds: finish,
            loaded_rows: Some(100),
            filtered_rows: Some(filtered),
            unselected_rows: Some(0),
            scan_bytes: None,
            total_seconds: Some(1),
            phases_ms: [None; LOAD_PHASES.len()],
            has_error_log: false,
        }
    }

    fn complete(start: i64, end: i64) -> RunRecord {
        RunRecord {
            started_at: end,
            slice_start: start,
            slice_end: end,
            state: RunState::Complete,
            reason: None,
            rows_read: 1,
            attributed: 1,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 1,
        }
    }

    #[test]
    fn audit_lh_02_history_days_sum_buckets_and_leave_uncollected_days_empty() {
        let store = HistoryStore::open_in_memory().unwrap();
        let day = DAY;
        let now = 10 * day + 12 * 3_600;
        // Day 9: two Stream Loads (one failed, 5 filtered) and one INSERT.
        let loads = [
            load(LoadHistoryKind::StreamLoad, 9 * day + 100, false, 5),
            load(LoadHistoryKind::StreamLoad, 9 * day + 200, true, 0),
            load(LoadHistoryKind::Insert, 9 * day + 300, false, 0),
        ];
        store
            .record_load_slice(
                "t",
                "3.5.14",
                "r",
                &complete(9 * day, 9 * day + 3_600),
                9 * day + 3_600,
                &aggregate_loads(&loads),
                &[],
            )
            .unwrap();
        let mut partial = complete(9 * day + 3_600, 9 * day + 7_200);
        partial.state = RunState::Partial;
        partial.reason = Some("row_limit,stream_records_cleared".into());
        store
            .record_load_slice("t", "3.5.14", "r", &partial, 9 * day + 7_200, &[], &[])
            .unwrap();
        store
            .save_error_log("t", "1", ErrorLogOutcome::NoLog, now)
            .unwrap();
        let request = LoadHistoryRequest {
            connection_id: None,
            target: None,
            days: 7,
            kind: None,
            table: None,
            include_system: false,
        };
        let response = build_history(&store, Some("t".into()), &request, now).unwrap();
        assert_eq!(response.days.len(), 7);
        let read = &response.days[5];
        assert_eq!(read.loads.as_deref(), Some("3"));
        assert_eq!(read.failed.as_deref(), Some("1"));
        assert_eq!(read.filtered_rows.as_deref(), Some("5"));
        assert_eq!(read.partial_hours, 1);
        assert_eq!(
            read.partial_reasons,
            ["row_limit", "stream_records_cleared"]
        );
        assert_eq!(read.by_kind[0].kind, LoadKindGroup::StreamLoad);
        assert_eq!(read.by_kind[0].loads, "2");
        // 1 failed of 3, 5 filtered of 305 input rows.
        assert_eq!(read.failed_rate_ppm, Some(333_333));
        assert_eq!(read.filtered_rate_ppm, Some(16_393));
        // Days without a run are not collected: nothing, not zero.
        assert_eq!(response.days[0].coverage, HistoryLoadCoverage::NotCollected);
        assert_eq!(response.days[0].loads, None);
        assert_eq!(response.kinds.len(), 2);
        assert_eq!(response.totals.loads, "3");
        assert_eq!(response.tables[0].table, "orders");
        assert_eq!(response.tables[0].filtered_rows, "5");
        assert_eq!(response.table_options, ["ods.gmv", "ods.orders"]);
        assert_eq!(
            response.collection.watermark_seconds,
            Some(seconds(9 * day + 7_200))
        );

        let only_insert = LoadHistoryRequest {
            kind: Some(LoadKindGroup::Insert),
            ..request.clone()
        };
        let insert = build_history(&store, Some("t".into()), &only_insert, now).unwrap();
        assert_eq!(insert.days[5].loads.as_deref(), Some("1"));
        // The chips still count every kind.
        assert_eq!(insert.kinds.len(), 2);
        assert_eq!(insert.all_loads, "3");
    }

    #[test]
    fn audit_lh_04_routine_history_cells_detail_and_absent_hours() {
        let store = HistoryStore::open_in_memory().unwrap();
        let day = DAY;
        let now = 10 * day + 3 * 3_600;
        let job = |id: &str, state: &str, loaded: u128| RoutineSnapshotJob {
            job_id: id.into(),
            name: format!("job{id}"),
            database: "db".into(),
            table: "t".into(),
            state: state.into(),
            reason: None,
            max_lag: Some(loaded / 10),
            loaded_rows: Some(loaded),
            error_rows: Some(0),
            committed_tasks: Some(1),
            aborted_tasks: Some(0),
        };
        let base = 10 * day;
        store
            .record_routine_snapshot(
                "t",
                base,
                900,
                &[job("1", "RUNNING", 100), job("2", "RUNNING", 5)],
                false,
                None,
            )
            .unwrap();
        store
            .record_routine_snapshot(
                "t",
                base + 300,
                900,
                &[job("1", "RUNNING", 400), job("2", "RUNNING", 5)],
                false,
                None,
            )
            .unwrap();
        store
            .record_routine_snapshot(
                "t",
                base + 3_600,
                900,
                &[job("1", "PAUSED", 400)],
                false,
                None,
            )
            .unwrap();
        // A truncated snapshot (job limit) cannot show a job absent.
        store
            .record_routine_snapshot(
                "t",
                base + 7_200,
                900,
                &[job("1", "PAUSED", 400)],
                true,
                None,
            )
            .unwrap();
        let request = LoadHistoryRoutineRequest {
            connection_id: None,
            target: None,
            days: 7,
            job_id: None,
        };
        let response = build_routine(&store, Some("t".into()), &request, now).unwrap();
        assert_eq!(response.cell_seconds, 3_600);
        assert_eq!(response.jobs.len(), 2);
        let first = &response.jobs[0];
        assert_eq!(first.loaded_rows.as_deref(), Some("300"));
        assert_eq!(first.current_state, "PAUSED");
        assert_eq!(first.pauses, 1);
        let today: Vec<_> = first
            .cells
            .iter()
            .filter(|cell| i64::from(cell.start_seconds) >= base)
            .collect();
        assert_eq!(today[0].state, RoutineHistoryState::Running);
        assert_eq!(today[1].state, RoutineHistoryState::Paused);
        assert_eq!(today[2].state, RoutineHistoryState::Paused);
        assert_eq!(first.cells[0].state, RoutineHistoryState::NotCollected);
        // Job 2 was missing from the 01:00 snapshot.
        let second: Vec<_> = response.jobs[1]
            .cells
            .iter()
            .filter(|cell| i64::from(cell.start_seconds) >= base)
            .collect();
        assert_eq!(second[1].state, RoutineHistoryState::Absent);
        assert_eq!(second[2].state, RoutineHistoryState::Unknown);
        // Job 2's counters did not move between two close snapshots: a known zero.
        assert_eq!(response.jobs[1].loaded_rows.as_deref(), Some("0"));
        let detail = response.selected.unwrap();
        assert_eq!(detail.job_id, "1");
        assert!(detail.lag_known);
        let last_day = detail.days.last().unwrap();
        assert_eq!(last_day.loaded_rows.as_deref(), Some("300"));
        assert!(detail.days[0].loaded_rows.is_none());
        assert_eq!(detail.events[0].to_state, "PAUSED");
    }

    #[test]
    fn audit_lh_06_error_log_lists_carry_facts_and_the_text_needs_one_load() {
        let store = HistoryStore::open_in_memory().unwrap();
        let day = DAY;
        let now = 10 * day;
        let mut filtered = load(LoadHistoryKind::StreamLoad, now - 100, false, 3);
        filtered.job_id = 42;
        let candidate = ErrorLogCandidate {
            job_id: 42,
            label: filtered.label.clone(),
            database: "ods".into(),
            table: "orders".into(),
            kind: LoadHistoryKind::StreamLoad,
            finish_time: now - 100,
            filtered_rows: Some(3),
        };
        store
            .record_load_slice(
                "t",
                "v",
                "r",
                &complete(now - 3_600, now),
                now,
                &aggregate_loads(&[filtered]),
                &[candidate],
            )
            .unwrap();
        let secret_row = "3\ttoolongname-secret\t3";
        store
            .save_error_log(
                "t",
                "42",
                ErrorLogOutcome::Saved {
                    entries: vec![LoadErrorEntry {
                        index: 1,
                        category: LoadErrorCategory::TooLong,
                        column: Some("name".into()),
                        target_type: Some("VARCHAR(5)".into()),
                        reason: "Value: toolongname-secret".into(),
                        row: Some(secret_row.into()),
                        row_converted: false,
                        truncated: false,
                    }],
                    categories: vec![LoadErrorCategoryCount {
                        category: LoadErrorCategory::TooLong,
                        column: Some("name".into()),
                        target_type: Some("VARCHAR(5)".into()),
                        count: 1,
                    }],
                    truncated: false,
                },
                now,
            )
            .unwrap();
        let request = LoadHistoryErrorLogsRequest {
            connection_id: None,
            target: None,
            days: 7,
            table: None,
            category: None,
            offset: 0,
        };
        let list = build_error_logs(&store, Some("t".into()), &request, now).unwrap();
        assert_eq!(list.total, 1);
        assert_eq!(list.logs[0].state, StoredLoadErrorLogState::Saved);
        assert_eq!(list.reasons[0].category, LoadErrorCategory::TooLong);
        let history = build_history(
            &store,
            Some("t".into()),
            &LoadHistoryRequest {
                connection_id: None,
                target: None,
                days: 7,
                kind: None,
                table: None,
                include_system: false,
            },
            now,
        )
        .unwrap();
        assert_eq!(
            history.tables[0]
                .top_reason
                .as_ref()
                .unwrap()
                .sample_share_permyriad,
            10_000
        );
        assert_eq!(history.tables[0].saved_logs, 1);
        // Lists and summaries never carry the reason or row text.
        for text in [
            serde_json::to_string(&list).unwrap(),
            serde_json::to_string(&history).unwrap(),
        ] {
            assert!(!text.contains("toolongname-secret"));
        }
        assert_eq!(
            store.error_log_entries("t", "42").unwrap()[0]
                .row
                .as_deref(),
            Some(secret_row)
        );
        let none = LoadHistoryErrorLogsRequest {
            category: Some(LoadErrorCategory::JsonCast),
            ..request
        };
        assert_eq!(
            build_error_logs(&store, Some("t".into()), &none, now)
                .unwrap()
                .total,
            0
        );
    }
}
