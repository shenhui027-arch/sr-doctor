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
    AuditCollectionRow, AuditSourceStatementSample, AuditStatementProof,
    AuditStatementSampleSource, CollectionTimeProof, ConnectorError, ConnectorErrorCode,
    OperationControl, SessionManager,
};
use doctor_contracts::{
    CollectionBudget, CollectionRun, CollectionRunState, CollectionState, CollectionStoppedReason,
    CoverageHour, CoverageHourState, HistoryCollectionAction, HistoryCollectionRequest,
    HistoryStatusRequest, HistoryStatusResponse, HistoryStoreState, HistoryStoreUnavailableReason,
};
use doctor_core::{
    AuditFingerprintStatement, CollectedExecution, HISTORY_BUCKET_SECONDS, history_rule_version,
};
use doctor_history::{
    AUDIT_REPLAY_MAX_AGE_SECONDS, AuditReplayCandidate, AuditSliceCommit, AuditSliceWrite,
    HistoryStore, RunRecord, RunState, SliceEvidence, audit_identity,
};

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
    /// Task 72 review: every collection cycle holds this shared; clearing
    /// history takes it exclusively, so no cycle that started before the clear
    /// writes after it.
    pub(crate) cycle_gate: std::sync::RwLock<()>,
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
                Ok(store) => {
                    // Retention does not wait for a collector: a store left
                    // closed past it drops expired rows (error log text too).
                    let _ =
                        store.purge_before(unix_now() - i64::from(config.retention_days) * 86_400);
                    (Some(Arc::new(store)), None)
                }
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
            cycle_gate: std::sync::RwLock::new(()),
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

    /// The "host:port" target the collector of this connection writes to.
    pub(crate) fn entry_target(&self, connection_id: &str) -> Option<String> {
        self.entry(connection_id).map(|entry| entry.target.clone())
    }

    fn entry(&self, connection_id: &str) -> Option<Arc<CollectorEntry>> {
        self.collectors().get(connection_id).cloned()
    }

    /// The collector a page should see (task 72): its own, unless that one
    /// waits behind the collector of the same cluster (the saved connection
    /// collecting in the background); with no page session, whichever runs.
    fn resolve(&self, connection_id: Option<&str>) -> Option<Arc<CollectorEntry>> {
        let collectors = self.collectors();
        let running = |entry: &&Arc<CollectorEntry>| {
            !entry.stop.load(Ordering::SeqCst)
                && !matches!(
                    *entry.status.lock().unwrap_or_else(|p| p.into_inner()),
                    CollectorStatus::Stopped(_)
                )
        };
        let active = collectors.values().find(running).cloned();
        match (
            connection_id.and_then(|id| collectors.get(id).cloned()),
            active,
        ) {
            (Some(own), Some(active))
                if own.target == active.target
                    && *own.status.lock().unwrap_or_else(|p| p.into_inner())
                        == CollectorStatus::Stopped(
                            CollectionStoppedReason::AnotherSessionCollecting,
                        ) =>
            {
                Some(active)
            }
            (Some(own), _) => Some(own),
            (None, active) => active,
        }
    }

    /// Whether this session's collector is the one reading (task 72).
    pub(crate) fn collector_running(&self, connection_id: &str) -> bool {
        self.collectors().get(connection_id).is_some_and(|entry| {
            !entry.stop.load(Ordering::SeqCst)
                && !matches!(
                    *entry.status.lock().unwrap_or_else(|p| p.into_inner()),
                    CollectorStatus::Stopped(_)
                )
        })
    }

    pub(crate) fn stop(&self, connection_id: &str) {
        if let Some(entry) = self.collectors().remove(connection_id) {
            entry.stop.store(true, Ordering::SeqCst);
            entry.wake.notify_one();
        }
    }
}

/// Stops a session's collector and hands collection to a waiting session.
/// Retention runs hourly on its own, so expired rows (error log text
/// included) go even while collection is paused or nothing is connected.
pub(crate) async fn retention_worker(state: ApiState) {
    let Some(store) = state.history.store.clone() else {
        return;
    };
    let days = i64::from(state.history.config.retention_days);
    loop {
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        let store = Arc::clone(&store);
        let _ = tokio::task::spawn_blocking(move || store.purge_before(unix_now() - days * 86_400))
            .await;
    }
}

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
    // One bounded collector per process: a second session waits its turn. The
    // check and the insert happen under one lock (task 72 review), so a page
    // session and the saved connection's background session that connect at
    // the same moment cannot both start a collector.
    let mut collectors = runtime.collectors();
    let busy = collectors.values().any(|entry| {
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
    collectors.insert(connection_id.to_owned(), Arc::clone(&entry));
    drop(collectors);
    if !gated && !busy {
        tokio::spawn(collector_loop(
            state.clone(),
            connection_id.to_owned(),
            entry,
        ));
    }
}

enum CycleOutcome {
    /// Whether each due source read successfully; `None` when not due.
    Read {
        audit: Option<bool>,
        loads: Option<bool>,
    },
    Paused,
}

/// One source of the single collector (audit or loads). Each backs off on
/// its own failures, so a source that keeps failing neither stops nor is
/// masked by the other.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SourceClock {
    pub(crate) failures: u32,
    pub(crate) next_run: i64,
}

impl SourceClock {
    pub(crate) fn due(&self, now: i64) -> bool {
        now >= self.next_run
    }

    pub(crate) fn record(&mut self, ok: bool, now: i64, period: i64) {
        if ok {
            self.failures = 0;
            self.next_run = now + period;
        } else {
            self.failures = self.failures.saturating_add(1);
            self.next_run = now + period.saturating_mul(1 << self.failures.min(10)).min(1800);
        }
    }
}

fn collected(rows: Vec<AuditCollectionRow>) -> Vec<CollectedExecution> {
    rows.into_iter()
        .map(|row| {
            let AuditCollectionRow {
                occurrence: row,
                statement_proof,
                ..
            } = row;
            CollectedExecution {
                query_id: row.query_id,
                catalog: row.catalog,
                database: row.database,
                native_digest: row.native_digest,
                statement: match row.statement_sample {
                    AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                        value,
                        ..
                    }) => AuditFingerprintStatement::Text {
                        value,
                        truncated: statement_proof != AuditStatementProof::Complete,
                    },
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
            }
        })
        .collect()
}

/// One cycle: the due audit slices, then the due load part (task 58). Each
/// part's success is reported so it backs off on its own; only an ended
/// session (or a store failure) fails the cycle.
#[allow(clippy::too_many_arguments)]
fn run_cycle(
    sessions: &SessionManager,
    store: &HistoryStore,
    config: &HistoryConfig,
    connection_id: &str,
    target: &str,
    now: i64,
    audit_due: bool,
    loads_due: bool,
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
    let expired = |error: &ConnectorError| error.code == ConnectorErrorCode::ConnectionExpired;
    let audit = audit_due.then(|| {
        run_audit_slices(
            sessions,
            store,
            config,
            connection_id,
            target,
            state.watermark,
            now,
        )
    });
    if let Some(Err(error)) = &audit
        && expired(error)
    {
        return Err(error.clone());
    }
    // Task 58: loads are read even when the audit part failed (a cluster
    // without AuditLoader still has load history), and the other way round.
    let loads = loads_due.then(|| {
        crate::history_loads::run_load_cycle(sessions, store, config, connection_id, target, now)
    });
    if let Some(Err(error)) = &loads
        && expired(error)
    {
        return Err(error.clone());
    }
    let cutoff = now - i64::from(config.retention_days) * 86_400;
    let _ = store.purge_before(cutoff);
    // ADR-6: new data may raise, refresh or resolve in-app alerts; task 59
    // adds the load alerts of the same cycle.
    let loads_raised =
        crate::load_alerts::evaluate_load_alerts(store, target, now).unwrap_or_default();
    let _ = crate::governance::evaluate_alerts_with_loads(store, target, now, loads_raised);
    Ok(CycleOutcome::Read {
        audit: audit.map(|result| result.is_ok()),
        loads: loads.map(|result| result.is_ok()),
    })
}

fn plan_audit_replay(
    store: &HistoryStore,
    target: &str,
    settled: i64,
    _config: &HistoryConfig,
) -> doctor_history::Result<Option<AuditReplayCandidate>> {
    store.audit_replay_candidate(
        target,
        settled - AUDIT_REPLAY_MAX_AGE_SECONDS,
        settled - 30 * 60,
    )
}

/// The audit slices of one cycle, each atomically recorded. A failed read
/// is recorded and returned; the watermark stays at the last good slice.
fn run_audit_slices(
    sessions: &SessionManager,
    store: &HistoryStore,
    config: &HistoryConfig,
    connection_id: &str,
    target: &str,
    watermark: Option<i64>,
    now: i64,
) -> Result<(), ConnectorError> {
    let plan = plan_slices(watermark, now, config);
    let mut watermark = watermark;
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
        watermark = Some(end);
    }
    let mut remaining =
        i64::from(config.slice_seconds.max(1)) * i64::from(config.max_slices_per_cycle);
    let mut forward = plan.slices.into_iter();
    let source_identity = sessions.audit_collection_source_identity(connection_id, target)?;
    let rules = history_rule_version();
    let mut replayed = false;
    loop {
        if replayed
            || store
                .target_state(target)
                .map_err(|_| history_error("history_store"))?
                .paused
        {
            break;
        }
        let (start, end, mode) = if let Some((start, end)) = forward.next() {
            (
                start,
                end,
                AuditSliceWrite::Forward {
                    expected_watermark: watermark,
                },
            )
        } else {
            let Some(candidate) = plan_audit_replay(store, target, now, config)
                .map_err(|_| history_error("history_store"))?
            else {
                break;
            };
            replayed = true;
            (
                candidate.start,
                candidate.end,
                AuditSliceWrite::Replay {
                    expected_revision: candidate.revision,
                },
            )
        };
        let width = end - start;
        let context_start = seconds(start).saturating_sub(seconds(width));
        let context_end = seconds(end).saturating_add(seconds(width));
        let scan = i64::from(context_end - context_start);
        if scan > remaining {
            if replayed {
                break;
            }
            // No more forward reads fit. An older, narrower replay may
            // still fit, and planning seals expired hourly snapshots.
            forward = Vec::new().into_iter();
            continue;
        }
        remaining -= scan;

        let began = Instant::now();
        let timeout = Duration::from_secs(u64::from(doctor_connector::AUDIT_QUERY_TIMEOUT_SECONDS));
        let control = OperationControl::new(timeout);
        let (done, finished) = tokio::sync::oneshot::channel::<()>();
        let timer_control = Arc::clone(&control);
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(began + timeout)) => timer_control.stop(),
                _ = finished => {},
            }
        });
        let read = sessions.audit_collection_slice_controlled(
            connection_id,
            seconds(start),
            seconds(end),
            Some(&control),
        );
        let finish = sessions.finish_audit_operation(connection_id, &control);
        let _ = done.send(());
        let slice = match finish.and(read) {
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
                    if replayed {
                        let _ = store.record_collected_audit_slice(
                            target,
                            mode,
                            AuditSliceCommit {
                                run: &run,
                                evidence: &SliceEvidence {
                                    read_range: None,
                                    excluded_count: None,
                                    reasons: Vec::new(),
                                },
                                engine_version: "",
                                rule_version: &rules,
                                source_identity: &source_identity,
                                rows: Vec::new(),
                                excluded_ids: &[],
                                affected_neighbours: &[],
                            },
                        );
                    } else {
                        let _ = store.record_run(target, &run, None);
                    }
                }
                return Err(error);
            }
        };
        let mut evidence = SliceEvidence {
            read_range: None,
            excluded_count: None,
            reasons: Vec::new(),
        };
        let mut neighbours = Vec::new();
        if slice.time == CollectionTimeProof::AmbiguousLocalTime {
            evidence.reasons.push("ambiguous_local_time");
        } else if let Some(counts) = slice.counts {
            if counts.limited {
                evidence.reasons.push("row_limit");
            } else {
                evidence.read_range = Some((start, end));
            }
            evidence.excluded_count = (!counts.missing_conflict_ids).then_some(counts.excluded);
            if counts.excluded != 0 || counts.previous_slice_conflict || counts.next_slice_conflict
            {
                evidence.reasons.push("conflicting_query_id");
            }
            if counts.missing_conflict_ids && counts.previous_slice_conflict {
                neighbours.push((i64::from(context_start), start));
            }
            if counts.missing_conflict_ids && counts.next_slice_conflict {
                neighbours.push((end, i64::from(context_end)));
            }
        }
        let (usable, excluded): (Vec<_>, Vec<_>) = slice.rows.into_iter().partition(|row| {
            !matches!(
                row.occurrence.statement_sample,
                AuditSourceStatementSample::ConflictingSourceRows
            )
        });
        let excluded_ids: Vec<_> = excluded
            .iter()
            .filter_map(|row| {
                row.occurrence
                    .query_id
                    .as_deref()
                    .map(|id| (audit_identity(id), row.conflict_neighbours))
            })
            .collect();
        let rows = collected(usable);
        let run = RunRecord {
            started_at: now,
            slice_start: start,
            slice_end: end,
            state: RunState::Complete,
            reason: None,
            rows_read: 0,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: began.elapsed().as_millis() as u64,
        };
        let recorded = store
            .record_collected_audit_slice(
                target,
                mode,
                AuditSliceCommit {
                    run: &run,
                    evidence: &evidence,
                    engine_version: &slice.session_identity.detected_version,
                    rule_version: &rules,
                    source_identity: &source_identity,
                    rows,
                    excluded_ids: &excluded_ids,
                    affected_neighbours: &neighbours,
                },
            )
            .map_err(|_| history_error("history_store"))?;
        if !recorded {
            // Another writer is ahead of this plan; replan next cycle.
            break;
        }
        watermark = Some(end);
    }
    Ok(())
}

pub(crate) fn history_error(stage: &'static str) -> ConnectorError {
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
    let (mut audit_clock, mut loads_clock) = (SourceClock::default(), SourceClock::default());
    while !entry.stop.load(Ordering::SeqCst) {
        let (sessions, store_for_cycle, config_for_cycle, id, target) = (
            Arc::clone(&state.sessions),
            Arc::clone(&store),
            config.clone(),
            connection_id.clone(),
            entry.target.clone(),
        );
        let now = unix_now();
        let (audit_due, loads_due) = (audit_clock.due(now), loads_clock.due(now));
        // Collector reads never extend the session's idle lease (task 72 review):
        // a page session lives as long as its page, the saved connection's
        // background session as long as its supervisor keeps it.
        let (history, stopping) = (Arc::clone(&state.history), Arc::clone(&entry));
        let outcome = tokio::task::spawn_blocking(move || {
            let _cycle = history
                .cycle_gate
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            // A collector stopped while this cycle waited for the gate (e.g. by
            // a clear) reads and writes nothing.
            if stopping.stop.load(Ordering::SeqCst) {
                return Ok(CycleOutcome::Read {
                    audit: None,
                    loads: None,
                });
            }
            doctor_connector::without_lease(|| {
                run_cycle(
                    &sessions,
                    &store_for_cycle,
                    &config_for_cycle,
                    &id,
                    &target,
                    now,
                    audit_due,
                    loads_due,
                )
            })
        })
        .await;
        let period = i64::from(config.period_seconds);
        let delay = match outcome {
            Ok(Ok(CycleOutcome::Paused)) => {
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) = CollectorStatus::Paused;
                period
            }
            Ok(Ok(CycleOutcome::Read { audit, loads })) => {
                failures = 0;
                if let Some(ok) = audit {
                    audit_clock.record(ok, now, period);
                }
                if let Some(ok) = loads {
                    loads_clock.record(ok, now, period);
                }
                let next_run = audit_clock.next_run.min(loads_clock.next_run);
                // Retrying only when every source is failing; one healthy
                // source keeps the collector running.
                *entry.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    if audit_clock.failures > 0 && loads_clock.failures > 0 {
                        CollectorStatus::Backoff {
                            failures: audit_clock.failures.min(loads_clock.failures),
                            next_run,
                        }
                    } else {
                        CollectorStatus::Running { next_run }
                    };
                next_run - now
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
            _ = entry.wake.notified() => {
                // Resumed or asked to read now: both sources are due.
                audit_clock.next_run = 0;
                loads_clock.next_run = 0;
            }
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
    let entry = runtime.resolve(request.connection_id.as_deref());
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
    let (watermark, runs, coverage_runs, open_alerts, load_revision) =
        match (runtime.store(), &target) {
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
                let load_counts = store
                    .load_alert_counts(target)
                    .map_err(|_| internal_failure(&state))?;
                let open = store
                    .alerts(target, 500)
                    .map_err(|_| internal_failure(&state))?
                    .iter()
                    .filter(|alert| alert.state == "open")
                    .count() as u64
                    + load_counts[0];
                let load_revision = format!(
                    "{}|{}",
                    store
                        .load_target_state(target)
                        .map_err(|_| internal_failure(&state))?
                        .watermark
                        .unwrap_or(0),
                    store
                        .load_alert_revision(target)
                        .map_err(|_| internal_failure(&state))?
                );
                (
                    watermark,
                    runs,
                    coverage_runs,
                    Some(open.to_string()),
                    Some(load_revision),
                )
            }
            _ => (None, Vec::new(), Vec::new(), None, None),
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
        load_revision,
        limitations: [
            if state.saved.background() == doctor_contracts::SavedConnectionBackground::Collecting {
                "collection_runs_in_the_background_with_the_saved_connection"
            } else {
                "collection_runs_only_while_this_session_is_connected"
            },
            "audit_replay_can_collect_late_rows_30_to_60_minutes_after_slice_end",
            "audit_rows_arriving_after_replay_can_be_missed",
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
    let (Some(store), Some(entry)) = (
        runtime.store(),
        runtime.resolve(request.connection_id.as_deref()),
    ) else {
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
    // Clearing also forgets the saved connection (task 72 review). Under the
    // exclusive cycle gate: no cycle is running, the supervisor is not between
    // its check and registering a collector, and the background collector is
    // stopped before the store is cleared, so nothing collected before the
    // clear is written after it.
    let (gate_state, store) = (state.clone(), Arc::clone(store));
    tokio::task::spawn_blocking(move || {
        let _gate = gate_state
            .history
            .cycle_gate
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(id) = gate_state.saved.background_id() {
            stop_collector(&gate_state, &id);
        }
        store.clear()
    })
    .await
    .map_err(|_| internal_failure(&state))?
    .map_err(|_| internal_failure(&state))?;
    state.saved.changed();
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_connector::{
        CreateConnectionRequest, Remote, RemoteConnectOptions, RemoteError, RemoteFactory,
        RemoteOperation, RemoteResponse, TlsMode,
    };
    use std::collections::VecDeque;

    #[derive(Clone)]
    struct CollectionFixture(Arc<Mutex<VecDeque<Result<RemoteResponse, RemoteError>>>>);

    impl RemoteFactory for CollectionFixture {
        fn open(&self, _: RemoteConnectOptions) -> Result<Box<dyn Remote>, RemoteError> {
            Ok(Box::new(self.clone()))
        }
    }

    impl Remote for CollectionFixture {
        fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError> {
            match operation {
                RemoteOperation::CurrentVersion => Ok(RemoteResponse::Version("3.5.14".into())),
                RemoteOperation::AuditSchemaProbe { .. } => Ok(RemoteResponse::NullableRows(
                    [
                        ("queryId", "VARCHAR"),
                        ("timestamp", "DATETIME"),
                        ("state", "VARCHAR"),
                        ("queryTime", "BIGINT"),
                        ("scanBytes", "BIGINT"),
                        ("scanRows", "BIGINT"),
                        ("cpuCostNs", "BIGINT"),
                        ("memCostBytes", "BIGINT"),
                        ("returnRows", "BIGINT"),
                        ("catalog", "VARCHAR"),
                        ("db", "VARCHAR"),
                        ("resourceGroup", "VARCHAR"),
                        ("digest", "VARCHAR"),
                        ("stmt", "VARCHAR"),
                    ]
                    .into_iter()
                    .map(|(name, kind)| vec![Some(name.into()), Some(kind.into())])
                    .collect(),
                )),
                RemoteOperation::AuditCollectionSlice {
                    table,
                    scope,
                    schema,
                    window,
                } => {
                    let sql = doctor_connector::audit_collection_slice_statement(
                        &table, &scope, &schema, &window,
                    )
                    .unwrap();
                    assert!(
                        sql.contains(
                            "COUNT(CASE WHEN row_kind = 2 AND in_previous = 0 THEN 1 END)"
                        )
                    );
                    assert!(sql.contains("TIMESTAMPDIFF(SECOND"));
                    assert!(sql.contains("MAX(LENGTH(`stmt`)) BETWEEN 4093 AND 4096"));
                    self.0
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected Audit read")
                }
                RemoteOperation::LoadHistorySlice { start, end } => {
                    let sql = doctor_connector::load_history_slice_statement(start, end);
                    assert!(sql.contains(&format!("LOAD_FINISH_TIME >= FROM_UNIXTIME({start}) AND LOAD_FINISH_TIME < FROM_UNIXTIME({end})")));
                    self.0
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected Load read")
                }
                RemoteOperation::LoadRetention => {
                    assert!(
                        doctor_connector::load_retention_statement()
                            .contains("TIMESTAMPDIFF(SECOND")
                    );
                    self.0
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected retention read")
                }
                RemoteOperation::RoutineLoadJobs { .. } => Ok(RemoteResponse::NullableRows(vec![])),
                _ => Err(RemoteError::Unsupported),
            }
        }
    }

    fn collection_fixture(
        responses: Vec<Result<RemoteResponse, RemoteError>>,
    ) -> (SessionManager, String, CollectionFixture) {
        let fixture = CollectionFixture(Arc::new(Mutex::new(responses.into())));
        let sessions = SessionManager::new(Arc::new(fixture.clone()));
        let connection = sessions
            .connect(CreateConnectionRequest {
                host: "127.0.0.1".into(),
                port: 9030,
                username: "reader".into(),
                password: String::new(),
                default_catalog: None,
                default_database: None,
                audit_table: None,
                tls_mode: TlsMode::DisabledLoopbackDocker,
            })
            .unwrap();
        (sessions, connection.connection_id, fixture)
    }

    fn audit_summary(values: [&str; 8]) -> Vec<Option<String>> {
        let mut summary = vec![None; 21];
        for (cell, value) in summary.iter_mut().zip(values) {
            *cell = Some(value.into());
        }
        summary[20] = Some("0".into());
        summary
    }

    fn audit_detail(
        epoch: i64,
        conflict: bool,
        statement: Option<&str>,
        proof: &str,
        digest: Option<&str>,
    ) -> Vec<Option<String>> {
        let mut row = vec![None; 21];
        row[0] = Some("00000000-0000-0000-0000-000000000001".into());
        row[1] = Some("2026-11-01 01:10:00".into());
        row[2] = Some(epoch.to_string());
        row[3] = Some("EOF".into());
        row[4] = Some("17".into());
        row[10] = Some("default_catalog".into());
        row[11] = Some("demo".into());
        row[13] = digest.map(str::to_owned);
        row[18] = statement.map(str::to_owned);
        row[19] = Some(proof.into());
        row[20] = Some(if conflict { "2" } else { "1" }.into());
        row
    }

    #[tokio::test]
    async fn audit_replay_adds_late_row_and_is_idempotent() {
        let empty = audit_summary(["0", "0", "0", "0", "0", "0", "1", "0"]);
        let late = vec![
            audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
            audit_detail(
                3_601,
                false,
                Some("SELECT id FROM t WHERE id = 7"),
                "0",
                None,
            ),
        ];
        let (sessions, id, fixture) = collection_fixture(vec![
            Ok(RemoteResponse::NullableRows(vec![empty])),
            Ok(RemoteResponse::NullableRows(late.clone())),
            Ok(RemoteResponse::NullableRows(late)),
        ]);
        let store = HistoryStore::open_in_memory().unwrap();
        let mut config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
        assert_eq!(
            store.runs("t", 3_600, 3_900, 10).unwrap()[0]
                .reason
                .as_deref(),
            Some("first_look_unproven")
        );
        assert!(store.buckets("t", 3_600, 7_200, None).unwrap().is_empty());
        config.lag_seconds = 1_800;
        run_audit_slices(&sessions, &store, &config, &id, "t", Some(3_900), 5_699).unwrap();
        assert_eq!(fixture.0.lock().unwrap().len(), 2);
        run_audit_slices(&sessions, &store, &config, &id, "t", Some(3_900), 5_700).unwrap();
        assert_eq!(store.target_state("t").unwrap().watermark, Some(3_900));
        let before = store.buckets("t", 3_600, 7_200, None).unwrap();
        let runs = store.runs("t", 3_600, 3_900, 10).unwrap();
        assert_eq!(runs[0].state, RunState::Complete);

        // A completed slice leaves the replay queue. Apply the same clean
        // snapshot again at its current revision to exercise replacement.
        let repeat = sessions
            .audit_collection_slice_controlled(&id, 3_600, 3_900, None)
            .unwrap();
        let counts = repeat.counts.unwrap();
        assert!(!counts.limited);
        assert!(
            store
                .record_collected_audit_slice(
                    "t",
                    AuditSliceWrite::Replay {
                        expected_revision: 2
                    },
                    AuditSliceCommit {
                        run: &RunRecord {
                            started_at: 6_000,
                            ..runs.into_iter().next().unwrap()
                        },
                        evidence: &SliceEvidence {
                            read_range: Some((3_600, 3_900)),
                            excluded_count: Some(counts.excluded),
                            reasons: vec![],
                        },
                        engine_version: &repeat.session_identity.detected_version,
                        rule_version: &history_rule_version(),
                        source_identity: &sessions
                            .audit_collection_source_identity(&id, "t")
                            .unwrap(),
                        rows: collected(repeat.rows),
                        excluded_ids: &[],
                        affected_neighbours: &[],
                    },
                )
                .unwrap()
        );
        assert!(fixture.0.lock().unwrap().is_empty());
        let buckets = store.buckets("t", 3_600, 7_200, None).unwrap();
        assert_eq!(buckets, before);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].delta.executions, 1);
        assert_eq!(buckets[0].delta.query_time.sum, 17);
        assert_eq!(buckets[0].delta.query_time.histogram.count(), 1);
        assert_eq!(
            store
                .exemplars(&buckets[0].template_key, 3_600, 7_200, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.target_state("t").unwrap().watermark, Some(3_900));
        let runs = store.runs("t", 3_600, 3_900, 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Complete);
        assert_eq!(runs[0].reason, None);
    }

    #[tokio::test]
    async fn audit_reconnect_keeps_source_for_replay_and_limited_neighbour_summary() {
        let target = "127.0.0.1:9030";
        let mut limited = vec![audit_summary(["1000", "0", "0", "1", "1", "0", "1", "1"])];
        limited.extend((2..=1_001).map(|index| {
            let mut row = audit_detail(3_901, false, Some("SELECT 7"), "0", Some("ab12"));
            row[0] = Some(format!("00000000-0000-0000-0000-{index:012x}"));
            row
        }));
        for switch_table in [false, true] {
            let (sessions, first, fixture) = collection_fixture(vec![
                Ok(RemoteResponse::NullableRows(vec![audit_summary([
                    "0", "0", "0", "0", "0", "0", "1", "0",
                ])])),
                Ok(RemoteResponse::NullableRows(vec![
                    audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
                    audit_detail(3_601, false, Some("SELECT 7"), "0", Some("ab12")),
                ])),
                Ok(RemoteResponse::NullableRows(limited.clone())),
            ]);
            let connect = |port, table: &str| {
                sessions
                    .connect(CreateConnectionRequest {
                        host: "127.0.0.1".into(),
                        port,
                        username: "reader".into(),
                        password: String::new(),
                        default_catalog: None,
                        default_database: Some("starrocks_audit_db__".into()),
                        audit_table: Some(table.into()),
                        tls_mode: TlsMode::DisabledLoopbackDocker,
                    })
                    .unwrap()
                    .connection_id
            };
            let store = HistoryStore::open_in_memory().unwrap();
            let mut config = HistoryConfig {
                initial_backfill_seconds: 300,
                lag_seconds: 0,
                max_slices_per_cycle: 3, // Budget for one read with both neighbours.
                ..HistoryConfig::default()
            };
            run_audit_slices(&sessions, &store, &config, &first, target, None, 3_900).unwrap();
            sessions.close(&first);

            let reconnected = connect(9030, "starrocks_audit_tbl__");
            assert_ne!(first, reconnected);
            config.lag_seconds = 1_800;
            let watermark = Some(3_900);
            run_audit_slices(
                &sessions,
                &store,
                &config,
                &reconnected,
                target,
                watermark,
                5_700,
            )
            .unwrap();
            let neighbour = || store.runs(target, 3_600, 3_900, 10).unwrap().remove(0);
            assert_eq!(neighbour().state, RunState::Complete);
            assert_eq!(
                store.buckets(target, 3_600, 3_900, None).unwrap()[0]
                    .delta
                    .executions,
                1
            );
            let other_cluster = connect(9031, "starrocks_audit_tbl__");
            assert_ne!(
                sessions
                    .audit_collection_source_identity(&reconnected, target)
                    .unwrap(),
                sessions
                    .audit_collection_source_identity(&other_cluster, "127.0.0.1:9031")
                    .unwrap()
            );

            let next = if switch_table {
                connect(9030, "other_audit_tbl")
            } else {
                reconnected
            };
            run_audit_slices(&sessions, &store, &config, &next, target, watermark, 6_000).unwrap();
            if switch_table {
                assert_eq!(neighbour().state, RunState::Complete);
            } else {
                let run = neighbour();
                assert_eq!(run.state, RunState::Partial);
                assert_eq!(run.reason.as_deref(), Some("conflicting_query_id"));
            }
            assert!(fixture.0.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn audit_clean_replay_keeps_recorded_neighbour_conflict() {
        let read = |epoch| {
            Ok(RemoteResponse::NullableRows(vec![
                audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
                audit_detail(epoch, false, Some("SELECT 7"), "0", Some("ab12")),
            ]))
        };
        let (sessions, id, fixture) = collection_fixture(vec![
            read(3_601),
            read(3_901),
            Ok(RemoteResponse::NullableRows(vec![audit_summary([
                "0", "0", "0", "0", "0", "0", "1", "0",
            ])])),
        ]);
        let store = HistoryStore::open_in_memory().unwrap();
        let mut config = HistoryConfig {
            initial_backfill_seconds: 600,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        run_audit_slices(&sessions, &store, &config, &id, "t", None, 4_200).unwrap();
        assert_eq!(
            store.runs("t", 3_900, 4_200, 10).unwrap()[0]
                .reason
                .as_deref(),
            Some("conflicting_query_id")
        );
        let before = store.buckets("t", 3_600, 7_200, None).unwrap();
        assert_eq!(before[0].delta.executions, 1);

        // A is past the replay horizon. B can accept an empty replay because
        // the first write suppressed its copy of the execution.
        config.lag_seconds = 3_600;
        run_audit_slices(&sessions, &store, &config, &id, "t", Some(4_200), 7_501).unwrap();
        let runs = store.runs("t", 3_900, 4_200, 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].started_at, 7_501);
        assert_eq!(runs[0].state, RunState::Partial);
        assert_eq!(runs[0].reason.as_deref(), Some("conflicting_query_id"));
        assert_eq!(store.buckets("t", 3_600, 7_200, None).unwrap(), before);
        assert_eq!(store.target_state("t").unwrap().watermark, Some(4_200));
        assert!(fixture.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn audit_replay_preserves_snapshot_on_budget_or_source_loss() {
        for second in [
            Err(RemoteError::Timeout),
            Err(RemoteError::OutputBudgetExceeded),
            Ok(RemoteResponse::NullableRows(vec![audit_summary([
                "0", "0", "0", "0", "0", "0", "1", "0",
            ])])),
        ] {
            let failed = second.is_err();
            let first = RemoteResponse::NullableRows(vec![
                audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
                audit_detail(3_601, false, Some("SELECT 7"), "0", Some("ab12")),
            ]);
            let (sessions, id, fixture) = collection_fixture(vec![Ok(first), second]);
            let store = HistoryStore::open_in_memory().unwrap();
            let mut config = HistoryConfig {
                initial_backfill_seconds: 300,
                lag_seconds: 0,
                ..HistoryConfig::default()
            };
            run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
            let before = store.buckets("t", 3_600, 7_200, None).unwrap();
            config.lag_seconds = 1_800;
            let mut clock = SourceClock::default();
            let CycleOutcome::Read {
                audit: Some(ok), ..
            } = run_cycle(
                &sessions,
                &store,
                &config,
                &id,
                "t",
                5_700,
                clock.due(5_700),
                false,
            )
            .unwrap()
            else {
                panic!("expected a due Audit cycle")
            };
            assert_eq!(ok, !failed);
            clock.record(ok, 5_700, 300);
            assert_eq!(store.buckets("t", 3_600, 7_200, None).unwrap(), before);
            assert_eq!(
                store.runs("t", 3_600, 3_900, 10).unwrap()[0].state,
                RunState::Partial
            );
            assert_eq!(store.target_state("t").unwrap().watermark, Some(3_900));
            if failed {
                assert!(!clock.due(6_000));
                assert!(matches!(
                    run_cycle(
                        &sessions,
                        &store,
                        &config,
                        &id,
                        "t",
                        6_000,
                        clock.due(6_000),
                        false
                    )
                    .unwrap(),
                    CycleOutcome::Read { audit: None, .. }
                ));
            }
            assert!(fixture.0.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn audit_collection_excludes_execution_on_optional_field_conflict() {
        use doctor_connector::{AuditCollectionWindow, AuditSchema, AuditTableName};
        use doctor_contracts::{AuditScope, AuditStatementFilter, AuditWindow};

        let statement = doctor_connector::audit_collection_slice_statement(
            &AuditTableName::parse("starrocks_audit_db__.starrocks_audit_tbl__").unwrap(),
            &AuditScope {
                window: AuditWindow {
                    start_seconds: 3_600,
                    end_seconds: 3_900,
                },
                state: None,
                user: None,
                native_digest: None,
                catalog: None,
                database: None,
                resource_group: None,
                statement_filter: AuditStatementFilter::All,
            },
            &AuditSchema::official_3_5_14(),
            &AuditCollectionWindow {
                start: 3_600,
                end: 3_900,
                context_start: 3_300,
                context_end: 4_200,
            },
        )
        .unwrap();
        // The generated row-kind CASE uses SQL aggregates shared by SQLite
        // and StarRocks. Feed it two observations of the same execution.
        let (before_kind, _) = statement.split_once(" END AS row_kind").unwrap();
        let (_, classify) = before_kind.rsplit_once("CASE WHEN ").unwrap();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let mut query = db
            .prepare(&format!(
                "WITH optional_fields(error_code, pending_time_ms) AS (VALUES ('0', 1), (?1, ?2)),
                 candidates AS (
                   SELECT '00000000-0000-0000-0000-000000000001' AS query_id,
                          3601 AS event_time, 'EOF' AS state, 17 AS query_time, 0 AS scan_bytes,
                          0 AS scan_rows, 0 AS cpu_cost_ns, 0 AS mem_cost_bytes, 1 AS return_rows,
                          'default_catalog' AS catalog_name, 'demo' AS database_name,
                          NULL AS resource_group, 'ab12' AS digest, 'SELECT 7' AS stmt,
                          error_code, pending_time_ms FROM optional_fields
                 )
                 SELECT CASE WHEN {classify} END FROM candidates GROUP BY query_id"
            ))
            .unwrap();
        for (error_code, pending, expected) in [
            (Some("0"), Some(1), 1),
            (Some("1"), Some(1), 2),
            (Some("0"), Some(2), 2),
            (None, Some(1), 2),
            (Some("0"), None, 2),
        ] {
            let row_kind = query
                .query_row(rusqlite::params![error_code, pending], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            assert_eq!(row_kind, expected, "{error_code:?}, {pending:?}");
        }
    }

    #[tokio::test]
    async fn audit_conflict_only_slice_is_partial_with_zero_usable_rows() {
        let conflict = vec![
            audit_summary(["0", "1", "0", "1", "0", "0", "1", "0"]),
            audit_detail(3_601, true, None, "0", None),
        ];
        let (sessions, id, _) = collection_fixture(vec![
            Ok(RemoteResponse::NullableRows(conflict.clone())),
            Ok(RemoteResponse::NullableRows(conflict)),
        ]);
        let counts = sessions
            .audit_collection_slice_controlled(&id, 3_600, 3_900, None)
            .unwrap()
            .counts
            .unwrap();
        assert_eq!((counts.eligible, counts.excluded), (0, 1));
        let store = HistoryStore::open_in_memory().unwrap();
        let config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
        let runs = store.runs("t", 3_600, 3_900, 10).unwrap();
        assert_eq!(runs[0].rows_read, 0);
        assert_eq!(runs[0].attributed, 0);
        assert_eq!(runs[0].reason.as_deref(), Some("conflicting_query_id"));
        assert_eq!(
            coverage_interval(&runs, 3_600, 3_900),
            CoverageHourState::Partial
        );
        assert!(store.buckets("t", 3_600, 7_200, None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn audit_adjacent_slice_conflict_has_one_exclusion_owner() {
        let first = RemoteResponse::NullableRows(vec![
            audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
            audit_detail(3_599, false, Some("SELECT 7"), "0", Some("ab12")),
        ]);
        let later = vec![
            audit_summary(["0", "0", "0", "1", "1", "0", "1", "0"]),
            audit_detail(3_599, true, None, "1", None),
        ];
        let earlier = vec![
            audit_summary(["0", "1", "0", "1", "0", "1", "1", "0"]),
            audit_detail(3_599, true, None, "2", None),
        ];
        let (sessions, id, fixture) = collection_fixture(vec![
            Ok(first),
            Ok(RemoteResponse::NullableRows(later.clone())),
            Ok(RemoteResponse::NullableRows(earlier)),
            Ok(RemoteResponse::NullableRows(later)),
        ]);
        let path = std::env::temp_dir().join(format!(
            "doctor-audit-adjacent-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let store = HistoryStore::open(&path).unwrap();
        let mut config = HistoryConfig {
            initial_backfill_seconds: 600,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
        assert_eq!(
            store.buckets("t", 0, 7_200, None).unwrap()[0]
                .delta
                .executions,
            1
        );
        config.lag_seconds = 1_800;
        for _ in 0..3 {
            run_audit_slices(&sessions, &store, &config, &id, "t", Some(3_900), 5_700).unwrap();
        }
        let runs = store.runs("t", 3_300, 3_900, 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|run| run.state == RunState::Partial
            && run.reason.as_deref() == Some("conflicting_query_id")));
        assert!(store.buckets("t", 0, 7_200, None).unwrap().is_empty());
        assert_eq!(store.target_state("t").unwrap().watermark, Some(3_900));
        let db = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let stored: Vec<_> = db.prepare(
            "SELECT excluded_count FROM collection_slice_evidence WHERE source = 'audit' ORDER BY slice_start"
        ).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(stored, ["1", "0"]);
        drop(db);
        drop(store);
        std::fs::remove_file(path).unwrap();
        assert!(fixture.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn collection_dst_fold_never_fabricates_an_empty_or_duplicate_slice() {
        // New York's 01:55 → 01:00 fold, and the two UTC slices that
        // individually map to 01:10 → 01:15.
        let starts = [1_793_512_500, 1_793_509_800, 1_793_513_400];
        let config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        for start in starts {
            let end = start + 300;
            let mut load_summary = vec![None; 23];
            load_summary[0] = Some("0".into());
            let (sessions, id, fixture) = collection_fixture(vec![
                Ok(RemoteResponse::NullableRows(vec![audit_summary([
                    "0", "0", "0", "0", "0", "0", "0", "0",
                ])])),
                Ok(RemoteResponse::NullableRows(vec![load_summary])),
            ]);
            let store = HistoryStore::open_in_memory().unwrap();
            run_audit_slices(&sessions, &store, &config, &id, "t", None, end).unwrap();
            crate::history_loads::run_load_cycle(&sessions, &store, &config, &id, "t", end)
                .unwrap();
            for runs in [
                store.runs("t", start, end, 10).unwrap(),
                store.load_runs("t", start, end).unwrap(),
            ] {
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].reason.as_deref(), Some("ambiguous_local_time"));
                assert_eq!(
                    coverage_interval(&runs, start, end),
                    CoverageHourState::Partial
                );
            }
            assert!(store.buckets("t", 0, end, None).unwrap().is_empty());
            assert!(store.load_buckets("t", 0, end).unwrap().is_empty());
            assert!(!store.load_target_state("t").unwrap().observed);
            assert!(fixture.0.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn missing_load_summary_records_failure_without_advancing_watermark() {
        let mut verified_empty = vec![None; 23];
        verified_empty[0] = Some("1".into());
        let (sessions, id, fixture) = collection_fixture(vec![
            Ok(RemoteResponse::NullableRows(vec![verified_empty])),
            Ok(RemoteResponse::NullableRows(vec![])), // Empty retention probe.
            Ok(RemoteResponse::NullableRows(vec![])), // Missing slice summary.
        ]);
        let store = HistoryStore::open_in_memory().unwrap();
        let config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        crate::history_loads::run_load_cycle(&sessions, &store, &config, &id, "t", 3_900).unwrap();
        let before = store.load_target_state("t").unwrap();
        assert_eq!(before.watermark, Some(3_900));

        let error =
            crate::history_loads::run_load_cycle(&sessions, &store, &config, &id, "t", 4_200)
                .unwrap_err();
        assert_eq!(error.code, ConnectorErrorCode::ConnectionFailed);
        assert_eq!(store.load_target_state("t").unwrap(), before);
        let runs = store.load_runs("t", 3_900, 4_200).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Failed);
        assert_eq!(
            runs[0].reason.as_deref(),
            Some("read_failed:audit_response")
        );
        assert!(store.load_buckets("t", 3_900, 4_200).unwrap().is_empty());
        assert!(fixture.0.lock().unwrap().is_empty());
    }

    #[test]
    fn load_retention_ignores_unproven_wall_clock() {
        let mut summary = vec![None; 23];
        summary[0] = Some("1".into());
        let load = |id: &str, finish: &str| {
            let mut row = vec![None; 23];
            for (index, value) in [
                (0, "2"),
                (1, id),
                (4, "ods"),
                (5, "t"),
                (7, "FINISHED"),
                (8, "STREAM_LOAD"),
                (22, finish),
            ] {
                row[index] = Some(value.into());
            }
            row
        };
        // Simulate a fixed-offset slice followed by an ambiguous
        // retention minimum from the repeated wall-clock hour.
        let (sessions, id, fixture) = collection_fixture(vec![
            Ok(RemoteResponse::NullableRows(vec![
                summary,
                load("1", "3600"),
                load("2", "3900"),
            ])),
            Ok(RemoteResponse::NullableRows(vec![
                ["STREAM_LOAD", "5", "1793513400", "0"]
                    .map(|cell| Some(cell.into()))
                    .to_vec(),
            ])),
        ]);
        let store = HistoryStore::open_in_memory().unwrap();
        let before = doctor_history::LoadTargetState {
            earliest_stream: Some(3_200),
            cleared_stream: Some(3_000),
            unproven_until: Some(3_400),
            observed: true,
            ..doctor_history::LoadTargetState::default()
        };
        store.set_load_retention("t", &before, 3_600).unwrap();
        let config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        crate::history_loads::run_load_cycle(&sessions, &store, &config, &id, "t", 3_900).unwrap();
        assert_eq!(
            store.load_target_state("t").unwrap(),
            doctor_history::LoadTargetState {
                watermark: Some(3_900),
                ..before
            }
        );
        let buckets = store.load_buckets("t", 3_600, 7_200).unwrap();
        assert_eq!(buckets[0].delta.loads, 1);
        let runs = store.load_runs("t", 3_600, 3_900).unwrap();
        assert_eq!(runs[0].unavailable_statements, 1);
        assert_eq!(runs[0].state, RunState::Partial);
        assert!(fixture.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn audit_collection_needs_proof_before_fingerprinting_text() {
        let exact_limit = format!("SELECT 1{}", " ".repeat(4096 - "SELECT 1".len()));
        let multibyte = "SELECT 'é'";
        let short_limit = format!("{multibyte}{}", " ".repeat(4093 - multibyte.len()));
        let mut reply = vec![audit_summary(["3", "0", "0", "0", "0", "0", "1", "0"])];
        for (index, (text, proof)) in [
            (exact_limit.as_str(), "2"),
            (short_limit.as_str(), "2"),
            ("SELECT id FROM t WHERE id = 7", "0"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut row = audit_detail(3_600 + index as i64, false, Some(text), proof, None);
            row[0] = Some(format!("00000000-0000-0000-0000-{index:012x}"));
            reply.push(row);
        }
        let (sessions, id, _) = collection_fixture(vec![Ok(RemoteResponse::NullableRows(reply))]);
        let store = HistoryStore::open_in_memory().unwrap();
        let config = HistoryConfig {
            initial_backfill_seconds: 300,
            lag_seconds: 0,
            ..HistoryConfig::default()
        };
        run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
        let run = &store.runs("t", 3_600, 3_900, 10).unwrap()[0];
        assert_eq!(
            (
                run.partial_statements,
                run.attributed,
                run.unavailable_statements
            ),
            (2, 1, 0)
        );
        assert_eq!(store.templates("t").unwrap().len(), 1);
        assert_eq!(
            store.buckets("t", 3_600, 7_200, None).unwrap()[0]
                .delta
                .executions,
            1
        );
    }

    #[tokio::test]
    async fn audit_collection_known_source_cut_uses_task77_partial_path() {
        let text = format!("SELECT 1{}", " ".repeat(4096 - "SELECT 1".len()));
        for native_digest in [Some("ab12"), None] {
            let reply = vec![
                audit_summary(["1", "0", "0", "0", "0", "0", "1", "0"]),
                audit_detail(3_600, false, Some(&text), "2", native_digest),
            ];
            let (sessions, id, _) =
                collection_fixture(vec![Ok(RemoteResponse::NullableRows(reply))]);
            let store = HistoryStore::open_in_memory().unwrap();
            let config = HistoryConfig {
                initial_backfill_seconds: 300,
                lag_seconds: 0,
                ..HistoryConfig::default()
            };
            run_audit_slices(&sessions, &store, &config, &id, "t", None, 3_900).unwrap();
            let run = &store.runs("t", 3_600, 3_900, 10).unwrap()[0];
            assert_eq!(
                (
                    run.partial_statements,
                    run.attributed,
                    run.unavailable_statements
                ),
                (1, 0, 0)
            );
            assert_eq!(run.reason.as_deref(), Some("statement_truncated"));
            assert!(store.buckets("t", 3_600, 7_200, None).unwrap().is_empty());
        }
    }

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

    #[test]
    fn audit_lh_08_each_source_backs_off_on_its_own() {
        let (mut audit, mut loads) = (SourceClock::default(), SourceClock::default());
        assert!(audit.due(0) && loads.due(0));
        audit.record(false, 0, 5);
        loads.record(true, 0, 5);
        audit.record(false, 10, 5);
        // The failing source waits longer each time; the healthy one keeps its period.
        assert_eq!((audit.failures, audit.next_run), (2, 30));
        assert_eq!((loads.failures, loads.next_run), (0, 5));
        assert!(!audit.due(20) && loads.due(20));
        for _ in 0..20 {
            audit.record(false, 100, 300);
        }
        assert_eq!(audit.next_run, 100 + 1800);
        audit.record(true, 2000, 300);
        assert_eq!((audit.failures, audit.next_run), (0, 2300));
    }

    #[test]
    fn c1_a_page_waiting_behind_the_background_collector_sees_it() {
        let runtime = HistoryRuntime::open(HistoryConfig::default());
        let entry = |target: &str, status| {
            Arc::new(CollectorEntry {
                target: target.into(),
                status: Mutex::new(status),
                stop: AtomicBool::new(false),
                wake: tokio::sync::Notify::new(),
            })
        };
        let waiting = CollectorStatus::Stopped(CollectionStoppedReason::AnotherSessionCollecting);
        runtime.collectors().insert(
            "background".into(),
            entry("127.0.0.1:9030", CollectorStatus::Running { next_run: 5 }),
        );
        runtime
            .collectors()
            .insert("page".into(), entry("127.0.0.1:9030", waiting.clone()));
        runtime
            .collectors()
            .insert("other".into(), entry("10.0.0.5:9030", waiting.clone()));
        let status = |id: Option<&str>| {
            runtime
                .resolve(id)
                .map(|entry| entry.status.lock().unwrap().clone())
        };
        let running = Some(CollectorStatus::Running { next_run: 5 });
        assert_eq!(status(Some("page")), running);
        // Another cluster's page keeps its own state; no page sees whichever runs.
        assert_eq!(status(Some("other")), Some(waiting));
        assert_eq!(status(None), running);
        assert_eq!(status(Some("unknown")), running);
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
