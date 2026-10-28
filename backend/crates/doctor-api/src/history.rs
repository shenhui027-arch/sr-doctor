//! Stage B: the local history store and one bounded collector per connected
//! session (ADR-2, ADR-3, ADR-8). A collector reads closed audit slices
//! behind a fixed lag, oldest first, never more than a few per cycle; it
//! records gaps instead of inventing data, backs off on failure, stops when
//! its session ends and never runs against a remote cluster until the
//! operator enables that after G1.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use doctor_connector::{
    AuditSourceStatementSample, AuditStatementSampleSource, ConnectorError, ConnectorErrorCode,
    SessionManager,
};
use doctor_contracts::{
    CollectionBudget, CollectionRun, CollectionRunState, CollectionState, CollectionStoppedReason,
    CoverageHour, CoverageHourState, HistoryCollectionAction, HistoryCollectionRequest,
    HistoryStatusRequest, HistoryStatusResponse, HistoryStoreState, HistoryStoreUnavailableReason,
};
use doctor_core::{
    AuditFingerprintStatement, CollectedExecution, HISTORY_BUCKET_SECONDS, aggregate_collection,
    history_rule_version,
};
use doctor_history::{HistoryStore, RunRecord, RunState};

use super::*;
use crate::audit::no_store;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryConfig {
    /// `None` disables history and collection entirely.
    pub path: Option<PathBuf>,
    /// Remote (TLS) clusters are collected only after the G1 pilot passes.
    pub allow_remote_collection: bool,
    pub period_seconds: u32,
    pub slice_seconds: u32,
    pub lag_seconds: u32,
    pub catch_up_seconds: u32,
    /// How far back a target with no watermark starts.
    pub initial_backfill_seconds: u32,
    pub max_slices_per_cycle: u32,
    pub retention_days: u16,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            path: None,
            allow_remote_collection: false,
            period_seconds: 300,
            slice_seconds: 300,
            lag_seconds: 120,
            catch_up_seconds: 24 * 3600,
            initial_backfill_seconds: 3600,
            max_slices_per_cycle: 6,
            retention_days: 30,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlicePlan {
    /// An unread span older than the catch-up horizon: recorded, never read.
    pub gap: Option<(i64, i64)>,
    pub slices: Vec<(i64, i64)>,
}

/// The slices one cycle reads: aligned, closed for at least `lag`, starting at
/// the watermark, at most `max_slices_per_cycle`. Pure, for fake-clock tests.
pub fn plan_slices(watermark: Option<i64>, now: i64, config: &HistoryConfig) -> SlicePlan {
    let slice = i64::from(config.slice_seconds.max(1));
    let settled = (now - i64::from(config.lag_seconds)).div_euclid(slice) * slice;
    let horizon = settled - i64::from(config.catch_up_seconds);
    let mut start = watermark.unwrap_or_else(|| {
        (settled - i64::from(config.initial_backfill_seconds)).div_euclid(slice) * slice
    });
    let mut gap = None;
    if start < horizon {
        let aligned = horizon.div_euclid(slice) * slice;
        gap = Some((start, aligned));
        start = aligned;
    }
    let mut slices = Vec::new();
    while start + slice <= settled && slices.len() < config.max_slices_per_cycle as usize {
        slices.push((start, start + slice));
        start += slice;
    }
    SlicePlan { gap, slices }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CollectorStatus {
    Running { next_run: i64 },
    Paused,
    Backoff { failures: u32, next_run: i64 },
    Stopped(CollectionStoppedReason),
}

pub(crate) struct CollectorEntry {
    target: String,
    status: Mutex<CollectorStatus>,
    stop: AtomicBool,
    wake: tokio::sync::Notify,
}

pub(crate) struct HistoryRuntime {
    pub(crate) config: HistoryConfig,
    store: Option<Arc<HistoryStore>>,
    open_failure: Option<HistoryStoreUnavailableReason>,
    collectors: Mutex<BTreeMap<String, Arc<CollectorEntry>>>,
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn seconds(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

impl HistoryRuntime {
    pub(crate) fn open(config: HistoryConfig) -> Self {
        let (store, open_failure) = match &config.path {
            None => (None, None),
            Some(path) => match HistoryStore::open(path) {
                Ok(store) => (Some(Arc::new(store)), None),
                Err(error) => {
                    let reason = error.unavailable_reason();
                    eprintln!("history_store_unavailable code={}", reason.code());
                    (None, Some(reason))
                }
            },
        };
        Self {
            config,
            store,
            open_failure,
            collectors: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn store(&self) -> Option<&Arc<HistoryStore>> {
        self.store.as_ref()
    }

    fn collectors(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<CollectorEntry>>> {
        self.collectors
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn entry(&self, connection_id: &str) -> Option<Arc<CollectorEntry>> {
        self.collectors().get(connection_id).cloned()
    }

    pub(crate) fn stop(&self, connection_id: &str) {
        if let Some(entry) = self.collectors().remove(connection_id) {
            entry.stop.store(true, Ordering::SeqCst);
            entry.wake.notify_one();
        }
    }
}

/// Stops a session's collector and hands collection to a waiting session.
pub(crate) fn stop_collector(state: &ApiState, connection_id: &str) {
    state.history.stop(connection_id);
    promote_waiting(state);
}

fn promote_waiting(state: &ApiState) {
    let next = {
        let collectors = state.history.collectors();
        let active = collectors.values().any(|entry| {
            !entry.stop.load(Ordering::SeqCst)
                && !matches!(
                    *entry.status.lock().unwrap_or_else(|p| p.into_inner()),
                    CollectorStatus::Stopped(_)
                )
        });
        if active {
            return;
        }
        collectors.iter().find_map(|(id, entry)| {
            let mut status = entry.status.lock().unwrap_or_else(|p| p.into_inner());
            (*status == CollectorStatus::Stopped(CollectionStoppedReason::AnotherSessionCollecting))
                .then(|| {
                    *status = CollectorStatus::Running {
                        next_run: unix_now(),
                    };
                    (id.clone(), Arc::clone(entry))
                })
        })
    };
    if let Some((id, entry)) = next {
        tokio::spawn(collector_loop(state.clone(), id, entry));
    }
}

/// Registers a collector for a newly connected session; spawns its loop
/// unless the target is remote and production collection is not enabled.
pub(crate) fn start_collector(
    state: &ApiState,
    connection_id: &str,
    host: &str,
    port: u16,
    remote: bool,
) {
    let runtime = &state.history;
    if runtime.store.is_none() {
        return;
    }
    let gated = remote && !runtime.config.allow_remote_collection;
    // One bounded collector per process: a second session waits its turn.
    let busy = runtime.collectors().values().any(|entry| {
        !entry.stop.load(Ordering::SeqCst)
            && !matches!(
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()),
                CollectorStatus::Stopped(_)
            )
    });
    let entry = Arc::new(CollectorEntry {
        target: format!("{host}:{port}"),
        status: Mutex::new(if gated {
            CollectorStatus::Stopped(CollectionStoppedReason::ProductionGate)
        } else if busy {
            CollectorStatus::Stopped(CollectionStoppedReason::AnotherSessionCollecting)
        } else {
            CollectorStatus::Running {
                next_run: unix_now(),
            }
        }),
        stop: AtomicBool::new(false),
        wake: tokio::sync::Notify::new(),
    });
    runtime
        .collectors()
        .insert(connection_id.to_owned(), Arc::clone(&entry));
    if !gated && !busy {
        tokio::spawn(collector_loop(
            state.clone(),
            connection_id.to_owned(),
            entry,
        ));
    }
}

enum CycleOutcome {
    Read,
    Paused,
}

fn collected(rows: Vec<doctor_connector::AuditOccurrenceSourceRow>) -> Vec<CollectedExecution> {
    rows.into_iter()
        .map(|row| CollectedExecution {
            query_id: row.query_id,
            catalog: row.catalog,
            database: row.database,
            native_digest: row.native_digest,
            statement: match row.statement_sample {
                AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                    value,
                    truncated,
                }) => AuditFingerprintStatement::Text { value, truncated },
                AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull) => {
                    AuditFingerprintStatement::SqlNull
                }
                AuditSourceStatementSample::ConflictingSourceRows => {
                    AuditFingerprintStatement::Conflicting
                }
                _ => AuditFingerprintStatement::NotRead,
            },
            error: row.state.as_deref() == Some("ERR"),
            event_epoch_seconds: row
                .event_epoch_seconds
                .as_deref()
                .and_then(|value| value.parse().ok()),
            metrics: row.metrics,
        })
        .collect()
}

/// One cycle: the planned slices, each atomically recorded. A failed read
/// is recorded and returned; the watermark stays at the last good slice.
fn run_cycle(
    sessions: &SessionManager,
    store: &HistoryStore,
    config: &HistoryConfig,
    connection_id: &str,
    target: &str,
    now: i64,
) -> Result<CycleOutcome, ConnectorError> {
    let state = store
        .target_state(target)
        .map_err(|_| history_error("history_store"))?;
    if state.paused {
        if !sessions.is_active(connection_id) {
            return Err(ConnectorError {
                code: ConnectorErrorCode::ConnectionExpired,
                stage: "connection",
                limitation: None,
                affected_side: None,
            });
        }
        return Ok(CycleOutcome::Paused);
    }
    let plan = plan_slices(state.watermark, now, config);
    if let Some((start, end)) = plan.gap {
        let run = RunRecord {
            started_at: now,
            slice_start: start,
            slice_end: end,
            state: RunState::Gap,
            reason: Some("beyond_catch_up".into()),
            rows_read: 0,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 0,
        };
        store
            .record_run(target, &run, Some(end))
            .map_err(|_| history_error("history_store"))?;
    }
    for (start, end) in plan.slices {
        let began = Instant::now();
        let slice = match sessions.audit_collection_slice_controlled(
            connection_id,
            seconds(start),
            seconds(end),
            None,
        ) {
            Ok(slice) => slice,
            Err(error) => {
                if error.code != ConnectorErrorCode::ConnectionExpired {
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
                    let _ = store.record_run(target, &run, None);
                }
                return Err(error);
            }
        };
        let total: u64 = slice.slice_count.parse().unwrap_or(0);
        let read = slice.rows.len() as u64;
        let engine = slice.session_identity.detected_version.clone();
        let rows = collected(slice.rows);
        let (templates, buckets, tally) = aggregate_collection(target, &rows);
        let partial = read < total;
        let run = RunRecord {
            started_at: now,
            slice_start: start,
            slice_end: end,
            state: if partial {
                RunState::Partial
            } else {
                RunState::Complete
            },
            reason: partial.then(|| "row_limit".to_owned()),
            rows_read: read,
            attributed: tally.attributed,
            partial_statements: tally.partial,
            unavailable_statements: tally.unavailable + tally.missing_context + tally.missing_time,
            duration_ms: began.elapsed().as_millis() as u64,
        };
        let recorded = store
            .record_slice(
                target,
                &engine,
                &history_rule_version(),
                &run,
                end,
                &templates,
                &buckets,
            )
            .map_err(|_| history_error("history_store"))?;
        if !recorded {
            // Another writer is ahead of this plan; replan next cycle.
            break;
        }
    }
    let cutoff = now - i64::from(config.retention_days) * 86_400;
    let _ = store.purge_before(cutoff);
    // ADR-6: new data may raise, refresh or resolve in-app alerts.
    let _ = crate::governance::evaluate_alerts(store, target, now);
    Ok(CycleOutcome::Read)
}

fn history_error(stage: &'static str) -> ConnectorError {
    ConnectorError {
        code: ConnectorErrorCode::CapabilityUnavailable,
        stage,
        limitation: Some("history_store_unavailable"),
        affected_side: None,
    }
}

async fn collector_loop(state: ApiState, connection_id: String, entry: Arc<CollectorEntry>) {
    let Some(store) = state.history.store.clone() else {
        return;
    };
    let config = state.history.config.clone();
    let mut failures = 0u32;
    while !entry.stop.load(Ordering::SeqCst) {
        let (sessions, store_for_cycle, config_for_cycle, id, target) = (
            Arc::clone(&state.sessions),
            Arc::clone(&store),
            config.clone(),
            connection_id.clone(),
            entry.target.clone(),
        );
        let now = unix_now();
        let outcome = tokio::task::spawn_blocking(move || {
            run_cycle(
                &sessions,
                &store_for_cycle,
                &config_for_cycle,
                &id,
                &target,
                now,
            )
        })
        .await;
        let period = i64::from(config.period_seconds);
        let delay = match outcome {
            Ok(Ok(CycleOutcome::Paused)) => {
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) = CollectorStatus::Paused;
                period
            }
            Ok(Ok(CycleOutcome::Read)) => {
                failures = 0;
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    CollectorStatus::Running {
                        next_run: now + period,
                    };
                period
            }
            Ok(Err(error)) if error.code == ConnectorErrorCode::ConnectionExpired => {
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    CollectorStatus::Stopped(CollectionStoppedReason::NotConnected);
                state.history.collectors().remove(&connection_id);
                promote_waiting(&state);
                return;
            }
            _ => {
                failures = failures.saturating_add(1);
                let delay = (period.saturating_mul(1 << failures.min(10))).min(1800);
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    CollectorStatus::Backoff {
                        failures,
                        next_run: now + delay,
                    };
                delay
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(delay.max(1) as u64)) => {}
            _ = entry.wake.notified() => {}
        }
    }
}

fn run_state(state: RunState) -> CollectionRunState {
    match state {
        RunState::Complete => CollectionRunState::Complete,
        RunState::Partial => CollectionRunState::Partial,
        RunState::Failed => CollectionRunState::Failed,
        RunState::Gap => CollectionRunState::Gap,
    }
}

/// Each of the last 24 hours, from the runs that cover it.
pub(crate) fn coverage_interval(runs: &[RunRecord], start: i64, end: i64) -> CoverageHourState {
    let covered = |complete_only: bool| {
        let mut intervals: Vec<_> = runs
            .iter()
            .filter(|run| {
                run.slice_start < end
                    && run.slice_end > start
                    && (run.state == RunState::Complete
                        || !complete_only && run.state == RunState::Partial)
            })
            .map(|run| (run.slice_start.max(start), run.slice_end.min(end)))
            .collect();
        intervals.sort_unstable();
        let mut cursor = start;
        let mut total = 0;
        for (left, right) in intervals {
            let left = left.max(cursor);
            if right > left {
                total += right - left;
                cursor = right;
            }
        }
        total
    };
    // Successful coverage survives failed attempts. Union intervals rather
    // than summing overlaps, which could otherwise conceal an unread gap.
    if covered(true) == end - start {
        CoverageHourState::Complete
    } else if covered(false) > 0 {
        CoverageHourState::Partial
    } else {
        CoverageHourState::NotCollected
    }
}

pub(crate) fn coverage_hours(runs: &[RunRecord], now: i64) -> Vec<CoverageHour> {
    let current = now.div_euclid(HISTORY_BUCKET_SECONDS) * HISTORY_BUCKET_SECONDS;
    (0..24)
        .rev()
        .map(|back| {
            let start = current - back * HISTORY_BUCKET_SECONDS;
            let end = start + HISTORY_BUCKET_SECONDS;
            let state = coverage_interval(runs, start, end);
            CoverageHour {
                hour_start_seconds: seconds(start),
                state,
            }
        })
        .collect()
}

pub(super) async fn status(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistoryStatusRequest = connector_json_request(&state, request).await?;
    let runtime = &state.history;
    let now = unix_now();
    let config = &runtime.config;
    let entry = request
        .connection_id
        .as_deref()
        .and_then(|id| runtime.entry(id));
    // Not connected: show the most recently collected target, so the
    // watermark and coverage stay visible between sessions.
    let target = match (&entry, runtime.store()) {
        (Some(entry), _) => Some(entry.target.clone()),
        (None, Some(store)) => store
            .targets()
            .map_err(|_| internal_failure(&state))?
            .into_iter()
            .next(),
        (None, None) => None,
    };
    let store_state = match (runtime.store(), runtime.open_failure) {
        (None, Some(reason)) => HistoryStoreState::Unavailable {
            reason,
            code: reason.code().into(),
        },
        (None, None) => HistoryStoreState::Disabled,
        (Some(store), _) => {
            let _ = store.checkpoint();
            let stats = store.stats().map_err(|_| internal_failure(&state))?;
            HistoryStoreState::Available {
                retention_days: config.retention_days,
                file_bytes: stats.file_bytes.map(|bytes| bytes.to_string()),
                templates: stats.templates.to_string(),
                buckets: stats.buckets.to_string(),
            }
        }
    };
    let collection = match (&entry, runtime.store()) {
        (_, None) => CollectionState::Stopped {
            reason: CollectionStoppedReason::HistoryDisabled,
        },
        (None, _) => CollectionState::Stopped {
            reason: CollectionStoppedReason::NotConnected,
        },
        (Some(entry), _) => match entry
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            CollectorStatus::Running { next_run } => CollectionState::Running {
                next_run_seconds: seconds(next_run),
            },
            CollectorStatus::Paused => CollectionState::Paused,
            CollectorStatus::Backoff { failures, next_run } => CollectionState::Backoff {
                failures,
                next_run_seconds: seconds(next_run),
            },
            CollectorStatus::Stopped(reason) => CollectionState::Stopped { reason },
        },
    };
    let (watermark, runs, coverage_runs, open_alerts) = match (runtime.store(), &target) {
        (Some(store), Some(target)) => {
            let watermark = store
                .target_state(target)
                .map_err(|_| internal_failure(&state))?
                .watermark;
            let runs = store
                .runs(target, now - 24 * 3600, now + 1, 20)
                .map_err(|_| internal_failure(&state))?;
            let current = now.div_euclid(HISTORY_BUCKET_SECONDS) * HISTORY_BUCKET_SECONDS;
            let coverage_runs = store
                .runs(
                    target,
                    current - 23 * HISTORY_BUCKET_SECONDS,
                    current + HISTORY_BUCKET_SECONDS,
                    u32::MAX,
                )
                .map_err(|_| internal_failure(&state))?;
            let open = store
                .alerts(target, 500)
                .map_err(|_| internal_failure(&state))?
                .iter()
                .filter(|alert| alert.state == "open")
                .count();
            (watermark, runs, coverage_runs, Some(open.to_string()))
        }
        _ => (None, Vec::new(), Vec::new(), None),
    };
    let response = HistoryStatusResponse {
        store: store_state,
        target,
        collection,
        watermark_seconds: watermark.map(seconds),
        budget: CollectionBudget {
            period_seconds: config.period_seconds,
            slice_seconds: config.slice_seconds,
            lag_seconds: config.lag_seconds,
            slice_row_limit: u32::from(doctor_connector::AUDIT_COLLECTION_SLICE_LIMIT),
            query_timeout_seconds: u32::from(doctor_connector::AUDIT_QUERY_TIMEOUT_SECONDS),
            catch_up_seconds: config.catch_up_seconds,
            production_budget_measured: false,
        },
        coverage: coverage_hours(&coverage_runs, now),
        runs: runs
            .iter()
            .take(20)
            .map(|run| CollectionRun {
                started_seconds: seconds(run.started_at),
                slice_start_seconds: seconds(run.slice_start),
                slice_end_seconds: seconds(run.slice_end),
                state: run_state(run.state),
                reason: run.reason.clone(),
                rows_read: run.rows_read.to_string(),
                attributed: run.attributed.to_string(),
                partial_statements: run.partial_statements.to_string(),
                unavailable_statements: run.unavailable_statements.to_string(),
                duration_ms: run.duration_ms.to_string(),
            })
            .collect(),
        remote_collection_enabled: config.allow_remote_collection,
        open_alerts,
        limitations: [
            "collection_runs_only_while_this_session_is_connected",
            "rows_arriving_later_than_the_lag_are_not_read",
            "quantiles_are_log_histogram_approximations",
            "production_budget_pending_g1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    };
    Ok(no_store(response))
}

pub(super) async fn collection(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistoryCollectionRequest = connector_json_request(&state, request).await?;
    let runtime = &state.history;
    let (Some(store), Some(entry)) = (runtime.store(), runtime.entry(&request.connection_id))
    else {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::CONFLICT,
            "collection_unavailable",
            "history",
            "collection_unavailable",
            "change_input",
        ));
    };
    let paused = request.action == HistoryCollectionAction::Pause;
    store
        .set_paused(&entry.target, paused, unix_now())
        .map_err(|_| internal_failure(&state))?;
    {
        let mut status = entry.status.lock().unwrap_or_else(|p| p.into_inner());
        if !matches!(*status, CollectorStatus::Stopped(_)) {
            *status = if paused {
                CollectorStatus::Paused
            } else {
                CollectorStatus::Running {
                    next_run: unix_now(),
                }
            };
        }
    }
    entry.wake.notify_one();
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn clear(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let _: serde_json::Value = connector_json_request(&state, request).await?;
    let Some(store) = state.history.store() else {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::CONFLICT,
            "history_disabled",
            "history",
            "history_disabled",
            "change_input",
        ));
    };
    store.clear().map_err(|_| internal_failure(&state))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_v1_11_overlaps_never_fill_a_gap_and_late_rows_remain_partial() {
        let run = |start, end, state| RunRecord {
            started_at: end,
            slice_start: start,
            slice_end: end,
            state,
            reason: None,
            rows_read: 0,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 0,
        };
        let duplicates = vec![
            run(0, 1800, RunState::Complete),
            run(0, 1800, RunState::Complete),
        ];
        assert_eq!(
            coverage_interval(&duplicates, 0, 3600),
            CoverageHourState::Partial
        );
        let partial = vec![
            run(0, 1800, RunState::Partial),
            run(1800, 3600, RunState::Complete),
        ];
        assert_eq!(
            coverage_interval(&partial, 0, 3600),
            CoverageHourState::Partial
        );
        let store = HistoryStore::open_in_memory().unwrap();
        store
            .record_slice(
                "t",
                "3.5.14",
                "v1",
                &run(0, 3600, RunState::Complete),
                3600,
                &[],
                &[],
            )
            .unwrap();
        store.mark_partial("t", 0, 3600, "late_rows").unwrap();
        assert_eq!(
            coverage_interval(&store.runs("t", 0, 3600, u32::MAX).unwrap(), 0, 3600),
            CoverageHourState::Partial
        );
    }

    #[test]
    fn audit_v1_11_successful_retry_restores_coverage_in_both_views() {
        let store = HistoryStore::open_in_memory().unwrap();
        let start = 9 * 3600;
        let failed = RunRecord {
            started_at: start + 3601,
            slice_start: start,
            slice_end: start + 3600,
            state: RunState::Failed,
            reason: Some("read_failed".into()),
            rows_read: 0,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 1,
        };
        store.record_run("t", &failed, None).unwrap();
        store
            .record_slice(
                "t",
                "3.5.14",
                "v1",
                &RunRecord {
                    state: RunState::Complete,
                    started_at: start + 3602,
                    reason: None,
                    ..failed
                },
                start + 3600,
                &[],
                &[],
            )
            .unwrap();
        let runs = store.runs("t", start, start + 3600, u32::MAX).unwrap();
        assert_eq!(runs.len(), 2, "failure evidence stays available");
        assert_eq!(
            store.target_state("t").unwrap().watermark,
            Some(start + 3600)
        );
        assert_eq!(
            coverage_hours(&runs, start + 3601)[22].state,
            CoverageHourState::Complete
        );
        assert_eq!(
            crate::history_trend::coverage_map(&runs, start, start + 3600)[&start],
            CoverageHourState::Complete
        );
    }

    fn config() -> HistoryConfig {
        HistoryConfig {
            slice_seconds: 300,
            lag_seconds: 120,
            catch_up_seconds: 3600,
            initial_backfill_seconds: 900,
            max_slices_per_cycle: 3,
            ..HistoryConfig::default()
        }
    }

    #[test]
    fn slices_are_aligned_settled_bounded_and_resume_from_the_watermark() {
        // now = 10:10 + 30 s; settled = 10:05 (lag 2 min).
        let now = 36_600 + 30;
        let first = plan_slices(None, now, &config());
        assert_eq!(first.gap, None);
        assert_eq!(
            first.slices,
            vec![(35_400, 35_700), (35_700, 36_000), (36_000, 36_300)]
        );
        // Resuming from the last slice end never re-reads it.
        let next = plan_slices(Some(36_300), now, &config());
        assert!(next.slices.is_empty());
        let later = plan_slices(Some(36_300), now + 300, &config());
        assert_eq!(later.slices, vec![(36_300, 36_600)]);
        // A long disconnect: the unread span past the catch-up horizon is a gap.
        let reconnect = plan_slices(Some(0), now, &config());
        assert_eq!(reconnect.gap, Some((0, 32_700)));
        assert_eq!(reconnect.slices.first(), Some(&(32_700, 33_000)));
    }

    #[test]
    fn coverage_marks_gaps_and_partial_hours_without_inventing_data() {
        let run = |start: i64, end: i64, state: RunState| RunRecord {
            started_at: end,
            slice_start: start,
            slice_end: end,
            state,
            reason: None,
            rows_read: 0,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 0,
        };
        let now = 10 * 3600 + 10;
        let runs = vec![
            run(9 * 3600, 10 * 3600, RunState::Complete),
            run(8 * 3600, 8 * 3600 + 1800, RunState::Complete),
            run(7 * 3600, 8 * 3600, RunState::Partial),
            run(6 * 3600, 7 * 3600, RunState::Gap),
        ];
        let hours = coverage_hours(&runs, now);
        assert_eq!(hours.len(), 24);
        let at = |hour: i64| {
            hours
                .iter()
                .find(|h| h.hour_start_seconds == seconds(hour * 3600))
                .unwrap()
                .state
        };
        assert_eq!(at(9), CoverageHourState::Complete);
        assert_eq!(at(8), CoverageHourState::Partial);
        assert_eq!(at(7), CoverageHourState::Partial);
        assert_eq!(at(6), CoverageHourState::NotCollected);
        assert_eq!(at(10), CoverageHourState::NotCollected);
    }
}
