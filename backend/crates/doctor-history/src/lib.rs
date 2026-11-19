//! The only embedded database (AGENTS §4, ADR-2): one SQLite file outside the
//! repository holding literal-free templates, hourly bucket aggregates, run
//! records, collection watermarks and (stage D) alert and governance records.
//! It never stores raw SQL literals, Profile text or StarRocks credentials.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use doctor_core::{
    CollectedExecution, HISTORY_EXEMPLARS_PER_BUCKET, HistoryBucketDelta, HistoryMetric,
    HistoryTemplate, HistoryTemplateKind, LogHistogram, TrendBucket, aggregate_collection,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

mod governance;
mod load_alerts;
mod loads;
mod notify;
mod saved_connection;
pub use governance::{
    AlertFacts, AlertUpsert, ItemUpdate, StoreChange, StoredAlert, StoredEvent, StoredItem,
};
pub use load_alerts::{LoadAlertFacts, StoredLoadAlert};
pub use loads::{
    ERROR_LOG_MAX_ATTEMPTS, ERROR_LOG_MAX_BYTES_PER_LOAD, ERROR_LOG_MAX_ENTRIES,
    ERROR_LOG_MAX_TOTAL_BYTES, ErrorLogCandidate, ErrorLogOutcome, ErrorLogState, ErrorLogStorage,
    LoadTargetState, RoutineSnapshotJob, StoredErrorLog, StoredLoadBucket, StoredRoutineEvent,
    StoredRoutineHour,
};
pub use loads::{RoutineSnapshotHour, RoutineSnapshotLast};
pub use notify::{
    DeliveryAttempt, DeliveryFilter, NewDelivery, SecretUpdate, StoredChannel, StoredDelivery,
    StoredTarget, TargetWrite,
};
pub use saved_connection::StoredConnection;

pub const SCHEMA_VERSION: i64 = 10;
pub const AUDIT_REPLAY_MAX_AGE_SECONDS: i64 = 60 * 60;

/// One template's contribution to a slice as stored in `audit_slice_deltas`:
/// key, executions, errors, four metrics (valid, sum, max, histogram) and exemplars.
type StoredSliceDelta = (
    String,
    u64,
    u64,
    [(i64, String, Option<String>, Vec<u8>); 4],
    Vec<(String, u64, i64)>,
);
const MIGRATION_10: &str = "
CREATE TABLE collection_slice_evidence (
  source TEXT NOT NULL,
  target TEXT NOT NULL,
  slice_start INTEGER NOT NULL,
  slice_end INTEGER NOT NULL,
  run_id INTEGER NOT NULL,
  read_start INTEGER,
  read_end INTEGER,
  excluded_count TEXT,
  revision INTEGER NOT NULL DEFAULT 1,
  source_identity TEXT,
  engine_version TEXT,
  rule_version TEXT,
  replay_pending INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source, target, slice_start, slice_end)
);
CREATE TABLE audit_slice_deltas (
  target TEXT NOT NULL,
  slice_start INTEGER NOT NULL,
  slice_end INTEGER NOT NULL,
  bucket_start INTEGER NOT NULL,
  observed_ids BLOB NOT NULL,
  conflict_ids BLOB NOT NULL,
  owned_conflict_ids BLOB NOT NULL,
  deltas TEXT,
  PRIMARY KEY (target, slice_start, slice_end)
);
CREATE INDEX audit_slice_deltas_hour ON audit_slice_deltas (target, bucket_start);
";
// Privacy policy has its own revision, independent of fingerprint identity.
const TEMPLATE_REDACTION_VERSION: &str = "literal-free-template.v2";

fn redacted_template(text: &str, catalog: Option<&str>, database: Option<&str>) -> Option<String> {
    match doctor_core::product_sql_fingerprint(doctor_core::SqlFingerprintInput {
        statement: Some(text),
        truncated: false,
        catalog,
        database,
    }) {
        doctor_contracts::ProductSqlFingerprint::Exact { template, .. } => Some(template),
        _ => None,
    }
}

fn redacted_snapshot(text: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    let template = value.get_mut("view")?.get_mut("template")?;
    if let Some(text) = template.as_str() {
        *template = redacted_template(text, None, None)
            .map_or(serde_json::Value::Null, serde_json::Value::String);
    } else if !template.is_null() {
        return None;
    }
    serde_json::to_string(&value).ok()
}

#[derive(Debug)]
pub enum HistoryError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Internal(&'static str),
    MigrationRequiresManualResolution,
}

impl HistoryError {
    /// Public startup diagnostics are classified from typed errors, never
    /// from SQLite messages that may contain paths, SQL or stored values.
    pub fn unavailable_reason(&self) -> doctor_contracts::HistoryStoreUnavailableReason {
        use doctor_contracts::HistoryStoreUnavailableReason as Reason;
        use rusqlite::ErrorCode;
        match self {
            Self::MigrationRequiresManualResolution => Reason::MigrationRequiresManualResolution,
            Self::Io(_) => Reason::IoOrPermission,
            Self::Sqlite(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(
                        ErrorCode::PermissionDenied
                            | ErrorCode::ReadOnly
                            | ErrorCode::SystemIoFailure
                            | ErrorCode::CannotOpen
                            | ErrorCode::DiskFull
                    )
                ) =>
            {
                Reason::IoOrPermission
            }
            _ => Reason::Other,
        }
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "history_store:{error}"),
            Self::Io(error) => write!(formatter, "history_store:dir:{error}"),
            Self::Internal(code) => write!(formatter, "history_store:{code}"),
            Self::MigrationRequiresManualResolution => formatter.write_str(
                "history_store:duplicate_active_governance_items:manual_resolution_required",
            ),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<rusqlite::Error> for HistoryError {
    fn from(error: rusqlite::Error) -> Self {
        HistoryError::Sqlite(error)
    }
}

pub type Result<T> = std::result::Result<T, HistoryError>;

/// Stage C: the slowest few executions of each template hour, by query ID
/// only, so a Profile can be pulled on demand. No SQL text.
const MIGRATION_2: &str = "
CREATE TABLE exemplars (
  template_key TEXT NOT NULL,
  bucket_start INTEGER NOT NULL,
  query_id TEXT NOT NULL,
  query_time INTEGER NOT NULL,
  event_time INTEGER NOT NULL,
  PRIMARY KEY (template_key, bucket_start, query_id)
);
CREATE INDEX exemplars_bucket ON exemplars (bucket_start);
";

const MIGRATION_1: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE targets (
  target TEXT PRIMARY KEY,
  watermark INTEGER,
  paused INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE TABLE templates (
  template_key TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  kind TEXT NOT NULL,
  identity TEXT NOT NULL,
  catalog TEXT NOT NULL,
  database TEXT NOT NULL,
  template TEXT,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL
);
CREATE INDEX templates_target ON templates (target);
CREATE TABLE buckets (
  template_key TEXT NOT NULL,
  bucket_start INTEGER NOT NULL,
  engine_version TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  executions INTEGER NOT NULL,
  errors INTEGER NOT NULL,
  qt_valid INTEGER NOT NULL, qt_sum TEXT NOT NULL, qt_max TEXT, qt_hist BLOB NOT NULL,
  scan_valid INTEGER NOT NULL, scan_sum TEXT NOT NULL, scan_max TEXT, scan_hist BLOB NOT NULL,
  cpu_valid INTEGER NOT NULL, cpu_sum TEXT NOT NULL, cpu_max TEXT, cpu_hist BLOB NOT NULL,
  mem_valid INTEGER NOT NULL, mem_sum TEXT NOT NULL, mem_max TEXT, mem_hist BLOB NOT NULL,
  PRIMARY KEY (template_key, bucket_start, engine_version, rule_version)
);
CREATE INDEX buckets_start ON buckets (bucket_start);
CREATE TABLE runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  slice_start INTEGER NOT NULL,
  slice_end INTEGER NOT NULL,
  state TEXT NOT NULL,
  reason TEXT,
  rows_read INTEGER NOT NULL,
  attributed INTEGER NOT NULL,
  partial_statements INTEGER NOT NULL,
  unavailable_statements INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL
);
CREATE INDEX runs_target_slice ON runs (target, slice_start);
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunState {
    /// Every eligible row of the slice was read.
    Complete,
    /// Read, but bounded (row limit) or later found to have late rows.
    Partial,
    /// The read failed; the watermark did not move.
    Failed,
    /// Not read: skipped beyond the catch-up horizon, or never connected.
    Gap,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Gap => "gap",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "complete" => Self::Complete,
            "partial" => Self::Partial,
            "failed" => Self::Failed,
            "gap" => Self::Gap,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRecord {
    pub started_at: i64,
    pub slice_start: i64,
    pub slice_end: i64,
    pub state: RunState,
    pub reason: Option<String>,
    pub rows_read: u64,
    pub attributed: u64,
    pub partial_statements: u64,
    pub unavailable_statements: u64,
    pub duration_ms: u64,
}

pub struct SliceEvidence {
    pub read_range: Option<(i64, i64)>,
    pub excluded_count: Option<u64>,
    pub reasons: Vec<&'static str>,
}

pub struct AuditReplayCandidate {
    pub start: i64,
    pub end: i64,
    pub revision: u64,
}

pub enum AuditSliceWrite {
    Forward { expected_watermark: Option<i64> },
    Replay { expected_revision: u64 },
}

pub struct AuditSliceCommit<'a> {
    pub run: &'a RunRecord,
    pub evidence: &'a SliceEvidence,
    pub engine_version: &'a str,
    pub rule_version: &'a str,
    pub source_identity: &'a str,
    pub rows: Vec<CollectedExecution>,
    /// Excluded identity and whether the read saw it in the previous/next slice.
    pub excluded_ids: &'a [([u8; 32], (bool, bool))],
    /// Neighbours reported by a summary whose conflict identities were cut by the row limit.
    pub affected_neighbours: &'a [(i64, i64)],
}

pub fn audit_identity(id: &str) -> [u8; 32] {
    let hash = doctor_contracts::canonical_hash(&id);
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(hash.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    bytes
}

fn audit_ids(bytes: &[u8]) -> BTreeSet<[u8; 32]> {
    bytes
        .chunks_exact(32)
        .map(|id| id.try_into().unwrap())
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetState {
    pub watermark: Option<i64>,
    pub paused: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredBucket {
    pub template_key: String,
    pub engine_version: String,
    pub rule_version: String,
    pub delta: HistoryBucketDelta,
}

/// The additive totals of one target hour under one version pair, summed
/// over every template. Quantile histograms are deliberately not read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HourlyTotal {
    pub bucket_start: i64,
    pub engine_version: String,
    pub rule_version: String,
    pub executions: u64,
    pub errors: u64,
    pub query_time_valid: u64,
    /// Exact: stored decimal sums are added as integers, never as floats.
    pub query_time_sum: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredExemplar {
    pub query_id: String,
    /// Audit source units (ms).
    pub query_time: u64,
    pub event_time: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredTemplate {
    pub template: HistoryTemplate,
    pub first_seen: i64,
    pub last_seen: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoreStats {
    pub templates: u64,
    pub buckets: u64,
    pub runs: u64,
    pub file_bytes: Option<u64>,
}

pub struct HistoryStore {
    connection: Mutex<Connection>,
    path: Option<std::path::PathBuf>,
}

fn encode_metric(metric: &HistoryMetric) -> (i64, String, Option<String>, Vec<u8>) {
    (
        metric.valid_count as i64,
        metric.sum.to_string(),
        metric.max.map(|max| max.to_string()),
        metric.histogram.encode(),
    )
}

fn decode_metric(
    valid: i64,
    sum: String,
    max: Option<String>,
    hist: Vec<u8>,
) -> Result<HistoryMetric> {
    let invalid = || HistoryError::Internal("corrupt_metric");
    Ok(HistoryMetric {
        valid_count: u64::try_from(valid).map_err(|_| invalid())?,
        sum: sum.parse().map_err(|_| invalid())?,
        max: max
            .map(|max| max.parse())
            .transpose()
            .map_err(|_| invalid())?,
        histogram: LogHistogram::decode(&hist).ok_or_else(invalid)?,
    })
}

impl HistoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(HistoryError::Io)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let store = Self {
            connection: Mutex::new(connection),
            path: Some(path.to_owned()),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            connection: Mutex::new(Connection::open_in_memory()?),
            path: None,
        };
        store.migrate()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn migrate(&self) -> Result<()> {
        let mut connection = self.lock();
        connection.pragma_update(None, "secure_delete", "ON")?;
        let tx = connection.transaction()?;
        let has_meta: bool = tx.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |row| row.get::<_, i64>(0),
        )? == 1;
        let version: i64 = if has_meta {
            tx.query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
        } else {
            0
        };
        if version > SCHEMA_VERSION {
            return Err(HistoryError::Internal("newer_schema"));
        }
        if version < 1 {
            tx.execute_batch(MIGRATION_1)?;
        }
        if version < 2 {
            tx.execute_batch(MIGRATION_2)?;
        }
        if version < 3 {
            tx.execute_batch(governance::MIGRATION_3)?;
        }
        if version < 4 {
            let duplicates: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM governance_items WHERE state NOT IN ('closed', 'wont_fix') GROUP BY target, template_key HAVING COUNT(*) > 1)",
                [], |row| row.get(0),
            )?;
            // Preserve all historical user decisions. Choosing which item to
            // retire is not a schema migration's authority.
            if duplicates {
                return Err(HistoryError::MigrationRequiresManualResolution);
            }
            tx.execute_batch(governance::MIGRATION_4)?;
        }
        if version < 5 {
            tx.execute_batch(notify::MIGRATION_5)?;
        }
        if version < 6 {
            tx.execute_batch(loads::MIGRATION_6)?;
        }
        if version < 7 {
            tx.execute_batch(load_alerts::MIGRATION_7)?;
        }
        if version < 8 {
            tx.execute_batch(saved_connection::MIGRATION_8)?;
        }
        if version < SCHEMA_VERSION {
            // v9 coverage and rule keys are retained. Their new evidence
            // fields are unknown, and only v10 slices can be replayed.
            tx.execute_batch(MIGRATION_10)?;
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }
        let privacy_version: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'template_redaction_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let needs_privacy_upgrade = privacy_version.as_deref() != Some(TEMPLATE_REDACTION_VERSION);
        if needs_privacy_upgrade {
            let old = {
                let mut statement = tx.prepare("SELECT template_key, template, catalog, database FROM templates WHERE template IS NOT NULL")?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (key, text, catalog, database) in old {
                let clean = redacted_template(&text, Some(&catalog), Some(&database));
                if clean.as_deref() != Some(text.as_str()) {
                    tx.execute(
                        "UPDATE templates SET template = ?1 WHERE template_key = ?2",
                        params![clean, key],
                    )?;
                }
            }
            let old = {
                let mut statement = tx.prepare("SELECT id, snapshot_json FROM governance_items WHERE snapshot_json IS NOT NULL")?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (id, text) in old {
                let clean = redacted_snapshot(&text);
                if clean.as_deref() != Some(text.as_str()) {
                    tx.execute(
                        "UPDATE governance_items SET snapshot_json = ?1 WHERE id = ?2",
                        params![clean, id],
                    )?;
                }
            }
        }
        tx.commit()?;
        if needs_privacy_upgrade && self.path.is_some() {
            // Remove superseded SQL bytes from free pages and old WAL frames.
            // Busy checkpoint fails opening the store; never serve unsafe text.
            connection.execute_batch("VACUUM")?;
            let busy: i64 =
                connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
            if busy != 0 {
                return Err(HistoryError::Internal("privacy_checkpoint_busy"));
            }
        }
        // Record completion only after physical scrubbing succeeded. A busy
        // checkpoint is retried on the next open, never silently skipped.
        if needs_privacy_upgrade {
            connection.execute(
                "INSERT OR REPLACE INTO meta VALUES ('template_redaction_version', ?1)",
                [TEMPLATE_REDACTION_VERSION],
            )?;
        }
        Ok(())
    }

    pub fn target_state(&self, target: &str) -> Result<TargetState> {
        Ok(self
            .lock()
            .query_row(
                "SELECT watermark, paused FROM targets WHERE target = ?1",
                params![target],
                |row| {
                    Ok(TargetState {
                        watermark: row.get(0)?,
                        paused: row.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn set_paused(&self, target: &str, paused: bool, now: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO targets (target, watermark, paused, updated_at) VALUES (?1, NULL, ?2, ?3)
             ON CONFLICT(target) DO UPDATE SET paused = excluded.paused, updated_at = excluded.updated_at",
            params![target, i64::from(paused), now],
        )?;
        Ok(())
    }

    fn set_watermark(tx: &Transaction<'_>, target: &str, watermark: i64, now: i64) -> Result<()> {
        tx.execute(
            "INSERT INTO targets (target, watermark, paused, updated_at) VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(target) DO UPDATE SET watermark = excluded.watermark, updated_at = excluded.updated_at",
            params![target, watermark, now],
        )?;
        Ok(())
    }

    fn insert_run(tx: &Transaction<'_>, target: &str, run: &RunRecord) -> Result<()> {
        tx.execute(
            "INSERT INTO runs (target, started_at, slice_start, slice_end, state, reason, rows_read, attributed, partial_statements, unavailable_statements, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                target, run.started_at, run.slice_start, run.slice_end, run.state.as_str(), run.reason,
                run.rows_read as i64, run.attributed as i64, run.partial_statements as i64,
                run.unavailable_statements as i64, run.duration_ms as i64
            ],
        )?;
        Ok(())
    }

    /// One slice, atomically: templates, merged buckets, the run record and
    /// the new watermark. A failure leaves the store exactly as it was.
    #[allow(clippy::too_many_arguments)]
    pub fn record_slice(
        &self,
        target: &str,
        engine_version: &str,
        rule_version: &str,
        run: &RunRecord,
        watermark: i64,
        templates: &[HistoryTemplate],
        buckets: &[HistoryBucketDelta],
    ) -> Result<bool> {
        let mut connection = self.lock();
        // IMMEDIATE takes the write lock first, so two processes sharing one
        // file serialize here; a slice behind the watermark was already
        // recorded and is dropped instead of being counted twice.
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT watermark FROM targets WHERE target = ?1",
                params![target],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if current.is_some_and(|current| current > run.slice_start) {
            return Ok(false);
        }
        let storage_rule_version = TrendBucket::storage_rule_version(engine_version, rule_version);
        Self::write_audit_contributions(
            &tx,
            target,
            engine_version,
            &storage_rule_version,
            run.slice_end,
            templates,
            buckets,
        )?;
        Self::insert_run(&tx, target, run)?;
        Self::set_watermark(&tx, target, watermark, run.started_at)?;
        tx.commit()?;
        Ok(true)
    }

    fn write_audit_contributions(
        tx: &Transaction<'_>,
        target: &str,
        engine_version: &str,
        rule_version: &str,
        seen_at: i64,
        templates: &[HistoryTemplate],
        buckets: &[HistoryBucketDelta],
    ) -> Result<()> {
        for template in templates {
            let clean = template.template.as_deref().and_then(|text| {
                redacted_template(text, Some(&template.catalog), Some(&template.database))
            });
            tx.execute(
                "INSERT INTO templates (template_key, target, kind, identity, catalog, database, template, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(template_key) DO UPDATE SET template = COALESCE(templates.template, excluded.template), last_seen = MAX(templates.last_seen, excluded.last_seen)",
                params![template.template_key, target, template.kind.as_str(), template.identity, template.catalog, template.database, clean, seen_at],
            )?;
        }
        for delta in buckets {
            let existing: Option<StoredBucket> = tx
                .query_row(
                    "SELECT executions, errors, qt_valid, qt_sum, qt_max, qt_hist, scan_valid, scan_sum, scan_max, scan_hist, cpu_valid, cpu_sum, cpu_max, cpu_hist, mem_valid, mem_sum, mem_max, mem_hist
                     FROM buckets WHERE template_key = ?1 AND bucket_start = ?2 AND engine_version = ?3 AND rule_version = ?4",
                    params![delta.template_key, delta.bucket_start, engine_version, rule_version],
                    |row| Ok(bucket_from_row(row, delta.template_key.clone(), delta.bucket_start, engine_version, rule_version, 0)),
                )
                .optional()?
                .transpose()?;
            let mut merged =
                existing
                    .map(|bucket| bucket.delta)
                    .unwrap_or_else(|| HistoryBucketDelta {
                        template_key: delta.template_key.clone(),
                        bucket_start: delta.bucket_start,
                        ..HistoryBucketDelta::default()
                    });
            merged.merge(delta);
            let (qv, qs, qm, qh) = encode_metric(&merged.query_time);
            let (sv, ss, sm, sh) = encode_metric(&merged.scan_bytes);
            let (cv, cs, cm, ch) = encode_metric(&merged.cpu_cost);
            let (mv, ms, mm, mh) = encode_metric(&merged.memory_cost);
            tx.execute(
                "INSERT OR REPLACE INTO buckets VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                params![
                    merged.template_key, merged.bucket_start, engine_version, rule_version,
                    merged.executions as i64, merged.errors as i64,
                    qv, qs, qm, qh, sv, ss, sm, sh, cv, cs, cm, ch, mv, ms, mm, mh
                ],
            )?;
            for exemplar in &delta.exemplars {
                tx.execute(
                    "INSERT OR IGNORE INTO exemplars VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        delta.template_key,
                        delta.bucket_start,
                        exemplar.query_id,
                        i64::try_from(exemplar.query_time).unwrap_or(i64::MAX),
                        exemplar.event_seconds
                    ],
                )?;
            }
            if !delta.exemplars.is_empty() {
                tx.execute(
                    "DELETE FROM exemplars WHERE template_key = ?1 AND bucket_start = ?2 AND query_id NOT IN (
                       SELECT query_id FROM exemplars WHERE template_key = ?1 AND bucket_start = ?2 ORDER BY query_time DESC, query_id LIMIT ?3)",
                    params![delta.template_key, delta.bucket_start, HISTORY_EXEMPLARS_PER_BUCKET as i64],
                )?;
            }
        }
        Ok(())
    }

    fn prune_audit_slice_deltas(
        tx: &Transaction<'_>,
        target: &str,
        earliest_end: i64,
    ) -> Result<()> {
        // Forward catch-up can be far behind the replay horizon. Its next
        // slice can still meet the slice ending at the watermark. A replay can
        // meet the predecessor of any slice still inside the horizon, using
        // that slice's stored width even if the configured width has changed.
        let identity_cutoff: i64 = tx.query_row(
            "SELECT COALESCE(MIN(boundary), ?2) FROM (
               SELECT watermark AS boundary FROM targets WHERE target = ?1
               UNION ALL
               SELECT slice_start FROM audit_slice_deltas
                 WHERE target = ?1 AND slice_end >= ?2 AND deltas IS NOT NULL
             )",
            params![target, earliest_end],
            |row| row.get(0),
        )?;
        tx.execute(
            "DELETE FROM audit_slice_deltas WHERE target = ?1
               AND bucket_start < ?2 AND slice_end < MIN(?2, ?3)",
            params![
                target,
                earliest_end - doctor_core::HISTORY_BUCKET_SECONDS,
                identity_cutoff
            ],
        )?;
        // An older slice's delta can still be needed to rebuild the full
        // hour after its identities are outside every reachable boundary.
        tx.execute(
            "UPDATE audit_slice_deltas
             SET observed_ids = X'', conflict_ids = X'', owned_conflict_ids = X''
             WHERE target = ?1 AND slice_end < ?2
               AND (length(observed_ids) > 0 OR length(conflict_ids) > 0 OR length(owned_conflict_ids) > 0)",
            params![target, identity_cutoff],
        )?;
        Ok(())
    }

    pub fn audit_replay_candidate(
        &self,
        target: &str,
        earliest_end: i64,
        latest_end: i64,
    ) -> Result<Option<AuditReplayCandidate>> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::prune_audit_slice_deltas(&tx, target, earliest_end)?;
        let candidate = tx
            .query_row(
                "SELECT e.slice_start, e.slice_end, e.revision FROM collection_slice_evidence e
             JOIN audit_slice_deltas d USING (target, slice_start, slice_end)
              WHERE e.source = 'audit' AND e.target = ?1 AND e.replay_pending = 1
                AND d.deltas IS NOT NULL AND e.slice_end BETWEEN ?2 AND ?3
             ORDER BY e.slice_end, e.slice_start LIMIT 1",
                params![target, earliest_end, latest_end],
                |row| {
                    Ok(AuditReplayCandidate {
                        start: row.get(0)?,
                        end: row.get(1)?,
                        revision: row.get(2)?,
                    })
                },
            )
            .optional()?;
        tx.commit()?;
        Ok(candidate)
    }

    pub fn record_collected_audit_slice(
        &self,
        target: &str,
        mode: AuditSliceWrite,
        slice: AuditSliceCommit<'_>,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run = slice.run;
        let (start, end) = (run.slice_start, run.slice_end);
        let hour_width = doctor_core::HISTORY_BUCKET_SECONDS;
        let hour = start.div_euclid(hour_width) * hour_width;
        let stored_rules =
            TrendBucket::storage_rule_version(slice.engine_version, slice.rule_version);
        let mut reasons: Vec<&str> = slice.evidence.reasons.to_vec();
        let (mut read_start, mut read_end) = slice.evidence.read_range.unzip();
        let mut excluded_count = slice.evidence.excluded_count;
        let row_ids: Vec<_> = slice
            .rows
            .iter()
            .map(|row| row.query_id.as_deref().map(audit_identity))
            .collect();
        let mut observed: BTreeSet<_> = row_ids.iter().flatten().copied().collect();
        let mut conflict_ids = BTreeSet::new();
        let mut owned_conflicts = BTreeSet::new();
        let mut old_ids = BTreeSet::new();
        let mut old_reason = None;
        let mut previously_unknown_exclusions = false;
        let mut same_source = true;
        let target_state: Option<(Option<i64>, bool)> = tx
            .query_row(
                "SELECT watermark, paused FROM targets WHERE target = ?1",
                [target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (watermark, paused) = target_state.unwrap_or_default();
        if paused {
            return Ok(false);
        }
        // A v9 or legacy write has no reversible delta. A slice spanning
        // hours is append-only too, so a later replay cannot erase its data.
        let rebuildable = end <= hour + hour_width
            && !tx.query_row(
                "SELECT EXISTS (
               SELECT 1 FROM runs r WHERE r.target = ?1 AND r.slice_end > ?2 AND r.slice_start < ?3
                 AND r.state IN ('complete', 'partial') AND NOT EXISTS (
                   SELECT 1 FROM audit_slice_deltas d WHERE d.target = r.target
                      AND d.slice_start = r.slice_start AND d.slice_end = r.slice_end AND d.deltas IS NOT NULL)
             )",
                params![target, hour, hour + hour_width],
                |row| row.get::<_, bool>(0),
            )?;
        let (run_id, revision, replay_pending, replace) = match mode {
            AuditSliceWrite::Forward { expected_watermark } => {
                if watermark != expected_watermark || watermark.is_some_and(|value| value > start) {
                    return Ok(false);
                }
                Self::insert_run(&tx, target, run)?;
                (tx.last_insert_rowid(), 1, rebuildable, true)
            }
            AuditSliceWrite::Replay { expected_revision } => {
                let old = tx.query_row(
                    "SELECT e.run_id, e.revision, e.source_identity, e.engine_version, e.rule_version,
                            d.observed_ids, d.conflict_ids, d.owned_conflict_ids, r.reason, e.excluded_count
                     FROM collection_slice_evidence e JOIN audit_slice_deltas d USING (target, slice_start, slice_end)
                     JOIN runs r ON r.id = e.run_id
                     WHERE e.source = 'audit' AND e.target = ?1 AND e.slice_start = ?2 AND e.slice_end = ?3",
                    params![target, start, end],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u64>(1)?, row.get::<_, String>(2)?,
                         row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, Vec<u8>>(5)?,
                         row.get::<_, Vec<u8>>(6)?, row.get::<_, Vec<u8>>(7)?,
                         row.get::<_, Option<String>>(8)?, row.get::<_, Option<String>>(9)?)),
                ).optional()?;
                let Some((
                    run_id,
                    revision,
                    source,
                    engine,
                    rules,
                    ids,
                    conflicts,
                    owned,
                    reason,
                    excluded,
                )) = old
                else {
                    return Ok(false);
                };
                if revision != expected_revision {
                    return Ok(false);
                }
                same_source = source == slice.source_identity
                    && (run.state == RunState::Failed
                        || (engine == slice.engine_version && rules == stored_rules));
                old_ids = audit_ids(&ids);
                conflict_ids = audit_ids(&conflicts);
                owned_conflicts = audit_ids(&owned);
                old_reason = reason;
                previously_unknown_exclusions = excluded.is_none();
                let still_visible = old_ids.iter().all(|id| {
                    observed.contains(id)
                        || slice
                            .excluded_ids
                            .iter()
                            .any(|(excluded, _)| excluded == id)
                });
                let replace = rebuildable && same_source && still_visible && read_start.is_some();
                if !same_source {
                    (read_start, read_end, excluded_count) = (None, None, None);
                }
                // A transport failure can retry within the same horizon.
                let retry = same_source && run.state == RunState::Failed;
                (run_id, revision + 1, retry, replace)
            }
        };
        let mut suppressed = BTreeSet::new();
        let mut reported_owned = 0;
        if same_source {
            for &(id, (previous, _)) in slice.excluded_ids {
                let new = conflict_ids.insert(id);
                if previous {
                    owned_conflicts.remove(&id);
                } else {
                    reported_owned += 1;
                    if new {
                        owned_conflicts.insert(id);
                    }
                }
            }
            let mut statement = tx.prepare(
                "SELECT d.slice_start, d.slice_end, d.observed_ids, d.conflict_ids, d.owned_conflict_ids,
                        e.run_id, e.excluded_count, e.source_identity = ?4
                 FROM audit_slice_deltas d JOIN collection_slice_evidence e USING (target, slice_start, slice_end)
                 WHERE e.source = 'audit' AND d.target = ?1 AND (d.slice_end = ?2 OR d.slice_start = ?3)",
            )?;
            let neighbours = statement
                .query_map(params![target, start, end, slice.source_identity], |row| {
                    Ok((
                        (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?),
                        audit_ids(&row.get::<_, Vec<u8>>(2)?),
                        audit_ids(&row.get::<_, Vec<u8>>(3)?),
                        audit_ids(&row.get::<_, Vec<u8>>(4)?),
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, bool>(7)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (
                bounds,
                neighbour_observed,
                mut neighbour_conflicts,
                mut neighbour_owned,
                neighbour_run,
                count,
                same_neighbour_source,
            ) in neighbours
            {
                let previous = bounds.1 == start;
                let mut shared = BTreeSet::new();
                // AuditLoader is append-only. Dedup does not follow source-side
                // timestamp rewrites of one row across multiple slices.
                for &id in observed.intersection(&neighbour_observed) {
                    shared.insert(id);
                    suppressed.insert(id);
                    let new = conflict_ids.insert(id);
                    if previous {
                        owned_conflicts.remove(&id);
                    } else if new {
                        owned_conflicts.insert(id);
                    }
                }
                if same_neighbour_source {
                    shared.extend(slice.excluded_ids.iter().filter_map(
                        |&(id, (before, after))| {
                            (if previous { before } else { after }).then_some(id)
                        },
                    ));
                }
                // A limited summary can prove the neighbour is affected
                // even when no conflict identity fits in the detail rows.
                let unidentified =
                    same_neighbour_source && slice.affected_neighbours.contains(&bounds);
                let mark = !shared.is_empty() || unidentified;
                let new_ids: Vec<_> = shared
                    .iter()
                    .copied()
                    .filter(|&id| neighbour_conflicts.insert(id))
                    .collect();
                let has_new_ids = !new_ids.is_empty();
                // The earlier slice now owns the exclusion. A later neighbour
                // can already know the ID and still need to give up its count.
                let removed = if previous {
                    0
                } else {
                    let prior = neighbour_owned.len();
                    neighbour_owned.retain(|id| !shared.contains(id));
                    (prior - neighbour_owned.len()) as u64
                };
                if has_new_ids || removed != 0 || (unidentified && count.is_some()) {
                    let added = if previous { new_ids.len() as u64 } else { 0 };
                    if previous {
                        neighbour_owned.extend(new_ids);
                    }
                    let count = count
                        .filter(|_| !unidentified)
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(|prior| {
                            prior
                                .saturating_sub(removed)
                                .saturating_add(added)
                                .to_string()
                        });
                    tx.execute(
                        "UPDATE audit_slice_deltas SET conflict_ids = ?4, owned_conflict_ids = ?5
                         WHERE target = ?1 AND slice_start = ?2 AND slice_end = ?3",
                        params![
                            target,
                            bounds.0,
                            bounds.1,
                            neighbour_conflicts
                                .into_iter()
                                .collect::<Vec<_>>()
                                .as_flattened(),
                            neighbour_owned
                                .into_iter()
                                .collect::<Vec<_>>()
                                .as_flattened()
                        ],
                    )?;
                    tx.execute(
                        "UPDATE collection_slice_evidence AS e SET revision = revision + 1, excluded_count = ?3,
                            replay_pending = CASE WHEN NOT ?5 THEN replay_pending
                             WHEN slice_end >= ?4 AND EXISTS (
                                SELECT 1 FROM audit_slice_deltas d WHERE d.target = e.target
                                  AND d.slice_start = e.slice_start AND d.slice_end = e.slice_end AND d.deltas IS NOT NULL
                            ) THEN 1 ELSE 0 END
                         WHERE source = 'audit' AND target = ?1 AND run_id = ?2",
                        params![target, neighbour_run, count, run.started_at - AUDIT_REPLAY_MAX_AGE_SECONDS, has_new_ids],
                    )?;
                }
                if mark {
                    tx.execute(
                        "UPDATE runs SET state = 'partial', reason = CASE WHEN reason IS NULL OR reason = 'first_look_unproven'
                           THEN 'conflicting_query_id' ELSE reason || ',conflicting_query_id' END
                         WHERE id = ?1 AND instr(',' || COALESCE(reason, '') || ',', ',conflicting_query_id,') = 0",
                        [neighbour_run],
                    )?;
                }
            }
        }
        observed.retain(|id| !suppressed.contains(id));
        let rows: Vec<_> = slice
            .rows
            .into_iter()
            .zip(row_ids)
            .filter_map(|(row, id)| (!id.is_some_and(|id| suppressed.contains(&id))).then_some(row))
            .collect();
        let (templates, buckets, tally) = aggregate_collection(target, &rows);
        let unavailable = tally.unavailable + tally.missing_context + tally.missing_time;
        if tally.partial != 0 {
            reasons.push("statement_truncated");
        }
        if !reasons.contains(&"conflicting_query_id")
            && (!conflict_ids.is_empty()
                || old_reason.as_deref().is_some_and(|reason| {
                    reason
                        .split(',')
                        .any(|reason| reason == "conflicting_query_id")
                }))
        {
            reasons.retain(|reason| *reason != "first_look_unproven");
            reasons.push("conflicting_query_id");
        }
        // A capped replay cannot restore a count invalidated by an omitted identity.
        excluded_count = excluded_count
            .filter(|_| !(previously_unknown_exclusions && reasons.contains(&"row_limit")))
            .map(|count| {
                count
                    .saturating_sub(reported_owned)
                    .saturating_add(owned_conflicts.len() as u64)
            });
        // Product decision: a clean bounded replay completes the slice unless
        // a conflict was already recorded.
        // Rows arriving after the replay horizon can still go uncollected.
        if reasons.is_empty()
            && run.reason.is_none()
            && (!matches!(mode, AuditSliceWrite::Replay { .. }) || !replace || unavailable != 0)
        {
            reasons.push("first_look_unproven");
        }
        if same_source
            && matches!(mode, AuditSliceWrite::Replay { .. })
            && !reasons.is_empty()
            && observed.iter().any(|id| !old_ids.contains(id))
        {
            reasons.push("late_rows");
        }
        let reason = match (reasons.is_empty(), run.reason.as_deref()) {
            (true, reason) => reason.map(str::to_owned),
            (false, Some(reason)) => Some(format!("{},{}", reasons.join(","), reason)),
            (false, None) => Some(reasons.join(",")),
        };
        let state =
            if reason.is_some() || (matches!(mode, AuditSliceWrite::Replay { .. }) && !replace) {
                RunState::Partial
            } else {
                run.state
            };
        tx.execute(
            "UPDATE runs SET state = ?2, reason = ?3, started_at = ?4, duration_ms = ?5,
               rows_read = CASE WHEN ?6 THEN ?7 ELSE rows_read END,
               attributed = CASE WHEN ?6 THEN ?8 ELSE attributed END,
               partial_statements = CASE WHEN ?6 THEN ?9 ELSE partial_statements END,
               unavailable_statements = CASE WHEN ?6 THEN ?10 ELSE unavailable_statements END
             WHERE id = ?1",
            params![
                run_id,
                state.as_str(),
                reason,
                run.started_at,
                run.duration_ms,
                replace,
                tally.read + suppressed.len() as u64,
                tally.attributed,
                tally.partial,
                unavailable
            ],
        )?;
        tx.execute(
            "INSERT INTO collection_slice_evidence
               (source, target, slice_start, slice_end, run_id, read_start, read_end, excluded_count,
                revision, source_identity, engine_version, rule_version, replay_pending)
             VALUES ('audit', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(source, target, slice_start, slice_end) DO UPDATE SET
               read_start = excluded.read_start, read_end = excluded.read_end, excluded_count = excluded.excluded_count,
               revision = excluded.revision, replay_pending = excluded.replay_pending",
             params![target, start, end, run_id, read_start, read_end, excluded_count.map(|count| count.to_string()), revision,
                slice.source_identity, slice.engine_version, stored_rules, replay_pending],
        )?;
        if replace {
            Self::write_audit_contributions(
                &tx,
                target,
                slice.engine_version,
                &stored_rules,
                end,
                &templates,
                if matches!(mode, AuditSliceWrite::Forward { .. }) {
                    &buckets
                } else {
                    &[]
                },
            )?;
            let payload = if rebuildable {
                let deltas: Vec<_> = buckets
                    .iter()
                    .map(|delta| {
                        let exemplars: Vec<_> = delta
                            .exemplars
                            .iter()
                            .map(|example| {
                                (&example.query_id, example.query_time, example.event_seconds)
                            })
                            .collect();
                        (
                            &delta.template_key,
                            delta.executions,
                            delta.errors,
                            [
                                encode_metric(&delta.query_time),
                                encode_metric(&delta.scan_bytes),
                                encode_metric(&delta.cpu_cost),
                                encode_metric(&delta.memory_cost),
                            ],
                            exemplars,
                        )
                    })
                    .collect();
                Some(
                    serde_json::to_string(&deltas)
                        .map_err(|_| HistoryError::Internal("audit_slice_deltas"))?,
                )
            } else {
                None
            };
            tx.execute(
                "INSERT OR REPLACE INTO audit_slice_deltas VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    target,
                    start,
                    end,
                    hour,
                    observed.into_iter().collect::<Vec<_>>().as_flattened(),
                    conflict_ids.into_iter().collect::<Vec<_>>().as_flattened(),
                    owned_conflicts
                        .into_iter()
                        .collect::<Vec<_>>()
                        .as_flattened(),
                    payload
                ],
            )?;
            if matches!(mode, AuditSliceWrite::Replay { .. }) {
                tx.execute(
                        "DELETE FROM buckets WHERE bucket_start = ?2 AND template_key IN (SELECT template_key FROM templates WHERE target = ?1)",
                        params![target, hour],
                    )?;
                tx.execute(
                        "DELETE FROM exemplars WHERE bucket_start = ?2 AND template_key IN (SELECT template_key FROM templates WHERE target = ?1)",
                        params![target, hour],
                    )?;
                let mut statement = tx.prepare(
                    "SELECT e.engine_version, e.rule_version, d.deltas FROM audit_slice_deltas d
                         JOIN collection_slice_evidence e USING (target, slice_start, slice_end)
                         WHERE e.source = 'audit' AND d.target = ?1 AND d.bucket_start = ?2
                           AND d.deltas IS NOT NULL ORDER BY d.slice_start",
                )?;
                let mut rows = statement.query(params![target, hour])?;
                while let Some(row) = rows.next()? {
                    let (engine, rules, payload): (String, String, String) =
                        (row.get(0)?, row.get(1)?, row.get(2)?);
                    let encoded: Vec<StoredSliceDelta> = serde_json::from_str(&payload)
                        .map_err(|_| HistoryError::Internal("audit_slice_deltas"))?;
                    let deltas = encoded
                        .into_iter()
                        .map(|(template_key, executions, errors, metrics, exemplars)| {
                            let [query_time, scan_bytes, cpu_cost, memory_cost] =
                                metrics.map(|(valid, sum, max, hist)| {
                                    decode_metric(valid, sum, max, hist)
                                });
                            Ok(HistoryBucketDelta {
                                template_key,
                                bucket_start: hour,
                                executions,
                                errors,
                                query_time: query_time?,
                                scan_bytes: scan_bytes?,
                                cpu_cost: cpu_cost?,
                                memory_cost: memory_cost?,
                                exemplars: exemplars
                                    .into_iter()
                                    .map(|(query_id, query_time, event_seconds)| {
                                        doctor_core::HistoryExemplar {
                                            query_id,
                                            query_time,
                                            event_seconds,
                                        }
                                    })
                                    .collect(),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Self::write_audit_contributions(
                        &tx,
                        target,
                        &engine,
                        &rules,
                        end,
                        &[],
                        &deltas,
                    )?;
                }
            }
        } else {
            tx.execute(
                "UPDATE audit_slice_deltas SET conflict_ids = ?4, owned_conflict_ids = ?5
                 WHERE target = ?1 AND slice_start = ?2 AND slice_end = ?3",
                params![
                    target,
                    start,
                    end,
                    conflict_ids.into_iter().collect::<Vec<_>>().as_flattened(),
                    owned_conflicts
                        .into_iter()
                        .collect::<Vec<_>>()
                        .as_flattened()
                ],
            )?;
        }
        if matches!(mode, AuditSliceWrite::Forward { .. }) {
            Self::set_watermark(&tx, target, end, run.started_at)?;
            Self::prune_audit_slice_deltas(
                &tx,
                target,
                run.started_at - AUDIT_REPLAY_MAX_AGE_SECONDS,
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// A run that moved no data: a failure (watermark unchanged) or a gap
    /// (watermark moved past an unread span, which stays visible as a gap).
    pub fn record_run(&self, target: &str, run: &RunRecord, watermark: Option<i64>) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        Self::insert_run(&tx, target, run)?;
        if let Some(watermark) = watermark {
            Self::set_watermark(&tx, target, watermark, run.started_at)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Late rows found after a slice was read: the slice becomes partial; the
    /// rows are not read, so nothing is counted twice.
    pub fn mark_partial(
        &self,
        target: &str,
        slice_start: i64,
        slice_end: i64,
        reason: &str,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE runs SET state = 'partial', reason = ?4 WHERE target = ?1 AND slice_start = ?2 AND slice_end = ?3 AND state = 'complete'",
            params![target, slice_start, slice_end, reason],
        )?;
        Ok(())
    }

    pub fn runs(&self, target: &str, from: i64, to: i64, limit: u32) -> Result<Vec<RunRecord>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT started_at, slice_start, slice_end, state, reason, rows_read, attributed, partial_statements, unavailable_statements, duration_ms
             FROM runs WHERE target = ?1 AND slice_end > ?2 AND slice_start < ?3 ORDER BY slice_start DESC, id DESC LIMIT ?4",
        )?;
        let rows = statement.query_map(params![target, from, to, limit], |row| {
            Ok(RunRecord {
                started_at: row.get(0)?,
                slice_start: row.get(1)?,
                slice_end: row.get(2)?,
                state: RunState::parse(&row.get::<_, String>(3)?).unwrap_or(RunState::Failed),
                reason: row.get(4)?,
                rows_read: row.get::<_, i64>(5)? as u64,
                attributed: row.get::<_, i64>(6)? as u64,
                partial_statements: row.get::<_, i64>(7)? as u64,
                unavailable_statements: row.get::<_, i64>(8)? as u64,
                duration_ms: row.get::<_, i64>(9)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn templates(&self, target: &str) -> Result<Vec<StoredTemplate>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT template_key, kind, identity, catalog, database, template, first_seen, last_seen FROM templates WHERE target = ?1 ORDER BY template_key",
        )?;
        let rows = statement.query_map(params![target], |row| {
            Ok(StoredTemplate {
                template: HistoryTemplate {
                    template_key: row.get(0)?,
                    kind: if row.get::<_, String>(1)? == "official_digest" {
                        HistoryTemplateKind::OfficialDigest
                    } else {
                        HistoryTemplateKind::ProductFingerprint
                    },
                    identity: row.get(2)?,
                    catalog: row.get(3)?,
                    database: row.get(4)?,
                    template: row.get(5)?,
                },
                first_seen: row.get(6)?,
                last_seen: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Buckets of one target in `[from, to)`, optionally for one template.
    pub fn buckets(
        &self,
        target: &str,
        from: i64,
        to: i64,
        template_key: Option<&str>,
    ) -> Result<Vec<StoredBucket>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT b.executions, b.errors, b.qt_valid, b.qt_sum, b.qt_max, b.qt_hist, b.scan_valid, b.scan_sum, b.scan_max, b.scan_hist, b.cpu_valid, b.cpu_sum, b.cpu_max, b.cpu_hist, b.mem_valid, b.mem_sum, b.mem_max, b.mem_hist,
                    b.template_key, b.bucket_start, b.engine_version, b.rule_version
             FROM buckets b JOIN templates t ON t.template_key = b.template_key
             WHERE t.target = ?1 AND b.bucket_start >= ?2 AND b.bucket_start < ?3 AND (?4 IS NULL OR b.template_key = ?4)
             ORDER BY b.template_key, b.bucket_start",
        )?;
        let rows = statement.query_map(params![target, from, to, template_key], |row| {
            let key: String = row.get(18)?;
            let start: i64 = row.get(19)?;
            let engine: String = row.get(20)?;
            let rule: String = row.get(21)?;
            Ok(bucket_from_row(row, key, start, &engine, &rule, 0))
        })?;
        rows.map(|row| row.map_err(HistoryError::from).and_then(|bucket| bucket))
            .collect()
    }

    /// Hourly totals of one target in `[from, to)`, ascending by hour then
    /// version pair. Reads only the additive columns of each template row.
    pub fn hourly_totals(&self, target: &str, from: i64, to: i64) -> Result<Vec<HourlyTotal>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT b.bucket_start, b.engine_version, b.rule_version, b.executions, b.errors, b.qt_valid, b.qt_sum
             FROM buckets b JOIN templates t ON t.template_key = b.template_key
             WHERE t.target = ?1 AND b.bucket_start >= ?2 AND b.bucket_start < ?3",
        )?;
        let rows = statement.query_map(params![target, from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let invalid = || HistoryError::Internal("corrupt_hourly_total");
        let mut totals: std::collections::BTreeMap<(i64, String, String), HourlyTotal> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (start, engine, rule, executions, errors, valid, sum) = row?;
            let count = |value: i64| u64::try_from(value).map_err(|_| invalid());
            let sum = sum.parse::<u128>().map_err(|_| invalid())?;
            let entry = totals
                .entry((start, engine.clone(), rule.clone()))
                .or_insert_with(|| HourlyTotal {
                    bucket_start: start,
                    engine_version: engine,
                    rule_version: rule,
                    executions: 0,
                    errors: 0,
                    query_time_valid: 0,
                    query_time_sum: 0,
                });
            entry.executions = entry
                .executions
                .checked_add(count(executions)?)
                .ok_or_else(invalid)?;
            entry.errors = entry
                .errors
                .checked_add(count(errors)?)
                .ok_or_else(invalid)?;
            entry.query_time_valid = entry
                .query_time_valid
                .checked_add(count(valid)?)
                .ok_or_else(invalid)?;
            entry.query_time_sum = entry.query_time_sum.checked_add(sum).ok_or_else(invalid)?;
        }
        Ok(totals.into_values().collect())
    }

    /// Targets with any recorded state, most recently updated first.
    pub fn targets(&self) -> Result<Vec<String>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT target FROM targets ORDER BY updated_at DESC, target")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| row.map_err(HistoryError::from)).collect()
    }

    /// The slowest kept executions of one template in `[from, to)`.
    pub fn exemplars(
        &self,
        template_key: &str,
        from: i64,
        to: i64,
        limit: u32,
    ) -> Result<Vec<StoredExemplar>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT query_id, query_time, event_time FROM exemplars WHERE template_key = ?1 AND bucket_start >= ?2 AND bucket_start < ?3
             ORDER BY query_time DESC, query_id LIMIT ?4",
        )?;
        let rows =
            statement.query_map(params![template_key, from, to, i64::from(limit)], |row| {
                Ok(StoredExemplar {
                    query_id: row.get(0)?,
                    query_time: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    event_time: row.get(2)?,
                })
            })?;
        rows.map(|row| row.map_err(HistoryError::from)).collect()
    }

    /// Retention: drops buckets, runs and templates last seen before `cutoff`.
    pub fn purge_before(&self, cutoff: i64) -> Result<u64> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let mut removed = tx.execute(
            "DELETE FROM buckets WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM exemplars WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        // Governance records stay: they are the user's own log, not samples.
        removed += tx.execute(
            "DELETE FROM alerts WHERE state = 'resolved' AND resolved_at < ?1",
            params![cutoff],
        )? as u64;
        tx.execute(governance::PURGE_ALERT_HANDOFFS, [])?;
        removed += tx.execute(
            "DELETE FROM audit_slice_deltas WHERE bucket_start < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM collection_slice_evidence WHERE slice_end < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute("DELETE FROM runs WHERE slice_end < ?1", params![cutoff])? as u64;
        removed += tx.execute(
            "DELETE FROM notify_deliveries WHERE created_at < ?1 AND state <> 'pending'",
            params![cutoff],
        )? as u64;
        removed += Self::purge_loads_before(&tx, cutoff)?;
        removed += tx.execute(load_alerts::PURGE_LOAD_ALERTS, params![cutoff])? as u64;
        removed += tx.execute(load_alerts::PURGE_SNAPSHOT_LAST, params![cutoff])? as u64;
        removed += tx.execute(
            "DELETE FROM templates WHERE last_seen < ?1 AND NOT EXISTS (SELECT 1 FROM buckets b WHERE b.template_key = templates.template_key)",
            params![cutoff],
        )? as u64;
        tx.commit()?;
        Ok(removed)
    }

    /// Everything goes, watermarks, alerts, governance records, the
    /// delivery log, load history, saved error log text and the saved
    /// StarRocks connection (task 72) included; notification channels and targets stay.
    /// The next run starts fresh.
    pub fn clear(&self) -> Result<()> {
        let connection = self.lock();
        connection.execute_batch(loads::CLEAR_LOADS)?;
        connection.execute_batch(load_alerts::CLEAR_LOAD_ALERTS)?;
        connection.execute_batch(saved_connection::CLEAR_SAVED_CONNECTION)?;
        connection.execute_batch(
            &format!("DELETE FROM notify_deliveries; DELETE FROM governance_events; DELETE FROM governance_items; DELETE FROM alerts; {}; DELETE FROM audit_slice_deltas; DELETE FROM collection_slice_evidence; DELETE FROM exemplars; DELETE FROM buckets; DELETE FROM runs; DELETE FROM templates; DELETE FROM targets;", governance::PURGE_ALERT_HANDOFFS),
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<StoreStats> {
        let connection = self.lock();
        let count = |table: &str| -> Result<u64> {
            Ok(
                connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })? as u64,
            )
        };
        Ok(StoreStats {
            templates: count("templates")?,
            buckets: count("buckets")?,
            runs: count("runs")?,
            file_bytes: self
                .path
                .as_ref()
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|meta| meta.len()),
        })
    }

    /// Flushes the WAL into the main file (for size reporting and file scans).
    pub fn checkpoint(&self) -> Result<()> {
        self.lock()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}

fn bucket_from_row(
    row: &rusqlite::Row<'_>,
    template_key: String,
    bucket_start: i64,
    engine_version: &str,
    rule_version: &str,
    offset: usize,
) -> Result<StoredBucket> {
    let get_i = |index: usize| {
        row.get::<_, i64>(offset + index)
            .map_err(HistoryError::from)
    };
    let get_s = |index: usize| {
        row.get::<_, String>(offset + index)
            .map_err(HistoryError::from)
    };
    let get_o = |index: usize| {
        row.get::<_, Option<String>>(offset + index)
            .map_err(HistoryError::from)
    };
    let get_b = |index: usize| {
        row.get::<_, Vec<u8>>(offset + index)
            .map_err(HistoryError::from)
    };
    let metric = |base: usize| -> Result<HistoryMetric> {
        decode_metric(
            get_i(base)?,
            get_s(base + 1)?,
            get_o(base + 2)?,
            get_b(base + 3)?,
        )
    };
    Ok(StoredBucket {
        engine_version: engine_version.to_owned(),
        rule_version: rule_version.to_owned(),
        delta: HistoryBucketDelta {
            template_key: template_key.clone(),
            bucket_start,
            executions: get_i(0)? as u64,
            errors: get_i(1)? as u64,
            query_time: metric(2)?,
            scan_bytes: metric(6)?,
            cpu_cost: metric(10)?,
            memory_cost: metric(14)?,
            // Kept in their own table; read with `exemplars`.
            exemplars: Vec::new(),
        },
        template_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::{AuditMetricId, AuditMetricInput};
    use doctor_core::{AuditFingerprintStatement, CollectedExecution, aggregate_collection};

    #[test]
    fn hourly_totals_add_templates_and_keep_version_pairs_apart() {
        let store = HistoryStore::open_in_memory().unwrap();
        {
            let connection = store.lock();
            connection
                .execute_batch(
                    "INSERT INTO templates VALUES ('a','t','digest','x','c','d',NULL,0,0),('b','t','digest','y','c','d',NULL,0,0),('z','other','digest','w','c','d',NULL,0,0);",
                )
                .unwrap();
            let insert = "INSERT INTO buckets VALUES (?1,?2,?3,'r1',?4,?5,?6,?7,NULL,x'',0,'0',NULL,x'',0,'0',NULL,x'',0,'0',NULL,x'')";
            for (key, start, engine, executions, errors, valid, sum) in [
                ("a", 3600, "3.5.14", 3, 1, 3, "18446744073709551615"),
                ("b", 3600, "3.5.14", 2, 0, 1, "5"),
                ("a", 3600, "3.5.13", 1, 0, 1, "7"),
                ("a", 7200, "3.5.14", 4, 0, 4, "40"),
                ("z", 3600, "3.5.14", 9, 9, 9, "9"),
            ] {
                connection
                    .execute(
                        insert,
                        params![key, start, engine, executions, errors, valid, sum],
                    )
                    .unwrap();
            }
        }
        let totals = store.hourly_totals("t", 0, 7200).unwrap();
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].engine_version, "3.5.13");
        assert_eq!(totals[1].executions, 5);
        assert_eq!(totals[1].errors, 1);
        assert_eq!(totals[1].query_time_valid, 4);
        // Beyond u64: the decimal sums are added exactly.
        assert_eq!(totals[1].query_time_sum, 18_446_744_073_709_551_620);
        assert!(store.hourly_totals("t", 7200, 10800).unwrap()[0].bucket_start == 7200);
    }

    #[test]
    fn audit_v1_10_partial_unique_index_guards_the_database_itself() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = store
            .create_item("t", "k", "first", "manual", None, 1)
            .unwrap()
            .unwrap();
        let connection = store.lock();
        let sql = "INSERT INTO governance_items (target,template_key,title,origin,state,created_at,updated_at) VALUES (?1,'k','other','manual',?2,2,2)";
        assert!(connection.execute(sql, params!["t", "triaged"]).is_err());
        assert!(
            connection
                .execute(sql, params!["other-target", "triaged"])
                .is_ok()
        );
        connection.execute(sql, params!["t", "wont_fix"]).unwrap();
        let terminal = connection.last_insert_rowid();
        assert!(
            connection
                .execute(
                    "UPDATE governance_items SET state='in_progress' WHERE id=?1",
                    [terminal]
                )
                .is_err()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM governance_items WHERE id=?1",
                    [id],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "new"
        );
    }

    #[test]
    fn audit_v1_10_legacy_duplicates_fail_migration_without_retiring_user_items() {
        let dir =
            std::env::temp_dir().join(format!("doctor-duplicate-items-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(MIGRATION_1).unwrap();
            connection.execute_batch(MIGRATION_2).unwrap();
            connection.execute_batch(governance::MIGRATION_3).unwrap();
            connection
                .execute("INSERT INTO meta VALUES ('schema_version','3')", [])
                .unwrap();
            connection.execute_batch("INSERT INTO governance_items (target,template_key,title,origin,state,created_at,updated_at) VALUES ('t','k','first','manual','new',1,1),('t','k','second','manual','triaged',2,2)").unwrap();
        }
        let error = match HistoryStore::open(&path) {
            Ok(_) => panic!("must refuse duplicate active items"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("manual_resolution_required"));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM governance_items WHERE state IN ('new','triaged')",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "3"
        );
        drop(connection);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn run(start: i64, end: i64, state: RunState) -> RunRecord {
        RunRecord {
            started_at: end + 60,
            slice_start: start,
            slice_end: end,
            state,
            reason: None,
            rows_read: 2,
            attributed: 2,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 5,
        }
    }

    fn rows(literal: &str, epoch: i64) -> Vec<CollectedExecution> {
        ["1", "2"]
            .iter()
            .map(|value| CollectedExecution {
                query_id: None,
                catalog: Some("default_catalog".into()),
                database: Some("demo".into()),
                native_digest: None,
                statement: AuditFingerprintStatement::Text {
                    value: format!("SELECT id FROM t WHERE name = '{literal}' AND id = {value}"),
                    truncated: false,
                },
                error: false,
                event_epoch_seconds: Some(epoch),
                metrics: vec![(
                    AuditMetricId::QueryTime,
                    AuditMetricInput::from_sql_cell(Some(format!("{value}0"))),
                )],
            })
            .collect()
    }

    #[test]
    fn audit_replay_replaces_effective_coverage_atomically() {
        let store = HistoryStore::open_in_memory().unwrap();
        let mut executions = rows("x", 3_601);
        executions.truncate(1);
        let query_id = "00000000-0000-0000-0000-000000000009";
        executions[0].query_id = Some(query_id.into());
        let id = audit_identity(query_id);
        let commit = |mode, state, reasons, observed: &[[u8; 32]], excluded: &[[u8; 32]]| {
            let evidence = SliceEvidence {
                read_range: Some((3_600, 3_900)),
                excluded_count: Some(excluded.len() as u64),
                reasons,
            };
            store
                .record_collected_audit_slice(
                    "t1",
                    mode,
                    AuditSliceCommit {
                        run: &run(3_600, 3_900, state),
                        evidence: &evidence,
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows: if observed.is_empty() {
                            Vec::new()
                        } else {
                            executions.clone()
                        },
                        excluded_ids: &excluded
                            .iter()
                            .map(|&id| (id, (false, false)))
                            .collect::<Vec<_>>(),
                        affected_neighbours: &[],
                    },
                )
                .unwrap()
        };
        assert!(commit(
            AuditSliceWrite::Forward {
                expected_watermark: None
            },
            RunState::Partial,
            vec!["first_look_unproven"],
            &[id],
            &[]
        ));
        assert!(commit(
            AuditSliceWrite::Replay {
                expected_revision: 1
            },
            RunState::Complete,
            vec![],
            &[id],
            &[]
        ));
        assert_eq!(
            store.runs("t1", 3_600, 3_900, 10).unwrap()[0].state,
            RunState::Complete
        );
        assert!(commit(
            AuditSliceWrite::Replay {
                expected_revision: 2
            },
            RunState::Partial,
            vec!["conflicting_query_id"],
            &[],
            &[id]
        ));
        assert!(!commit(
            AuditSliceWrite::Replay {
                expected_revision: 2
            },
            RunState::Complete,
            vec![],
            &[id],
            &[]
        ));
        let runs = store.runs("t1", 3_600, 3_900, 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Partial);
        assert_eq!(runs[0].reason.as_deref(), Some("conflicting_query_id"));
        assert!(store.buckets("t1", 3_600, 7_200, None).unwrap().is_empty());
        let (revision, excluded): (u64, String) = store.lock().query_row(
            "SELECT revision, excluded_count FROM collection_slice_evidence WHERE source = 'audit'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!((revision, excluded.as_str()), (3, "1"));
        assert_eq!(store.target_state("t1").unwrap().watermark, Some(3_900));
    }

    #[test]
    fn audit_adjacent_commit_keeps_the_stored_execution_once() {
        for (start, replay_earlier) in [(3_300, false), (3_600, false), (3_300, true)] {
            let store = HistoryStore::open_in_memory().unwrap();
            let mut first = rows("x", start + 1).remove(0);
            let id = "00000000-0000-0000-0000-000000000001";
            first.query_id = Some(id.into());
            let mut moved = first.clone();
            moved.event_epoch_seconds = Some(start + 301);
            moved.metrics[0].1 = AuditMetricInput::from_sql_cell(Some("999".into()));
            let commit = |slice_start, mode, rows| {
                let end = slice_start + 300;
                store
                    .record_collected_audit_slice(
                        "t1",
                        mode,
                        AuditSliceCommit {
                            run: &run(slice_start, end, RunState::Partial),
                            evidence: &SliceEvidence {
                                read_range: Some((slice_start, end)),
                                excluded_count: Some(0),
                                reasons: Vec::new(),
                            },
                            engine_version: "3.5.14",
                            rule_version: "v1",
                            source_identity: "s1",
                            rows,
                            excluded_ids: &[],
                            affected_neighbours: &[],
                        },
                    )
                    .unwrap()
            };
            let forward = |expected_watermark| AuditSliceWrite::Forward { expected_watermark };
            let first_read = if replay_earlier {
                vec![]
            } else {
                vec![first.clone()]
            };
            assert!(commit(start, forward(None), first_read));
            assert!(commit(start + 300, forward(Some(start + 300)), vec![moved]));
            if replay_earlier {
                assert!(commit(
                    start,
                    AuditSliceWrite::Replay {
                        expected_revision: 1
                    },
                    vec![first]
                ));
            }

            let buckets = store.buckets("t1", 0, 7_200, None).unwrap();
            assert_eq!(buckets.len(), 1);
            let bucket = &buckets[0].delta;
            let expected = if replay_earlier {
                (3_600, 999, (1, 0))
            } else {
                (start.div_euclid(3_600) * 3_600, 10, (0, 1))
            };
            assert_eq!(bucket.bucket_start, expected.0);
            assert_eq!(bucket.executions, 1);
            assert_eq!(
                (bucket.query_time.valid_count, bucket.query_time.max),
                (1, Some(expected.1))
            );
            assert_eq!(bucket.query_time.histogram.count(), 1);
            let exemplars = store.exemplars(&bucket.template_key, 0, 7_200, 10).unwrap();
            assert_eq!(exemplars.len(), 1);
            assert_eq!(exemplars[0].query_id, id);
            assert_eq!(
                exemplars[0].event_time,
                if replay_earlier {
                    start + 301
                } else {
                    start + 1
                }
            );
            let runs = store.runs("t1", start, start + 600, 10).unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!((runs[0].attributed, runs[1].attributed), expected.2);
            assert!(runs.iter().all(|run| run.state == RunState::Partial
                && run.reason.as_deref() == Some("conflicting_query_id")));
        }
    }

    #[test]
    fn audit_new_conflict_requeues_an_already_partial_earliest_slice_once() {
        let store = HistoryStore::open_in_memory().unwrap();
        let mut execution = rows("y", 3_601).remove(0);
        let y = "00000000-0000-0000-0000-000000000002";
        execution.query_id = Some(y.into());
        let x = audit_identity("00000000-0000-0000-0000-000000000001");
        let y = audit_identity(y);
        let commit = |start, mode, rows, excluded_ids: &[([u8; 32], (bool, bool))]| {
            let end = start + 300;
            let mut run = run(start, end, RunState::Partial);
            run.started_at = 6_000;
            store
                .record_collected_audit_slice(
                    "t1",
                    mode,
                    AuditSliceCommit {
                        run: &run,
                        evidence: &SliceEvidence {
                            read_range: Some((start, end)),
                            excluded_count: Some(
                                excluded_ids
                                    .iter()
                                    .filter(|(_, (previous, _))| !*previous)
                                    .count() as u64,
                            ),
                            reasons: if excluded_ids.is_empty() {
                                vec![]
                            } else {
                                vec!["conflicting_query_id"]
                            },
                        },
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows,
                        excluded_ids,
                        affected_neighbours: &[],
                    },
                )
                .unwrap()
        };
        let forward = |expected_watermark| AuditSliceWrite::Forward { expected_watermark };
        let replay = |expected_revision| AuditSliceWrite::Replay { expected_revision };
        assert!(commit(3_600, forward(None), vec![execution.clone()], &[]));
        assert!(commit(3_900, forward(Some(3_900)), vec![], &[]));
        assert!(commit(
            3_600,
            replay(1),
            vec![execution],
            &[(x, (false, false))]
        ));
        assert!(commit(3_900, replay(1), vec![], &[(y, (true, false))]));
        assert!(commit(3_900, replay(2), vec![], &[(y, (true, false))]));

        let candidate = store
            .audit_replay_candidate("t1", 2_400, 4_200)
            .unwrap()
            .unwrap();
        assert_eq!(
            (candidate.start, candidate.end, candidate.revision),
            (3_600, 3_900, 3)
        );
        let exclusions = |start| {
            store
                .lock()
                .query_row(
                    "SELECT excluded_count FROM collection_slice_evidence
             WHERE source = 'audit' AND target = 't1' AND slice_start = ?1",
                    [start],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        assert_eq!(exclusions(3_600), "2");
        assert_eq!(exclusions(3_900), "0");
        assert!(commit(
            3_600,
            replay(candidate.revision),
            vec![],
            &[(x, (false, false)), (y, (false, true)),]
        ));
        assert_eq!(exclusions(3_600), "2");
        assert!(store.buckets("t1", 3_600, 7_200, None).unwrap().is_empty());
    }

    #[test]
    fn audit_prunes_identities_only_after_forward_and_replay_neighbours_expire() {
        for (earliest_end, replayable) in [(3_900, true), (10_800, false)] {
            let store = HistoryStore::open_in_memory().unwrap();
            let now = earliest_end + AUDIT_REPLAY_MAX_AGE_SECONDS;
            let mut execution = rows("x", 3_301).remove(0);
            execution.query_id = Some("00000000-0000-0000-0000-000000000001".into());
            let commit = |start, mode, epoch| {
                let end = start + 300;
                let mut run = run(start, end, RunState::Complete);
                run.started_at = now;
                let mut execution = execution.clone();
                execution.event_epoch_seconds = Some(epoch);
                store
                    .record_collected_audit_slice(
                        "t1",
                        mode,
                        AuditSliceCommit {
                            run: &run,
                            evidence: &SliceEvidence {
                                read_range: Some((start, end)),
                                excluded_count: Some(0),
                                reasons: vec![],
                            },
                            engine_version: "3.5.14",
                            rule_version: "v1",
                            source_identity: "s1",
                            rows: vec![execution],
                            excluded_ids: &[],
                            affected_neighbours: &[],
                        },
                    )
                    .unwrap()
            };
            let has_identity = || -> bool {
                store
                    .lock()
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM audit_slice_deltas
                     WHERE target = 't1' AND slice_end = 3600 AND length(observed_ids) > 0)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap()
            };
            let forward = |expected_watermark| AuditSliceWrite::Forward { expected_watermark };

            assert!(commit(3_300, forward(None), 3_301));
            let latest_end = now - 1_800;
            assert!(
                store
                    .audit_replay_candidate("t1", earliest_end, latest_end)
                    .unwrap()
                    .is_none()
            );
            assert!(has_identity());
            assert!(commit(3_600, forward(Some(3_600)), 3_601));
            assert_eq!(has_identity(), replayable);
            if replayable {
                let candidate = store
                    .audit_replay_candidate("t1", earliest_end, latest_end)
                    .unwrap()
                    .unwrap();
                assert_eq!((candidate.start, candidate.end), (3_600, 3_900));
                assert!(commit(
                    3_600,
                    AuditSliceWrite::Replay {
                        expected_revision: candidate.revision,
                    },
                    3_601
                ));
                assert!(
                    store
                        .audit_replay_candidate("t1", earliest_end + 1, latest_end)
                        .unwrap()
                        .is_none()
                );
                assert!(!has_identity());
            }
            let buckets = store.buckets("t1", 0, 7_200, None).unwrap();
            assert_eq!(buckets.len(), 1);
            assert_eq!(buckets[0].delta.bucket_start, 0);
            assert_eq!(buckets[0].delta.executions, 1);
        }
    }

    #[test]
    fn audit_limited_summary_marks_neighbour_without_requeuing_it() {
        let store = HistoryStore::open_in_memory().unwrap();
        let commit = |start, mode, limited: bool| {
            let end = start + 300;
            let mut run = run(start, end, RunState::Complete);
            run.started_at = 6_000;
            store
                .record_collected_audit_slice(
                    "t1",
                    mode,
                    AuditSliceCommit {
                        run: &run,
                        evidence: &SliceEvidence {
                            read_range: (!limited).then_some((start, end)),
                            excluded_count: Some(0),
                            reasons: if limited {
                                vec!["row_limit", "conflicting_query_id"]
                            } else {
                                vec![]
                            },
                        },
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows: vec![],
                        excluded_ids: &[],
                        affected_neighbours: if limited { &[(3_600, 3_900)] } else { &[] },
                    },
                )
                .unwrap()
        };
        let neighbour = || store.runs("t1", 3_600, 3_900, 10).unwrap().remove(0);
        assert!(commit(
            3_600,
            AuditSliceWrite::Forward {
                expected_watermark: None
            },
            false
        ));
        assert!(commit(
            3_600,
            AuditSliceWrite::Replay {
                expected_revision: 1
            },
            false
        ));
        assert_eq!(neighbour().state, RunState::Complete);

        assert!(commit(
            3_900,
            AuditSliceWrite::Forward {
                expected_watermark: Some(3_900)
            },
            true
        ));
        assert_eq!(neighbour().state, RunState::Partial);
        assert_eq!(neighbour().reason.as_deref(), Some("conflicting_query_id"));
        assert!(
            store
                .audit_replay_candidate("t1", 2_400, 3_900)
                .unwrap()
                .is_none()
        );
        assert!(commit(
            3_600,
            AuditSliceWrite::Replay {
                expected_revision: 3
            },
            false
        ));
        assert_eq!(neighbour().state, RunState::Partial);
        assert_eq!(neighbour().reason.as_deref(), Some("conflicting_query_id"));
    }

    #[test]
    fn audit_known_conflict_moves_exclusion_to_earlier_slice() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = audit_identity("00000000-0000-0000-0000-000000000001");
        let other_id = audit_identity("00000000-0000-0000-0000-000000000002");
        let commit = |start, mode, excluded_ids: &[([u8; 32], (bool, bool))], limited: bool| {
            let end = start + 300;
            let mut run = run(start, end, RunState::Complete);
            run.started_at = 6_000;
            let mut reasons = Vec::new();
            if limited {
                reasons.push("row_limit");
            }
            if !excluded_ids.is_empty() {
                reasons.push("conflicting_query_id");
            }
            store
                .record_collected_audit_slice(
                    "t1",
                    mode,
                    AuditSliceCommit {
                        run: &run,
                        evidence: &SliceEvidence {
                            read_range: (!limited).then_some((start, end)),
                            excluded_count: Some(excluded_ids.len() as u64),
                            reasons,
                        },
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows: vec![],
                        excluded_ids,
                        affected_neighbours: &[],
                    },
                )
                .unwrap()
        };
        let exclusions = |start| {
            store
                .lock()
                .query_row(
                    "SELECT excluded_count FROM collection_slice_evidence
                 WHERE source = 'audit' AND target = 't1' AND slice_start = ?1",
                    [start],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        let forward = |expected_watermark| AuditSliceWrite::Forward { expected_watermark };
        let replay = |expected_revision| AuditSliceWrite::Replay { expected_revision };

        assert!(commit(3_600, forward(None), &[], false));
        let local_conflicts = [(id, (false, false)), (other_id, (false, false))];
        assert!(commit(3_900, forward(Some(3_900)), &local_conflicts, false));
        assert_eq!(exclusions(3_600), "0");
        assert_eq!(exclusions(3_900), "2");

        assert!(commit(3_600, replay(1), &[(id, (false, true))], false));
        assert_eq!(exclusions(3_600), "1");
        assert_eq!(exclusions(3_900), "1");
        // A later limited read still sees the ID as locally owned. The
        // details identify it, so it must not reclaim the exclusion count.
        assert!(commit(3_900, replay(2), &local_conflicts, true));
        assert_eq!(exclusions(3_600), "1");
        assert_eq!(exclusions(3_900), "1");
        let runs = store.runs("t1", 3_600, 4_200, 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|run| {
            run.state == RunState::Partial
                && run
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("conflicting_query_id")
        }));
    }

    #[test]
    fn audit_unidentified_conflict_transfer_keeps_exclusions_unknown_until_full_replay() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = audit_identity("00000000-0000-0000-0000-000000000001");
        let commit = |start,
                      mode,
                      count,
                      excluded_ids: &[([u8; 32], (bool, bool))],
                      affected_neighbours: &[(i64, i64)]| {
            let end = start + 300;
            let mut run = run(start, end, RunState::Complete);
            run.started_at = 6_000;
            store
                .record_collected_audit_slice(
                    "t1",
                    mode,
                    AuditSliceCommit {
                        run: &run,
                        evidence: &SliceEvidence {
                            read_range: affected_neighbours.is_empty().then_some((start, end)),
                            excluded_count: count,
                            reasons: if !affected_neighbours.is_empty() {
                                vec!["row_limit", "conflicting_query_id"]
                            } else if !excluded_ids.is_empty() {
                                vec!["conflicting_query_id"]
                            } else {
                                vec![]
                            },
                        },
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows: vec![],
                        excluded_ids,
                        affected_neighbours,
                    },
                )
                .unwrap()
        };
        let exclusions = || {
            let connection = store.lock();
            let mut statement = connection
                .prepare(
                    "SELECT excluded_count FROM collection_slice_evidence
                     WHERE source = 'audit' AND target = 't1' ORDER BY slice_start",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, Option<String>>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        let forward = |expected_watermark| AuditSliceWrite::Forward { expected_watermark };
        let replay = |expected_revision| AuditSliceWrite::Replay { expected_revision };

        assert!(commit(3_600, forward(None), Some(0), &[], &[]));
        assert!(commit(
            3_900,
            forward(Some(3_900)),
            Some(1),
            &[(id, (false, false))],
            &[]
        ));
        assert_eq!(exclusions(), [Some("0".into()), Some("1".into())]);

        // A late copy in the earlier slice takes ownership, but neither capped
        // read includes its identity. The later slice cannot keep its old 1.
        assert!(commit(3_600, replay(1), None, &[], &[(3_900, 4_200)]));
        assert_eq!(exclusions(), [None, None]);
        assert!(commit(3_900, replay(2), None, &[], &[(3_600, 3_900)]));
        assert_eq!(exclusions(), [None, None]);

        assert!(commit(
            3_900,
            replay(3),
            Some(0),
            &[(id, (true, false))],
            &[]
        ));
        assert_eq!(exclusions(), [None, Some("0".into())]);
        assert!(commit(
            3_600,
            replay(3),
            Some(1),
            &[(id, (false, true))],
            &[]
        ));
        assert_eq!(exclusions(), [Some("1".into()), Some("0".into())]);
    }

    #[test]
    fn schema_v9_preserves_history_and_downgrades_unproven_coverage() {
        let store = HistoryStore::open_in_memory().unwrap();
        let (templates, buckets, _) = aggregate_collection("t1", &rows("x", 3_601));
        let first = run(3_600, 3_900, RunState::Complete);
        store
            .record_slice("t1", "3.5.14", "v1", &first, 3_900, &templates, &buckets)
            .unwrap();
        let load = run(0, 3_600, RunState::Complete);
        store
            .record_load_slice("t1", "3.5.14", "v1", &load, 3_600, &[], &[])
            .unwrap();
        let key = &templates[0].template_key;
        let item = store
            .create_item("t1", key, "old item", "manual", None, 3_601)
            .unwrap()
            .unwrap();
        let alert = {
            let connection = store.lock();
            connection.execute(
                "INSERT INTO alerts (target, template_key, state, first_seen, last_seen, severity,
                 confidence, evidence, workload, bucket_start, engine_version, rule_version)
                 VALUES ('t1', ?1, 'open', 3601, 3601, 'high', 'high', '{}', '{}', 3600, '3.5.14', 'v1')",
                [key],
            ).unwrap();
            let alert = connection.last_insert_rowid();
            connection
                .execute_batch(
                    "DROP TABLE audit_slice_deltas; DROP TABLE collection_slice_evidence;
                 UPDATE meta SET value = '9' WHERE key = 'schema_version';",
                )
                .unwrap();
            alert
        };
        store.migrate().unwrap();
        assert_eq!(store.target_state("t1").unwrap().watermark, Some(3_900));
        assert_eq!(
            store.load_target_state("t1").unwrap().watermark,
            Some(3_600)
        );
        assert_eq!(store.runs("t1", 3_600, 3_900, 10).unwrap(), [first]);
        assert_eq!(
            store.load_runs("t1", 0, 3_600).unwrap()[0].state,
            RunState::Complete
        );
        assert_eq!(store.items(Some("t1")).unwrap()[0].id, item);
        assert_eq!(store.alerts("t1", 10).unwrap()[0].id, alert);
        assert_eq!(
            store.buckets("t1", 3_600, 7_200, None).unwrap()[0]
                .delta
                .executions,
            2
        );
        assert!(
            store
                .audit_replay_candidate("t1", 3_600, 3_900)
                .unwrap()
                .is_none()
        );
        let (version, legacy_read, legacy_excluded): (String, Option<i64>, Option<String>) = store
            .lock()
            .query_row(
                "SELECT m.value, e.read_start, e.excluded_count FROM meta m
                 JOIN runs r ON r.target = 't1'
                 LEFT JOIN collection_slice_evidence e ON e.source = 'audit' AND e.run_id = r.id
                 WHERE m.key = 'schema_version'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (version.as_str(), legacy_read, legacy_excluded),
            ("10", None, None)
        );

        let next = run(3_900, 4_200, RunState::Partial);
        let write = || {
            store
                .record_collected_audit_slice(
                    "t1",
                    AuditSliceWrite::Forward {
                        expected_watermark: Some(3_900),
                    },
                    AuditSliceCommit {
                        run: &next,
                        evidence: &SliceEvidence {
                            read_range: Some((3_900, 4_200)),
                            excluded_count: Some(0),
                            reasons: vec!["first_look_unproven"],
                        },
                        engine_version: "3.5.14",
                        rule_version: "v1",
                        source_identity: "s1",
                        rows: rows("y", 3_901),
                        excluded_ids: &[],
                        affected_neighbours: &[],
                    },
                )
                .unwrap()
        };
        assert!(write());
        assert!(!write());
        assert_eq!(
            store.buckets("t1", 3_600, 7_200, None).unwrap()[0]
                .delta
                .executions,
            4
        );
        assert!(
            store
                .audit_replay_candidate("t1", 3_600, 4_200)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn slices_merge_into_hourly_buckets_and_move_the_watermark() {
        let store = HistoryStore::open_in_memory().unwrap();
        let (templates, buckets, _) = aggregate_collection("t1", &rows("x", 3_600));
        store
            .record_slice(
                "t1",
                "3.5.14",
                "v1",
                &run(3_600, 3_900, RunState::Complete),
                3_900,
                &templates,
                &buckets,
            )
            .unwrap();
        let (templates, buckets, _) = aggregate_collection("t1", &rows("y", 4_000));
        store
            .record_slice(
                "t1",
                "3.5.14",
                "v1",
                &run(3_900, 4_200, RunState::Complete),
                4_200,
                &templates,
                &buckets,
            )
            .unwrap();
        assert_eq!(
            store.target_state("t1").unwrap(),
            TargetState {
                watermark: Some(4_200),
                paused: false
            }
        );
        let stored = store.buckets("t1", 0, 10_000, None).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].delta.executions, 4);
        assert_eq!(stored[0].delta.query_time.sum, 60);
        assert_eq!(stored[0].delta.query_time.histogram.count(), 4);
        // A different engine version is a different bucket, never merged.
        store
            .record_slice(
                "t1",
                "3.5.15",
                "v1",
                &run(4_200, 4_500, RunState::Complete),
                4_500,
                &templates,
                &buckets,
            )
            .unwrap();
        assert_eq!(store.buckets("t1", 0, 10_000, None).unwrap().len(), 2);
        // A second writer replaying a slice behind the watermark changes nothing.
        assert!(
            !store
                .record_slice(
                    "t1",
                    "3.5.14",
                    "v1",
                    &run(3_900, 4_200, RunState::Complete),
                    4_200,
                    &templates,
                    &buckets
                )
                .unwrap()
        );
        assert_eq!(
            store.buckets("t1", 0, 4_199, None).unwrap()[0]
                .delta
                .executions,
            4
        );
        assert_eq!(store.target_state("t1").unwrap().watermark, Some(4_500));
        store.mark_partial("t1", 3_600, 3_900, "late_rows").unwrap();
        let runs = store.runs("t1", 0, 10_000, 10).unwrap();
        assert_eq!(
            runs.iter().filter(|r| r.state == RunState::Partial).count(),
            1
        );
        store.set_paused("t1", true, 5_000).unwrap();
        assert!(store.target_state("t1").unwrap().paused);
        assert!(store.purge_before(4_000).unwrap() > 0);
        store.clear().unwrap();
        assert_eq!(store.stats().unwrap().templates, 0);
    }

    #[test]
    fn same_release_pack_upgrade_splits_one_hour_bucket() {
        let store = HistoryStore::open_in_memory().unwrap();
        let rule = doctor_core::history_rule_version();
        let active_rule = TrendBucket::active_storage_rule_version();
        let old_rule = format!("{rule}|vp=starrocks-3.5.14@0.1.0:old-manifest:old-deltas");
        let (templates, buckets, _) = aggregate_collection("t1", &rows("x", 3_600));
        for start in [3_600, 3_900, 4_200] {
            assert!(
                store
                    .record_slice(
                        "t1",
                        "3.5.14",
                        &rule,
                        &run(start, start + 300, RunState::Complete),
                        start + 300,
                        &templates,
                        &buckets,
                    )
                    .unwrap()
            );
            if start == 3_600 {
                store
                    .lock()
                    .execute(
                        "UPDATE buckets SET rule_version = ?1 WHERE rule_version = ?2",
                        params![old_rule, active_rule],
                    )
                    .unwrap();
            }
        }
        let buckets = store.buckets("t1", 3_600, 7_200, None).unwrap();
        let totals = store.hourly_totals("t1", 3_600, 7_200).unwrap();
        assert_eq!(buckets.len(), 2);
        assert_eq!(totals.len(), 2);
        for (rule, count) in [(&old_rule, 2), (&active_rule, 4)] {
            let bucket = buckets
                .iter()
                .find(|bucket| &bucket.rule_version == rule)
                .unwrap();
            let total = totals
                .iter()
                .find(|total| &total.rule_version == rule)
                .unwrap();
            assert_eq!(bucket.delta.executions, count);
            assert_eq!(bucket.delta.query_time.histogram.count(), count);
            assert_eq!(total.executions, count);
        }
        assert_eq!(store.target_state("t1").unwrap().watermark, Some(4_500));
    }

    #[test]
    fn the_database_file_never_holds_a_literal() {
        let dir = std::env::temp_dir().join(format!("doctor-history-test-{}", std::process::id()));
        let path = dir.join("history.sqlite");
        let _ = std::fs::remove_dir_all(&dir);
        let store = HistoryStore::open(&path).unwrap();
        let (templates, buckets, _) =
            aggregate_collection("t1", &rows("canary_literal_7731", 3_600));
        store
            .record_slice(
                "t1",
                "3.5.14",
                "v1",
                &run(3_600, 3_900, RunState::Complete),
                3_900,
                &templates,
                &buckets,
            )
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let mut bytes = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            bytes.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        assert!(
            !bytes
                .windows(19)
                .any(|window| window == b"canary_literal_7731")
        );
        assert!(bytes.windows(7).any(|window| window == b"name = "));
        let reopened = HistoryStore::open(&path).unwrap();
        assert_eq!(reopened.stats().unwrap().templates, 1);
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audit_v1_01_hint_literals_never_reach_sqlite_or_wal() {
        let dir = std::env::temp_dir().join(format!("doctor-hint-privacy-{}", std::process::id()));
        let path = dir.join("history.sqlite");
        let store = HistoryStore::open(&path).unwrap();
        let sentinel = "AUDIT_PRIVATE_SENTINEL";
        for (i, statement) in [
            format!("SELECT DISTINCT /*+ SET_VAR(resource_group = '{sentinel}') */ id FROM t WHERE x='ORDINARY_PRIVATE_SENTINEL'"),
            format!("INSERT /*+ SET_VAR(query_timeout=123, custom_variable='{sentinel}') */ INTO t VALUES ('ORDINARY_PRIVATE_SENTINEL')"),
            format!("INSERT /*+ SET_USER_VARIABLE(@a='{sentinel}') */ INTO t VALUES ('ORDINARY_PRIVATE_SENTINEL')"),
            format!("SELECT /*+ SET_VAR(resource_group='{sentinel}') */ id FROM t WHERE x='ORDINARY_PRIVATE_SENTINEL'"),
        ].into_iter().enumerate() {
            for digest in [None, Some("ab12".to_owned())] {
                let mut row = rows("unused", 3_600)[0].clone();
                row.native_digest = digest;
                row.statement = AuditFingerprintStatement::Text { value: statement.clone(), truncated: false };
                let target = format!("target-{i}-{}", row.native_digest.is_some());
                let (templates, buckets, _) = aggregate_collection(&target, &[row]);
                store.record_slice(&target, "3.5.14", "v1", &run(3_600, 3_900, RunState::Complete), 3_900, &templates, &buckets).unwrap();
            }
        }
        store.checkpoint().unwrap();
        // Scan the actual database and any journal/WAL while the connection is open.
        for entry in std::fs::read_dir(&dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            for secret in [sentinel, "ORDINARY_PRIVATE_SENTINEL"] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes()),
                    "{secret}"
                );
            }
        }
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_v1_01_legacy_templates_and_pinned_snapshots_are_scrubbed_on_open() {
        let dir =
            std::env::temp_dir().join(format!("doctor-legacy-privacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let secret = "AUDIT_LEGACY_PRIVATE_SENTINEL";
        let sql = format!("SELECT DISTINCT /*+ SET_VAR(resource_group='{secret}') */ id FROM t");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(MIGRATION_1).unwrap();
            connection.execute_batch(MIGRATION_2).unwrap();
            connection.execute_batch(governance::MIGRATION_3).unwrap();
            connection
                .execute("INSERT INTO meta VALUES ('schema_version', '3')", [])
                .unwrap();
            connection.execute("INSERT INTO templates VALUES ('k','t','official_digest','ab12','c','d',?1,1,1)", [&sql]).unwrap();
            let snapshot = serde_json::json!({"view":{"template":sql},"executions":20}).to_string();
            connection.execute("INSERT INTO governance_items (target,template_key,title,origin,state,created_at,updated_at,snapshot_json) VALUES ('t','k','test','manual','in_progress',1,1,?1)", [&snapshot]).unwrap();
        }
        let store = HistoryStore::open(&path).unwrap();
        assert!(store.templates("t").unwrap()[0].template.template.is_none());
        let json: Option<String> = store
            .lock()
            .query_row("SELECT snapshot_json FROM governance_items", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!json.unwrap().contains(secret));
        for entry in std::fs::read_dir(&dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
        }
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exemplars_keep_the_slowest_per_hour_and_go_with_retention_and_clear() {
        let store = HistoryStore::open_in_memory().unwrap();
        let with_ids = |epoch: i64, base: u32| -> Vec<CollectedExecution> {
            (0..4)
                .map(|n| CollectedExecution {
                    query_id: Some(format!("q{epoch}-{n}")),
                    ..rows("x", epoch)[0].clone()
                })
                .zip(0..)
                .map(|(mut row, n)| {
                    row.metrics = vec![(
                        AuditMetricId::QueryTime,
                        AuditMetricInput::from_sql_cell(Some((base + n).to_string())),
                    )];
                    row
                })
                .collect()
        };
        let (templates, buckets, _) = aggregate_collection("t1", &with_ids(3_600, 10));
        assert!(
            store
                .record_slice(
                    "t1",
                    "3.5.14",
                    "v1",
                    &run(3_600, 3_900, RunState::Complete),
                    3_900,
                    &templates,
                    &buckets
                )
                .unwrap()
        );
        let (templates, buckets, _) = aggregate_collection("t1", &with_ids(3_950, 100));
        assert!(
            store
                .record_slice(
                    "t1",
                    "3.5.14",
                    "v1",
                    &run(3_900, 4_200, RunState::Complete),
                    4_200,
                    &templates,
                    &buckets
                )
                .unwrap()
        );
        let key = &templates[0].template_key;
        let kept = store.exemplars(key, 0, 10_000, 10).unwrap();
        assert_eq!(
            kept.iter().map(|e| e.query_time).collect::<Vec<_>>(),
            vec![103, 102, 101]
        );
        assert_eq!(store.targets().unwrap(), vec!["t1".to_owned()]);
        store.purge_before(7_200).unwrap();
        assert!(store.exemplars(key, 0, 10_000, 10).unwrap().is_empty());
        store.clear().unwrap();
        assert!(store.targets().unwrap().is_empty());
    }

    #[test]
    fn a_stage_b_file_migrates_to_schema_2_in_place() {
        let dir =
            std::env::temp_dir().join(format!("doctor-history-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(MIGRATION_1).unwrap();
            connection
                .execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', '1')",
                    [],
                )
                .unwrap();
            connection
                .execute("INSERT INTO targets VALUES ('t1', 3600, 0, 1)", [])
                .unwrap();
        }
        let store = HistoryStore::open(&path).unwrap();
        assert_eq!(store.target_state("t1").unwrap().watermark, Some(3_600));
        assert!(store.exemplars("k", 0, 1, 1).unwrap().is_empty());
        let version: String = store
            .lock()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION.to_string());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
