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

const LADDER: &[Step] = &[migrate_v1];

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
        assert_eq!(current_version(&conn).unwrap(), 1);
        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 1);
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
        assert_eq!(current_version(&conn).unwrap(), 1);
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
        assert_eq!(current_version(&conn).unwrap(), 1);
        assert_eq!(
            columns_of(&conn, "query_log").len(),
            15,
            "no duplicate columns added"
        );
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
