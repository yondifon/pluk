//! The query audit log: writes, retention, and keyset-paged reads.
//!
//! Mirrors `pluk/src/store/queryLog.ts`, including the two storage caps
//! (`result_json` holds at most [`LOG_RESULT_ROWS`] rows; `response_text` is
//! truncated at [`LOG_RESPONSE_LIMIT`] characters with the same marker the TS
//! viewer recognizes) and the exact keyset-page contract behind today's
//! `GET /api/logs`.

use std::fmt;
use std::sync::Arc;

use rusqlite::types::{ToSql, ToSqlOutput};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Store;
use crate::error::Result;
use crate::models::{LogEntry, Verdict};

/// Max rows kept in one log entry's `result_json`.
pub const LOG_RESULT_ROWS: usize = 100;
/// Max characters kept of one entry's raw agent-visible response. The
/// TypeScript server capped by UTF-16 length; characters are the closest Rust
/// measure, and the marker below must stay byte-identical because the existing
/// log viewers render it.
pub const LOG_RESPONSE_LIMIT: usize = 100_000;
const TRUNCATION_MARKER: &str = "\n…[truncated]";
/// Page size of the keyset-paged log reads (the `/api/logs` contract).
pub const LOG_PAGE_SIZE: usize = 100;

/// The group a call was routed through, recorded on its log row so the group
/// view can show every member's activity in one place. Absent for calls that
/// hit an integration's own endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct LogGroup {
    pub id: String,
    pub name: String,
}

/// A new pending log entry, finalized later via [`Store::update_log_entry`].
#[derive(Debug, Clone, Default)]
pub struct LogDraft {
    pub connection_id: String,
    pub connection_name: String,
    pub sql: String,
    /// New entries start `pending`; blocked/error paths pass their verdict here.
    pub verdict: Verdict,
    /// Comma-separated statement categories.
    pub categories: Option<String>,
    pub reason: Option<String>,
    /// Originating tool or operation (e.g. `query`, `list_tables`).
    pub source: Option<String>,
    pub group: Option<LogGroup>,
    /// Target database when a call selects one (multi-db connections).
    pub database: Option<String>,
}

impl LogDraft {
    pub fn new(
        connection_id: impl Into<String>,
        connection_name: impl Into<String>,
        sql: impl Into<String>,
    ) -> Self {
        LogDraft {
            connection_id: connection_id.into(),
            connection_name: connection_name.into(),
            sql: sql.into(),
            ..Default::default()
        }
    }

    pub fn with_verdict(mut self, verdict: Verdict) -> Self {
        self.verdict = verdict;
        self
    }

    pub fn with_group(mut self, group: LogGroup) -> Self {
        self.group = Some(group);
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Fields captured from a successful call, stored capped alongside the total
/// row count observed before capping.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub fields: Vec<String>,
    pub rows: Vec<serde_json::Value>,
}

/// A finalized verdict plus optional payload for an existing entry.
#[derive(Debug, Clone, Default)]
pub struct LogUpdate {
    /// Replace the recorded SQL (used when a multi-statement script is split).
    pub sql: Option<String>,
    pub verdict: Verdict,
    pub reason: Option<String>,
    pub result: Option<QueryResult>,
    pub response_text: Option<String>,
}

impl LogUpdate {
    pub fn new(verdict: Verdict) -> Self {
        LogUpdate {
            verdict,
            ..Default::default()
        }
    }
}

/// Which entity a log read is scoped to. Exactly one per request.
#[derive(Debug, Clone, PartialEq)]
pub enum LogScope {
    Connection(String),
    Group(String),
}

/// Time window of a log read. Cutoffs are evaluated by SQLite against the same
/// clock that stamps `created_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogRange {
    Hour,
    Today,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
    All,
}

impl LogRange {
    /// The SQL cutoff expression, mirroring the TypeScript `RANGE_CUTOFFS`
    /// verbatim (`today` is local midnight converted back to UTC).
    fn cutoff_sql(self) -> Option<&'static str> {
        Some(match self {
            LogRange::Hour => "datetime('now', '-1 hour')",
            LogRange::Today => "datetime('now', 'localtime', 'start of day', 'utc')",
            LogRange::SevenDays => "datetime('now', '-7 days')",
            LogRange::ThirtyDays => "datetime('now', '-30 days')",
            LogRange::All => return None,
        })
    }
}

impl fmt::Display for LogRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LogRange::Hour => "hour",
            LogRange::Today => "today",
            LogRange::SevenDays => "7d",
            LogRange::ThirtyDays => "30d",
            LogRange::All => "all",
        })
    }
}

/// Keyset cursor: the last row seen, ordered `(created_at DESC, id DESC)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCursor {
    pub created_at: String,
    pub id: i64,
}

/// One page of log entries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub entries: Vec<LogEntry>,
    pub next_cursor: Option<LogCursor>,
    pub has_more: bool,
}

/// One dynamically-bound parameter of the paged query.
enum Param {
    Text(String),
    Int(i64),
}

impl ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Param::Text(s) => s.to_sql(),
            Param::Int(i) => i.to_sql(),
        }
    }
}

const ENTRY_COLUMNS: &str = "id, connection_id, connection_name, sql, verdict, reason, categories, source, \
     result_json, row_count, response_text, group_id, group_name, database, created_at";

//
// Subscribers learn about every new or updated log row the moment it is
// written, so the app can update its log views without polling. The feed is
// cursor-based: the row id is monotonic, and a subscriber that comes in late
// can be caught up with `log_rows_after`. Heavy fields (`result_json`,
// `response_text`) stay in the shared DB — the app re-reads rows from there;
// the feed carries only what a collapsed log row needs.

/// A collapsed log row, as pushed to activity subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogActivity {
    pub id: i64,
    pub connection_id: String,
    pub connection_name: String,
    pub sql: String,
    pub verdict: String,
    pub reason: Option<String>,
    pub categories: Option<String>,
    pub source: Option<String>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub database: Option<String>,
    pub row_count: Option<i64>,
    pub created_at: String,
}

/// Called with each written row. Must not block for long: it runs inline on
/// the writer's thread.
pub type ActivityHandler = Arc<dyn Fn(&LogActivity) + Send + Sync>;

#[derive(Default)]
pub(crate) struct ActivityFeed {
    next_subscription: u64,
    handlers: Vec<(u64, ActivityHandler)>,
}

impl ActivityFeed {
    fn subscribe(&mut self, handler: ActivityHandler) -> u64 {
        let id = self.next_subscription;
        self.next_subscription += 1;
        self.handlers.push((id, handler));
        id
    }

    fn unsubscribe(&mut self, subscription: u64) {
        self.handlers.retain(|(id, _)| *id != subscription);
    }

    fn dispatch(&self, row: &LogActivity) {
        for (_, handler) in &self.handlers {
            handler(row);
        }
    }
}

const ACTIVITY_COLUMNS: &str = "id, connection_id, connection_name, sql, verdict, reason, categories, source, \
     group_id, group_name, database, row_count, created_at";

fn map_activity(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogActivity> {
    Ok(LogActivity {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        connection_name: row.get(2)?,
        sql: row.get(3)?,
        verdict: row.get(4)?,
        reason: row.get(5)?,
        categories: row.get(6)?,
        source: row.get(7)?,
        group_id: row.get(8)?,
        group_name: row.get(9)?,
        database: row.get(10)?,
        row_count: row.get(11)?,
        created_at: row.get(12)?,
    })
}

impl Store {
    /// Insert a new log entry and return its row id for later finalization.
    ///
    /// Retention runs opportunistically before returning (throttled to at
    /// most once every fifteen minutes).
    pub fn create_log_entry(&self, draft: LogDraft) -> Result<i64> {
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO query_log (connection_id, connection_name, sql, verdict, reason, categories, source, group_id, group_name, database)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                draft.connection_id,
                draft.connection_name,
                draft.sql,
                draft.verdict.as_str(),
                draft.reason,
                draft.categories,
                draft.source,
                draft.group.as_ref().map(|g| g.id.as_str()),
                draft.group.as_ref().map(|g| g.name.as_str()),
                draft.database,
            ],
        )?;
        let id = conn.last_insert_rowid();
        drop(conn);
        self.purge_if_due()?;
        self.notify_activity(id);
        Ok(id)
    }

    /// Finalize an entry: verdict, reason, and optionally the packed result /
    /// response payload. `row_count` records the pre-cap total so viewers can
    /// say "showing 100 of 4,312".
    pub fn update_log_entry(&self, id: i64, update: LogUpdate) -> Result<()> {
        let result_json = update.result.as_ref().map(pack_result);
        let row_count = update.result.as_ref().map(|r| r.rows.len() as i64);
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "UPDATE query_log
             SET sql = COALESCE(?, sql), verdict = ?, reason = ?, result_json = ?, row_count = ?, response_text = ?
             WHERE id = ?",
            rusqlite::            params![
                update.sql,
                update.verdict.as_str(),
                update.reason,
                result_json,
                row_count,
                update.response_text.as_deref().map(cap_response),
                id,
            ],
        )?;
        drop(conn);
        self.notify_activity(id);
        Ok(())
    }

    /// Subscribe to the activity feed; every later write (insert or update)
    /// calls `handler` with the row. Returns a subscription id to pass to
    /// [`Store::unsubscribe_log_activity`].
    pub fn subscribe_log_activity(&self, handler: ActivityHandler) -> u64 {
        let mut feed = self.activity.lock().expect("activity feed");
        feed.subscribe(handler)
    }

    pub fn unsubscribe_log_activity(&self, subscription: u64) {
        let mut feed = self.activity.lock().expect("activity feed");
        feed.unsubscribe(subscription);
    }

    /// Read one row back in its light activity shape and hand it to every
    /// subscriber. Best-effort: a read or dispatch failure never fails the
    /// write that triggered it.
    fn notify_activity(&self, id: i64) {
        if id <= 0 {
            return;
        }
        let feed = self.activity.lock().expect("activity feed");
        if feed.handlers.is_empty() {
            return;
        }
        let row = {
            let conn = self.conn.lock().expect("store lock");
            conn.query_row(
                &format!("SELECT {ACTIVITY_COLUMNS} FROM query_log WHERE id = ?"),
                [id],
                map_activity,
            )
            .ok()
        };
        if let Some(row) = row {
            feed.dispatch(&row);
        }
    }

    /// Delete every log entry older than the retention window. Zero or
    /// negative retention means keep forever. Returns the rows removed.
    pub fn purge_old_logs(&self) -> Result<usize> {
        let days = self.retention_days()?;
        if days <= 0 {
            return Ok(0);
        }
        let conn = self.conn.lock().expect("store lock");
        let deleted = conn.execute(
            "DELETE FROM query_log WHERE created_at < datetime('now', ?)",
            [format!("-{days} days")],
        )?;
        Ok(deleted)
    }

    /// Highest log row id — the SSE feed's high-water mark.
    pub fn log_high_water(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("store lock");
        Ok(
            conn.query_row("SELECT COALESCE(MAX(id), 0) FROM query_log", [], |row| {
                row.get(0)
            })?,
        )
    }

    /// Every log row after `after` id, ascending — catch-up reads for the
    /// activity feed, carrying only what a collapsed row needs plus the heavy
    /// columns the app re-reads anyway.
    pub fn log_rows_after(&self, after: i64) -> Result<Vec<LogEntry>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM query_log WHERE id > ? ORDER BY id ASC"
        ))?;
        let rows = stmt.query_map([after], map_entry)?;
        Ok(rows.collect::<std::result::Result<Vec<LogEntry>, rusqlite::Error>>()?)
    }

    pub fn read_log_page(
        &self,
        scope: &LogScope,
        range: LogRange,
        cursor: Option<&LogCursor>,
    ) -> Result<LogPage> {
        let (scope_column, scope_value) = match scope {
            LogScope::Connection(id) => ("connection_id", id),
            LogScope::Group(id) => ("group_id", id),
        };

        let mut conditions = vec![format!("{scope_column} = ?")];
        let mut params = vec![Param::Text(scope_value.clone())];
        if let Some(cutoff) = range.cutoff_sql() {
            conditions.push(format!("created_at >= {cutoff}"));
        }
        if let Some(cursor) = cursor {
            conditions.push("(created_at < ? OR (created_at = ? AND id < ?))".to_string());
            params.extend([
                Param::Text(cursor.created_at.clone()),
                Param::Text(cursor.created_at.clone()),
                Param::Int(cursor.id),
            ]);
        }
        params.push(Param::Int(LOG_PAGE_SIZE as i64 + 1));

        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM query_log
             WHERE {}
             ORDER BY created_at DESC, id DESC
             LIMIT ?",
            conditions.join(" AND ")
        );

        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), map_entry)?;
        let all: Vec<LogEntry> = rows.collect::<std::result::Result<_, _>>()?;

        let has_more = all.len() > LOG_PAGE_SIZE;
        let entries: Vec<LogEntry> = all.into_iter().take(LOG_PAGE_SIZE).collect();
        let next_cursor = entries.last().filter(|_| has_more).map(|last| LogCursor {
            created_at: last.created_at.clone(),
            id: last.id,
        });
        Ok(LogPage {
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Clear one entity's entire log (the log view's clear button).
    pub fn clear_logs(&self, scope: &LogScope) -> Result<usize> {
        let (column, value) = match scope {
            LogScope::Connection(id) => ("connection_id", id),
            LogScope::Group(id) => ("group_id", id),
        };
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            &format!("DELETE FROM query_log WHERE {column} = ?"),
            [value],
        )?)
    }
}

/// Serialize a captured result, capping stored rows while preserving the
/// observed total for `row_count`.
fn pack_result(result: &QueryResult) -> String {
    let capped = result.rows.iter().take(LOG_RESULT_ROWS);
    json!({ "fields": result.fields, "rows": capped.collect::<Vec<_>>() }).to_string()
}

fn cap_response(text: &str) -> String {
    if text.chars().count() <= LOG_RESPONSE_LIMIT {
        return text.to_string();
    }
    let head: String = text.chars().take(LOG_RESPONSE_LIMIT).collect();
    format!("{head}{TRUNCATION_MARKER}")
}

fn map_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        connection_name: row.get(2)?,
        sql: row.get(3)?,
        verdict: row.get(4)?,
        reason: row.get(5)?,
        categories: row.get(6)?,
        source: row.get(7)?,
        result_json: row.get(8)?,
        row_count: row.get(9)?,
        response_text: row.get(10)?,
        group_id: row.get(11)?,
        group_name: row.get(12)?,
        database: row.get(13)?,
        created_at: row.get(14)?,
    })
}

#[cfg(test)]
mod tests;
