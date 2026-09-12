//! Key/value settings (`settings` table) and the retention window.

use rusqlite::OptionalExtension;

use crate::Store;
use crate::error::Result;

/// Default log retention in days, matching both existing readers.
const DEFAULT_RETENTION_DAYS: i64 = 30;

pub const LOG_RETENTION_DAYS_KEY: &str = "log_retention_days";
/// SSE resume high-water mark for the log stream.
pub const LOG_CURSOR_KEY: &str = "log_cursor";
/// The Pluk ID the browser extension presents to `/wande/...`.
pub const BROWSER_PAIRING_TOKEN_KEY: &str = "browser_pairing_token";

impl Store {
    /// Read one setting; `None` when unset. Callers apply their own defaults.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row("SELECT value FROM settings WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Write one setting (insert or replace).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
            [key, value],
        )?;
        Ok(())
    }

    /// Days of query-log history to keep. Zero means keep forever.
    pub fn retention_days(&self) -> Result<i64> {
        Ok(self
            .get_setting(LOG_RETENTION_DAYS_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RETENTION_DAYS))
    }

    pub fn set_retention_days(&self, days: i64) -> Result<()> {
        self.set_setting(LOG_RETENTION_DAYS_KEY, &days.to_string())
    }

    /// The Pluk ID the browser extension pairs with, minted on first read so
    /// the app never ships a default one.
    pub fn browser_pairing_token(&self) -> Result<String> {
        if let Some(existing) = self.get_setting(BROWSER_PAIRING_TOKEN_KEY)? {
            return Ok(existing);
        }
        let token = crate::ids::new_token();
        self.set_setting(BROWSER_PAIRING_TOKEN_KEY, &token)?;
        Ok(token)
    }

    /// Read or mint the pairing token owned by one browser integration.
    pub fn browser_pairing_token_for(&self, integration_id: &str) -> Result<String> {
        let conn = self.conn.lock().expect("store lock");
        let existing = conn
            .query_row(
                "SELECT token FROM browser_pairing_tokens WHERE integration_id = ?",
                [integration_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(token) = existing {
            return Ok(token);
        }
        let token = crate::ids::new_token();
        conn.execute(
            "INSERT INTO browser_pairing_tokens (integration_id, token) VALUES (?, ?)",
            rusqlite::params![integration_id, token],
        )?;
        Ok(token)
    }

    /// Resolve a browser token to its integration, when it is persisted.
    pub fn browser_integration_id_by_token(&self, token: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(
                "SELECT integration_id FROM browser_pairing_tokens WHERE token = ?",
                [token],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn delete_browser_pairing_token(&self, integration_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "DELETE FROM browser_pairing_tokens WHERE integration_id = ?",
            [integration_id],
        )?;
        Ok(())
    }
}
