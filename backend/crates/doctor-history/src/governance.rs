//! Stage D storage: in-app alerts and the governance queue. States are plain
//! text checked by the API; transitions are compare-and-set on the state so
//! a stale request never overwrites a newer one. Snapshot and verification
//! payloads arrive as JSON text built from literal-free contract types.
use rusqlite::{OptionalExtension, Row, params};

use crate::{HistoryStore, Result};

/// Stage D tables. `governance_item_id` links an alert to the item made from it.
pub(crate) const MIGRATION_3: &str = "
CREATE TABLE alerts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target TEXT NOT NULL,
  template_key TEXT NOT NULL,
  state TEXT NOT NULL,
  muted_until INTEGER,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  resolved_at INTEGER,
  severity TEXT NOT NULL,
  confidence TEXT NOT NULL,
  evidence TEXT NOT NULL,
  workload TEXT NOT NULL,
  baseline_median INTEGER,
  current_median INTEGER,
  relative_permille INTEGER,
  bucket_start INTEGER NOT NULL,
  exemplar TEXT,
  engine_version TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  governance_item_id INTEGER
);
CREATE INDEX alerts_key ON alerts (target, template_key, state);
CREATE TABLE governance_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target TEXT NOT NULL,
  template_key TEXT NOT NULL,
  title TEXT NOT NULL,
  origin TEXT NOT NULL,
  alert_id INTEGER,
  state TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  change_at INTEGER,
  snapshot_json TEXT,
  verification_json TEXT
);
CREATE TABLE governance_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id INTEGER NOT NULL,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL,
  actor TEXT NOT NULL,
  note TEXT
);
CREATE INDEX governance_events_item ON governance_events (item_id, id);
";

pub(crate) const MIGRATION_4: &str = "
CREATE UNIQUE INDEX governance_one_active_item ON governance_items (target, template_key)
WHERE state NOT IN ('closed', 'wont_fix');
";

const ACTIVE: &str = "('open', 'acknowledged', 'muted')";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlertFacts {
    pub severity: String,
    pub confidence: String,
    pub evidence: String,
    pub workload: String,
    /// Audit source units (ms).
    pub baseline_median: Option<u64>,
    pub current_median: Option<u64>,
    pub relative_permille: Option<i64>,
    pub bucket_start: i64,
    pub exemplar: Option<String>,
    pub engine_version: String,
    pub rule_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAlert {
    pub id: i64,
    pub target: String,
    pub template_key: String,
    pub state: String,
    pub muted_until: Option<i64>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub resolved_at: Option<i64>,
    pub facts: AlertFacts,
    pub governance_item_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertUpsert {
    /// A new alert with this id.
    Created(i64),
    /// A muted alert whose window ended while the regression is still there.
    Reopened(i64),
    Updated,
    /// Muted and still inside the mute window: facts refreshed, no new alert.
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreChange {
    Applied,
    NotFound,
    /// The current state does not allow it, or it changed meanwhile.
    Rejected,
    /// Another active item owns this target/template. No event was written.
    ActiveItemExists,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredItem {
    pub id: i64,
    pub target: String,
    pub template_key: String,
    pub title: String,
    pub origin: String,
    pub alert_id: Option<i64>,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub change_at: Option<i64>,
    pub snapshot_json: Option<String>,
    pub verification_json: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvent {
    pub at: i64,
    pub kind: String,
    pub from_state: Option<String>,
    pub to_state: String,
    pub actor: String,
    pub note: Option<String>,
}

/// One state change of an item, applied only if the item is still in `from`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemUpdate {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub actor: String,
    pub note: Option<String>,
    pub change_at: Option<i64>,
    pub snapshot_json: Option<String>,
    pub verification_json: Option<String>,
}

fn alert_from_row(row: &Row<'_>) -> rusqlite::Result<StoredAlert> {
    Ok(StoredAlert {
        id: row.get(0)?,
        target: row.get(1)?,
        template_key: row.get(2)?,
        state: row.get(3)?,
        muted_until: row.get(4)?,
        first_seen: row.get(5)?,
        last_seen: row.get(6)?,
        resolved_at: row.get(7)?,
        facts: AlertFacts {
            severity: row.get(8)?,
            confidence: row.get(9)?,
            evidence: row.get(10)?,
            workload: row.get(11)?,
            baseline_median: row.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
            current_median: row.get::<_, Option<i64>>(13)?.map(|v| v.max(0) as u64),
            relative_permille: row.get(14)?,
            bucket_start: row.get(15)?,
            exemplar: row.get(16)?,
            engine_version: row.get(17)?,
            rule_version: row.get(18)?,
        },
        governance_item_id: row.get(19)?,
    })
}

const ALERT_COLUMNS: &str = "id, target, template_key, state, muted_until, first_seen, last_seen, resolved_at, severity, confidence, evidence, workload, baseline_median, current_median, relative_permille, bucket_start, exemplar, engine_version, rule_version, governance_item_id";

fn item_from_row(row: &Row<'_>) -> rusqlite::Result<StoredItem> {
    Ok(StoredItem {
        id: row.get(0)?,
        target: row.get(1)?,
        template_key: row.get(2)?,
        title: row.get(3)?,
        origin: row.get(4)?,
        alert_id: row.get(5)?,
        state: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        change_at: row.get(9)?,
        snapshot_json: row.get(10)?,
        verification_json: row.get(11)?,
    })
}

const ITEM_COLUMNS: &str = "id, target, template_key, title, origin, alert_id, state, created_at, updated_at, change_at, snapshot_json, verification_json";

fn clamp(value: Option<u64>) -> Option<i64> {
    value.map(|v| i64::try_from(v).unwrap_or(i64::MAX))
}

impl HistoryStore {
    /// One active alert per template (dedup); a muted one stays muted until
    /// its window ends, then reopens if the regression is still there.
    pub fn upsert_alert(
        &self,
        target: &str,
        template_key: &str,
        facts: &AlertFacts,
        now: i64,
    ) -> Result<AlertUpsert> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let active: Option<(i64, String, Option<i64>)> = tx
            .query_row(
                &format!("SELECT id, state, muted_until FROM alerts WHERE target = ?1 AND template_key = ?2 AND state IN {ACTIVE} ORDER BY id DESC LIMIT 1"),
                params![target, template_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let f = facts;
        let outcome = match active {
            Some((id, state, muted_until)) => {
                let still_muted = state == "muted" && muted_until.is_some_and(|until| until > now);
                let next_state = if state == "muted" && !still_muted {
                    "open".to_owned()
                } else {
                    state.clone()
                };
                tx.execute(
                    "UPDATE alerts SET state = ?2, muted_until = CASE WHEN ?2 = 'muted' THEN muted_until ELSE NULL END, last_seen = ?3,
                       severity = ?4, confidence = ?5, evidence = ?6, workload = ?7, baseline_median = ?8, current_median = ?9,
                       relative_permille = ?10, bucket_start = ?11, exemplar = COALESCE(?12, exemplar), engine_version = ?13, rule_version = ?14
                     WHERE id = ?1",
                    params![id, next_state, now, f.severity, f.confidence, f.evidence, f.workload, clamp(f.baseline_median), clamp(f.current_median), f.relative_permille, f.bucket_start, f.exemplar, f.engine_version, f.rule_version],
                )?;
                if still_muted {
                    AlertUpsert::Suppressed
                } else if state == "muted" {
                    AlertUpsert::Reopened(id)
                } else {
                    AlertUpsert::Updated
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO alerts (target, template_key, state, muted_until, first_seen, last_seen, resolved_at, severity, confidence, evidence, workload,
                       baseline_median, current_median, relative_permille, bucket_start, exemplar, engine_version, rule_version, governance_item_id)
                     VALUES (?1, ?2, 'open', NULL, ?3, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL)",
                    params![target, template_key, now, f.severity, f.confidence, f.evidence, f.workload, clamp(f.baseline_median), clamp(f.current_median), f.relative_permille, f.bucket_start, f.exemplar, f.engine_version, f.rule_version],
                )?;
                AlertUpsert::Created(tx.last_insert_rowid())
            }
        };
        tx.commit()?;
        Ok(outcome)
    }

    /// Back inside the baseline band: active alerts of the template resolve.
    /// Returns the ids that changed.
    pub fn resolve_alerts(&self, target: &str, template_key: &str, now: i64) -> Result<Vec<i64>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!("UPDATE alerts SET state = 'resolved', resolved_at = ?3, muted_until = NULL WHERE target = ?1 AND template_key = ?2 AND state IN {ACTIVE} RETURNING id"))?;
        let ids = statement.query_map(params![target, template_key, now], |row| row.get(0))?;
        ids.map(|id| id.map_err(Into::into)).collect()
    }

    /// Resolves the target's active alerts raised under another rule version.
    /// The current rules call their trends not comparable, so these alerts
    /// would otherwise never resolve; they can still be reopened by hand.
    pub fn resolve_rule_changed_alerts(
        &self,
        target: &str,
        current_rule_version: &str,
        now: i64,
    ) -> Result<u64> {
        Ok(self.lock().execute(
            &format!("UPDATE alerts SET state = 'resolved', resolved_at = ?3, muted_until = NULL WHERE target = ?1 AND rule_version <> ?2 AND state IN {ACTIVE}"),
            params![target, current_rule_version, now],
        )? as u64)
    }

    pub fn alerts(&self, target: &str, limit: u32) -> Result<Vec<StoredAlert>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!("SELECT {ALERT_COLUMNS} FROM alerts WHERE target = ?1 ORDER BY last_seen DESC, id DESC LIMIT ?2"))?;
        let rows = statement.query_map(params![target, i64::from(limit)], alert_from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub fn alert(&self, id: i64) -> Result<Option<StoredAlert>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {ALERT_COLUMNS} FROM alerts WHERE id = ?1"),
                params![id],
                alert_from_row,
            )
            .optional()?)
    }

    /// `acknowledge` from open; `mute` from open or acknowledged; `reopen`
    /// from resolved when no other alert of the template is active.
    pub fn alert_action(
        &self,
        id: i64,
        action: &str,
        now: i64,
        mute_seconds: i64,
    ) -> Result<StoreChange> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some((state, target, key)): Option<(String, String, String)> = tx
            .query_row(
                "SELECT state, target, template_key FROM alerts WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
        else {
            return Ok(StoreChange::NotFound);
        };
        let applied = match (action, state.as_str()) {
            ("acknowledge", "open") => tx.execute(
                "UPDATE alerts SET state = 'acknowledged' WHERE id = ?1",
                params![id],
            )?,
            ("mute", "open" | "acknowledged") => tx.execute(
                "UPDATE alerts SET state = 'muted', muted_until = ?2 WHERE id = ?1",
                params![id, now + mute_seconds],
            )?,
            ("reopen", "resolved") => {
                let other: i64 = tx.query_row(
                    &format!("SELECT COUNT(*) FROM alerts WHERE target = ?1 AND template_key = ?2 AND state IN {ACTIVE}"),
                    params![target, key],
                    |row| row.get(0),
                )?;
                if other > 0 {
                    0
                } else {
                    tx.execute("UPDATE alerts SET state = 'open', resolved_at = NULL, last_seen = ?2 WHERE id = ?1", params![id, now])?
                }
            }
            _ => 0,
        };
        tx.commit()?;
        Ok(if applied == 1 {
            StoreChange::Applied
        } else {
            StoreChange::Rejected
        })
    }

    /// A new item in `new`; refused while another item of the same template
    /// is still open (not closed or won't fix).
    pub fn create_item(
        &self,
        target: &str,
        template_key: &str,
        title: &str,
        origin: &str,
        alert_id: Option<i64>,
        now: i64,
    ) -> Result<Option<i64>> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let open: i64 = tx.query_row(
            "SELECT COUNT(*) FROM governance_items WHERE target = ?1 AND template_key = ?2 AND state NOT IN ('closed', 'wont_fix')",
            params![target, template_key],
            |row| row.get(0),
        )?;
        if open > 0 {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO governance_items (target, template_key, title, origin, alert_id, state, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'new', ?6, ?6)",
            params![target, template_key, title, origin, alert_id, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO governance_events (item_id, at, kind, from_state, to_state, actor, note) VALUES (?1, ?2, 'created', NULL, 'new', 'local_user', NULL)",
            params![id, now],
        )?;
        if let Some(alert_id) = alert_id {
            tx.execute(
                "UPDATE alerts SET governance_item_id = ?2 WHERE id = ?1",
                params![alert_id, id],
            )?;
        }
        tx.commit()?;
        Ok(Some(id))
    }

    pub fn items(&self, target: Option<&str>) -> Result<Vec<StoredItem>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM governance_items WHERE (?1 IS NULL OR target = ?1)
             ORDER BY CASE WHEN state IN ('closed', 'wont_fix') THEN 1 ELSE 0 END, updated_at DESC, id DESC LIMIT 500"
        ))?;
        let rows = statement.query_map(params![target], item_from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub fn item(&self, id: i64) -> Result<Option<StoredItem>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM governance_items WHERE id = ?1"),
                params![id],
                item_from_row,
            )
            .optional()?)
    }

    pub fn item_events(&self, id: i64) -> Result<Vec<StoredEvent>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT at, kind, from_state, to_state, actor, note FROM governance_events WHERE item_id = ?1 ORDER BY id")?;
        let rows = statement.query_map(params![id], |row| {
            Ok(StoredEvent {
                at: row.get(0)?,
                kind: row.get(1)?,
                from_state: row.get(2)?,
                to_state: row.get(3)?,
                actor: row.get(4)?,
                note: row.get(5)?,
            })
        })?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// Compare-and-set on the state, with one event per change.
    pub fn update_item(&self, id: i64, update: &ItemUpdate, now: i64) -> Result<StoreChange> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some((state, target, template_key)): Option<(String, String, String)> = tx
            .query_row(
                "SELECT state, target, template_key FROM governance_items WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
        else {
            return Ok(StoreChange::NotFound);
        };
        if state != update.from {
            return Ok(StoreChange::Rejected);
        }
        if !matches!(update.to.as_str(), "closed" | "wont_fix") {
            let other: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM governance_items WHERE target = ?1 AND template_key = ?2 AND id != ?3 AND state NOT IN ('closed', 'wont_fix'))",
                params![target, template_key, id], |row| row.get(0),
            )?;
            if other {
                return Ok(StoreChange::ActiveItemExists);
            }
        }
        tx.execute(
            "UPDATE governance_items SET state = ?2, updated_at = ?3, change_at = COALESCE(?4, change_at),
               snapshot_json = COALESCE(?5, snapshot_json), verification_json = COALESCE(?6, verification_json) WHERE id = ?1",
            params![id, update.to, now, update.change_at, update.snapshot_json, update.verification_json],
        )?;
        tx.execute(
            "INSERT INTO governance_events (item_id, at, kind, from_state, to_state, actor, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, now, update.kind, (update.from != update.to).then_some(&update.from), update.to, update.actor, update.note],
        )?;
        tx.commit()?;
        Ok(StoreChange::Applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_v1_10_reopening_cannot_create_two_active_items() {
        let store = HistoryStore::open_in_memory().unwrap();
        let first = store
            .create_item("t", "k", "first", "manual", None, 1)
            .unwrap()
            .unwrap();
        let step = |from: &str, to: &str| ItemUpdate {
            from: from.into(),
            to: to.into(),
            ..ItemUpdate::default()
        };
        assert_eq!(
            store
                .update_item(first, &step("new", "wont_fix"), 2)
                .unwrap(),
            StoreChange::Applied
        );
        let second = store
            .create_item("t", "k", "second", "manual", None, 3)
            .unwrap()
            .unwrap();
        assert_ne!(
            store
                .update_item(first, &step("wont_fix", "triaged"), 4)
                .unwrap(),
            StoreChange::Applied
        );
        assert_eq!(
            store
                .items(Some("t"))
                .unwrap()
                .iter()
                .filter(|i| i.state != "wont_fix" && i.state != "closed")
                .count(),
            1
        );
        assert_eq!(store.item(first).unwrap().unwrap().state, "wont_fix");
        assert_eq!(store.item_events(first).unwrap().len(), 2);
        assert_eq!(
            store
                .update_item(second, &step("new", "wont_fix"), 5)
                .unwrap(),
            StoreChange::Applied
        );
        assert_eq!(
            store
                .update_item(first, &step("wont_fix", "triaged"), 6)
                .unwrap(),
            StoreChange::Applied
        );
    }

    fn facts(severity: &str) -> AlertFacts {
        AlertFacts {
            severity: severity.into(),
            confidence: "high".into(),
            evidence: "complete".into(),
            workload: "increased".into(),
            baseline_median: Some(100),
            current_median: Some(250),
            relative_permille: Some(1_500),
            bucket_start: 3_600,
            exemplar: Some("q1".into()),
            engine_version: "3.5.14".into(),
            rule_version: "v1".into(),
        }
    }

    #[test]
    fn rd01_one_alert_per_template_and_mute_suppresses_until_it_ends() {
        let store = HistoryStore::open_in_memory().unwrap();
        assert_eq!(
            store.upsert_alert("t", "k", &facts("high"), 100).unwrap(),
            AlertUpsert::Created(1)
        );
        assert_eq!(
            store.upsert_alert("t", "k", &facts("medium"), 200).unwrap(),
            AlertUpsert::Updated
        );
        let alerts = store.alerts("t", 10).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            (
                alerts[0].first_seen,
                alerts[0].last_seen,
                alerts[0].facts.severity.as_str()
            ),
            (100, 200, "medium")
        );
        let id = alerts[0].id;
        assert_eq!(
            store.alert_action(id, "acknowledge", 210, 0).unwrap(),
            StoreChange::Applied
        );
        assert_eq!(
            store.alert_action(id, "acknowledge", 211, 0).unwrap(),
            StoreChange::Rejected
        );
        assert_eq!(
            store.alert_action(id, "mute", 220, 86_400).unwrap(),
            StoreChange::Applied
        );
        // Inside the mute window: refreshed, still muted, still one alert.
        assert_eq!(
            store.upsert_alert("t", "k", &facts("high"), 1_000).unwrap(),
            AlertUpsert::Suppressed
        );
        assert_eq!(store.alerts("t", 10).unwrap()[0].state, "muted");
        // After it: the same alert reopens; no duplicate is created.
        assert_eq!(
            store
                .upsert_alert("t", "k", &facts("high"), 220 + 86_401)
                .unwrap(),
            AlertUpsert::Reopened(id)
        );
        let alerts = store.alerts("t", 10).unwrap();
        assert_eq!(
            (
                alerts.len(),
                alerts[0].state.as_str(),
                alerts[0].muted_until
            ),
            (1, "open", None)
        );
        // Back in band: resolved; a later regression is a new alert; the old
        // one cannot be reopened while the new one is active.
        assert_eq!(store.resolve_alerts("t", "k", 90_000).unwrap(), vec![id]);
        assert_eq!(
            store
                .upsert_alert("t", "k", &facts("high"), 91_000)
                .unwrap(),
            AlertUpsert::Created(2)
        );
        assert_eq!(
            store.alert_action(id, "reopen", 91_001, 0).unwrap(),
            StoreChange::Rejected
        );
        assert_eq!(store.alerts("t", 10).unwrap().len(), 2);
        assert_eq!(
            store.alert_action(999, "mute", 1, 1).unwrap(),
            StoreChange::NotFound
        );
    }

    #[test]
    fn alerts_from_an_older_rule_version_resolve_together() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.upsert_alert("t", "old", &facts("high"), 100).unwrap();
        store
            .upsert_alert("t", "muted", &facts("high"), 100)
            .unwrap();
        let current = AlertFacts {
            rule_version: "v2".into(),
            ..facts("high")
        };
        store.upsert_alert("t", "new", &current, 100).unwrap();
        store
            .upsert_alert("u", "other", &facts("high"), 100)
            .unwrap();
        let muted = store
            .alerts("t", 10)
            .unwrap()
            .into_iter()
            .find(|alert| alert.template_key == "muted")
            .unwrap()
            .id;
        store.alert_action(muted, "mute", 110, 86_400).unwrap();

        assert_eq!(
            store.resolve_rule_changed_alerts("t", "v2", 200).unwrap(),
            2
        );
        let states: Vec<_> = store
            .alerts("t", 10)
            .unwrap()
            .into_iter()
            .map(|alert| (alert.template_key, alert.state, alert.resolved_at))
            .collect();
        assert!(states.contains(&("old".into(), "resolved".into(), Some(200))));
        assert!(states.contains(&("muted".into(), "resolved".into(), Some(200))));
        assert!(states.contains(&("new".into(), "open".into(), None)));
        // Other targets are untouched; a second run finds nothing left.
        assert_eq!(store.alerts("u", 10).unwrap()[0].state, "open");
        assert_eq!(
            store.resolve_rule_changed_alerts("t", "v2", 300).unwrap(),
            0
        );
    }

    #[test]
    fn rd02_items_change_state_only_from_the_state_they_are_in() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.upsert_alert("t", "k", &facts("high"), 100).unwrap();
        let alert = store.alerts("t", 1).unwrap()[0].id;
        let id = store
            .create_item("t", "k", "Slow join", "alert", Some(alert), 110)
            .unwrap()
            .unwrap();
        // One open item per template.
        assert_eq!(
            store
                .create_item("t", "k", "again", "manual", None, 111)
                .unwrap(),
            None
        );
        assert_eq!(
            store.alert(alert).unwrap().unwrap().governance_item_id,
            Some(id)
        );
        let step = |from: &str, to: &str| ItemUpdate {
            from: from.into(),
            to: to.into(),
            kind: "transition".into(),
            actor: "local_user".into(),
            ..ItemUpdate::default()
        };
        assert_eq!(
            store.update_item(id, &step("new", "triaged"), 120).unwrap(),
            StoreChange::Applied
        );
        // A stale request (still thinks it is new) is rejected.
        assert_eq!(
            store
                .update_item(id, &step("new", "wont_fix"), 121)
                .unwrap(),
            StoreChange::Rejected
        );
        assert_eq!(
            store.update_item(77, &step("new", "triaged"), 121).unwrap(),
            StoreChange::NotFound
        );
        let pin = ItemUpdate {
            snapshot_json: Some("{\"pinned\":1}".into()),
            kind: "pinned".into(),
            ..step("triaged", "triaged")
        };
        assert_eq!(
            store.update_item(id, &pin, 130).unwrap(),
            StoreChange::Applied
        );
        let item = store.item(id).unwrap().unwrap();
        assert_eq!(
            (item.state.as_str(), item.snapshot_json.as_deref()),
            ("triaged", Some("{\"pinned\":1}"))
        );
        let events = store.item_events(id).unwrap();
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec!["created", "transition", "pinned"]
        );
        assert_eq!(events[2].from_state, None);
        assert_eq!(
            store
                .update_item(id, &step("triaged", "wont_fix"), 140)
                .unwrap(),
            StoreChange::Applied
        );
        // Closed or won't-fix items no longer block a new item.
        assert!(
            store
                .create_item("t", "k", "again", "manual", None, 150)
                .unwrap()
                .is_some()
        );
        store.clear().unwrap();
        assert!(store.items(None).unwrap().is_empty() && store.alerts("t", 10).unwrap().is_empty());
    }
}
