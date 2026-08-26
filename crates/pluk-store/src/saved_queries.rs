//! Saved queries: user-stored SQL statements, run by name from the UI.
//!
//! Ids are full-dashed UUIDs (`crypto.randomUUID()` in the TypeScript
//! sources) — unlike integrations and groups, whose 16-char ids come from
//! [`crate::ids`].

use rusqlite::{OptionalExtension, Row, params};

use crate::Store;
use crate::error::Result;
use crate::models::SavedQuery;

/// Everything a caller supplies; `id` and `created_at` are minted here.
#[derive(Debug, Clone)]
pub struct SavedQueryInput {
    pub connection_id: String,
    pub name: String,
    pub sql: String,
}

fn map_saved_query(row: &Row<'_>) -> rusqlite::Result<SavedQuery> {
    Ok(SavedQuery {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        name: row.get(2)?,
        sql: row.get(3)?,
        created_at: row.get(4)?,
    })
}

impl Store {
    pub fn list_saved_queries(&self, connection_id: &str) -> Result<Vec<SavedQuery>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT id, connection_id, name, sql, created_at FROM saved_queries
             WHERE connection_id = ? ORDER BY name",
        )?;
        let rows = stmt.query_map([connection_id], map_saved_query)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?)
    }

    pub fn get_saved_query(&self, connection_id: &str, name: &str) -> Result<Option<SavedQuery>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(
                "SELECT id, connection_id, name, sql, created_at FROM saved_queries
                 WHERE connection_id = ? AND name = ?",
                [connection_id, name],
                map_saved_query,
            )
            .optional()?)
    }

    /// Save a query. A duplicate `(connection_id, name)` surfaces the UNIQUE
    /// violation to the caller.
    pub fn create_saved_query(&self, input: &SavedQueryInput) -> Result<SavedQuery> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO saved_queries (id, connection_id, name, sql) VALUES (?, ?, ?, ?)",
            params![id, input.connection_id, input.name, input.sql],
        )?;
        Ok(conn.query_row(
            "SELECT id, connection_id, name, sql, created_at FROM saved_queries WHERE id = ?",
            [&id],
            map_saved_query,
        )?)
    }

    pub fn delete_saved_query(&self, connection_id: &str, name: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "DELETE FROM saved_queries WHERE connection_id = ? AND name = ?",
            [connection_id, name],
        )? > 0)
    }
}
