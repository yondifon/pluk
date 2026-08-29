//! SQLite persistence for Pluk.
//!
//! Owns the single `pluk.db` file: integrations, groups, the query audit log,
//! settings, masked columns, and saved queries/commands. The file is a shared
//! contract — the TypeScript server (`pluk/src/store/*`) and the SwiftUI app
//! (`swift/Sources/ConnectionStore.swift`) still open it, so the schema moves
//! only through the `user_version` migration ladder and never reshapes beyond
//! what they expect.
//!
//! Open a store against the platform location with [`Store::open_default`]
//! (honors `PLUK_DATA_DIR`), or against any path with [`Store::open`] — tests
//! isolate themselves that way.

mod codec;
mod error;
mod groups;
mod ids;
mod integrations;
mod masked_columns;
mod migrate;
mod models;
mod query_log;
mod saved_commands;
mod saved_queries;
mod settings;
pub mod timestamp;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pluk_core::platform;

pub use codec::{
    QueryPolicy, ToolPolicy, parse_config, parse_members, parse_query_policy, serialize_config,
    serialize_members, serialize_query_policy,
};
pub use error::{Result, StoreError};
pub use groups::{GroupInput, GroupUpdate};
pub use ids::{new_id, new_token};
pub use integrations::{IntegrationInput, IntegrationUpdate};
pub use models::{
    Config, Environment, Group, GroupMember, Integration, LogEntry, MaskedColumn, ResolvedMember,
    SavedCommand, SavedQuery, Verdict,
};
pub use query_log::{
    ActivityHandler, LOG_PAGE_SIZE, LOG_RESPONSE_LIMIT, LOG_RESULT_ROWS, LogActivity, LogCursor,
    LogDraft, LogGroup, LogPage, LogRange, LogScope, LogUpdate, QueryResult,
};
pub use saved_commands::SavedCommandInput;
pub use saved_queries::SavedQueryInput;
pub use settings::{LOG_CURSOR_KEY, LOG_RETENTION_DAYS_KEY};

/// How often the automatic retention purge may run on the insert path. The
/// TypeScript server purged on every insert; the guarantee that matters — rows
/// older than the window eventually go — holds just as well at this cadence,
/// without a full-table scan inside every MCP call.
const PURGE_MIN_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// A handle to the Pluk SQLite database.
///
/// Access is serialized behind a mutex: one writer thread at a time within
/// this codebase. Cross-process concurrency (the SwiftUI app keeps the same
/// file open) is handled by WAL journaling plus a busy timeout — see the
/// journal-mode note in `docs/rust-rewrite.md`.
pub struct Store {
    conn: Mutex<rusqlite::Connection>,
    last_purge: Mutex<Option<Instant>>,
    activity: Mutex<query_log::ActivityFeed>,
}

impl Store {
    /// Open (creating and migrating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = rusqlite::Connection::open(path)?;
        configure(&mut conn)?;
        migrate::run(&mut conn)?;
        let store = Store {
            conn: Mutex::new(conn),
            last_purge: Mutex::new(None),
            activity: Mutex::new(query_log::ActivityFeed::default()),
        };
        store.purge_old_logs()?;
        *store.last_purge.lock().expect("purge clock") = Some(Instant::now());
        Ok(store)
    }

    /// Open the database at its platform location (`~/.pluk/pluk.db`, or
    /// `$PLUK_DATA_DIR/pluk.db` when set).
    pub fn open_default() -> Result<Store> {
        Self::open(&Self::db_path())
    }

    /// The platform database path, so callers can point diagnostics at the
    /// real file without duplicating resolution rules.
    pub fn db_path() -> PathBuf {
        platform::data_dir().join("pluk.db")
    }

    /// Run the retention purge now if enough time has passed since the last
    /// one. Called from the log-write paths; a no-op most of the time.
    fn purge_if_due(&self) -> Result<()> {
        let due = match *self.last_purge.lock().expect("purge clock") {
            Some(at) => at.elapsed() >= PURGE_MIN_INTERVAL,
            None => true,
        };
        if due {
            self.purge_old_logs()?;
            *self.last_purge.lock().expect("purge clock") = Some(Instant::now());
        }
        Ok(())
    }
}

/// Connection-level settings applied to every open.
fn configure(conn: &mut rusqlite::Connection) -> Result<()> {
    // A reader must not fail outright while another process holds a write lock
    // (the Swift app shares this file); wait instead, briefly.
    conn.busy_timeout(Duration::from_millis(5_000))?;
    // Write-ahead logging: readers never block the writer across processes,
    // which is exactly this file's access pattern. The mode is persistent in
    // the database header — once set here, the Swift app's plain sqlite3_open
    // picks it up unchanged.
    let _journal: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    // With WAL, NORMAL fsyncs at checkpoints rather than every commit: safe
    // against application crashes; trades away durability of the final seconds
    // on OS/power loss.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing {
    /// A store over a throwaway database, for unit tests inside this crate.
    pub(crate) fn temp_store() -> (tempfile::TempDir, crate::Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::Store::open(&dir.path().join("pluk.db")).expect("open");
        (dir, store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_store;

    #[test]
    fn opens_in_wal_mode_with_a_busy_timeout() {
        let (_dir, store) = temp_store();
        let conn = store.conn.lock().expect("store lock");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        let timeout_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout_ms, 5_000);
    }

    #[test]
    fn open_default_honors_pluk_data_dir() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Only this test touches the variable within this test binary; the
        // lock keeps future ones serialized.
        unsafe { std::env::set_var("PLUK_DATA_DIR", dir.path()) };
        let opened = Store::open_default();
        let resolved = Store::db_path();
        let file_created = dir.path().join("pluk.db").exists();
        unsafe { std::env::remove_var("PLUK_DATA_DIR") };

        opened.expect("open under PLUK_DATA_DIR");
        assert_eq!(resolved, dir.path().join("pluk.db"));
        assert!(file_created);
    }
}
