//! Notification storage (schema 5): channel settings, targets and the
//! delivery log. Secrets arrive already sealed by the caller and are stored
//! only as ciphertext; this module never sees a plaintext secret. Delivery
//! rows keep alert facts for retries, never message bodies or provider text.
use doctor_contracts::{NotifyChannelConfig, NotifyChannelKind, NotifySettings};
use rusqlite::{OptionalExtension, Row, params};

use crate::{HistoryError, HistoryStore, Result};

pub(crate) const MIGRATION_5: &str = "
CREATE TABLE notify_settings (id INTEGER PRIMARY KEY CHECK (id = 1), settings_json TEXT NOT NULL);
CREATE TABLE notify_channels (
  kind TEXT PRIMARY KEY,
  enabled INTEGER NOT NULL,
  config_json TEXT NOT NULL,
  secret BLOB,
  updated_at INTEGER NOT NULL
);
CREATE TABLE notify_targets (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  min_severity TEXT NOT NULL,
  address TEXT,
  address_hint TEXT NOT NULL,
  secret BLOB,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX notify_targets_kind ON notify_targets (kind, name);
CREATE TABLE notify_deliveries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at INTEGER NOT NULL,
  kind TEXT NOT NULL,
  target_id INTEGER,
  target_name TEXT NOT NULL,
  event TEXT NOT NULL,
  cluster TEXT,
  alert_count INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  state TEXT NOT NULL,
  attempts INTEGER NOT NULL,
  next_attempt_at INTEGER,
  last_attempt_at INTEGER,
  error_class TEXT,
  provider_code TEXT,
  duration_ms INTEGER
);
CREATE INDEX notify_deliveries_due ON notify_deliveries (state, next_attempt_at);
CREATE INDEX notify_deliveries_target ON notify_deliveries (target_id, id);
";

/// A stored secret field: keep it, replace it with new ciphertext, or drop it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretUpdate {
    Keep,
    Set(Vec<u8>),
    Clear,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredChannel {
    pub kind: NotifyChannelKind,
    pub enabled: bool,
    pub config: NotifyChannelConfig,
    pub secret: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredTarget {
    pub id: i64,
    pub kind: NotifyChannelKind,
    pub name: String,
    pub enabled: bool,
    /// `medium` or `high`.
    pub min_severity: String,
    /// Email address; `None` for phones and bots (they are in `secret`).
    pub address: Option<String>,
    pub address_hint: String,
    /// Sealed JSON of the target's private fields.
    pub secret: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetWrite {
    pub kind: NotifyChannelKind,
    pub name: String,
    pub enabled: bool,
    pub min_severity: String,
    pub address: Option<String>,
    pub address_hint: String,
    pub secret: SecretUpdate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDelivery {
    pub kind: NotifyChannelKind,
    pub target_id: Option<i64>,
    pub target_name: String,
    /// `new_alerts`, `resolved` or `test`.
    pub event: String,
    pub cluster: Option<String>,
    pub alert_count: u32,
    pub payload_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredDelivery {
    pub id: i64,
    pub created_at: i64,
    pub kind: NotifyChannelKind,
    pub target_id: Option<i64>,
    pub target_name: String,
    pub event: String,
    pub cluster: Option<String>,
    pub alert_count: u32,
    pub payload_json: String,
    pub state: String,
    pub attempts: u32,
    pub next_attempt_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub error_class: Option<String>,
    pub provider_code: Option<String>,
    pub duration_ms: Option<u32>,
}

/// The result of one attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryAttempt {
    /// `sent`, `pending` (retry at `next_attempt_at`), `failed` or `skipped`.
    pub state: String,
    pub next_attempt_at: Option<i64>,
    pub error_class: Option<String>,
    pub provider_code: Option<String>,
    pub duration_ms: Option<u32>,
    /// Whether this counts as an attempt (a skip does not).
    pub attempted: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeliveryFilter {
    pub kind: Option<NotifyChannelKind>,
    pub target_id: Option<i64>,
    pub state: Option<String>,
}

const TARGET_COLUMNS: &str = "id, kind, name, enabled, min_severity, address, address_hint, secret";
const DELIVERY_COLUMNS: &str = "id, created_at, kind, target_id, target_name, event, cluster, alert_count, payload_json, state, attempts, next_attempt_at, last_attempt_at, error_class, provider_code, duration_ms";

fn corrupt() -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, "notify".into(), rusqlite::types::Type::Text)
}

fn kind_of(text: String) -> rusqlite::Result<NotifyChannelKind> {
    NotifyChannelKind::parse(&text).ok_or_else(corrupt)
}

fn target_from_row(row: &Row<'_>) -> rusqlite::Result<StoredTarget> {
    Ok(StoredTarget {
        id: row.get(0)?,
        kind: kind_of(row.get(1)?)?,
        name: row.get(2)?,
        enabled: row.get(3)?,
        min_severity: row.get(4)?,
        address: row.get(5)?,
        address_hint: row.get(6)?,
        secret: row.get(7)?,
    })
}

fn delivery_from_row(row: &Row<'_>) -> rusqlite::Result<StoredDelivery> {
    Ok(StoredDelivery {
        id: row.get(0)?,
        created_at: row.get(1)?,
        kind: kind_of(row.get(2)?)?,
        target_id: row.get(3)?,
        target_name: row.get(4)?,
        event: row.get(5)?,
        cluster: row.get(6)?,
        alert_count: row.get(7)?,
        payload_json: row.get(8)?,
        state: row.get(9)?,
        attempts: row.get(10)?,
        next_attempt_at: row.get(11)?,
        last_attempt_at: row.get(12)?,
        error_class: row.get(13)?,
        provider_code: row.get(14)?,
        duration_ms: row.get(15)?,
    })
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| HistoryError::Internal("notify_json"))
}

impl HistoryStore {
    pub fn notify_settings(&self) -> Result<NotifySettings> {
        let text: Option<String> = self
            .lock()
            .query_row(
                "SELECT settings_json FROM notify_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(text
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default())
    }

    pub fn set_notify_settings(&self, settings: &NotifySettings) -> Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO notify_settings (id, settings_json) VALUES (1, ?1)",
            params![to_json(settings)?],
        )?;
        Ok(())
    }

    pub fn notify_channels(&self) -> Result<Vec<StoredChannel>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT kind, enabled, config_json, secret FROM notify_channels")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
            ))
        })?;
        let mut channels = Vec::new();
        for row in rows {
            let (kind, enabled, config, secret) = row?;
            let (Some(kind), Ok(config)) = (
                NotifyChannelKind::parse(&kind),
                serde_json::from_str(&config),
            ) else {
                return Err(HistoryError::Internal("notify_channel_corrupt"));
            };
            channels.push(StoredChannel {
                kind,
                enabled,
                config,
                secret,
            });
        }
        Ok(channels)
    }

    pub fn notify_channel(&self, kind: NotifyChannelKind) -> Result<Option<StoredChannel>> {
        Ok(self
            .notify_channels()?
            .into_iter()
            .find(|channel| channel.kind == kind))
    }

    pub fn save_notify_channel(
        &self,
        config: &NotifyChannelConfig,
        enabled: bool,
        secret: &SecretUpdate,
        now: i64,
    ) -> Result<()> {
        let kind = config.kind().as_str();
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let old: Option<Option<Vec<u8>>> = tx
            .query_row(
                "SELECT secret FROM notify_channels WHERE kind = ?1",
                params![kind],
                |row| row.get(0),
            )
            .optional()?;
        let secret = match secret {
            SecretUpdate::Keep => old.flatten(),
            SecretUpdate::Set(blob) => Some(blob.clone()),
            SecretUpdate::Clear => None,
        };
        tx.execute(
            "INSERT OR REPLACE INTO notify_channels (kind, enabled, config_json, secret, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, enabled, to_json(config)?, secret, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn notify_targets(&self) -> Result<Vec<StoredTarget>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {TARGET_COLUMNS} FROM notify_targets ORDER BY kind, name, id"
        ))?;
        let rows = statement.query_map([], target_from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub fn notify_target(&self, id: i64) -> Result<Option<StoredTarget>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {TARGET_COLUMNS} FROM notify_targets WHERE id = ?1"),
                params![id],
                target_from_row,
            )
            .optional()?)
    }

    /// `None` when the channel already has `max` targets.
    pub fn insert_notify_target(
        &self,
        write: &TargetWrite,
        max: u16,
        now: i64,
    ) -> Result<Option<i64>> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM notify_targets WHERE kind = ?1",
            params![write.kind.as_str()],
            |row| row.get(0),
        )?;
        if count >= i64::from(max) {
            return Ok(None);
        }
        let secret = match &write.secret {
            SecretUpdate::Set(blob) => Some(blob.clone()),
            SecretUpdate::Keep | SecretUpdate::Clear => None,
        };
        tx.execute(
            "INSERT INTO notify_targets (kind, name, enabled, min_severity, address, address_hint, secret, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![write.kind.as_str(), write.name, write.enabled, write.min_severity, write.address, write.address_hint, secret, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }

    /// `false` when no target of that id and kind exists.
    pub fn update_notify_target(&self, id: i64, write: &TargetWrite, now: i64) -> Result<bool> {
        let changed = match &write.secret {
            SecretUpdate::Keep => self.lock().execute(
                "UPDATE notify_targets SET name = ?3, enabled = ?4, min_severity = ?5, address = ?6, address_hint = ?7, updated_at = ?8 WHERE id = ?1 AND kind = ?2",
                params![id, write.kind.as_str(), write.name, write.enabled, write.min_severity, write.address, write.address_hint, now],
            )?,
            SecretUpdate::Set(_) | SecretUpdate::Clear => {
                let secret = match &write.secret {
                    SecretUpdate::Set(blob) => Some(blob.clone()),
                    _ => None,
                };
                self.lock().execute(
                    "UPDATE notify_targets SET name = ?3, enabled = ?4, min_severity = ?5, address = ?6, address_hint = ?7, secret = ?8, updated_at = ?9 WHERE id = ?1 AND kind = ?2",
                    params![id, write.kind.as_str(), write.name, write.enabled, write.min_severity, write.address, write.address_hint, secret, now],
                )?
            }
        };
        Ok(changed == 1)
    }

    /// Its pending deliveries are skipped; past ones keep its name.
    pub fn delete_notify_target(&self, id: i64) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let removed = tx.execute("DELETE FROM notify_targets WHERE id = ?1", params![id])?;
        tx.execute(
            "UPDATE notify_deliveries SET state = CASE WHEN state = 'pending' THEN 'skipped' ELSE state END,
               error_class = CASE WHEN state = 'pending' THEN 'not_configured' ELSE error_class END,
               next_attempt_at = NULL, target_id = NULL
             WHERE target_id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(removed == 1)
    }

    /// Queues deliveries, unless `max_pending` are already waiting (then
    /// nothing is queued and `false` is returned).
    pub fn enqueue_deliveries(
        &self,
        deliveries: &[NewDelivery],
        max_pending: u32,
        now: i64,
    ) -> Result<bool> {
        if deliveries.is_empty() {
            return Ok(true);
        }
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM notify_deliveries WHERE state = 'pending'",
            [],
            |row| row.get(0),
        )?;
        if pending + deliveries.len() as i64 > i64::from(max_pending) {
            return Ok(false);
        }
        for delivery in deliveries {
            tx.execute(
                "INSERT INTO notify_deliveries (created_at, kind, target_id, target_name, event, cluster, alert_count, payload_json, state, attempts, next_attempt_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, ?1)",
                params![now, delivery.kind.as_str(), delivery.target_id, delivery.target_name, delivery.event, delivery.cluster, delivery.alert_count, delivery.payload_json],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Records a delivery that already happened (a test send).
    pub fn record_delivery(
        &self,
        delivery: &NewDelivery,
        attempt: &DeliveryAttempt,
        now: i64,
    ) -> Result<i64> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO notify_deliveries (created_at, kind, target_id, target_name, event, cluster, alert_count, payload_json, state, attempts, next_attempt_at, last_attempt_at, error_class, provider_code, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?1, ?11, ?12, ?13)",
            params![now, delivery.kind.as_str(), delivery.target_id, delivery.target_name, delivery.event, delivery.cluster, delivery.alert_count, delivery.payload_json,
                    attempt.state, u32::from(attempt.attempted), attempt.error_class, attempt.provider_code, attempt.duration_ms],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn due_deliveries(&self, now: i64, limit: u32) -> Result<Vec<StoredDelivery>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM notify_deliveries WHERE state = 'pending' AND next_attempt_at <= ?1 ORDER BY next_attempt_at, id LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![now, limit], delivery_from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// Applies one attempt's result, only while the row is still pending.
    pub fn finish_delivery(&self, id: i64, attempt: &DeliveryAttempt, now: i64) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE notify_deliveries SET state = ?2, attempts = attempts + ?3, next_attempt_at = ?4, last_attempt_at = ?5,
               error_class = ?6, provider_code = ?7, duration_ms = ?8
             WHERE id = ?1 AND state = 'pending'",
            params![id, attempt.state, u32::from(attempt.attempted), attempt.next_attempt_at, now, attempt.error_class, attempt.provider_code, attempt.duration_ms],
        )? == 1)
    }

    /// Newest first.
    pub fn deliveries(&self, filter: &DeliveryFilter, limit: u32) -> Result<Vec<StoredDelivery>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM notify_deliveries
             WHERE (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR target_id = ?2) AND (?3 IS NULL OR state = ?3)
             ORDER BY id DESC LIMIT ?4"
        ))?;
        let rows = statement.query_map(
            params![
                filter.kind.map(NotifyChannelKind::as_str),
                filter.target_id,
                filter.state,
                limit
            ],
            delivery_from_row,
        )?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// The latest finished or pending delivery of each target.
    pub fn latest_deliveries(&self) -> Result<Vec<StoredDelivery>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM notify_deliveries WHERE id IN (SELECT MAX(id) FROM notify_deliveries WHERE target_id IS NOT NULL GROUP BY target_id)"
        ))?;
        let rows = statement.query_map([], delivery_from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// (sent, failed) since `since`, and pending now.
    pub fn delivery_counts(&self, since: i64) -> Result<(u32, u32, u32)> {
        Ok(self.lock().query_row(
            "SELECT COALESCE(SUM(state = 'sent' AND last_attempt_at >= ?1), 0),
                    COALESCE(SUM(state = 'failed' AND last_attempt_at >= ?1), 0),
                    COALESCE(SUM(state = 'pending'), 0)
             FROM notify_deliveries",
            params![since],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }

    /// Messages of a channel actually sent since `since` (tests included:
    /// they are billed too).
    pub fn sent_since(&self, kind: NotifyChannelKind, since: i64) -> Result<u32> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM notify_deliveries WHERE kind = ?1 AND state = 'sent' AND last_attempt_at >= ?2",
            params![kind.as_str(), since],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::{EmailSecurity, NotifyLanguage};

    fn email() -> NotifyChannelConfig {
        NotifyChannelConfig::Email {
            host: "smtp.example.com".into(),
            port: 465,
            security: EmailSecurity::Tls,
            username: None,
            from_address: "a@example.com".into(),
            from_name: None,
        }
    }

    fn target(name: &str, secret: SecretUpdate) -> TargetWrite {
        TargetWrite {
            kind: NotifyChannelKind::Email,
            name: name.into(),
            enabled: true,
            min_severity: "medium".into(),
            address: Some(format!("{name}@example.com")),
            address_hint: format!("{name}@example.com"),
            secret,
        }
    }

    fn delivery(target_id: i64) -> NewDelivery {
        NewDelivery {
            kind: NotifyChannelKind::Email,
            target_id: Some(target_id),
            target_name: "dba".into(),
            event: "new_alerts".into(),
            cluster: Some("127.0.0.1:9030".into()),
            alert_count: 1,
            payload_json: "{}".into(),
        }
    }

    #[test]
    fn n01_channel_secrets_keep_set_and_clear() {
        let store = HistoryStore::open_in_memory().unwrap();
        assert_eq!(store.notify_settings().unwrap(), NotifySettings::default());
        let settings = NotifySettings {
            notify_resolved: true,
            language: NotifyLanguage::En,
            ..NotifySettings::default()
        };
        store.set_notify_settings(&settings).unwrap();
        assert_eq!(store.notify_settings().unwrap(), settings);
        store
            .save_notify_channel(&email(), true, &SecretUpdate::Set(vec![1, 2]), 10)
            .unwrap();
        store
            .save_notify_channel(&email(), false, &SecretUpdate::Keep, 11)
            .unwrap();
        let channel = store
            .notify_channel(NotifyChannelKind::Email)
            .unwrap()
            .unwrap();
        assert_eq!((channel.enabled, channel.secret), (false, Some(vec![1, 2])));
        store
            .save_notify_channel(&email(), true, &SecretUpdate::Clear, 12)
            .unwrap();
        assert_eq!(
            store
                .notify_channel(NotifyChannelKind::Email)
                .unwrap()
                .unwrap()
                .secret,
            None
        );
        assert!(
            store
                .notify_channel(NotifyChannelKind::Feishu)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn targets_are_capped_per_channel_and_keep_secrets_on_update() {
        let store = HistoryStore::open_in_memory().unwrap();
        let first = store
            .insert_notify_target(&target("a", SecretUpdate::Set(vec![9])), 2, 1)
            .unwrap()
            .unwrap();
        store
            .insert_notify_target(&target("b", SecretUpdate::Keep), 2, 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .insert_notify_target(&target("c", SecretUpdate::Keep), 2, 1)
                .unwrap(),
            None
        );
        assert!(
            store
                .update_notify_target(first, &target("a2", SecretUpdate::Keep), 2)
                .unwrap()
        );
        let stored = store.notify_target(first).unwrap().unwrap();
        assert_eq!((stored.name.as_str(), stored.secret), ("a2", Some(vec![9])));
        let mut wrong_kind = target("x", SecretUpdate::Keep);
        wrong_kind.kind = NotifyChannelKind::Feishu;
        assert!(!store.update_notify_target(first, &wrong_kind, 3).unwrap());
        assert_eq!(store.notify_targets().unwrap().len(), 2);
    }

    #[test]
    fn n04_deliveries_queue_retry_finish_and_go_with_their_target() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = store
            .insert_notify_target(&target("dba", SecretUpdate::Keep), 20, 1)
            .unwrap()
            .unwrap();
        assert!(
            store
                .enqueue_deliveries(&[delivery(id), delivery(id)], 10, 100)
                .unwrap()
        );
        assert!(
            !store
                .enqueue_deliveries(&vec![delivery(id); 9], 10, 100)
                .unwrap()
        );
        let due = store.due_deliveries(100, 10).unwrap();
        assert_eq!(due.len(), 2);
        let retry = DeliveryAttempt {
            state: "pending".into(),
            next_attempt_at: Some(160),
            error_class: Some("timeout".into()),
            provider_code: None,
            duration_ms: Some(10_000),
            attempted: true,
        };
        assert!(store.finish_delivery(due[0].id, &retry, 100).unwrap());
        assert_eq!(store.due_deliveries(150, 10).unwrap().len(), 1);
        let sent = DeliveryAttempt {
            state: "sent".into(),
            next_attempt_at: None,
            error_class: None,
            provider_code: None,
            duration_ms: Some(900),
            attempted: true,
        };
        assert!(store.finish_delivery(due[0].id, &sent, 160).unwrap());
        assert!(!store.finish_delivery(due[0].id, &sent, 161).unwrap());
        let row = store
            .deliveries(&DeliveryFilter::default(), 10)
            .unwrap()
            .into_iter()
            .find(|row| row.id == due[0].id)
            .unwrap();
        assert_eq!(
            (row.state.as_str(), row.attempts, row.error_class),
            ("sent", 2, None)
        );
        assert_eq!(store.delivery_counts(0).unwrap(), (1, 0, 1));
        assert_eq!(store.sent_since(NotifyChannelKind::Email, 150).unwrap(), 1);
        assert_eq!(store.latest_deliveries().unwrap().len(), 1);
        assert!(store.delete_notify_target(id).unwrap());
        let rows = store.deliveries(&DeliveryFilter::default(), 10).unwrap();
        assert!(
            rows.iter()
                .all(|row| row.target_id.is_none() && row.state != "pending")
        );
        assert_eq!(rows[0].target_name, "dba");
        assert_eq!(
            store
                .deliveries(
                    &DeliveryFilter {
                        state: Some("skipped".into()),
                        ..DeliveryFilter::default()
                    },
                    10
                )
                .unwrap()
                .len(),
            1
        );
    }
}
