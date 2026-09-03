//! Saved commands: user-curated shell commands for SSH integrations, run by
//! name through the `run_saved_command` tool. There is no allowlist beyond
//! this table — saved commands run unrestricted as the connecting SSH user,
//! exactly like an ad-hoc command; the MCP confirm prompt is the only gate.
//!
//! Ids are full-dashed UUIDs (`crypto.randomUUID()` in the TypeScript
//! sources) — unlike integrations and groups, whose 16-char ids come from
//! [`crate::ids`].

use rusqlite::{OptionalExtension, Row, params};

use crate::Store;
use crate::error::Result;
use crate::models::SavedCommand;

/// Everything a caller supplies; `id` and `created_at` are minted here.
#[derive(Debug, Clone)]
pub struct SavedCommandInput {
    pub connection_id: String,
    pub name: String,
    pub command: String,
    pub working_dir: Option<String>,
}

fn map_saved_command(row: &Row<'_>) -> rusqlite::Result<SavedCommand> {
    Ok(SavedCommand {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        name: row.get(2)?,
        command: row.get(3)?,
        working_dir: row.get(4)?,
        created_at: row.get(5)?,
    })
}

impl Store {
    pub fn list_saved_commands(&self, connection_id: &str) -> Result<Vec<SavedCommand>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT id, connection_id, name, command, working_dir, created_at FROM saved_commands
             WHERE connection_id = ? ORDER BY name",
        )?;
        let rows = stmt.query_map([connection_id], map_saved_command)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?)
    }

    pub fn get_saved_command(
        &self,
        connection_id: &str,
        name: &str,
    ) -> Result<Option<SavedCommand>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(
                "SELECT id, connection_id, name, command, working_dir, created_at FROM saved_commands
                 WHERE connection_id = ? AND name = ?",
                [connection_id, name],
                map_saved_command,
            )
            .optional()?)
    }

    /// Save a command. A duplicate `(connection_id, name)` surfaces the UNIQUE
    /// violation to the caller.
    pub fn create_saved_command(&self, input: &SavedCommandInput) -> Result<SavedCommand> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO saved_commands (id, connection_id, name, command, working_dir)
             VALUES (?, ?, ?, ?, ?)",
            params![
                id,
                input.connection_id,
                input.name,
                input.command,
                input.working_dir
            ],
        )?;
        Ok(conn.query_row(
            "SELECT id, connection_id, name, command, working_dir, created_at FROM saved_commands WHERE id = ?",
            [&id],
            map_saved_command,
        )?)
    }

    pub fn delete_saved_command(&self, connection_id: &str, name: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "DELETE FROM saved_commands WHERE connection_id = ? AND name = ?",
            [connection_id, name],
        )? > 0)
    }
}
