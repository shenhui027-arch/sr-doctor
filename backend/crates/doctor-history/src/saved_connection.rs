//! The one saved StarRocks connection (task 72, design C1, schema 8). The
//! password is a sealed blob the API wrote; this crate never sees it in
//! plaintext. `secure_delete` is on, so a forgotten blob is overwritten.
use rusqlite::{OptionalExtension, params};

use crate::{HistoryStore, Result};

pub(crate) const MIGRATION_8: &str = "
CREATE TABLE saved_connection (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  host TEXT NOT NULL,
  port INTEGER NOT NULL,
  username TEXT NOT NULL,
  tls_mode TEXT NOT NULL,
  secret BLOB NOT NULL,
  needs_reentry INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
";

pub(crate) const CLEAR_SAVED_CONNECTION: &str = "DELETE FROM saved_connection;";

#[derive(Clone, Eq, PartialEq)]
pub struct StoredConnection {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `plaintext` or `verify_full`.
    pub tls_mode: String,
    /// AES-256-GCM blob sealed by the API.
    pub secret: Vec<u8>,
    pub needs_reentry: bool,
    pub updated_at: i64,
}

impl std::fmt::Debug for StoredConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredConnection { redacted: true }")
    }
}

impl HistoryStore {
    pub fn saved_connection(&self) -> Result<Option<StoredConnection>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT host, port, username, tls_mode, secret, needs_reentry, updated_at FROM saved_connection WHERE id = 1",
                [],
                |row| {
                    Ok(StoredConnection {
                        host: row.get(0)?,
                        port: u16::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        username: row.get(2)?,
                        tls_mode: row.get(3)?,
                        secret: row.get(4)?,
                        needs_reentry: row.get::<_, i64>(5)? != 0,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// Replaces the saved connection (there is only ever one).
    pub fn save_connection(&self, saved: &StoredConnection) -> Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO saved_connection (id, host, port, username, tls_mode, secret, needs_reentry, updated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                saved.host,
                i64::from(saved.port),
                saved.username,
                saved.tls_mode,
                saved.secret,
                i64::from(saved.needs_reentry),
                saved.updated_at
            ],
        )?;
        Ok(())
    }

    /// The password stopped working or can no longer be opened: keep the
    /// fields so the form can be refilled, but never try it again. Only the
    /// saved connection that was tried is marked (matched by its sealed blob):
    /// a late failure of a replaced connection leaves the new one alone.
    /// Returns whether it was marked.
    pub fn mark_saved_connection_needs_reentry(&self, tried: &[u8], now: i64) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE saved_connection SET needs_reentry = 1, updated_at = ?1 WHERE id = 1 AND secret = ?2",
            params![now, tried],
        )? == 1)
    }

    pub fn forget_saved_connection(&self) -> Result<()> {
        self.lock().execute_batch(CLEAR_SAVED_CONNECTION)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> StoredConnection {
        StoredConnection {
            host: "10.0.0.5".into(),
            port: 9030,
            username: "reader".into(),
            tls_mode: "plaintext".into(),
            secret: vec![1, 2, 3],
            needs_reentry: false,
            updated_at: 10,
        }
    }

    #[test]
    fn c1_one_saved_connection_is_replaced_marked_and_forgotten() {
        let store = HistoryStore::open_in_memory().unwrap();
        assert_eq!(store.saved_connection().unwrap(), None);
        store.save_connection(&saved()).unwrap();
        store
            .save_connection(&StoredConnection {
                host: "10.0.0.6".into(),
                ..saved()
            })
            .unwrap();
        assert_eq!(store.saved_connection().unwrap().unwrap().host, "10.0.0.6");
        // A late failure of the replaced connection does not touch the new one.
        assert!(
            !store
                .mark_saved_connection_needs_reentry(&[9, 9], 15)
                .unwrap()
        );
        assert!(!store.saved_connection().unwrap().unwrap().needs_reentry);
        assert!(
            store
                .mark_saved_connection_needs_reentry(&[1, 2, 3], 20)
                .unwrap()
        );
        let marked = store.saved_connection().unwrap().unwrap();
        assert!(marked.needs_reentry);
        assert_eq!(marked.updated_at, 20);
        assert_eq!(format!("{marked:?}"), "StoredConnection { redacted: true }");
        store.forget_saved_connection().unwrap();
        assert_eq!(store.saved_connection().unwrap(), None);
    }

    #[test]
    fn c1_clearing_history_forgets_the_saved_connection() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.save_connection(&saved()).unwrap();
        store.clear().unwrap();
        assert_eq!(store.saved_connection().unwrap(), None);
    }
}
