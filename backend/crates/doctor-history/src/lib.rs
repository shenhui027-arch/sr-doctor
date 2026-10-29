//! The only embedded database (AGENTS §4, ADR-2): one SQLite file outside the
//! repository holding literal-free templates, hourly bucket aggregates, run
//! records, collection watermarks and (stage D) alert and governance records.
//! It never stores raw SQL literals, Profile text or StarRocks credentials.
use std::path::Path;
use std::sync::Mutex;

use doctor_core::{
    HISTORY_EXEMPLARS_PER_BUCKET, HistoryBucketDelta, HistoryMetric, HistoryTemplate,
    HistoryTemplateKind, LogHistogram,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

mod governance;
mod notify;
pub use governance::{
    AlertFacts, AlertUpsert, ItemUpdate, StoreChange, StoredAlert, StoredEvent, StoredItem,
};
pub use notify::{
    DeliveryAttempt, DeliveryFilter, NewDelivery, SecretUpdate, StoredChannel, StoredDelivery,
    StoredTarget, TargetWrite,
};

pub const SCHEMA_VERSION: i64 = 5;
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
        if version < SCHEMA_VERSION {
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
        for template in templates {
            let clean = template.template.as_deref().and_then(|text| {
                redacted_template(text, Some(&template.catalog), Some(&template.database))
            });
            tx.execute(
                "INSERT INTO templates (template_key, target, kind, identity, catalog, database, template, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(template_key) DO UPDATE SET template = COALESCE(templates.template, excluded.template), last_seen = MAX(templates.last_seen, excluded.last_seen)",
                params![template.template_key, target, template.kind.as_str(), template.identity, template.catalog, template.database, clean, run.slice_end],
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
        Self::insert_run(&tx, target, run)?;
        Self::set_watermark(&tx, target, watermark, run.started_at)?;
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
        removed += tx.execute("DELETE FROM runs WHERE slice_end < ?1", params![cutoff])? as u64;
        removed += tx.execute(
            "DELETE FROM notify_deliveries WHERE created_at < ?1 AND state <> 'pending'",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM templates WHERE last_seen < ?1 AND NOT EXISTS (SELECT 1 FROM buckets b WHERE b.template_key = templates.template_key)",
            params![cutoff],
        )? as u64;
        tx.commit()?;
        Ok(removed)
    }

    /// Everything goes, watermarks, alerts, governance records and the
    /// delivery log included; notification channels and targets stay.
    /// The next run starts fresh.
    pub fn clear(&self) -> Result<()> {
        self.lock().execute_batch(
            "DELETE FROM notify_deliveries; DELETE FROM governance_events; DELETE FROM governance_items; DELETE FROM alerts; DELETE FROM exemplars; DELETE FROM buckets; DELETE FROM runs; DELETE FROM templates; DELETE FROM targets;",
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
