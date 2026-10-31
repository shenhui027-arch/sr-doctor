//! Load history (task 58, schema 6): hourly load buckets, load collection
//! runs and watermarks, hourly Routine Load snapshots and the saved error
//! logs. Error log reasons and rows are business data that the user allowed
//! to be stored in plain text (AGENTS §4, 2026-10-30): at most 50 entries and
//! 64 KB per load, 4 KB per row, 50 MB in all (oldest dropped first), 30 days.
//! They are returned only by `error_log_entries`, one load at a time.
use doctor_contracts::{LoadErrorCategory, LoadErrorCategoryCount, LoadErrorEntry};
use doctor_core::{LoadBucketDelta, LoadCount, LoadHistoryKind};
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::{
    HistoryError, HistoryStore, Result, RunRecord, RunState, decode_metric, encode_metric,
};

pub(crate) const MIGRATION_6: &str = "
CREATE TABLE load_targets (
  target TEXT PRIMARY KEY,
  watermark INTEGER,
  earliest_stream INTEGER,
  earliest_other INTEGER,
  observed INTEGER NOT NULL DEFAULT 0,
  cleared_stream INTEGER,
  cleared_other INTEGER,
  unproven_until INTEGER,
  updated_at INTEGER NOT NULL
);
CREATE TABLE load_buckets (
  target TEXT NOT NULL,
  bucket_start INTEGER NOT NULL,
  engine_version TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  db TEXT NOT NULL,
  tbl TEXT NOT NULL,
  kind TEXT NOT NULL,
  system INTEGER NOT NULL,
  loads INTEGER NOT NULL,
  failed INTEGER NOT NULL,
  loaded_sum TEXT NOT NULL, loaded_unknown INTEGER NOT NULL,
  filtered_sum TEXT NOT NULL, filtered_unknown INTEGER NOT NULL,
  unselected_sum TEXT NOT NULL, unselected_unknown INTEGER NOT NULL,
  bytes_sum TEXT NOT NULL, bytes_unknown INTEGER NOT NULL,
  total_valid INTEGER NOT NULL, total_sum TEXT NOT NULL, total_max TEXT, total_hist BLOB NOT NULL,
  txn_valid INTEGER NOT NULL, txn_sum TEXT NOT NULL, txn_max TEXT, txn_hist BLOB NOT NULL,
  plan_valid INTEGER NOT NULL, plan_sum TEXT NOT NULL, plan_max TEXT, plan_hist BLOB NOT NULL,
  recv_valid INTEGER NOT NULL, recv_sum TEXT NOT NULL, recv_max TEXT, recv_hist BLOB NOT NULL,
  PRIMARY KEY (target, bucket_start, engine_version, rule_version, db, tbl, kind)
);
CREATE INDEX load_buckets_start ON load_buckets (bucket_start);
CREATE TABLE load_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  slice_start INTEGER NOT NULL,
  slice_end INTEGER NOT NULL,
  state TEXT NOT NULL,
  reason TEXT,
  rows_read INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL
);
CREATE INDEX load_runs_target_slice ON load_runs (target, slice_start);
CREATE TABLE routine_hours (
  target TEXT NOT NULL,
  job_id TEXT NOT NULL,
  bucket_start INTEGER NOT NULL,
  name TEXT NOT NULL,
  db TEXT NOT NULL,
  tbl TEXT NOT NULL,
  last_state TEXT NOT NULL,
  worst_state TEXT NOT NULL,
  snapshots INTEGER NOT NULL,
  max_lag TEXT,
  lag_snapshots INTEGER NOT NULL,
  loaded_delta TEXT NOT NULL,
  error_delta TEXT NOT NULL,
  committed_delta TEXT NOT NULL,
  aborted_delta TEXT NOT NULL,
  known_intervals INTEGER NOT NULL,
  unknown_intervals INTEGER NOT NULL,
  PRIMARY KEY (target, job_id, bucket_start)
);
CREATE INDEX routine_hours_start ON routine_hours (bucket_start);
CREATE TABLE routine_snapshot_hours (
  target TEXT NOT NULL,
  bucket_start INTEGER NOT NULL,
  snapshots INTEGER NOT NULL,
  truncated_snapshots INTEGER NOT NULL,
  PRIMARY KEY (target, bucket_start)
);
CREATE TABLE routine_last (
  target TEXT NOT NULL,
  job_id TEXT NOT NULL,
  observed_at INTEGER NOT NULL,
  state TEXT NOT NULL,
  loaded TEXT,
  error TEXT,
  committed TEXT,
  aborted TEXT,
  PRIMARY KEY (target, job_id)
);
CREATE TABLE routine_events (
  target TEXT NOT NULL,
  job_id TEXT NOT NULL,
  observed_at INTEGER NOT NULL,
  name TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL,
  reason TEXT,
  PRIMARY KEY (target, job_id, observed_at)
);
CREATE INDEX routine_events_at ON routine_events (observed_at);
CREATE TABLE load_error_logs (
  target TEXT NOT NULL,
  job_id TEXT NOT NULL,
  label TEXT NOT NULL,
  db TEXT NOT NULL,
  tbl TEXT NOT NULL,
  kind TEXT NOT NULL,
  finish_time INTEGER NOT NULL,
  filtered TEXT,
  state TEXT NOT NULL,
  attempts INTEGER NOT NULL,
  next_try INTEGER NOT NULL,
  saved_at INTEGER,
  sampled INTEGER NOT NULL DEFAULT 0,
  truncated INTEGER NOT NULL DEFAULT 0,
  raw_bytes INTEGER NOT NULL DEFAULT 0,
  categories_json TEXT,
  PRIMARY KEY (target, job_id)
);
CREATE INDEX load_error_logs_finish ON load_error_logs (finish_time);
CREATE TABLE load_error_entries (
  target TEXT NOT NULL,
  job_id TEXT NOT NULL,
  idx INTEGER NOT NULL,
  category TEXT NOT NULL,
  column_name TEXT,
  target_type TEXT,
  reason TEXT NOT NULL,
  row_text TEXT,
  row_converted INTEGER NOT NULL,
  truncated INTEGER NOT NULL,
  PRIMARY KEY (target, job_id, idx)
);
";

/// AGENTS §4: entries kept per load.
pub const ERROR_LOG_MAX_ENTRIES: usize = 50;
/// Reason and row bytes kept per load.
pub const ERROR_LOG_MAX_BYTES_PER_LOAD: usize = 64 * 1024;
/// AGENTS §4: all saved reason and row text together.
pub const ERROR_LOG_MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
/// Failed reads tried again before giving up.
pub const ERROR_LOG_MAX_ATTEMPTS: u32 = 5;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadTargetState {
    pub watermark: Option<i64>,
    /// The earliest finish time of a finished record each FE manager still
    /// held at the last look, and whether there was a last look at all.
    pub earliest_stream: Option<i64>,
    pub earliest_other: Option<i64>,
    pub observed: bool,
    /// Finished records before these times were seen cleared by a manager
    /// before they were read: unread slices starting earlier are partial,
    /// however many rounds the catch-up takes.
    pub cleared_stream: Option<i64>,
    pub cleared_other: Option<i64>,
    /// On the first look nothing older than this was held: slices before it
    /// cannot be shown complete, in this round or a later catch-up round.
    pub unproven_until: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredLoadBucket {
    pub engine_version: String,
    pub rule_version: String,
    pub delta: LoadBucketDelta,
}

/// A load whose error log the collector should read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorLogCandidate {
    pub job_id: u64,
    pub label: String,
    pub database: String,
    pub table: String,
    pub kind: LoadHistoryKind,
    pub finish_time: i64,
    pub filtered_rows: Option<u128>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorLogState {
    /// Not read yet, or a read failed and will be tried again.
    Pending,
    Saved,
    /// The BE had already deleted the file.
    Purged,
    /// StarRocks has no error log for this load.
    NoLog,
    /// Reads kept failing.
    Failed,
    /// Not read before StarRocks' 48-hour retention ran out.
    Expired,
    /// Saved, then its text was dropped under the 50 MB limit.
    Trimmed,
}

impl ErrorLogState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Saved => "saved",
            Self::Purged => "purged",
            Self::NoLog => "no_log",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Trimmed => "trimmed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "saved" => Self::Saved,
            "purged" => Self::Purged,
            "no_log" => Self::NoLog,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "trimmed" => Self::Trimmed,
            _ => return None,
        })
    }
}

/// What one error log read produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorLogOutcome {
    Saved {
        entries: Vec<LoadErrorEntry>,
        categories: Vec<LoadErrorCategoryCount>,
        truncated: bool,
    },
    Purged,
    NoLog,
    ReadFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredErrorLog {
    pub job_id: String,
    pub label: String,
    pub database: String,
    pub table: String,
    pub kind: LoadHistoryKind,
    pub finish_time: i64,
    pub filtered_rows: Option<String>,
    pub state: ErrorLogState,
    pub attempts: u32,
    pub saved_at: Option<i64>,
    pub sampled: u32,
    pub truncated: bool,
    pub raw_bytes: u64,
    pub categories: Vec<LoadErrorCategoryCount>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ErrorLogStorage {
    pub logs: u64,
    pub saved: u64,
    pub raw_bytes: u64,
    pub earliest_saved: Option<i64>,
}

/// One job at one snapshot. Counters are the lifetime values StarRocks
/// reports; `None` when it did not report them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutineSnapshotJob {
    pub job_id: String,
    pub name: String,
    pub database: String,
    pub table: String,
    pub state: String,
    pub reason: Option<String>,
    pub max_lag: Option<u128>,
    pub loaded_rows: Option<u128>,
    pub error_rows: Option<u128>,
    pub committed_tasks: Option<u128>,
    pub aborted_tasks: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRoutineHour {
    pub job_id: String,
    pub bucket_start: i64,
    pub name: String,
    pub database: String,
    pub table: String,
    pub last_state: String,
    pub worst_state: String,
    pub snapshots: u32,
    pub max_lag: Option<u128>,
    pub lag_snapshots: u32,
    pub loaded_delta: u128,
    pub error_delta: u128,
    pub committed_delta: u128,
    pub aborted_delta: u128,
    pub known_intervals: u32,
    pub unknown_intervals: u32,
}

/// The latest snapshot of one target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutineSnapshotLast {
    pub observed_at: i64,
    pub truncated: bool,
    pub engine_version: Option<String>,
    pub rule_version: Option<String>,
}

/// The snapshots of one target hour.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutineSnapshotHour {
    pub bucket_start: i64,
    pub snapshots: u32,
    /// Snapshots that hit the job limit: they cannot show a job absent.
    pub truncated: u32,
    /// `None` before schema 7; `*` when it changed within the hour.
    pub engine_version: Option<String>,
    pub rule_version: Option<String>,
}

impl RoutineSnapshotHour {
    pub fn complete(&self) -> bool {
        self.truncated < self.snapshots
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRoutineEvent {
    pub job_id: String,
    pub observed_at: i64,
    pub name: String,
    pub from_state: Option<String>,
    pub to_state: String,
    pub reason: Option<String>,
}

/// Worse states win when an hour saw several.
fn state_rank(state: &str) -> u8 {
    match state {
        "CANCELLED" => 5,
        "STOPPED" => 4,
        "PAUSED" => 3,
        "NEED_SCHEDULE" => 2,
        "RUNNING" => 1,
        _ => 0,
    }
}

fn category_text(category: LoadErrorCategory) -> &'static str {
    match category {
        LoadErrorCategory::OutOfRange => "out_of_range",
        LoadErrorCategory::TooLong => "too_long",
        LoadErrorCategory::ColumnCount => "column_count",
        LoadErrorCategory::NullInNonNullable => "null_in_non_nullable",
        LoadErrorCategory::OutOfPartition => "out_of_partition",
        LoadErrorCategory::JsonCast => "json_cast",
        LoadErrorCategory::JsonStructure => "json_structure",
        LoadErrorCategory::Other => "other",
    }
}

fn category_parse(value: &str) -> LoadErrorCategory {
    match value {
        "out_of_range" => LoadErrorCategory::OutOfRange,
        "too_long" => LoadErrorCategory::TooLong,
        "column_count" => LoadErrorCategory::ColumnCount,
        "null_in_non_nullable" => LoadErrorCategory::NullInNonNullable,
        "out_of_partition" => LoadErrorCategory::OutOfPartition,
        "json_cast" => LoadErrorCategory::JsonCast,
        "json_structure" => LoadErrorCategory::JsonStructure,
        _ => LoadErrorCategory::Other,
    }
}

fn parse_u128(text: &str) -> Result<u128> {
    text.parse()
        .map_err(|_| HistoryError::Internal("corrupt_load_count"))
}

fn count_from(sum: String, unknown: i64) -> Result<LoadCount> {
    Ok(LoadCount {
        sum: parse_u128(&sum)?,
        unknown: u64::try_from(unknown)
            .map_err(|_| HistoryError::Internal("corrupt_load_count"))?,
    })
}

const LOAD_BUCKET_COLUMNS: &str = "engine_version, rule_version, bucket_start, db, tbl, kind, system, loads, failed, loaded_sum, loaded_unknown, filtered_sum, filtered_unknown, unselected_sum, unselected_unknown, bytes_sum, bytes_unknown, total_valid, total_sum, total_max, total_hist, txn_valid, txn_sum, txn_max, txn_hist, plan_valid, plan_sum, plan_max, plan_hist, recv_valid, recv_sum, recv_max, recv_hist";

fn load_bucket_from_row(row: &Row<'_>) -> Result<StoredLoadBucket> {
    let metric = |base: usize| -> Result<doctor_core::HistoryMetric> {
        decode_metric(
            row.get(base)?,
            row.get(base + 1)?,
            row.get(base + 2)?,
            row.get(base + 3)?,
        )
    };
    let kind: String = row.get(5)?;
    Ok(StoredLoadBucket {
        engine_version: row.get(0)?,
        rule_version: row.get(1)?,
        delta: LoadBucketDelta {
            bucket_start: row.get(2)?,
            database: row.get(3)?,
            table: row.get(4)?,
            kind: Some(
                LoadHistoryKind::parse(&kind).ok_or(HistoryError::Internal("corrupt_load_kind"))?,
            ),
            system: row.get::<_, i64>(6)? != 0,
            loads: row.get::<_, i64>(7)? as u64,
            failed: row.get::<_, i64>(8)? as u64,
            loaded_rows: count_from(row.get(9)?, row.get(10)?)?,
            filtered_rows: count_from(row.get(11)?, row.get(12)?)?,
            unselected_rows: count_from(row.get(13)?, row.get(14)?)?,
            scan_bytes: count_from(row.get(15)?, row.get(16)?)?,
            total_seconds: metric(17)?,
            phases_ms: [metric(21)?, metric(25)?, metric(29)?],
        },
    })
}

fn error_log_from_row(row: &Row<'_>) -> Result<StoredErrorLog> {
    let kind: String = row.get(4)?;
    let state: String = row.get(7)?;
    let categories: Option<String> = row.get(13)?;
    Ok(StoredErrorLog {
        job_id: row.get(0)?,
        label: row.get(1)?,
        database: row.get(2)?,
        table: row.get(3)?,
        kind: LoadHistoryKind::parse(&kind).ok_or(HistoryError::Internal("corrupt_load_kind"))?,
        finish_time: row.get(5)?,
        filtered_rows: row.get(6)?,
        state: ErrorLogState::parse(&state).ok_or(HistoryError::Internal("corrupt_log_state"))?,
        attempts: row.get::<_, i64>(8)? as u32,
        saved_at: row.get(9)?,
        sampled: row.get::<_, i64>(10)? as u32,
        truncated: row.get::<_, i64>(11)? != 0,
        raw_bytes: row.get::<_, i64>(12)? as u64,
        categories: categories
            .map(|text| {
                serde_json::from_str(&text)
                    .map_err(|_| HistoryError::Internal("corrupt_categories"))
            })
            .transpose()?
            .unwrap_or_default(),
    })
}

const ERROR_LOG_COLUMNS: &str = "job_id, label, db, tbl, kind, finish_time, filtered, state, attempts, saved_at, sampled, truncated, raw_bytes, categories_json";

/// Category counts of the entries actually kept, largest first.
fn kept_categories(entries: &[LoadErrorEntry]) -> Vec<LoadErrorCategoryCount> {
    let mut counts: Vec<LoadErrorCategoryCount> = Vec::new();
    for entry in entries {
        match counts.iter_mut().find(|count| {
            count.category == entry.category
                && count.column == entry.column
                && count.target_type == entry.target_type
        }) {
            Some(count) => count.count = count.count.saturating_add(1),
            None => counts.push(LoadErrorCategoryCount {
                category: entry.category,
                column: entry.column.clone(),
                target_type: entry.target_type.clone(),
                count: 1,
            }),
        }
    }
    counts.sort_by_key(|count| std::cmp::Reverse(count.count));
    counts
}

/// Keeps entries within the per-load limits: 50 entries, rows already cut
/// at 4 KB by the parser, 64 KB of reason and row text in all.
fn bounded_entries(
    entries: Vec<LoadErrorEntry>,
    truncated: bool,
) -> (Vec<LoadErrorEntry>, bool, u64) {
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    let mut cut = truncated || entries.len() > ERROR_LOG_MAX_ENTRIES;
    for entry in entries.into_iter().take(ERROR_LOG_MAX_ENTRIES) {
        let size = entry.reason.len() + entry.row.as_deref().map_or(0, str::len);
        if bytes + size > ERROR_LOG_MAX_BYTES_PER_LOAD {
            cut = true;
            break;
        }
        bytes += size;
        kept.push(entry);
    }
    (kept, cut, bytes as u64)
}

impl HistoryStore {
    pub fn load_target_state(&self, target: &str) -> Result<LoadTargetState> {
        Ok(self
            .lock()
            .query_row(
                "SELECT watermark, earliest_stream, earliest_other, observed, cleared_stream, cleared_other, unproven_until FROM load_targets WHERE target = ?1",
                params![target],
                |row| {
                    Ok(LoadTargetState {
                        watermark: row.get(0)?,
                        earliest_stream: row.get(1)?,
                        earliest_other: row.get(2)?,
                        observed: row.get::<_, i64>(3)? != 0,
                        cleared_stream: row.get(4)?,
                        cleared_other: row.get(5)?,
                        unproven_until: row.get(6)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// Remembers what each FE manager held at this look and how far a seen
    /// clearing reaches (see `LoadTargetState`).
    pub fn set_load_retention(
        &self,
        target: &str,
        state: &LoadTargetState,
        now: i64,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO load_targets (target, watermark, earliest_stream, earliest_other, observed, cleared_stream, cleared_other, unproven_until, updated_at) VALUES (?1, NULL, ?2, ?3, 1, ?4, ?5, ?6, ?7)
             ON CONFLICT(target) DO UPDATE SET earliest_stream = excluded.earliest_stream, earliest_other = excluded.earliest_other, observed = 1,
               cleared_stream = excluded.cleared_stream, cleared_other = excluded.cleared_other, unproven_until = excluded.unproven_until, updated_at = excluded.updated_at",
            params![target, state.earliest_stream, state.earliest_other, state.cleared_stream, state.cleared_other, state.unproven_until, now],
        )?;
        Ok(())
    }

    fn insert_load_run(tx: &Transaction<'_>, target: &str, run: &RunRecord) -> Result<()> {
        tx.execute(
            "INSERT INTO load_runs (target, started_at, slice_start, slice_end, state, reason, rows_read, duration_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                target, run.started_at, run.slice_start, run.slice_end, run.state.as_str(), run.reason,
                run.rows_read as i64, run.duration_ms as i64
            ],
        )?;
        Ok(())
    }

    fn set_load_watermark(
        tx: &Transaction<'_>,
        target: &str,
        watermark: i64,
        now: i64,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO load_targets (target, watermark, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(target) DO UPDATE SET watermark = excluded.watermark, updated_at = excluded.updated_at",
            params![target, watermark, now],
        )?;
        Ok(())
    }

    /// One load slice, atomically: merged buckets, error log candidates, the
    /// run record and the new watermark. A slice behind the watermark was
    /// already recorded and is dropped instead of being counted twice.
    #[allow(clippy::too_many_arguments)]
    pub fn record_load_slice(
        &self,
        target: &str,
        engine_version: &str,
        rule_version: &str,
        run: &RunRecord,
        watermark: i64,
        buckets: &[LoadBucketDelta],
        candidates: &[ErrorLogCandidate],
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT watermark FROM load_targets WHERE target = ?1",
                params![target],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if current.is_some_and(|current| current > run.slice_start) {
            return Ok(false);
        }
        for delta in buckets {
            let kind = delta
                .kind
                .ok_or(HistoryError::Internal("load_bucket_without_kind"))?;
            let existing = tx
                .query_row(
                    &format!(
                        "SELECT {LOAD_BUCKET_COLUMNS} FROM load_buckets WHERE target = ?1 AND bucket_start = ?2 AND engine_version = ?3 AND rule_version = ?4 AND db = ?5 AND tbl = ?6 AND kind = ?7"
                    ),
                    params![target, delta.bucket_start, engine_version, rule_version, delta.database, delta.table, kind.as_str()],
                    |row| Ok(load_bucket_from_row(row)),
                )
                .optional()?
                .transpose()?;
            let mut merged =
                existing
                    .map(|bucket| bucket.delta)
                    .unwrap_or_else(|| LoadBucketDelta {
                        bucket_start: delta.bucket_start,
                        database: delta.database.clone(),
                        table: delta.table.clone(),
                        kind: Some(kind),
                        system: delta.system,
                        ..LoadBucketDelta::default()
                    });
            merged.merge(delta);
            let (tv, ts, tm, th) = encode_metric(&merged.total_seconds);
            let (av, as_, am, ah) = encode_metric(&merged.phases_ms[0]);
            let (pv, ps, pm, ph) = encode_metric(&merged.phases_ms[1]);
            let (rv, rs, rm, rh) = encode_metric(&merged.phases_ms[2]);
            tx.execute(
                "INSERT OR REPLACE INTO load_buckets VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34)",
                params![
                    target, merged.bucket_start, engine_version, rule_version, merged.database, merged.table,
                    kind.as_str(), i64::from(merged.system), merged.loads as i64, merged.failed as i64,
                    merged.loaded_rows.sum.to_string(), merged.loaded_rows.unknown as i64,
                    merged.filtered_rows.sum.to_string(), merged.filtered_rows.unknown as i64,
                    merged.unselected_rows.sum.to_string(), merged.unselected_rows.unknown as i64,
                    merged.scan_bytes.sum.to_string(), merged.scan_bytes.unknown as i64,
                    tv, ts, tm, th, av, as_, am, ah, pv, ps, pm, ph, rv, rs, rm, rh
                ],
            )?;
        }
        for candidate in candidates {
            tx.execute(
                "INSERT OR IGNORE INTO load_error_logs (target, job_id, label, db, tbl, kind, finish_time, filtered, state, attempts, next_try)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, 0)",
                params![
                    target, candidate.job_id.to_string(), candidate.label, candidate.database, candidate.table,
                    candidate.kind.as_str(), candidate.finish_time, candidate.filtered_rows.map(|rows| rows.to_string())
                ],
            )?;
        }
        Self::insert_load_run(&tx, target, run)?;
        Self::set_load_watermark(&tx, target, watermark, run.started_at)?;
        tx.commit()?;
        Ok(true)
    }

    /// A load run that moved no data: a failure (watermark unchanged) or a
    /// gap (watermark moved past an unread span, which stays visible).
    pub fn record_load_run(
        &self,
        target: &str,
        run: &RunRecord,
        watermark: Option<i64>,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        Self::insert_load_run(&tx, target, run)?;
        if let Some(watermark) = watermark {
            Self::set_load_watermark(&tx, target, watermark, run.started_at)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_runs(&self, target: &str, from: i64, to: i64) -> Result<Vec<RunRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT started_at, slice_start, slice_end, state, reason, rows_read, duration_ms
             FROM load_runs WHERE target = ?1 AND slice_end > ?2 AND slice_start < ?3 ORDER BY slice_start DESC, id DESC",
        )?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok(RunRecord {
                started_at: row.get(0)?,
                slice_start: row.get(1)?,
                slice_end: row.get(2)?,
                state: RunState::parse(&row.get::<_, String>(3)?).unwrap_or(RunState::Failed),
                reason: row.get(4)?,
                rows_read: row.get::<_, i64>(5)? as u64,
                attributed: 0,
                partial_statements: 0,
                unavailable_statements: 0,
                duration_ms: row.get::<_, i64>(6)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn load_buckets(&self, target: &str, from: i64, to: i64) -> Result<Vec<StoredLoadBucket>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {LOAD_BUCKET_COLUMNS} FROM load_buckets WHERE target = ?1 AND bucket_start >= ?2 AND bucket_start < ?3 ORDER BY bucket_start, db, tbl, kind"
        ))?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok(load_bucket_from_row(row))
        })?;
        rows.map(|row| row.map_err(HistoryError::from).and_then(|bucket| bucket))
            .collect()
    }

    /// Load targets with any recorded state, most recently updated first.
    pub fn load_targets(&self) -> Result<Vec<String>> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT target FROM load_targets ORDER BY updated_at DESC, target")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| row.map_err(HistoryError::from)).collect()
    }

    /// One snapshot of every job. Counter deltas are attributed to the hour of
    /// this snapshot only when the previous snapshot is at most `max_gap`
    /// seconds old and no counter went down; otherwise the interval is
    /// counted as unknown. A state change is recorded with its reason. The
    /// snapshot itself is counted per hour, empty ones included; a truncated
    /// one (job limit reached) cannot show that a job is absent.
    pub fn record_routine_snapshot(
        &self,
        target: &str,
        observed_at: i64,
        max_gap: i64,
        jobs: &[RoutineSnapshotJob],
        truncated: bool,
        version: Option<(&str, &str)>,
    ) -> Result<()> {
        let bucket_start = observed_at.div_euclid(doctor_core::HISTORY_BUCKET_SECONDS)
            * doctor_core::HISTORY_BUCKET_SECONDS;
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        // A target with only Routine Load snapshots (3.2) is still found later.
        tx.execute(
            "INSERT INTO load_targets (target, updated_at) VALUES (?1, ?2) ON CONFLICT(target) DO UPDATE SET updated_at = excluded.updated_at",
            params![target, observed_at],
        )?;
        let (engine, rule) = version.unzip();
        tx.execute(
            "INSERT INTO routine_snapshot_hours (target, bucket_start, snapshots, truncated_snapshots, engine_version, rule_version) VALUES (?1, ?2, 1, ?3, ?4, ?5)
             ON CONFLICT(target, bucket_start) DO UPDATE SET snapshots = snapshots + 1, truncated_snapshots = truncated_snapshots + excluded.truncated_snapshots,
               engine_version = CASE WHEN routine_snapshot_hours.engine_version IS excluded.engine_version THEN routine_snapshot_hours.engine_version ELSE '*' END,
               rule_version = CASE WHEN routine_snapshot_hours.rule_version IS excluded.rule_version THEN routine_snapshot_hours.rule_version ELSE '*' END",
            params![target, bucket_start, i64::from(truncated), engine, rule],
        )?;
        tx.execute(
            "INSERT INTO routine_snapshot_last VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(target) DO UPDATE SET observed_at = excluded.observed_at, truncated = excluded.truncated,
               engine_version = excluded.engine_version, rule_version = excluded.rule_version",
            params![target, observed_at, i64::from(truncated), engine, rule],
        )?;
        for job in jobs {
            #[allow(clippy::type_complexity)]
            let last: Option<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT observed_at, state, loaded, error, committed, aborted FROM routine_last WHERE target = ?1 AND job_id = ?2",
                    params![target, job.job_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                )
                .optional()?;
            let mut deltas: Option<[u128; 4]> = None;
            let mut has_interval = false;
            if let Some((at, state, loaded, error, committed, aborted)) = &last {
                if *at < observed_at {
                    has_interval = true;
                    let close = observed_at - at <= max_gap;
                    let pairs = [
                        (loaded, job.loaded_rows),
                        (error, job.error_rows),
                        (committed, job.committed_tasks),
                        (aborted, job.aborted_tasks),
                    ];
                    let mut values = [0u128; 4];
                    let mut known = close;
                    for (index, (before, now)) in pairs.into_iter().enumerate() {
                        match (before.as_deref().and_then(|v| v.parse::<u128>().ok()), now) {
                            (Some(before), Some(now)) if now >= before => {
                                values[index] = now - before
                            }
                            _ => known = false,
                        }
                    }
                    if known {
                        deltas = Some(values);
                    }
                }
                if state != &job.state {
                    tx.execute(
                        "INSERT OR REPLACE INTO routine_events VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            target,
                            job.job_id,
                            observed_at,
                            job.name,
                            state,
                            job.state,
                            job.reason
                        ],
                    )?;
                }
            } else {
                tx.execute(
                    "INSERT OR REPLACE INTO routine_events VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
                    params![
                        target,
                        job.job_id,
                        observed_at,
                        job.name,
                        job.state,
                        job.reason
                    ],
                )?;
            }
            let existing: Option<StoredRoutineHour> = tx
                .query_row(
                    &format!("SELECT {ROUTINE_HOUR_COLUMNS} FROM routine_hours WHERE target = ?1 AND job_id = ?2 AND bucket_start = ?3"),
                    params![target, job.job_id, bucket_start],
                    |row| Ok(routine_hour_from_row(row)),
                )
                .optional()?
                .transpose()?;
            let mut hour = existing.unwrap_or_else(|| StoredRoutineHour {
                job_id: job.job_id.clone(),
                bucket_start,
                name: job.name.clone(),
                database: job.database.clone(),
                table: job.table.clone(),
                last_state: job.state.clone(),
                worst_state: job.state.clone(),
                snapshots: 0,
                max_lag: None,
                lag_snapshots: 0,
                loaded_delta: 0,
                error_delta: 0,
                committed_delta: 0,
                aborted_delta: 0,
                known_intervals: 0,
                unknown_intervals: 0,
            });
            hour.name = job.name.clone();
            hour.last_state = job.state.clone();
            if state_rank(&job.state) > state_rank(&hour.worst_state) {
                hour.worst_state = job.state.clone();
            }
            hour.snapshots += 1;
            if let Some(lag) = job.max_lag {
                hour.lag_snapshots += 1;
                hour.max_lag = Some(hour.max_lag.map_or(lag, |max| max.max(lag)));
            }
            match deltas {
                Some([loaded, error, committed, aborted]) => {
                    hour.known_intervals += 1;
                    hour.loaded_delta += loaded;
                    hour.error_delta += error;
                    hour.committed_delta += committed;
                    hour.aborted_delta += aborted;
                }
                None if has_interval => hour.unknown_intervals += 1,
                None => {}
            }
            tx.execute(
                "INSERT OR REPLACE INTO routine_hours VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    target, hour.job_id, hour.bucket_start, hour.name, hour.database, hour.table,
                    hour.last_state, hour.worst_state, hour.snapshots as i64,
                    hour.max_lag.map(|lag| lag.to_string()), hour.lag_snapshots as i64,
                    hour.loaded_delta.to_string(), hour.error_delta.to_string(),
                    hour.committed_delta.to_string(), hour.aborted_delta.to_string(),
                    hour.known_intervals as i64, hour.unknown_intervals as i64
                ],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO routine_last VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    target,
                    job.job_id,
                    observed_at,
                    job.state,
                    job.loaded_rows.map(|v| v.to_string()),
                    job.error_rows.map(|v| v.to_string()),
                    job.committed_tasks.map(|v| v.to_string()),
                    job.aborted_tasks.map(|v| v.to_string())
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn routine_hours(
        &self,
        target: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<StoredRoutineHour>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {ROUTINE_HOUR_COLUMNS} FROM routine_hours WHERE target = ?1 AND bucket_start >= ?2 AND bucket_start < ?3 ORDER BY job_id, bucket_start"
        ))?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok(routine_hour_from_row(row))
        })?;
        rows.map(|row| row.map_err(HistoryError::from).and_then(|hour| hour))
            .collect()
    }

    pub fn routine_snapshot_hours(
        &self,
        target: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<RoutineSnapshotHour>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT bucket_start, snapshots, truncated_snapshots, engine_version, rule_version FROM routine_snapshot_hours WHERE target = ?1 AND bucket_start >= ?2 AND bucket_start < ?3 ORDER BY bucket_start",
        )?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok(RoutineSnapshotHour {
                bucket_start: row.get(0)?,
                snapshots: row.get::<_, i64>(1)? as u32,
                truncated: row.get::<_, i64>(2)? as u32,
                engine_version: row.get(3)?,
                rule_version: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The latest snapshot of a target: when, whether it was truncated, and
    /// the StarRocks version and rule set it was taken under.
    pub fn routine_snapshot_last(&self, target: &str) -> Result<Option<RoutineSnapshotLast>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT observed_at, truncated, engine_version, rule_version FROM routine_snapshot_last WHERE target = ?1",
                params![target],
                |row| {
                    Ok(RoutineSnapshotLast {
                        observed_at: row.get(0)?,
                        truncated: row.get::<_, i64>(1)? != 0,
                        engine_version: row.get(2)?,
                        rule_version: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// When each job was last seen in a snapshot.
    pub fn routine_last_seen(
        &self,
        target: &str,
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT job_id, observed_at FROM routine_last WHERE target = ?1")?;
        let rows = statement.query_map(params![target], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn routine_events(
        &self,
        target: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<StoredRoutineEvent>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT job_id, observed_at, name, from_state, to_state, reason FROM routine_events WHERE target = ?1 AND observed_at >= ?2 AND observed_at < ?3 ORDER BY observed_at DESC, job_id",
        )?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok(StoredRoutineEvent {
                job_id: row.get(0)?,
                observed_at: row.get(1)?,
                name: row.get(2)?,
                from_state: row.get(3)?,
                to_state: row.get(4)?,
                reason: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Pending logs whose next try is due, oldest finish first (they expire
    /// first). Pending logs that finished before `min_finish` (StarRocks'
    /// retention) become `expired` instead.
    pub fn due_error_logs(
        &self,
        target: &str,
        now: i64,
        min_finish: i64,
        limit: u32,
    ) -> Result<Vec<StoredErrorLog>> {
        let connection = self.lock();
        connection.execute(
            "UPDATE load_error_logs SET state = 'expired' WHERE target = ?1 AND state = 'pending' AND finish_time < ?2",
            params![target, min_finish],
        )?;
        let mut statement = connection.prepare(&format!(
            "SELECT {ERROR_LOG_COLUMNS} FROM load_error_logs WHERE target = ?1 AND state = 'pending' AND next_try <= ?2 ORDER BY finish_time, job_id LIMIT ?3"
        ))?;
        let rows = statement.query_map(params![target, now, i64::from(limit)], |row| {
            Ok(error_log_from_row(row))
        })?;
        rows.map(|row| row.map_err(HistoryError::from).and_then(|log| log))
            .collect()
    }

    /// Stores one read's outcome. Saved text is bounded per load, then the
    /// oldest saved texts are dropped until the total fits 50 MB.
    pub fn save_error_log(
        &self,
        target: &str,
        job_id: &str,
        outcome: ErrorLogOutcome,
        now: i64,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        match outcome {
            ErrorLogOutcome::Saved {
                entries,
                // The parser's counts cover every parsed row; the kept
                // sample is recounted below.
                categories: _,
                truncated,
            } => {
                // History may have been cleared (or the log purged) while the
                // read was in flight: no row, no text.
                let pending: bool = tx
                    .query_row(
                        "SELECT 1 FROM load_error_logs WHERE target = ?1 AND job_id = ?2",
                        params![target, job_id],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                if !pending {
                    return Ok(());
                }
                let (entries, truncated, bytes) = bounded_entries(entries, truncated);
                // Counts describe the kept sample, not the text that was cut.
                let categories = kept_categories(&entries);
                tx.execute(
                    "DELETE FROM load_error_entries WHERE target = ?1 AND job_id = ?2",
                    params![target, job_id],
                )?;
                for entry in &entries {
                    tx.execute(
                        "INSERT INTO load_error_entries VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            target, job_id, i64::from(entry.index), category_text(entry.category),
                            entry.column, entry.target_type, entry.reason, entry.row,
                            i64::from(entry.row_converted), i64::from(entry.truncated)
                        ],
                    )?;
                }
                let categories = serde_json::to_string(&categories)
                    .map_err(|_| HistoryError::Internal("categories_encode"))?;
                tx.execute(
                    "UPDATE load_error_logs SET state = 'saved', saved_at = ?3, sampled = ?4, truncated = ?5, raw_bytes = ?6, categories_json = ?7 WHERE target = ?1 AND job_id = ?2",
                    params![target, job_id, now, entries.len() as i64, i64::from(truncated), bytes as i64, categories],
                )?;
                Self::trim_error_logs(&tx)?;
            }
            ErrorLogOutcome::Purged | ErrorLogOutcome::NoLog => {
                let state = if outcome == ErrorLogOutcome::Purged {
                    ErrorLogState::Purged
                } else {
                    ErrorLogState::NoLog
                };
                tx.execute(
                    "UPDATE load_error_logs SET state = ?3 WHERE target = ?1 AND job_id = ?2",
                    params![target, job_id, state.as_str()],
                )?;
            }
            ErrorLogOutcome::ReadFailed => {
                let attempts: i64 = tx.query_row(
                    "SELECT attempts FROM load_error_logs WHERE target = ?1 AND job_id = ?2",
                    params![target, job_id],
                    |row| row.get(0),
                )?;
                let attempts = attempts + 1;
                if attempts >= i64::from(ERROR_LOG_MAX_ATTEMPTS) {
                    tx.execute(
                        "UPDATE load_error_logs SET state = 'failed', attempts = ?3 WHERE target = ?1 AND job_id = ?2",
                        params![target, job_id, attempts],
                    )?;
                } else {
                    let delay = (300i64 << attempts.min(4)).min(3_600);
                    tx.execute(
                        "UPDATE load_error_logs SET attempts = ?3, next_try = ?4 WHERE target = ?1 AND job_id = ?2",
                        params![target, job_id, attempts, now + delay],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn trim_error_logs(tx: &Transaction<'_>) -> Result<()> {
        loop {
            let total: i64 = tx.query_row(
                "SELECT COALESCE(SUM(raw_bytes), 0) FROM load_error_logs",
                [],
                |row| row.get(0),
            )?;
            if total as u64 <= ERROR_LOG_MAX_TOTAL_BYTES {
                return Ok(());
            }
            let oldest: Option<(String, String)> = tx
                .query_row(
                    "SELECT target, job_id FROM load_error_logs WHERE raw_bytes > 0 ORDER BY saved_at, finish_time, job_id LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((target, job_id)) = oldest else {
                return Ok(());
            };
            tx.execute(
                "DELETE FROM load_error_entries WHERE target = ?1 AND job_id = ?2",
                params![target, job_id],
            )?;
            tx.execute(
                "UPDATE load_error_logs SET state = 'trimmed', raw_bytes = 0 WHERE target = ?1 AND job_id = ?2",
                params![target, job_id],
            )?;
        }
    }

    /// Error log facts of one target that finished in `[from, to)`, newest
    /// first. No reason or row text.
    pub fn error_logs(&self, target: &str, from: i64, to: i64) -> Result<Vec<StoredErrorLog>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {ERROR_LOG_COLUMNS} FROM load_error_logs WHERE target = ?1 AND finish_time >= ?2 AND finish_time < ?3 ORDER BY finish_time DESC, job_id DESC"
        ))?;
        let rows =
            statement.query_map(params![target, from, to], |row| Ok(error_log_from_row(row)))?;
        rows.map(|row| row.map_err(HistoryError::from).and_then(|log| log))
            .collect()
    }

    pub fn error_log(&self, target: &str, job_id: &str) -> Result<Option<StoredErrorLog>> {
        self.lock()
            .query_row(
                &format!("SELECT {ERROR_LOG_COLUMNS} FROM load_error_logs WHERE target = ?1 AND job_id = ?2"),
                params![target, job_id],
                |row| Ok(error_log_from_row(row)),
            )
            .optional()?
            .transpose()
    }

    /// The saved reason and row text of one load: the only read of it.
    pub fn error_log_entries(&self, target: &str, job_id: &str) -> Result<Vec<LoadErrorEntry>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT idx, category, column_name, target_type, reason, row_text, row_converted, truncated FROM load_error_entries WHERE target = ?1 AND job_id = ?2 ORDER BY idx",
        )?;
        let rows = statement.query_map(params![target, job_id], |row| {
            Ok(LoadErrorEntry {
                index: u16::try_from(row.get::<_, i64>(0)?).unwrap_or(u16::MAX),
                category: category_parse(&row.get::<_, String>(1)?),
                column: row.get(2)?,
                target_type: row.get(3)?,
                reason: row.get(4)?,
                row: row.get(5)?,
                row_converted: row.get::<_, i64>(6)? != 0,
                truncated: row.get::<_, i64>(7)? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn error_log_storage(&self) -> Result<ErrorLogStorage> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*), COALESCE(SUM(state = 'saved'), 0), COALESCE(SUM(raw_bytes), 0), MIN(CASE WHEN state = 'saved' THEN saved_at END) FROM load_error_logs",
            [],
            |row| {
                Ok(ErrorLogStorage {
                    logs: row.get::<_, i64>(0)? as u64,
                    saved: row.get::<_, i64>(1)? as u64,
                    raw_bytes: row.get::<_, i64>(2)? as u64,
                    earliest_saved: row.get(3)?,
                })
            },
        )?)
    }

    pub(crate) fn purge_loads_before(tx: &Transaction<'_>, cutoff: i64) -> Result<u64> {
        let mut removed = tx.execute(
            "DELETE FROM load_buckets WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM load_runs WHERE slice_end < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM routine_hours WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM routine_events WHERE observed_at < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM load_error_entries WHERE EXISTS (SELECT 1 FROM load_error_logs l WHERE l.target = load_error_entries.target AND l.job_id = load_error_entries.job_id AND l.finish_time < ?1)",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM load_error_logs WHERE finish_time < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM load_error_entries WHERE NOT EXISTS (SELECT 1 FROM load_error_logs l WHERE l.target = load_error_entries.target AND l.job_id = load_error_entries.job_id)",
            [],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM routine_snapshot_hours WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM routine_last WHERE observed_at < ?1",
            params![cutoff],
        )? as u64;
        Ok(removed)
    }
}

pub(crate) const CLEAR_LOADS: &str = "DELETE FROM load_error_entries; DELETE FROM load_error_logs; DELETE FROM routine_events; DELETE FROM routine_last; DELETE FROM routine_hours; DELETE FROM routine_snapshot_hours; DELETE FROM load_runs; DELETE FROM load_buckets; DELETE FROM load_targets;";

const ROUTINE_HOUR_COLUMNS: &str = "job_id, bucket_start, name, db, tbl, last_state, worst_state, snapshots, max_lag, lag_snapshots, loaded_delta, error_delta, committed_delta, aborted_delta, known_intervals, unknown_intervals";

fn routine_hour_from_row(row: &Row<'_>) -> Result<StoredRoutineHour> {
    let max_lag: Option<String> = row.get(8)?;
    Ok(StoredRoutineHour {
        job_id: row.get(0)?,
        bucket_start: row.get(1)?,
        name: row.get(2)?,
        database: row.get(3)?,
        table: row.get(4)?,
        last_state: row.get(5)?,
        worst_state: row.get(6)?,
        snapshots: row.get::<_, i64>(7)? as u32,
        max_lag: max_lag.as_deref().map(parse_u128).transpose()?,
        lag_snapshots: row.get::<_, i64>(9)? as u32,
        loaded_delta: parse_u128(&row.get::<_, String>(10)?)?,
        error_delta: parse_u128(&row.get::<_, String>(11)?)?,
        committed_delta: parse_u128(&row.get::<_, String>(12)?)?,
        aborted_delta: parse_u128(&row.get::<_, String>(13)?)?,
        known_intervals: row.get::<_, i64>(14)? as u32,
        unknown_intervals: row.get::<_, i64>(15)? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_core::{CollectedLoad, aggregate_loads};

    fn run(start: i64, end: i64) -> RunRecord {
        RunRecord {
            started_at: end + 10,
            slice_start: start,
            slice_end: end,
            state: RunState::Complete,
            reason: None,
            rows_read: 2,
            attributed: 0,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 3,
        }
    }

    fn load(finish: i64, filtered: Option<u128>) -> CollectedLoad {
        CollectedLoad {
            job_id: finish as u64,
            label: format!("l{finish}"),
            database: "db".into(),
            table: "t".into(),
            kind: LoadHistoryKind::StreamLoad,
            cancelled: false,
            system: false,
            finish_epoch_seconds: finish,
            loaded_rows: Some(10),
            filtered_rows: filtered,
            unselected_rows: Some(0),
            scan_bytes: Some(100),
            total_seconds: Some(1),
            phases_ms: [Some(3), Some(4), None],
            has_error_log: false,
        }
    }

    fn candidate(job_id: u64, finish: i64) -> ErrorLogCandidate {
        ErrorLogCandidate {
            job_id,
            label: format!("l{job_id}"),
            database: "db".into(),
            table: "t".into(),
            kind: LoadHistoryKind::StreamLoad,
            finish_time: finish,
            filtered_rows: Some(2),
        }
    }

    fn entry(index: u16, reason: &str, row: &str) -> LoadErrorEntry {
        LoadErrorEntry {
            index,
            category: LoadErrorCategory::TooLong,
            column: Some("name".into()),
            target_type: Some("VARCHAR(5)".into()),
            reason: reason.into(),
            row: Some(row.into()),
            row_converted: false,
            truncated: false,
        }
    }

    #[test]
    fn audit_lh_01_02_load_slices_merge_once_and_clear_removes_everything() {
        let store = HistoryStore::open_in_memory().unwrap();
        let first = aggregate_loads(&[load(3_600, Some(0)), load(3_700, None)]);
        assert!(
            store
                .record_load_slice(
                    "t",
                    "3.5.14",
                    "r1",
                    &run(3_600, 3_900),
                    3_900,
                    &first,
                    &[candidate(7, 3_700)]
                )
                .unwrap()
        );
        // Behind the watermark: dropped, never counted twice.
        assert!(
            !store
                .record_load_slice("t", "3.5.14", "r1", &run(3_600, 3_900), 3_900, &first, &[])
                .unwrap()
        );
        let second = aggregate_loads(&[load(3_950, Some(4))]);
        store
            .record_load_slice("t", "3.5.14", "r1", &run(3_900, 4_200), 4_200, &second, &[])
            .unwrap();
        let buckets = store.load_buckets("t", 0, 10_000).unwrap();
        assert_eq!(buckets.len(), 1);
        let delta = &buckets[0].delta;
        assert_eq!(delta.loads, 3);
        assert_eq!(delta.filtered_rows, LoadCount { sum: 4, unknown: 1 });
        assert_eq!(delta.phases_ms[0].valid_count, 3);
        assert_eq!(delta.phases_ms[2].valid_count, 0);
        assert_eq!(store.load_target_state("t").unwrap().watermark, Some(4_200));
        assert_eq!(store.load_runs("t", 0, 10_000).unwrap().len(), 2);
        // A version change starts its own row.
        store
            .record_load_slice("t", "3.5.15", "r1", &run(4_200, 4_500), 4_500, &second, &[])
            .unwrap();
        assert_eq!(store.load_buckets("t", 0, 10_000).unwrap().len(), 2);
        store
            .save_error_log(
                "t",
                "7",
                ErrorLogOutcome::Saved {
                    entries: vec![entry(1, "r", "x")],
                    categories: vec![],
                    truncated: false,
                },
                5_000,
            )
            .unwrap();
        store.clear().unwrap();
        assert!(store.load_buckets("t", 0, 10_000).unwrap().is_empty());
        assert!(store.error_log_entries("t", "7").unwrap().is_empty());
        assert!(store.error_log("t", "7").unwrap().is_none());
        assert_eq!(
            store.load_target_state("t").unwrap(),
            LoadTargetState::default()
        );
    }

    #[test]
    fn audit_lh_04_routine_deltas_need_close_snapshots_and_rising_counters() {
        let store = HistoryStore::open_in_memory().unwrap();
        let job = |state: &str, loaded: u128, lag: Option<u128>| RoutineSnapshotJob {
            job_id: "9".into(),
            name: "events_rl".into(),
            database: "db".into(),
            table: "t".into(),
            state: state.into(),
            reason: (state == "PAUSED").then(|| "too many errors".into()),
            max_lag: lag,
            loaded_rows: Some(loaded),
            error_rows: Some(1),
            committed_tasks: Some(loaded / 10),
            aborted_tasks: Some(0),
        };
        store
            .record_routine_snapshot(
                "t",
                3_600,
                900,
                &[job("RUNNING", 100, Some(5))],
                false,
                None,
            )
            .unwrap();
        store
            .record_routine_snapshot(
                "t",
                3_900,
                900,
                &[job("RUNNING", 160, Some(8))],
                false,
                None,
            )
            .unwrap();
        // Counter went down (job recreated): unknown, not negative.
        store
            .record_routine_snapshot("t", 4_200, 900, &[job("PAUSED", 20, None)], false, None)
            .unwrap();
        // Too far apart: unknown.
        store
            .record_routine_snapshot("t", 9_000, 900, &[job("PAUSED", 50, None)], true, None)
            .unwrap();
        assert_eq!(
            store
                .routine_snapshot_hours("t", 0, 20_000)
                .unwrap()
                .iter()
                .map(|hour| (
                    hour.bucket_start,
                    hour.snapshots,
                    hour.truncated,
                    hour.complete()
                ))
                .collect::<Vec<_>>(),
            [(3_600, 3, 0, true), (7_200, 1, 1, false)]
        );
        // Snapshots alone register the target (3.2 has no load slices).
        assert_eq!(store.load_targets().unwrap(), ["t"]);
        let hours = store.routine_hours("t", 0, 20_000).unwrap();
        assert_eq!(hours.len(), 2);
        let first = &hours[0];
        assert_eq!(first.snapshots, 3);
        assert_eq!(first.loaded_delta, 60);
        assert_eq!(first.committed_delta, 6);
        assert_eq!((first.known_intervals, first.unknown_intervals), (1, 1));
        assert_eq!(first.max_lag, Some(8));
        assert_eq!(first.lag_snapshots, 2);
        assert_eq!(
            (first.last_state.as_str(), first.worst_state.as_str()),
            ("PAUSED", "PAUSED")
        );
        assert_eq!(
            (hours[1].known_intervals, hours[1].unknown_intervals),
            (0, 1)
        );
        let events = store.routine_events("t", 0, 20_000).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].to_state, "PAUSED");
        assert_eq!(events[0].from_state.as_deref(), Some("RUNNING"));
        assert_eq!(events[0].reason.as_deref(), Some("too many errors"));
        assert_eq!(events[1].from_state, None);
    }

    #[test]
    fn audit_lh_05_error_logs_are_bounded_retried_expired_and_trimmed() {
        let store = HistoryStore::open_in_memory().unwrap();
        let candidates: Vec<_> = (1..=3).map(|id| candidate(id, 1_000 + id as i64)).collect();
        store
            .record_load_slice("t", "v", "r", &run(0, 3_600), 3_600, &[], &candidates)
            .unwrap();
        // The oldest pending log is past StarRocks' retention.
        let due = store.due_error_logs("t", 10_000, 1_002, 5).unwrap();
        assert_eq!(
            due.iter()
                .map(|log| log.job_id.as_str())
                .collect::<Vec<_>>(),
            ["2", "3"]
        );
        assert_eq!(
            store.error_log("t", "1").unwrap().unwrap().state,
            ErrorLogState::Expired
        );

        let long_row = "x".repeat(4 * 1024);
        let entries: Vec<_> = (0..60).map(|i| entry(i, "reason", &long_row)).collect();
        let parsed = vec![LoadErrorCategoryCount {
            category: LoadErrorCategory::TooLong,
            column: Some("name".into()),
            target_type: Some("VARCHAR(5)".into()),
            count: 60,
        }];
        store
            .save_error_log(
                "t",
                "2",
                ErrorLogOutcome::Saved {
                    entries,
                    categories: parsed,
                    truncated: false,
                },
                10_000,
            )
            .unwrap();
        let saved = store.error_log("t", "2").unwrap().unwrap();
        assert_eq!(saved.state, ErrorLogState::Saved);
        assert!(saved.truncated);
        // 64 KB per load: 15 entries of about 4 KB fit.
        assert_eq!(saved.sampled, 15);
        // The counts describe the 15 kept rows, not the 60 that were read.
        assert_eq!(saved.categories[0].count, 15);
        assert!(saved.raw_bytes <= ERROR_LOG_MAX_BYTES_PER_LOAD as u64);
        assert_eq!(store.error_log_entries("t", "2").unwrap().len(), 15);

        store
            .save_error_log("t", "3", ErrorLogOutcome::ReadFailed, 10_000)
            .unwrap();
        let retry = store.error_log("t", "3").unwrap().unwrap();
        assert_eq!((retry.state, retry.attempts), (ErrorLogState::Pending, 1));
        assert!(store.due_error_logs("t", 10_000, 0, 5).unwrap().is_empty());
        for _ in 1..ERROR_LOG_MAX_ATTEMPTS {
            store
                .save_error_log("t", "3", ErrorLogOutcome::ReadFailed, 10_000)
                .unwrap();
        }
        assert_eq!(
            store.error_log("t", "3").unwrap().unwrap().state,
            ErrorLogState::Failed
        );

        // The 50 MB limit drops the oldest saved text first.
        {
            let connection = store.lock();
            connection
                .execute(
                    "UPDATE load_error_logs SET raw_bytes = ?1 WHERE job_id = '2'",
                    params![(ERROR_LOG_MAX_TOTAL_BYTES - 10) as i64],
                )
                .unwrap();
        }
        store
            .record_load_slice(
                "t",
                "v",
                "r",
                &run(3_600, 7_200),
                7_200,
                &[],
                &[candidate(4, 5_000)],
            )
            .unwrap();
        store
            .save_error_log(
                "t",
                "4",
                ErrorLogOutcome::Saved {
                    entries: vec![entry(1, "reason", "row-text")],
                    categories: vec![],
                    truncated: false,
                },
                11_000,
            )
            .unwrap();
        assert_eq!(
            store.error_log("t", "2").unwrap().unwrap().state,
            ErrorLogState::Trimmed
        );
        assert!(store.error_log_entries("t", "2").unwrap().is_empty());
        assert_eq!(store.error_log_entries("t", "4").unwrap().len(), 1);
        assert_eq!(store.error_log_storage().unwrap().saved, 1);

        // A read that returns after the history was cleared stores nothing.
        store.clear().unwrap();
        store
            .save_error_log(
                "t",
                "4",
                ErrorLogOutcome::Saved {
                    entries: vec![entry(1, "reason", "late-row")],
                    categories: vec![],
                    truncated: false,
                },
                12_000,
            )
            .unwrap();
        assert!(store.error_log_entries("t", "4").unwrap().is_empty());
        assert_eq!(store.error_log_storage().unwrap().raw_bytes, 0);
        store
            .record_load_slice(
                "t",
                "v",
                "r",
                &run(7_200, 10_800),
                10_800,
                &[],
                &[candidate(5, 5_000)],
            )
            .unwrap();
        store
            .save_error_log(
                "t",
                "5",
                ErrorLogOutcome::Saved {
                    entries: vec![entry(1, "reason", "row-text")],
                    categories: vec![],
                    truncated: false,
                },
                11_000,
            )
            .unwrap();
        // 30-day retention removes the facts and the text.
        assert!(store.purge_before(6_000).unwrap() > 0);
        assert!(store.error_log("t", "5").unwrap().is_none());
        assert!(store.error_log_entries("t", "5").unwrap().is_empty());
    }
}
