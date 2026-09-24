//! SQLite persistence for Pluk.
//!
//! Owns the single `pluk.db` file: integrations, groups, the query audit log,
//! settings, masked columns, and saved queries/commands. The schema evolves
//! only through the `user_version` migration ladder.
//!
//! Open a store against the platform location with [`Store::open_default`]
//! (honors `PLUK_DATA_DIR`), or against any path with [`Store::open`] — tests
//! isolate themselves that way.

pub mod browser;
mod codec;
mod error;
mod groups;
mod ids;
mod integrations;
mod launch_approvals;
mod masked_columns;
mod migrate;
mod models;
mod proxy_auth;
mod proxy_secrets;
mod proxy_tools;
mod query_log;
mod saved_commands;
mod saved_queries;
mod settings;
pub mod timestamp;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pluk_core::platform;

pub use codec::{
    Approvals, QueryPolicy, ToolPolicy, parse_config, parse_members, parse_query_policy, serialize_config,
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
pub use proxy_auth::{AuthStatus, ProxyAuth, ProxyAuthInput, RefreshedTokens};
pub use proxy_secrets::{ProxySecret, SecretKind, SecretWrite};
pub use proxy_tools::{DiscoveredTool, ProxyTool, ToolState};
pub use query_log::{
    ActivityHandler, LOG_PAGE_SIZE, LOG_RESPONSE_LIMIT, LOG_RESULT_ROWS, LogActivity, LogCursor,
    LogDraft, LogGroup, LogPage, LogRange, LogScope, LogUpdate, QueryResult,
};
pub use saved_commands::SavedCommandInput;
pub use saved_queries::SavedQueryInput;
pub use settings::{BROWSER_PAIRING_TOKEN_KEY, LOG_CURSOR_KEY, LOG_RETENTION_DAYS_KEY};

/// How often the automatic retention purge may run on the insert path. The
/// TypeScript server purged on every insert; the guarantee that matters — rows
/// older than the window eventually go — holds just as well at this cadence,
/// without a full-table scan inside every MCP call.
const PURGE_MIN_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// A handle to the Pluk SQLite database.
///
/// Access is serialized behind a mutex: one writer thread at a time within
/// this codebase. WAL journaling plus a busy timeout handle any concurrent access.
pub struct Store {
    conn: Arc<Mutex<rusqlite::Connection>>,
    _maintenance: ArtifactMaintenance,
    last_purge: Mutex<Option<Instant>>,
    activity: Mutex<query_log::ActivityFeed>,
    /// Where staged post images are kept. Always beside the database's own
    /// file, so a copy of one carries the other.
    images_dir: PathBuf,
}

impl Store {
    /// Open (creating and migrating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        let files_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&files_dir)?;
        let mut conn = rusqlite::Connection::open(path)?;
        configure(&mut conn)?;
        migrate::run(&mut conn)?;
        // `last_purge` starts unset, so retention runs on the first log write.
        // Opening the database sits on the app's startup path, and a purge is
        // not worth delaying the window for.
        let conn = Arc::new(Mutex::new(conn));
        let maintenance = ArtifactMaintenance::start(&conn, Duration::from_secs(60))?;
        Ok(Store {
            conn,
            _maintenance: maintenance,
            last_purge: Mutex::new(None),
            activity: Mutex::new(query_log::ActivityFeed::default()),
            images_dir: files_dir.join("wande-images"),
        })
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

const ARTIFACT_PURGE_BATCH: i64 = 8;

struct ArtifactMaintenance {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ArtifactMaintenance {
    fn start(conn: &Arc<Mutex<rusqlite::Connection>>, interval: Duration) -> std::io::Result<Self> {
        let conn = Arc::downgrade(conn);
        let (stop, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pluk-artifact-retention".to_owned())
            .spawn(move || {
                while let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                    let Some(conn) = conn.upgrade() else {
                        break;
                    };
                    match conn.try_lock() {
                        Ok(conn) => {
                            let now = browser::now_millis();
                            if let Err(error) = purge_expired_artifacts(&conn, now) {
                                eprintln!("Artifact retention failed: {error}");
                            }
                        }
                        Err(std::sync::TryLockError::WouldBlock) => {}
                        Err(std::sync::TryLockError::Poisoned(error)) => {
                            eprintln!("Artifact retention stopped: {error}");
                            break;
                        }
                    }
                }
            })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for ArtifactMaintenance {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("Artifact retention thread panicked");
        }
    }
}

fn purge_expired_artifacts(conn: &rusqlite::Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM browser_artifacts WHERE id IN (SELECT id FROM browser_artifacts WHERE expires_at <= ? ORDER BY expires_at LIMIT ?)",
        rusqlite::params![now, ARTIFACT_PURGE_BATCH],
    )
}

/// Connection-level settings applied to every open.
fn configure(conn: &mut rusqlite::Connection) -> Result<()> {
    // Wait instead of failing when a write lock is held, avoiding spurious errors.
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // Write-ahead logging: readers never block the writer. The mode is persistent in
    // the database header.
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

    fn seed_artifacts(conn: &rusqlite::Connection, count: i64) {
        for i in 0..count {
            let id = i.to_string();
            conn.execute(
                "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, integration_id) VALUES (?, ?, 'x', 'inspect', '', '{}', 'succeeded', 0, 1, ?)",
                rusqlite::params![id, id, if i % 2 == 0 { "idle" } else { "deleted-owner" }],
            ).unwrap();
            conn.execute(
                "INSERT INTO browser_artifacts (id, job_id, kind, content_type, bytes, data, created_at, expires_at) VALUES (?, ?, 'screenshot', 'image/png', 1, X'00', 0, 1)",
                rusqlite::params![id, id],
            ).unwrap();
        }
    }

    #[test]
    fn artifact_maintenance_is_bounded_and_covers_idle_and_deleted_owners() {
        let (_dir, store) = temp_store();
        let conn = store.conn.lock().unwrap();
        seed_artifacts(&conn, ARTIFACT_PURGE_BATCH + 2);
        conn.execute("UPDATE browser_artifacts SET expires_at = 101 WHERE id = '0'", []).unwrap();
        assert_eq!(purge_expired_artifacts(&conn, 100).unwrap(), ARTIFACT_PURGE_BATCH as usize);
        assert_eq!(purge_expired_artifacts(&conn, 100).unwrap(), 1);
        assert_eq!(conn.query_row("SELECT id FROM browser_artifacts", [], |row| row.get::<_, String>(0)).unwrap(), "0");
        assert_eq!(purge_expired_artifacts(&conn, 101).unwrap(), 1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM browser_jobs", [], |row| row.get::<_, i64>(0)).unwrap(), ARTIFACT_PURGE_BATCH + 2);
    }

    #[test]
    fn artifact_timer_reaps_without_inserts_and_stops_on_drop() {
        let (_dir, store) = temp_store();
        seed_artifacts(&store.conn.lock().unwrap(), 1);
        let worker = ArtifactMaintenance::start(&store.conn, Duration::from_millis(10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining: i64 = store.conn.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM browser_artifacts", [], |row| row.get(0),
            ).unwrap();
            if remaining == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "maintenance did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(worker);
        let worker = ArtifactMaintenance::start(&store.conn, Duration::from_secs(3600)).unwrap();
        let start = Instant::now();
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

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
