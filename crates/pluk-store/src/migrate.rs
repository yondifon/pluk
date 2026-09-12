//! The schema migration ladder.
//!
//! The TypeScript side has no version marker: it re-runs `CREATE TABLE IF NOT
//! EXISTS` plus a try/catch loop of `ALTER TABLE ADD COLUMN` on every startup,
//! which makes a genuinely failed migration indistinguishable from "column
//! already exists". This module replaces that with a real ladder keyed off
//! `PRAGMA user_version`:
//!
//! - each step runs once, inside one transaction, and bumps `user_version`;
//! - a failing step aborts the transaction and returns
//!   [`StoreError::Migration`] — loudly;
//! - step 1 produces the exact shape the TypeScript migrations leave behind,
//!   so an existing `~/.pluk.db` opens unchanged, and completes any tail
//!   columns an old database is still missing.

use rusqlite::{Connection, Transaction};

use crate::error::{Result, StoreError};

/// A single migration step: upgrades the database by one version.
type Step = fn(&mut Connection) -> Result<()>;

const LADDER: &[Step] = &[migrate_v1, migrate_v2, migrate_v3, migrate_v4, migrate_v5];

/// Bring `conn` up to the latest version.
pub(crate) fn run(conn: &mut Connection) -> Result<()> {
    let mut version = current_version(conn)?;
    for (index, step) in LADDER.iter().enumerate() {
        let target = (index + 1) as u32;
        if version >= target {
            continue;
        }
        step(conn).map_err(|source| StoreError::Migration {
            version: target,
            source: Box::new(source),
        })?;
        version = current_version(conn)?;
        debug_assert_eq!(version, target);
    }
    Ok(())
}

fn current_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Version 1: every table in its final shared-contract shape.
///
/// The `CREATE` statements are the union of what both existing writers leave
/// behind (column-for-column, in the historical order the TypeScript
/// migrations produce). `ensure_query_log_columns` then brings databases that
/// predate some columns up to the same shape.
fn migrate_v1(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS integrations (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            type TEXT NOT NULL,
            config TEXT NOT NULL DEFAULT '{}',
            environment TEXT DEFAULT 'development',
            read_only INTEGER NOT NULL DEFAULT 0,
            query_policy TEXT,
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            environment TEXT DEFAULT 'production',
            member_ids TEXT NOT NULL DEFAULT '[]',
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS query_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            connection_id TEXT NOT NULL,
            connection_name TEXT NOT NULL,
            sql TEXT NOT NULL,
            verdict TEXT NOT NULL,
            reason TEXT,
            categories TEXT,
            result_json TEXT,
            row_count INTEGER,
            response_text TEXT,
            source TEXT,
            group_id TEXT,
            group_name TEXT,
            database TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS masked_columns (
            id TEXT PRIMARY KEY,
            connection_id TEXT NOT NULL,
            column_name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(connection_id, column_name)
        );

        CREATE TABLE IF NOT EXISTS saved_queries (
            id TEXT PRIMARY KEY,
            connection_id TEXT NOT NULL,
            name TEXT NOT NULL,
            sql TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(connection_id, name)
        );

        CREATE TABLE IF NOT EXISTS saved_commands (
            id TEXT PRIMARY KEY,
            connection_id TEXT NOT NULL,
            name TEXT NOT NULL,
            command TEXT NOT NULL,
            working_dir TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(connection_id, name)
        );
        ",
    )?;

    // Columns before indexes: an old database may not have `group_id` yet,
    // and the group index needs it to exist.
    ensure_query_log_columns(&tx)?;

    tx.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS query_log_connection_time_id_idx
            ON query_log(connection_id, created_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS query_log_group_time_id_idx
            ON query_log(group_id, created_at DESC, id DESC);
        ",
    )?;

    // Existing rows may carry the retired GitHub REST adapter id; the gh-CLI
    // bridge id is 'github-cli' (both other writers mirror this on open).
    tx.execute(
        "UPDATE integrations SET type = 'github-cli' WHERE type = 'github'",
        [],
    )?;

    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()?;
    Ok(())
}

/// Version 2: the index the retention purge reads.
///
/// The v1 indexes both lead with `connection_id` / `group_id`, so neither one
/// serves `DELETE FROM query_log WHERE created_at < …`.
fn migrate_v2(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS query_log_created_at_idx ON query_log(created_at);",
    )?;
    tx.pragma_update(None, "user_version", 2)?;
    tx.commit()?;
    Ok(())
}

/// Version 3: the browser control tables.
///
/// Names carry a `browser_` prefix because `jobs`, `drafts` and `artifacts`
/// are too generic to own unprefixed in a database this one shares.
fn migrate_v3(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS browser_jobs (
            id TEXT PRIMARY KEY,
            command_id TEXT NOT NULL UNIQUE,
            platform TEXT NOT NULL,
            action TEXT NOT NULL,
            target_url TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'expired', 'unknown')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            started_at INTEGER,
            finished_at INTEGER,
            error_code TEXT,
            error_message TEXT,
            result_json TEXT,
            dispatch_count INTEGER NOT NULL DEFAULT 0,
            draft_id TEXT
        );
        CREATE INDEX IF NOT EXISTS browser_jobs_status_created_idx ON browser_jobs (status, created_at);
        CREATE INDEX IF NOT EXISTS browser_jobs_recovery_idx ON browser_jobs (action, status, draft_id);

        CREATE TABLE IF NOT EXISTS browser_drafts (
            id TEXT PRIMARY KEY,
            platform TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('reply', 'post')),
            target_url TEXT NOT NULL,
            post_id TEXT,
            text TEXT NOT NULL,
            parts_json TEXT NOT NULL DEFAULT '[]',
            debug INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'submitted', 'failed', 'unknown', 'cancelled', 'expired')),
            created_at INTEGER NOT NULL,
            confirmed_at INTEGER,
            submitted_at INTEGER,
            scheduled_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS browser_drafts_status_idx ON browser_drafts (status, created_at);

        CREATE TABLE IF NOT EXISTS browser_schedule_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            window_start TEXT NOT NULL,
            window_end TEXT NOT NULL,
            min_gap_minutes INTEGER NOT NULL,
            max_gap_minutes INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS browser_schedule_reservations (
            id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL UNIQUE REFERENCES browser_drafts(id) ON DELETE CASCADE,
            platform TEXT NOT NULL,
            scheduled_at INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('reserved', 'committed', 'released', 'unknown')),
            created_at INTEGER NOT NULL,
            committed_at INTEGER,
            released_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS browser_schedule_reservations_platform_idx ON browser_schedule_reservations (platform, status, scheduled_at);

        CREATE TABLE IF NOT EXISTS browser_artifacts (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL REFERENCES browser_jobs(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK (kind IN ('screenshot', 'extract')),
            content_type TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            data BLOB NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS browser_artifacts_job_created_idx ON browser_artifacts (job_id, created_at);
        CREATE INDEX IF NOT EXISTS browser_artifacts_expires_idx ON browser_artifacts (expires_at);
        ",
    )?;

    let defaults = crate::browser::schedule::ScheduleSettings::defaults();
    tx.execute(
        "INSERT OR IGNORE INTO browser_schedule_settings (id, window_start, window_end, min_gap_minutes, max_gap_minutes) VALUES (1, ?, ?, ?, ?)",
        rusqlite::params![
            defaults.window_start,
            defaults.window_end,
            defaults.min_gap_minutes,
            defaults.max_gap_minutes,
        ],
    )?;

    tx.pragma_update(None, "user_version", 3)?;
    tx.commit()?;
    Ok(())
}

/// Version 4: give each browser integration its own pairing token and queue
/// ownership. Existing rows move to the only Wande integration when there is
/// exactly one, otherwise they keep the standalone `browser` owner.
fn migrate_v4(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        ALTER TABLE browser_jobs ADD COLUMN integration_id TEXT NOT NULL DEFAULT 'browser';
        ALTER TABLE browser_drafts ADD COLUMN integration_id TEXT NOT NULL DEFAULT 'browser';

        CREATE INDEX IF NOT EXISTS browser_jobs_integration_status_created_idx
            ON browser_jobs (integration_id, status, created_at);
        CREATE INDEX IF NOT EXISTS browser_drafts_integration_status_idx
            ON browser_drafts (integration_id, status, created_at);

        CREATE TABLE IF NOT EXISTS browser_pairing_tokens (
            integration_id TEXT PRIMARY KEY,
            token TEXT NOT NULL UNIQUE
        );
        ",
    )?;

    tx.execute(
        "UPDATE browser_jobs
         SET integration_id = (SELECT id FROM integrations WHERE type = 'wande' LIMIT 1)
         WHERE integration_id = 'browser'
           AND (SELECT COUNT(*) FROM integrations WHERE type = 'wande') = 1",
        [],
    )?;
    tx.execute(
        "UPDATE browser_drafts
         SET integration_id = (SELECT id FROM integrations WHERE type = 'wande' LIMIT 1)
         WHERE integration_id = 'browser'
           AND (SELECT COUNT(*) FROM integrations WHERE type = 'wande') = 1",
        [],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO browser_pairing_tokens (integration_id, token)
         SELECT integrations.id, settings.value
         FROM integrations
         JOIN settings ON settings.key = 'browser_pairing_token'
         WHERE integrations.type = 'wande'
           AND (SELECT COUNT(*) FROM integrations WHERE type = 'wande') = 1",
        [],
    )?;

    tx.pragma_update(None, "user_version", 4)?;
    tx.commit()?;
    Ok(())
}

/// Version 5: retain the integration owner when a settled draft is pruned.
/// Orphaned reservations from a single Wande installation follow that
/// installation; ambiguous rows keep the standalone browser owner.
fn migrate_v5(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        ALTER TABLE browser_schedule_reservations ADD COLUMN integration_id TEXT NOT NULL DEFAULT 'browser';
        CREATE INDEX IF NOT EXISTS browser_schedule_reservations_integration_status_idx
            ON browser_schedule_reservations (integration_id, status, scheduled_at);
        ",
    )?;
    tx.execute(
        "UPDATE browser_schedule_reservations
         SET integration_id = COALESCE(
             (SELECT browser_drafts.integration_id
              FROM browser_drafts
              WHERE browser_drafts.id = browser_schedule_reservations.draft_id),
             CASE
                 WHEN (SELECT COUNT(*) FROM integrations WHERE type = 'wande') = 1
                 THEN (SELECT id FROM integrations WHERE type = 'wande' LIMIT 1)
                 ELSE 'browser'
             END
         )
         WHERE integration_id = 'browser'",
        [],
    )?;
    tx.pragma_update(None, "user_version", 5)?;
    tx.commit()?;
    Ok(())
}

/// Columns added to `query_log` over time by the TypeScript ALTER loop. Old
/// databases may lack any subset; add exactly what is missing and fail loudly
/// if an ALTER fails for any other reason.
fn ensure_query_log_columns(tx: &Transaction<'_>) -> Result<()> {
    const ADDED_OVER_TIME: [(&str, &str); 7] = [
        ("result_json", "TEXT"),
        ("row_count", "INTEGER"),
        ("source", "TEXT"),
        ("response_text", "TEXT"),
        ("group_id", "TEXT"),
        ("group_name", "TEXT"),
        ("database", "TEXT"),
    ];

    let mut present = std::collections::HashSet::new();
    let mut stmt = tx.prepare("PRAGMA table_info(query_log)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        present.insert(name?);
    }
    drop(stmt);

    for (name, kind) in ADDED_OVER_TIME {
        if !present.contains(name) {
            // `name`/`kind` are compile-time constants, never user input.
            tx.execute(
                &format!("ALTER TABLE query_log ADD COLUMN {name} {kind}"),
                [],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ladder_runs_each_step_once_and_reports_the_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), 0);
        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LADDER.len() as u32);
        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LADDER.len() as u32);
    }

    /// Build a database the way the current TypeScript code leaves one: its
    /// original `CREATE TABLE` statements plus the try/catch ALTER loop,
    /// stopped partway so tail columns are missing, and no `user_version`.
    ///
    /// This is the shape of real `~/.pluk/pluk.db` files written before the
    /// Rust port.
    fn typescript_database(path: &std::path::Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS integrations (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                type TEXT NOT NULL,
                config TEXT NOT NULL DEFAULT '{}',
                environment TEXT DEFAULT 'development',
                read_only INTEGER NOT NULL DEFAULT 0,
                query_policy TEXT,
                token TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            UPDATE integrations SET type = 'github-cli' WHERE type = 'github';

            CREATE TABLE IF NOT EXISTS groups (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                environment TEXT DEFAULT 'production',
                member_ids TEXT NOT NULL DEFAULT '[]',
                token TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS query_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                connection_id TEXT NOT NULL,
                connection_name TEXT NOT NULL,
                sql TEXT NOT NULL,
                verdict TEXT NOT NULL,
                reason TEXT,
                categories TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        // The ALTER loop, applied up to `applied` entries, swallowing errors
        // exactly like the TS startup does.
        let alters = [
            "ALTER TABLE query_log ADD COLUMN result_json TEXT",
            "ALTER TABLE query_log ADD COLUMN row_count INTEGER",
            "ALTER TABLE query_log ADD COLUMN source TEXT",
            "ALTER TABLE query_log ADD COLUMN response_text TEXT",
            "ALTER TABLE query_log ADD COLUMN group_id TEXT",
            "ALTER TABLE query_log ADD COLUMN group_name TEXT",
            "ALTER TABLE query_log ADD COLUMN database TEXT",
        ];
        for sql in &alters[..4] {
            let _ = db.execute(sql, []);
        }

        // Seed data a pre-port database really holds: a retired-adapter
        // integration, a group with legacy bare-string members, one log row.
        db.execute(
            "INSERT INTO integrations (id, name, type, config, environment, read_only, token)
             VALUES ('abcd1234abcd1234', 'Main DB', 'github', '{\"host\":\"db.local\"}', 'production', 0, 'pluk_olddatabase000000000000000')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO groups (id, name, environment, member_ids, token)
             VALUES ('group0000group0000', 'All', NULL, '[\"abcd1234abcd1234\",\"vanished1\"]', 'pluk_oldgroup000000000000000000')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO query_log (connection_id, connection_name, sql, verdict, categories)
             VALUES ('abcd1234abcd1234', 'Main DB', 'SELECT 1', 'allowed', 'read')",
            [],
        )
        .unwrap();
        db
    }

    #[test]
    fn migrates_a_typescript_created_database_without_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pluk.db");
        drop(typescript_database(&path));

        let mut conn = Connection::open(&path).unwrap();
        run(&mut conn).unwrap();

        // Version stamped, all tail columns completed, new tables created.
        assert_eq!(current_version(&conn).unwrap(), LADDER.len() as u32);
        let columns: HashSet<String> = columns_of(&conn, "query_log");
        for name in [
            "result_json",
            "row_count",
            "source",
            "response_text",
            "group_id",
            "group_name",
            "database",
        ] {
            assert!(columns.contains(name), "missing column {name}");
        }
        for table in ["masked_columns", "saved_queries", "saved_commands"] {
            let found = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap();
            assert_eq!(found, 1, "table {table}");
        }
        // The retired GitHub REST adapter id is rekeyed like every writer does.
        let kind: String = conn
            .query_row("SELECT type FROM integrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kind, "github-cli");
    }

    #[test]
    fn migrated_typescript_rows_read_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pluk.db");
        drop(typescript_database(&path));

        let store = crate::Store::open(&path).unwrap();
        let integration = store.list_integrations().unwrap().remove(0);
        assert_eq!(integration.r#type, "github-cli");
        assert_eq!(integration.config["host"], serde_json::json!("db.local"));

        let group = store.list_groups().unwrap().remove(0);
        assert_eq!(
            group.environment, None,
            "legacy NULL environment stays unscoped"
        );
        let member_ids: Vec<String> = group.members.iter().map(|m| m.id.clone()).collect();
        assert_eq!(member_ids, vec!["abcd1234abcd1234", "vanished1"]);

        let page = store
            .read_log_page(
                &crate::LogScope::Connection("abcd1234abcd1234".into()),
                crate::LogRange::All,
                None,
            )
            .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].sql, "SELECT 1");

        // Reopening is idempotent.
        drop(store);
        let mut conn = Connection::open(&path).unwrap();
        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LADDER.len() as u32);
        assert_eq!(
            columns_of(&conn, "query_log").len(),
            15,
            "no duplicate columns added"
        );
    }

    #[test]
    fn reservation_migration_keeps_orphaned_rows_with_the_only_wande() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_v1(&mut conn).unwrap();
        migrate_v2(&mut conn).unwrap();
        migrate_v3(&mut conn).unwrap();
        migrate_v4(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO integrations (id, name, type, token) VALUES ('wande-1', 'Wande', 'wande', 'token-1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO browser_drafts (id, platform, kind, target_url, text, status, created_at, integration_id) VALUES ('draft-1', 'x', 'post', 'https://x.com/compose/post', 'Saved post', 'submitted', 100, 'wande-1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO browser_schedule_reservations (id, draft_id, platform, scheduled_at, status, created_at) VALUES ('reservation-1', 'draft-1', 'x', 200, 'committed', 100)",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO browser_schedule_reservations (id, draft_id, platform, scheduled_at, status, created_at) VALUES ('reservation-2', 'missing-draft', 'x', 300, 'released', 100)",
            [],
        )
        .unwrap();

        migrate_v5(&mut conn).unwrap();

        let owners: Vec<(String, String)> = {
            let mut statement = conn
                .prepare("SELECT id, integration_id FROM browser_schedule_reservations ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        };
        assert_eq!(
            owners,
            vec![
                ("reservation-1".to_owned(), "wande-1".to_owned()),
                ("reservation-2".to_owned(), "wande-1".to_owned()),
            ]
        );
        assert_eq!(current_version(&conn).unwrap(), 5);
    }

    fn columns_of(conn: &Connection, table: &str) -> HashSet<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn fresh_schema_matches_the_shared_contract_exactly() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        let expected: &[&str] = &[
            "integrations",
            "groups",
            "query_log",
            "settings",
            "masked_columns",
            "saved_queries",
            "saved_commands",
            "sqlite_sequence",
        ];
        let tables: HashSet<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap();
            rows_unwrap(stmt.query_map([], |r| r.get::<_, String>(0)).unwrap())
        };
        for name in expected {
            assert!(tables.contains(*name), "missing table {name}");
        }

        let integrations = columns_of(&conn, "integrations");
        for name in [
            "id",
            "name",
            "type",
            "config",
            "environment",
            "read_only",
            "query_policy",
            "token",
            "created_at",
        ] {
            assert!(integrations.contains(name));
        }
        let groups = columns_of(&conn, "groups");
        for name in [
            "id",
            "name",
            "environment",
            "member_ids",
            "token",
            "created_at",
        ] {
            assert!(groups.contains(name));
        }
        // The legacy flag must stay populated-by-default for schema compatibility.
        let read_only_default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('integrations') WHERE name='read_only'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(read_only_default, "0");

        let indexes: HashSet<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'query_log%'",
                )
                .unwrap();
            rows_unwrap(stmt.query_map([], |r| r.get::<_, String>(0)).unwrap())
        };
        assert!(indexes.contains("query_log_connection_time_id_idx"));
        assert!(indexes.contains("query_log_group_time_id_idx"));
    }

    fn rows_unwrap(
        rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
    ) -> HashSet<String> {
        rows.map(|r| r.unwrap()).collect()
    }
}
