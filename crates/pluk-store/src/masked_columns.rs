//! Masked columns: result columns whose values never reach agents raw.
//!
//! Ids are full-dashed UUIDs (`crypto.randomUUID()` in the TypeScript
//! sources) — unlike integrations and groups, whose 16-char ids come from
//! [`crate::ids`].

use rusqlite::{Row, params};

use crate::Store;
use crate::error::Result;
use crate::models::MaskedColumn;

fn map_masked(row: &Row<'_>) -> rusqlite::Result<MaskedColumn> {
    Ok(MaskedColumn {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        column_name: row.get(2)?,
        created_at: row.get(3)?,
    })
}

impl Store {
    /// Column names masked for one connection, alphabetically (viewer order).
    pub fn list_masked_columns(&self, connection_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT column_name FROM masked_columns WHERE connection_id = ? ORDER BY column_name",
        )?;
        let rows = stmt.query_map([connection_id], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?)
    }

    /// Mask one more column. A duplicate of an existing pair is rejected by
    /// the table's UNIQUE constraint — surfaced as an error for the API layer.
    pub fn add_masked_column(
        &self,
        connection_id: &str,
        column_name: &str,
    ) -> Result<MaskedColumn> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO masked_columns (id, connection_id, column_name) VALUES (?, ?, ?)",
            params![id, connection_id, column_name],
        )?;
        Ok(conn.query_row(
            "SELECT id, connection_id, column_name, created_at FROM masked_columns WHERE id = ?",
            [&id],
            map_masked,
        )?)
    }

    pub fn remove_masked_column(&self, connection_id: &str, column_name: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "DELETE FROM masked_columns WHERE connection_id = ? AND column_name = ?",
            [connection_id, column_name],
        )? > 0)
    }
}
