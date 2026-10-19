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

const LADDER: &[Step] = &[
    migrate_v1,
    migrate_v2,
    retired::<3>,
    retired::<4>,
    retired::<5>,
    retired::<6>,
    retired::<7>,
    retired::<8>,
    retired::<9>,
    retired::<10>,
    migrate_v11,
    migrate_v12,
    migrate_v13,
    migrate_v14,
    migrate_v15,
];

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

/// Versions 3 to 10 built the Wande browser tables. Wande no longer lives in
/// Pluk, so a new database steps past them and version 15 drops whatever an
/// older one built.
fn retired<const VERSION: u32>(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "user_version", VERSION)?;
    Ok(())
}

/// Version 11: the MCP proxy's own tables — the tools discovered on an
/// upstream server, and the upstream credentials that reach it.
///
/// Credentials live here rather than in `integrations.config` because that
/// blob is handed to the UI whole; only the proxy adapter reads this table.
fn migrate_v11(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        CREATE TABLE proxy_tools (
            integration_id TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            schema_json TEXT NOT NULL DEFAULT '{}',
            annotations_json TEXT,
            content_hash TEXT NOT NULL,
            approved_hash TEXT,
            present INTEGER NOT NULL DEFAULT 1,
            discovered_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (integration_id, name)
        );

        CREATE TABLE proxy_auth (
            integration_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            access_token TEXT NOT NULL,
            refresh_token TEXT,
            expires_at INTEGER,
            client_id TEXT,
            client_secret TEXT,
            metadata_json TEXT,
            status TEXT NOT NULL CHECK (status IN ('connected', 'reconnect_needed')),
            version INTEGER NOT NULL DEFAULT 1
        );
        ",
    )?;
    tx.pragma_update(None, "user_version", 11)?;
    tx.commit()?;
    Ok(())
}

/// Version 12: the secret values of a proxied server's header and environment
/// rows, kept out of `integrations.config` for the reason `proxy_auth` is.
fn migrate_v12(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        CREATE TABLE proxy_secrets (
            integration_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('header', 'env')),
            name TEXT NOT NULL,
            value TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (integration_id, kind, name)
        );
        ",
    )?;
    tx.pragma_update(None, "user_version", 12)?;
    tx.commit()?;
    Ok(())
}

/// Version 13: the launch a user approved for a local MCP server, by hash.
///
/// Kept out of `integrations.config` because the window writes that blob
/// whole, and an approval must only ever come from the user approving it.
fn migrate_v13(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        CREATE TABLE proxy_launch_approvals (
            integration_id TEXT PRIMARY KEY,
            launch_hash TEXT NOT NULL,
            approved_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )?;
    tx.pragma_update(None, "user_version", 13)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v14(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch("ALTER TABLE query_log ADD COLUMN duration_ms INTEGER;")?;
    tx.pragma_update(None, "user_version", 14)?;
    tx.commit()?;
    Ok(())
}

/// Version 15: Wande moved out of Pluk. Its integrations go, along with the
/// browser tables and the pairing setting it kept. Their old activity stays
/// in the query log, as it does for any deleted integration.
fn migrate_v15(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "
        DELETE FROM integrations WHERE type = 'wande';
        DELETE FROM settings WHERE key = 'browser_pairing_token';

        DROP TABLE IF EXISTS browser_artifacts;
        DROP TABLE IF EXISTS browser_draft_images;
        DROP TABLE IF EXISTS browser_schedule_reservations;
        DROP TABLE IF EXISTS browser_schedule_settings;
        DROP TABLE IF EXISTS browser_jobs;
        DROP TABLE IF EXISTS browser_drafts;
        DROP TABLE IF EXISTS browser_pairing_tokens;
        ",
    )?;
    tx.pragma_update(None, "user_version", 15)?;
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
        let page = crate::Store::open(&path).unwrap()
            .read_log_page(&crate::LogScope::Connection("abcd1234abcd1234".into()), crate::LogRange::All, None).unwrap();
        assert_eq!(page.entries[0].sql, "SELECT 1");
        assert_eq!(page.entries[0].duration_ms, None);
    }

    #[test]
    fn upgrades_v10_with_the_proxy_tables_and_opens_a_fresh_database_on_them() {
        let mut upgraded = Connection::open_in_memory().unwrap();
        for step in &LADDER[..10] {
            step(&mut upgraded).unwrap();
        }
        migrate_v11(&mut upgraded).unwrap();
        let mut fresh = Connection::open_in_memory().unwrap();
        run(&mut fresh).unwrap();

        let tables: [(&str, &[&str]); 2] = [
            (
                "proxy_tools",
                &[
                    "integration_id",
                    "name",
                    "description",
                    "schema_json",
                    "annotations_json",
                    "content_hash",
                    "approved_hash",
                    "present",
                    "discovered_at",
                    "updated_at",
                ],
            ),
            (
                "proxy_auth",
                &[
                    "integration_id",
                    "kind",
                    "access_token",
                    "refresh_token",
                    "expires_at",
                    "client_id",
                    "client_secret",
                    "metadata_json",
                    "status",
                    "version",
                ],
            ),
        ];
        assert_eq!(current_version(&upgraded).unwrap(), 11);
        assert_eq!(current_version(&fresh).unwrap(), LADDER.len() as u32);
        for conn in [&upgraded, &fresh] {
            for (table, columns) in tables {
                let expected: HashSet<String> =
                    columns.iter().map(|name| (*name).to_owned()).collect();
                assert_eq!(columns_of(conn, table), expected, "{table}");
            }
        }
    }

    #[test]
    fn upgrades_v11_with_the_proxy_secrets_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        for step in &LADDER[..11] {
            step(&mut conn).unwrap();
        }
        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LADDER.len() as u32);
        let expected: HashSet<String> = ["integration_id", "kind", "name", "value", "version"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(columns_of(&conn, "proxy_secrets"), expected);
        let refused = conn.execute(
            "INSERT INTO proxy_secrets (integration_id, kind, name, value) VALUES ('i', 'cookie', 'n', 'v')",
            [],
        );
        assert!(refused.is_err(), "only header and env rows are kept");
    }

    /// A version-14 database from a Pluk that still carried Wande: a Wande
    /// integration beside an SSH one, its pairing token and setting, and
    /// browser rows linked by foreign key. The browser tables keep only the
    /// columns those links need.
    fn wande_era_database(path: &std::path::Path) {
        let mut conn = Connection::open(path).unwrap();
        for step in &LADDER[..14] {
            step(&mut conn).unwrap();
        }
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE browser_jobs (id TEXT PRIMARY KEY, integration_id TEXT NOT NULL);
            CREATE TABLE browser_artifacts (id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES browser_jobs(id) ON DELETE CASCADE);
            CREATE TABLE browser_drafts (id TEXT PRIMARY KEY, integration_id TEXT NOT NULL);
            CREATE TABLE browser_draft_images (id TEXT PRIMARY KEY, draft_id TEXT NOT NULL REFERENCES browser_drafts(id) ON DELETE CASCADE);
            CREATE TABLE browser_schedule_reservations (id TEXT PRIMARY KEY, draft_id TEXT NOT NULL UNIQUE REFERENCES browser_drafts(id) ON DELETE CASCADE);
            CREATE TABLE browser_schedule_settings (id INTEGER PRIMARY KEY CHECK (id = 1));
            CREATE TABLE browser_pairing_tokens (integration_id TEXT PRIMARY KEY, token TEXT NOT NULL UNIQUE);

            INSERT INTO integrations (id, name, type, config, token)
                VALUES ('ssh-1', 'Prod box', 'ssh', '{\"host\":\"prod.local\"}', 'pluk_ssh_token');
            INSERT INTO integrations (id, name, type, token)
                VALUES ('wande-1', 'Socials', 'wande', 'pluk_wande_token');
            INSERT INTO settings (key, value) VALUES ('browser_pairing_token', 'pairing');
            INSERT INTO settings (key, value) VALUES ('log_retention_days', '7');
            INSERT INTO browser_pairing_tokens VALUES ('wande-1', 'pairing');
            INSERT INTO browser_jobs VALUES ('job-1', 'wande-1');
            INSERT INTO browser_artifacts VALUES ('artifact-1', 'job-1');
            INSERT INTO browser_drafts VALUES ('draft-1', 'wande-1');
            INSERT INTO browser_draft_images VALUES ('image-1', 'draft-1');
            INSERT INTO browser_schedule_reservations VALUES ('reservation-1', 'draft-1');
            INSERT INTO browser_schedule_settings VALUES (1);
            ",
        )
        .unwrap();
    }

    #[test]
    fn a_store_saved_with_wande_opens_and_lists_the_other_integrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pluk.db");
        wande_era_database(&path);

        let store = crate::Store::open(&path).unwrap();
        let listed: Vec<(String, String)> = store
            .list_integrations()
            .unwrap()
            .into_iter()
            .map(|integration| (integration.name, integration.r#type))
            .collect();
        assert_eq!(listed, [("Prod box".to_owned(), "ssh".to_owned())]);
        let ssh = store.integration_by_token("pluk_ssh_token").unwrap().unwrap();
        assert_eq!(ssh.config["host"], serde_json::json!("prod.local"));
        assert!(store.integration_by_token("pluk_wande_token").unwrap().is_none());
        assert_eq!(store.get_setting("browser_pairing_token").unwrap(), None);
        assert_eq!(store.retention_days().unwrap(), 7);
        drop(store);

        let conn = Connection::open(&path).unwrap();
        let browser_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'browser\\_%' ESCAPE '\\'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(browser_tables, 0);
        drop(conn);

        let reopened = crate::Store::open(&path).unwrap();
        assert_eq!(reopened.list_integrations().unwrap().len(), 1);
    }

    fn columns_of(conn: &Connection, table: &str) -> HashSet<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

}
