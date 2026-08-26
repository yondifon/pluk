//! Tests for writes, caps, retention, and keyset paging.

use serde_json::{Value, json};

use super::*;
use crate::testing::temp_store;
use crate::timestamp::parse_to_unix_seconds;

const CONNECTION_A: &str = "aaaa1111aaaa1111";
const CONNECTION_B: &str = "bbbb2222bbbb2222";

/// Seed one row with an exact `created_at`, bypassing the column default.
fn seed_at(store: &Store, connection_id: &str, created_at: &str) -> i64 {
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            "INSERT INTO query_log (connection_id, connection_name, sql, verdict, created_at)
             VALUES (?, 'Seeded', 'SELECT 1', 'allowed', ?)",
            rusqlite::params![connection_id, created_at],
        )
        .expect("seed");
    store.conn.lock().expect("store lock").last_insert_rowid()
}

#[test]
fn create_and_finalize_round_trip() {
    let (_dir, store) = temp_store();
    let id = store
        .create_log_entry(
            LogDraft::new(CONNECTION_A, "Postgres", "DELETE FROM users")
                .with_verdict(Verdict::Pending)
                .with_source("query")
                .with_group(LogGroup {
                    id: "g1".into(),
                    name: "All".into(),
                }),
        )
        .unwrap();
    assert!(id > 0);

    let stored_at = store.log_rows_after(0).unwrap().remove(0);
    assert_eq!(
        stored_at.created_at.len(),
        19,
        "column default stamps sqlite format"
    );

    store
        .update_log_entry(
            id,
            LogUpdate {
                verdict: Verdict::Blocked,
                reason: Some("write blocked".into()),
                ..LogUpdate::new(Verdict::Blocked)
            },
        )
        .unwrap();

    let entry = &store
        .read_log_page(
            &LogScope::Connection(CONNECTION_A.into()),
            LogRange::All,
            None,
        )
        .unwrap()
        .entries[0];
    assert_eq!(entry.verdict, "blocked");
    assert_eq!(entry.reason.as_deref(), Some("write blocked"));
    assert_eq!(entry.source.as_deref(), Some("query"));
    assert_eq!(entry.group_id.as_deref(), Some("g1"));
    assert_eq!(entry.group_name.as_deref(), Some("All"));
    assert_eq!(entry.connection_name, "Postgres");
}

#[test]
fn result_payload_is_capped_with_total_row_count_preserved() {
    let (_dir, store) = temp_store();
    let id = store
        .create_log_entry(LogDraft::new(CONNECTION_A, "Pg", "SELECT 1"))
        .unwrap();

    let rows: Vec<Value> = (0..150).map(|i| json!({ "n": i })).collect();
    store
        .update_log_entry(
            id,
            LogUpdate {
                result: Some(QueryResult {
                    fields: vec!["n".into()],
                    rows,
                }),
                ..LogUpdate::new(Verdict::Allowed)
            },
        )
        .unwrap();

    let entry = &store
        .log_rows_after(0)
        .unwrap()
        .into_iter()
        .find(|e| e.id == id)
        .unwrap();
    let packed: Value = serde_json::from_str(entry.result_json.as_ref().unwrap()).unwrap();
    assert_eq!(packed["fields"], json!(["n"]));
    assert_eq!(packed["rows"].as_array().unwrap().len(), LOG_RESULT_ROWS);
    assert_eq!(entry.row_count, Some(150));
    // The first capped row must be the first observed row, not a random slice.
    assert_eq!(packed["rows"][0], json!({"n": 0}));
}

#[test]
fn oversized_response_text_is_truncated_with_the_known_marker() {
    let (_dir, store) = temp_store();
    let id = store
        .create_log_entry(LogDraft::new(CONNECTION_A, "Pg", "SELECT 1"))
        .unwrap();
    let huge = "x".repeat(LOG_RESPONSE_LIMIT + 5_000);
    store
        .update_log_entry(
            id,
            LogUpdate {
                response_text: Some(huge),
                ..LogUpdate::new(Verdict::Allowed)
            },
        )
        .unwrap();

    let entry = &store
        .log_rows_after(0)
        .unwrap()
        .into_iter()
        .find(|e| e.id == id)
        .unwrap();
    let stored = entry.response_text.as_ref().unwrap();
    assert!(stored.ends_with(TRUNCATION_MARKER));
    assert_eq!(
        stored.chars().count() - TRUNCATION_MARKER.chars().count(),
        LOG_RESPONSE_LIMIT
    );

    // Under the limit, text is stored verbatim.
    let small_id = store
        .create_log_entry(LogDraft::new(CONNECTION_A, "Pg", "SELECT 2"))
        .unwrap();
    store
        .update_log_entry(
            small_id,
            LogUpdate {
                response_text: Some("short".into()),
                ..LogUpdate::new(Verdict::Allowed)
            },
        )
        .unwrap();
    let entry = &store
        .log_rows_after(0)
        .unwrap()
        .into_iter()
        .find(|e| e.id == small_id)
        .unwrap();
    assert_eq!(entry.response_text.as_deref(), Some("short"));
}

#[test]
fn retention_purge_deletes_only_rows_past_the_window() {
    let (_dir, store) = temp_store();
    store.set_retention_days(30).unwrap();

    let old = unix_days_ago(40);
    let recent = unix_days_ago(10);
    seed_at(&store, CONNECTION_A, &old);
    seed_at(&store, CONNECTION_A, &recent);

    let deleted = store.purge_old_logs().unwrap();
    assert_eq!(deleted, 1);
    let remaining = store
        .read_log_page(
            &LogScope::Connection(CONNECTION_A.into()),
            LogRange::All,
            None,
        )
        .unwrap();
    assert_eq!(remaining.entries.len(), 1);
    assert_eq!(remaining.entries[0].created_at, recent);
}

#[test]
fn zero_retention_keeps_everything_forever() {
    let (_dir, store) = temp_store();
    store.set_retention_days(0).unwrap();
    seed_at(&store, CONNECTION_A, "2000-01-01 00:00:00");
    assert_eq!(store.purge_old_logs().unwrap(), 0);
    assert_eq!(
        store
            .read_log_page(
                &LogScope::Connection(CONNECTION_A.into()),
                LogRange::All,
                None
            )
            .unwrap()
            .entries
            .len(),
        1
    );
}

#[test]
fn keyset_paging_walks_every_row_exactly_once_across_the_boundary() {
    let (_dir, store) = temp_store();
    let base = parse_to_unix_seconds("2026-01-01 00:00:00").unwrap();
    for i in 0..250u32 {
        let created = crate::timestamp::format_unix_seconds(base + i64::from(i));
        seed_at(&store, CONNECTION_A, &created);
    }
    // Another connection's rows must never leak into A's pages.
    seed_at(&store, CONNECTION_B, "2026-12-31 23:59:59");

    let scope = LogScope::Connection(CONNECTION_A.into());
    let mut seen = Vec::new();
    let mut cursor: Option<LogCursor> = None;
    let mut pages = 0;
    loop {
        let page = store
            .read_log_page(&scope, LogRange::All, cursor.as_ref())
            .unwrap();
        assert!(page.entries.len() <= LOG_PAGE_SIZE);
        seen.extend(page.entries.iter().map(|e| e.id));
        pages += 1;
        match (page.has_more, page.next_cursor) {
            (false, None) => break,
            (true, Some(next)) => cursor = Some(next),
            other => panic!("has_more/cursor disagree: {other:?}"),
        }
    }

    assert_eq!(pages, 3, "250 rows at page size 100");
    assert_eq!(seen.len(), 250);
    assert_eq!(
        seen.iter().collect::<std::collections::HashSet<_>>().len(),
        250,
        "no duplicates"
    );
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(seen, sorted, "strictly newest-first across pages");
}

#[test]
fn paging_tie_breaks_on_id_within_identical_timestamps() {
    let (_dir, store) = temp_store();
    for _ in 0..5 {
        store.conn.lock().expect("store lock")
            .execute(
                "INSERT INTO query_log (connection_id, connection_name, sql, verdict) VALUES (?, 'S', 'q', 'allowed')",
                [CONNECTION_A],
            )
            .unwrap(); // identical datetime('now') second, ids differ
    }
    let scope = LogScope::Connection(CONNECTION_A.into());
    let page_one = store
        .read_log_page(&scope, LogRange::All, None)
        .unwrap()
        .entries;
    assert_eq!(page_one.len(), 5);
    let mut ids: Vec<i64> = page_one.iter().map(|e| e.id).collect();
    assert!(
        ids.windows(2).all(|w| w[0] > w[1]),
        "same-second rows order by id desc"
    );

    // Walking past the middle of the tie group neither skips nor repeats.
    ids.sort_unstable_by(|a, b| b.cmp(a));
    let mid_cursor = LogCursor {
        created_at: page_one[2].created_at.clone(),
        id: page_one[2].id,
    };
    let rest = store
        .read_log_page(&scope, LogRange::All, Some(&mid_cursor))
        .unwrap()
        .entries;
    let rest_ids: Vec<i64> = rest.iter().map(|e| e.id).collect();
    assert_eq!(rest_ids, ids[3..]);
}

#[test]
fn ranges_filter_against_sqlites_own_clock() {
    let (_dir, store) = temp_store();
    let fresh = seed_now(&store);
    let two_hours_ago = seed_relative(&store, "-2 hours");
    let eight_days_ago = seed_relative(&store, "-8 days");
    // Yesterday's local midnight in UTC terms — before today's cutoff no
    // matter which timezone the machine runs in.
    let before_today = seed_created(
        &store,
        "datetime('now', 'localtime', 'start of day', '-1 day', 'utc')",
    );
    seed_at(&store, CONNECTION_A, "2020-01-01 00:00:00");

    let scope = LogScope::Connection(CONNECTION_A.into());
    let ids_for = |range| -> Vec<i64> {
        let mut ids = store
            .read_log_page(&scope, range, None)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    };
    fn ids_of(ids: &[i64]) -> Vec<i64> {
        let mut v = ids.to_vec();
        v.sort_unstable();
        v
    }

    assert_eq!(ids_for(LogRange::Hour), vec![fresh]);
    // A two-hour-old row may still be today when the test runs late in the
    // local day, so only the guaranteed exclusions are asserted here.
    assert!(!ids_for(LogRange::Today).contains(&before_today));
    assert_eq!(
        ids_for(LogRange::SevenDays),
        ids_of(&[fresh, two_hours_ago, before_today]),
        "8-day-old and 2020 rows fall outside 7d"
    );
    assert_eq!(
        ids_for(LogRange::ThirtyDays),
        ids_of(&[fresh, two_hours_ago, eight_days_ago, before_today]),
    );
    assert_eq!(ids_for(LogRange::All).len(), 5);
}

#[test]
fn clear_logs_scopes_to_one_entity() {
    let (_dir, store) = temp_store();
    seed_now(&store);
    seed_at(&store, CONNECTION_B, "2026-01-01 00:00:00");
    let removed = store
        .clear_logs(&LogScope::Connection(CONNECTION_A.into()))
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(
        store
            .read_log_page(
                &LogScope::Connection(CONNECTION_B.into()),
                LogRange::All,
                None
            )
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(
        store.clear_logs(&LogScope::Group("nope".into())).unwrap(),
        0
    );
}

#[test]
fn high_water_and_catch_up_reads_track_inserts_in_order() {
    let (_dir, store) = temp_store();
    assert_eq!(store.log_high_water().unwrap(), 0);
    let first = store
        .create_log_entry(LogDraft::new(CONNECTION_A, "A", "q1"))
        .unwrap();
    let second = store
        .create_log_entry(LogDraft::new(CONNECTION_A, "A", "q2"))
        .unwrap();
    assert_eq!(store.log_high_water().unwrap(), second);

    let catch_up = store.log_rows_after(first).unwrap();
    assert_eq!(
        catch_up.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![second]
    );
    let everything = store.log_rows_after(0).unwrap();
    assert_eq!(
        everything
            .iter()
            .map(|e| e.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["q1", "q2"]
    );
}

fn seed_now(store: &Store) -> i64 {
    store
        .create_log_entry(LogDraft::new(CONNECTION_A, "A", "now"))
        .unwrap()
}

/// Insert a row whose age SQLite itself computes, so tests stay correct around
/// DST boundaries and slow machines.
fn seed_relative(store: &Store, modifier: &str) -> i64 {
    seed_created(store, &format!("datetime('now', '{modifier}')"))
}

fn seed_created(store: &Store, created_at_expr: &str) -> i64 {
    store
        .conn
        .lock()
        .expect("store lock")
        .execute(
            &format!(
                "INSERT INTO query_log (connection_id, connection_name, sql, verdict, created_at)
                 VALUES (?, 'R', 'old', 'allowed', {created_at_expr})"
            ),
            rusqlite::params![CONNECTION_A],
        )
        .expect("seed relative");
    store.conn.lock().expect("store lock").last_insert_rowid()
}

fn unix_days_ago(days: u32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    crate::timestamp::format_unix_seconds(now - i64::from(days) * 86_400)
}
