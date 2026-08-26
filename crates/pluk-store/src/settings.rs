//! Key/value settings (`settings` table) and the retention window.

use rusqlite::OptionalExtension;

use crate::Store;
use crate::error::Result;

/// Default log retention in days, matching both existing readers.
const DEFAULT_RETENTION_DAYS: i64 = 30;

pub const LOG_RETENTION_DAYS_KEY: &str = "log_retention_days";
/// The SwiftUI app's SSE resume high-water mark; owned by the app, stored here.
pub const LOG_CURSOR_KEY: &str = "log_cursor";

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
}
