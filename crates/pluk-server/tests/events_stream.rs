//! The live event stream: exact replay from a cursor, ready high-water, live
//! push, malformed cursors, and slow-consumer drops.

mod common;

use common::{spawn_app, spawn_app_with_events};
use std::time::Duration;

async fn connect_at_high_water(app: &common::TestApp) -> (i64, common::SseReader) {
    let high = app.store.log_high_water().unwrap();
    let mut reader = app.sse_connect(&format!("?after={high}")).await;
    let first = reader.next_frame().await;
    assert_eq!(first.event, "ready");
    let cursor = first.data["cursor"].as_i64().expect("ready cursor");
    assert_eq!(cursor, high);
    (cursor, reader)
}

#[tokio::test]
async fn a_reconnecting_client_replays_exactly_the_missed_rows() {
    let app = spawn_app().await;
    let ids = app.insert_logs(5, "conn-a");

    // Reconnect from the third row: exactly the two newer rows replay.
    let mut reader = app.sse_connect(&format!("?after={}", ids[2])).await;
    let replay1 = reader.next_frame().await;
    assert_eq!(replay1.event, "event");
    assert_eq!(replay1.data["id"], ids[3]);
    let replay2 = reader.next_frame().await;
    assert_eq!(replay2.event, "event");
    assert_eq!(replay2.data["id"], ids[4]);

    // Then ready carries the high-water mark at connection time.
    let ready = reader.next_frame().await;
    assert_eq!(ready.event, "ready");
    assert_eq!(ready.data["cursor"], ids[4]);
    drop(reader);
}

#[tokio::test]
async fn rows_written_after_connect_are_pushed_live() {
    let app = spawn_app().await;
    let (_cursor, mut reader) = connect_at_high_water(&app).await;

    let id = app
        .store
        .create_log_entry(
            pluk_store::LogDraft::new("conn-b", "Beta", "insert into t values (1)")
                .with_verdict(pluk_store::Verdict::Blocked),
        )
        .unwrap();
    let frame = reader.next_frame().await;
    assert_eq!(frame.event, "event");
    assert_eq!(frame.data["id"], id);
    assert_eq!(frame.data["connectionId"], "conn-b", "wire shape is camelCase");
    assert_eq!(frame.data["verdict"], "blocked");

    // Updates to existing rows are pushed too.
    app.store
        .update_log_entry(id, pluk_store::LogUpdate::new(pluk_store::Verdict::Allowed))
        .unwrap();
    let update = reader.next_frame().await;
    assert_eq!(update.event, "event");
    assert_eq!(update.data["id"], id);
    assert_eq!(update.data["verdict"], "allowed");
}

#[tokio::test]
async fn every_row_written_after_connect_arrives_once_in_order() {
    let app = spawn_app().await;
    let (_, mut reader) = connect_at_high_water(&app).await;

    // A burst written right after subscribing: each row delivered once, in id
    // order — no gap from the replay/feed handoff, no duplicates.
    let ids = app.insert_logs(50, "burst");
    for expected in &ids {
        let frame = reader.next_frame().await;
        assert_eq!(frame.data["id"], *expected);
    }
}

#[tokio::test]
async fn keepalives_flow_while_idle_and_carry_the_high_water() {
    let app = spawn_app().await;
    let (cursor, mut reader) = connect_at_high_water(&app).await;

    // The harness hub keeps alive every 60ms; expect one with no writes.
    let keepalive = tokio::time::timeout(Duration::from_secs(5), reader.next_frame())
        .await
        .expect("keepalive within timeout");
    assert_eq!(keepalive.event, "keepalive");
    assert_eq!(keepalive.data["cursor"], cursor);
}

#[tokio::test]
async fn a_malformed_cursor_is_a_400() {
    let app = spawn_app().await;
    for query in ["?after=abc", "?after=-1", "?after=1.5", "?after=12abc", "?after="] {
        let response = reqwest::get(format!("{}/api/events{query}", app.base_url))
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{query}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], "Invalid cursor");
    }
    // Absent means fresh client: the stream opens.
    assert_eq!(app.get_status("/api/events").await, 200);
}

/// A subscriber that stops reading is dropped at the next push instead of
/// being buffered without bound.
#[tokio::test]
async fn slow_consumers_get_dropped() {
    // Capacity 4, generous keepalive: flooding past capacity must evict.
    let app = spawn_app_with_events(Duration::from_secs(5), 4).await;

    {
        // Connect and never read again.
        let _reader = app.sse_connect("?after=0").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(app.events.active_subscribers(), 1);

        // Broadcasts run inline on insert, so by the time this returns the
        // subscriber is already gone.
        app.insert_logs(64, "flood");
        assert_eq!(
            app.events.active_subscribers(),
            0,
            "a full buffer must evict its consumer"
        );
    }

    // A fresh client connects cleanly afterwards.
    let (_, mut reader) = connect_at_high_water(&app).await;
    let id = app.insert_logs(1, "post-flood").remove(0);
    let frame = reader.next_frame().await;
    assert_eq!(frame.data["id"], id);
}
