//! Load alerts (task 59, schema 7): one active alert per rule and subject
//! (a Routine Load job, or a table and load kind), with the same lifecycle
//! as the SQL regression alerts. Facts are JSON built by the API from
//! stored counts only: no error log text and no state-change reason text.
use rusqlite::{OptionalExtension, Row, params};

use crate::{AlertUpsert, HistoryStore, Result, StoreChange};

pub(crate) const MIGRATION_7: &str = "
CREATE TABLE load_alerts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target TEXT NOT NULL,
  rule TEXT NOT NULL,
  subject TEXT NOT NULL,
  state TEXT NOT NULL,
  muted_until INTEGER,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  resolved_at INTEGER,
  severity TEXT NOT NULL,
  facts_json TEXT NOT NULL,
  engine_version TEXT NOT NULL,
  rule_version TEXT NOT NULL
);
CREATE INDEX load_alerts_key ON load_alerts (target, rule, subject, state);
-- Which StarRocks version and rule set took each hour's snapshots ('*' when
-- they changed within the hour), and the latest snapshot of each target, so
-- the lag rule never compares across versions and a job missing from the
-- latest complete snapshot is known to be gone.
ALTER TABLE routine_snapshot_hours ADD COLUMN engine_version TEXT;
ALTER TABLE routine_snapshot_hours ADD COLUMN rule_version TEXT;
CREATE TABLE routine_snapshot_last (
  target TEXT PRIMARY KEY,
  observed_at INTEGER NOT NULL,
  truncated INTEGER NOT NULL,
  engine_version TEXT,
  rule_version TEXT
);
";

const ACTIVE: &str = "('open', 'acknowledged', 'muted')";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredLoadAlert {
    pub id: i64,
    pub target: String,
    pub rule: String,
    pub subject: String,
    pub state: String,
    pub muted_until: Option<i64>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub resolved_at: Option<i64>,
    pub severity: String,
    pub facts_json: String,
    pub engine_version: String,
    pub rule_version: String,
}

/// What one evaluation found for a rule and subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadAlertFacts {
    pub severity: String,
    pub facts_json: String,
    pub engine_version: String,
    pub rule_version: String,
}

const COLUMNS: &str = "id, target, rule, subject, state, muted_until, first_seen, last_seen, resolved_at, severity, facts_json, engine_version, rule_version";

fn from_row(row: &Row<'_>) -> rusqlite::Result<StoredLoadAlert> {
    Ok(StoredLoadAlert {
        id: row.get(0)?,
        target: row.get(1)?,
        rule: row.get(2)?,
        subject: row.get(3)?,
        state: row.get(4)?,
        muted_until: row.get(5)?,
        first_seen: row.get(6)?,
        last_seen: row.get(7)?,
        resolved_at: row.get(8)?,
        severity: row.get(9)?,
        facts_json: row.get(10)?,
        engine_version: row.get(11)?,
        rule_version: row.get(12)?,
    })
}

impl HistoryStore {
    /// One active alert per rule and subject; a muted one stays muted until
    /// its window ends, then reopens if the condition still holds.
    pub fn upsert_load_alert(
        &self,
        target: &str,
        rule: &str,
        subject: &str,
        facts: &LoadAlertFacts,
        now: i64,
    ) -> Result<AlertUpsert> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let active: Option<(i64, String, Option<i64>)> = tx
            .query_row(
                &format!("SELECT id, state, muted_until FROM load_alerts WHERE target = ?1 AND rule = ?2 AND subject = ?3 AND state IN {ACTIVE} ORDER BY id DESC LIMIT 1"),
                params![target, rule, subject],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let outcome = match active {
            Some((id, state, muted_until)) => {
                let still_muted = state == "muted" && muted_until.is_some_and(|until| until > now);
                let next = if state == "muted" && !still_muted {
                    "open".to_owned()
                } else {
                    state.clone()
                };
                tx.execute(
                    "UPDATE load_alerts SET state = ?2, muted_until = CASE WHEN ?2 = 'muted' THEN muted_until ELSE NULL END, last_seen = ?3,
                       severity = ?4, facts_json = ?5, engine_version = ?6, rule_version = ?7 WHERE id = ?1",
                    params![id, next, now, facts.severity, facts.facts_json, facts.engine_version, facts.rule_version],
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
                    "INSERT INTO load_alerts (target, rule, subject, state, muted_until, first_seen, last_seen, resolved_at, severity, facts_json, engine_version, rule_version)
                     VALUES (?1, ?2, ?3, 'open', NULL, ?4, ?4, NULL, ?5, ?6, ?7, ?8)",
                    params![target, rule, subject, now, facts.severity, facts.facts_json, facts.engine_version, facts.rule_version],
                )?;
                AlertUpsert::Created(tx.last_insert_rowid())
            }
        };
        tx.commit()?;
        Ok(outcome)
    }

    /// The condition no longer holds: active alerts of the rule and subject
    /// resolve. Returns the ids that changed.
    pub fn resolve_load_alerts(
        &self,
        target: &str,
        rule: &str,
        subject: &str,
        now: i64,
    ) -> Result<Vec<i64>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "UPDATE load_alerts SET state = 'resolved', resolved_at = ?4, muted_until = NULL WHERE target = ?1 AND rule = ?2 AND subject = ?3 AND state IN {ACTIVE} RETURNING id"
        ))?;
        let ids = statement.query_map(params![target, rule, subject, now], |row| row.get(0))?;
        ids.map(|id| id.map_err(Into::into)).collect()
    }

    /// Active alerts of one rule, to resolve subjects that are gone.
    pub fn active_load_alerts(&self, target: &str, rule: &str) -> Result<Vec<StoredLoadAlert>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM load_alerts WHERE target = ?1 AND rule = ?2 AND state IN {ACTIVE} ORDER BY id"
        ))?;
        let rows = statement.query_map(params![target, rule], from_row)?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// Active alerts first, then the newest; the limit applies to each state
    /// on its own, so many muted or resolved alerts never hide an open one.
    /// `more` lists up to its own limit of one state instead.
    pub fn load_alerts(
        &self,
        target: &str,
        limit: u32,
        more: Option<(&str, u32)>,
    ) -> Result<Vec<StoredLoadAlert>> {
        let (more_state, more_limit) = more.unzip();
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY state ORDER BY last_seen DESC, id DESC) AS n FROM load_alerts WHERE target = ?1)
             WHERE n <= CASE WHEN state IS ?3 THEN ?4 ELSE ?2 END ORDER BY CASE WHEN state IN {ACTIVE} THEN 0 ELSE 1 END, last_seen DESC, id DESC"
        ))?;
        let rows = statement.query_map(
            params![
                target,
                i64::from(limit),
                more_state,
                more_limit.map(i64::from)
            ],
            from_row,
        )?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// `[open, acknowledged, resolved, muted]` over every alert of the
    /// target, counted apart from any list limit.
    pub fn load_alert_counts(&self, target: &str) -> Result<[u64; 4]> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT state, COUNT(*) FROM load_alerts WHERE target = ?1 GROUP BY state")?;
        let mut counts = [0u64; 4];
        let rows = statement.query_map(params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (state, count) = row?;
            let index = match state.as_str() {
                "open" => 0,
                "acknowledged" => 1,
                "resolved" => 2,
                "muted" => 3,
                _ => continue,
            };
            counts[index] = count.max(0) as u64;
        }
        Ok(counts)
    }

    /// Changes whenever an alert is raised, refreshed, resolved or acted on,
    /// so a view can re-read even when the open count stays the same.
    pub fn load_alert_revision(&self, target: &str) -> Result<String> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*), COALESCE(MAX(id), 0), COALESCE(MAX(last_seen), 0), COALESCE(MAX(resolved_at), 0), COALESCE(SUM(state = 'acknowledged'), 0), COALESCE(SUM(state = 'muted'), 0) FROM load_alerts WHERE target = ?1",
            params![target],
            |row| {
                Ok(format!(
                    "{}:{}:{}:{}:{}:{}",
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?
                ))
            },
        )?)
    }

    pub fn load_alert(&self, id: i64) -> Result<Option<StoredLoadAlert>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {COLUMNS} FROM load_alerts WHERE id = ?1"),
                params![id],
                from_row,
            )
            .optional()?)
    }

    /// `acknowledge` from open; `mute` from open or acknowledged; `reopen`
    /// from resolved when no other alert of the rule and subject is active.
    pub fn load_alert_action(
        &self,
        id: i64,
        action: &str,
        now: i64,
        mute_seconds: i64,
    ) -> Result<StoreChange> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some((state, target, rule, subject)): Option<(String, String, String, String)> = tx
            .query_row(
                "SELECT state, target, rule, subject FROM load_alerts WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
        else {
            return Ok(StoreChange::NotFound);
        };
        let applied = match (action, state.as_str()) {
            ("acknowledge", "open") => tx.execute(
                "UPDATE load_alerts SET state = 'acknowledged' WHERE id = ?1",
                params![id],
            )?,
            ("mute", "open" | "acknowledged") => tx.execute(
                "UPDATE load_alerts SET state = 'muted', muted_until = ?2 WHERE id = ?1",
                params![id, now + mute_seconds],
            )?,
            ("reopen", "resolved") => {
                let other: i64 = tx.query_row(
                    &format!("SELECT COUNT(*) FROM load_alerts WHERE target = ?1 AND rule = ?2 AND subject = ?3 AND state IN {ACTIVE}"),
                    params![target, rule, subject],
                    |row| row.get(0),
                )?;
                if other > 0 {
                    0
                } else {
                    tx.execute(
                        "UPDATE load_alerts SET state = 'open', resolved_at = NULL, last_seen = ?2 WHERE id = ?1",
                        params![id, now],
                    )?
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
}

pub(crate) const PURGE_LOAD_ALERTS: &str =
    "DELETE FROM load_alerts WHERE state = 'resolved' AND resolved_at < ?1";
pub(crate) const PURGE_SNAPSHOT_LAST: &str =
    "DELETE FROM routine_snapshot_last WHERE observed_at < ?1";
pub(crate) const CLEAR_LOAD_ALERTS: &str =
    "DELETE FROM load_alerts; DELETE FROM routine_snapshot_last;";

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(severity: &str) -> LoadAlertFacts {
        LoadAlertFacts {
            severity: severity.into(),
            facts_json: "{}".into(),
            engine_version: "3.5.14".into(),
            rule_version: "r".into(),
        }
    }

    #[test]
    fn audit_la_02_one_active_alert_per_subject_mute_then_reopen() {
        let store = HistoryStore::open_in_memory().unwrap();
        let first = store
            .upsert_load_alert("t", "routine_paused", "9", &facts("high"), 100)
            .unwrap();
        let AlertUpsert::Created(id) = first else {
            panic!("created")
        };
        assert_eq!(
            store
                .upsert_load_alert("t", "routine_paused", "9", &facts("high"), 200)
                .unwrap(),
            AlertUpsert::Updated
        );
        // Another subject is its own alert.
        assert!(matches!(
            store
                .upsert_load_alert("t", "routine_paused", "10", &facts("high"), 200)
                .unwrap(),
            AlertUpsert::Created(_)
        ));
        assert_eq!(
            store.load_alert_action(id, "mute", 300, 1_000).unwrap(),
            StoreChange::Applied
        );
        assert_eq!(
            store
                .upsert_load_alert("t", "routine_paused", "9", &facts("high"), 900)
                .unwrap(),
            AlertUpsert::Suppressed
        );
        assert_eq!(
            store
                .upsert_load_alert("t", "routine_paused", "9", &facts("high"), 1_400)
                .unwrap(),
            AlertUpsert::Reopened(id)
        );
        assert_eq!(
            store
                .resolve_load_alerts("t", "routine_paused", "9", 1_500)
                .unwrap(),
            [id]
        );
        assert_eq!(store.load_alert(id).unwrap().unwrap().state, "resolved");
        assert_eq!(
            store.load_alert_action(id, "reopen", 1_600, 0).unwrap(),
            StoreChange::Applied
        );
        assert_eq!(
            store
                .active_load_alerts("t", "routine_paused")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.load_alert_action(id, "reopen", 1_600, 0).unwrap(),
            StoreChange::Rejected
        );
        store
            .resolve_load_alerts("t", "routine_paused", "9", 1_700)
            .unwrap();
        // Resolved alerts go with retention; clear removes the rest.
        assert!(store.purge_before(2_000).unwrap() >= 1);
        assert!(store.load_alert(id).unwrap().is_none());
        store.clear().unwrap();
        assert!(store.load_alerts("t", 100, None).unwrap().is_empty());
    }

    #[test]
    fn audit_la_02_list_limit_is_per_state() {
        let store = HistoryStore::open_in_memory().unwrap();
        store
            .upsert_load_alert("t", "routine_paused", "old", &facts("high"), 100)
            .unwrap();
        for index in 0..3 {
            let subject = format!("muted-{index}");
            let AlertUpsert::Created(id) = store
                .upsert_load_alert("t", "routine_paused", &subject, &facts("high"), 200 + index)
                .unwrap()
            else {
                panic!("created")
            };
            store.load_alert_action(id, "mute", 300, 10_000).unwrap();
        }
        // Newer muted alerts beyond the limit never push the open one out.
        let listed = store.load_alerts("t", 2, None).unwrap();
        assert_eq!(listed.len(), 3);
        assert!(
            listed
                .iter()
                .any(|alert| alert.subject == "old" && alert.state == "open")
        );
        assert_eq!(store.load_alert_counts("t").unwrap(), [1, 0, 0, 3]);
        // More of one state than the limit: asking for more reaches them all.
        let muted = |listed: &[StoredLoadAlert]| {
            listed.iter().filter(|alert| alert.state == "muted").count()
        };
        assert_eq!(muted(&listed), 2);
        let more = store.load_alerts("t", 2, Some(("muted", 4))).unwrap();
        assert_eq!((more.len(), muted(&more)), (4, 3));
    }
}
