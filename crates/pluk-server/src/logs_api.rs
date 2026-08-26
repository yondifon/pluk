//! `GET /api/logs` — keyset-paged log reads.
//!
//! Exactly one of a connection or group scope, a range of hour/today/7d/30d/
//! all, a keyset cursor of created-at plus id, page size 100. The query
//! itself lives in the store (R02); this module validates the HTTP shape and
//! serializes the page in the camelCase form the frontend reads.
//!
//! Ported from `pluk/src/logs.ts`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use pluk_store::{LogCursor, LogEntry, LogPage, LogRange, LogScope, Store};

const CURSOR_TIME_LEN: usize = "YYYY-MM-DD HH:MM:SS".len();

fn is_log_range(raw: &str) -> bool {
    matches!(raw, "hour" | "today" | "7d" | "30d" | "all")
}

/// Absent means `all`; anything unrecognized is rejected.
pub fn parse_log_range(raw: Option<&str>) -> Option<LogRange> {
    match raw {
        None => Some(LogRange::All),
        Some(raw) if is_log_range(raw) => Some(match raw {
            "hour" => LogRange::Hour,
            "today" => LogRange::Today,
            "7d" => LogRange::SevenDays,
            "30d" => LogRange::ThirtyDays,
            _ => LogRange::All,
        }),
        _ => None,
    }
}

/// `Ok(None)` when both params absent (no cursor); `Err` on a malformed one.
fn parse_log_cursor(time: Option<&str>, id: Option<&str>) -> Result<Option<LogCursor>, ()> {
    let (Some(time), Some(id)) = (time, id) else {
        return match (time, id) {
            (None, None) => Ok(None),
            _ => Err(()),
        };
    };
    // The stored shape is exactly SQLite's `datetime('now')`:
    // yyyy-MM-dd HH:mm:ss.
    let looks_like_sqlite_datetime = time.len() == CURSOR_TIME_LEN && {
        let b = time.as_bytes();
        b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[13] == b':' && b[16] == b':'
    };
    if !looks_like_sqlite_datetime {
        return Err(());
    }
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let parsed_id: i64 = id.parse().map_err(|_| ())?;
    if parsed_id <= 0 {
        return Err(());
    }
    Ok(Some(LogCursor { created_at: time.to_string(), id: parsed_id }))
}

/// Exactly one scope must be present and non-empty.
fn parse_log_scope(connection_id: Option<&str>, group_id: Option<&str>) -> Option<LogScope> {
    if let Some(connection_id) = connection_id {
        return (group_id.is_none() && !connection_id.is_empty()).then(|| LogScope::Connection(connection_id.to_string()));
    }
    let group_id = group_id?;
    (!group_id.is_empty()).then(|| LogScope::Group(group_id.to_string()))
}

/// One log entry serialized for the frontend (camelCase).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntryJson {
    id: i64,
    connection_id: String,
    connection_name: String,
    sql: String,
    verdict: String,
    reason: Option<String>,
    categories: Option<String>,
    source: Option<String>,
    result_json: Option<String>,
    row_count: Option<i64>,
    response_text: Option<String>,
    group_id: Option<String>,
    group_name: Option<String>,
    database: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorJson<'a> {
    created_at: &'a str,
    id: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageJson<'a> {
    entries: Vec<LogEntryJson>,
    next_cursor: Option<CursorJson<'a>>,
    has_more: bool,
}

impl From<&LogEntry> for LogEntryJson {
    fn from(e: &LogEntry) -> Self {
        LogEntryJson {
            id: e.id,
            connection_id: e.connection_id.clone(),
            connection_name: e.connection_name.clone(),
            sql: e.sql.clone(),
            verdict: e.verdict.clone(),
            reason: e.reason.clone(),
            categories: e.categories.clone(),
            source: e.source.clone(),
            result_json: e.result_json.clone(),
            row_count: e.row_count,
            response_text: e.response_text.clone(),
            group_id: e.group_id.clone(),
            group_name: e.group_name.clone(),
            database: e.database.clone(),
            created_at: e.created_at.clone(),
        }
    }
}

fn from_entry(entry: LogEntry) -> LogEntryJson {
    LogEntryJson::from(&entry)
}

/// Serve one page, or `None` when this request is not for `/api/logs`.
pub fn handle(store: &Store, path: &str, method: &str, query: &str) -> Option<Response> {
    if path != "/api/logs" || method != "GET" {
        return None;
    }
    let params = parse_query(query);
    let get = |key: &str| params.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());

    let Some(scope) = parse_log_scope(get("connectionId"), get("groupId")) else {
        return Some(error_response("Exactly one log scope is required"));
    };
    let Some(range) = parse_log_range(get("range")) else {
        return Some(error_response("Invalid range"));
    };
    let cursor = match parse_log_cursor(get("cursorTime"), get("cursorId")) {
        Ok(cursor) => cursor,
        Err(()) => return Some(error_response("Invalid cursor")),
    };

    let page: LogPage = store.read_log_page(&scope, range, cursor.as_ref()).unwrap_or(LogPage {
        entries: Vec::new(),
        next_cursor: None,
        has_more: false,
    });

    let body = PageJson {
        entries: page.entries.into_iter().map(from_entry).collect(),
        next_cursor: page
            .next_cursor
            .as_ref()
            .map(|c| CursorJson { created_at: &c.created_at, id: c.id }),
        has_more: page.has_more,
    };
    Some(axum::Json(body).into_response())
}

fn error_response(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        serde_json::json!({ "ok": false, "error": message }).to_string(),
    )
        .into_response()
}

/// First-value-wins query parsing with `URLSearchParams` semantics: split on
/// `&`, then on the first `=`, percent-decoded with `+` meaning space.
pub(crate) fn parse_query(query: &str) -> Vec<(String, String)> {
    fn decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'+' => {
                    out.push(b' ');
                    index += 1;
                }
                b'%' if bytes.len() >= index + 3 => {
                    let hex = bytes.get(index + 1..index + 3);
                    match hex.and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        Some(byte) => {
                            out.push(byte);
                            index += 3;
                        }
                        None => {
                            out.push(b'%');
                            index += 1;
                        }
                    }
                }
                byte => {
                    out.push(byte);
                    index += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(part), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_defaults_to_all_and_rejects_unknowns() {
        assert_eq!(parse_log_range(None), Some(LogRange::All));
        assert_eq!(parse_log_range(Some("30d")), Some(LogRange::ThirtyDays));
        assert_eq!(parse_log_range(Some("tomorrow")), None);
    }

    #[test]
    fn cursor_needs_both_parts_and_a_valid_timestamp() {
        assert_eq!(parse_log_cursor(None, None), Ok(None));
        assert_eq!(parse_log_cursor(Some("2026-01-01 00:00:00"), None), Err(()));
        assert_eq!(parse_log_cursor(None, Some("1")), Err(()));
        assert_eq!(parse_log_cursor(Some("nope"), Some("1")), Err(()));
        assert_eq!(
            parse_log_cursor(Some("2026-01-01 00:00:00"), Some("7")),
            Ok(Some(LogCursor { created_at: "2026-01-01 00:00:00".into(), id: 7 }))
        );
        assert_eq!(parse_log_cursor(Some("2026-01-01 00:00:00"), Some("0")), Err(()));
    }

    #[test]
    fn scope_requires_exactly_one_side() {
        assert_eq!(parse_log_scope(Some("c1"), None), Some(LogScope::Connection("c1".into())));
        assert_eq!(parse_log_scope(None, Some("g1")), Some(LogScope::Group("g1".into())));
        assert_eq!(parse_log_scope(None, None), None);
        assert_eq!(parse_log_scope(Some(""), None), None);
        assert_eq!(parse_log_scope(Some("c1"), Some("g1")), None);
    }

    #[test]
    fn query_parsing_decodes_and_keeps_first_value() {
        let parsed = parse_query("connectionId=a%20b&x=1&x=2&flag");
        assert_eq!(parsed[0], ("connectionId".into(), "a b".into()));
        assert_eq!(parsed[1], ("x".into(), "1".into()));
        assert_eq!(parsed[3], ("flag".into(), "".into()));
    }
}
