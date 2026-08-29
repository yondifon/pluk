//! Server-Sent Events for the activity log (`GET /api/events?after=<rowid>`).
//!
//! Every insert/update in `query_log` reaches subscribers through the store's
//! activity feed; this module turns that feed into one held-open HTTP stream
//! per client. A reconnecting client replays exactly the rows written after
//! its cursor — the cursor is a query_log row id, so replay is exact: no gap,
//! no duplicates.
//!
//! Slow consumers are dropped instead of buffered without bound: a subscriber
//! whose channel is full (dead or too slow to read) is removed at the next
//! push, and the keepalive doubles as that garbage-collection trigger.
//!
//! The TypeScript server ran under Bun, which capped an idle stream's life at
//! 255 seconds and relied on clients reconnecting. Hyper imposes no idle cap:
//! streams live until the client disconnects or the server shuts down.
//!
//! Ported from `pluk/src/events.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use pluk_store::{LogActivity, LogEntry, Store};

const KEEPALIVE_MS: u64 = 15_000;
/// Matches the stream's high-water mark in the TypeScript server.
const CHANNEL_CAPACITY: usize = 256;

/// The `after` query param → cursor. Absent means 0 (fresh client); anything
/// that is not a non-negative integer is rejected, never coerced.
pub fn parse_after(raw: Option<&str>) -> Option<i64> {
    let Some(raw) = raw else { return Some(0) };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<i64>().ok()
}

/// One preformatted SSE frame.
pub(crate) struct Frame {
    event: &'static str,
    data: String,
}

impl Frame {
    fn new(event: &'static str, data: String) -> Arc<Frame> {
        Arc::new(Frame { event, data })
    }

    pub(crate) fn wire(&self) -> Vec<u8> {
        format!("event: {}\ndata: {}\n\n", self.event, self.data).into_bytes()
    }
}

fn row_frame(row: &LogActivity) -> Arc<Frame> {
    Frame::new("event", serde_json::to_string(row).unwrap_or_default())
}

fn ready_frame(cursor: i64) -> Arc<Frame> {
    Frame::new("ready", format!("{{\"cursor\":{cursor}}}"))
}

fn keepalive_frame(cursor: i64) -> Arc<Frame> {
    Frame::new("keepalive", format!("{{\"cursor\":{cursor}}}"))
}

fn to_activity(entry: LogEntry) -> LogActivity {
    LogActivity {
        id: entry.id,
        connection_id: entry.connection_id,
        connection_name: entry.connection_name,
        sql: entry.sql,
        verdict: entry.verdict,
        reason: entry.reason,
        categories: entry.categories,
        source: entry.source,
        group_id: entry.group_id,
        group_name: entry.group_name,
        database: entry.database,
        row_count: entry.row_count,
        created_at: entry.created_at,
    }
}

struct HubState {
    subscribers: HashMap<u64, mpsc::Sender<Arc<Frame>>>,
    next_id: u64,
    /// The store feed subscription backing every channel; live while at least
    /// one subscriber exists.
    subscription: Option<u64>,
    /// Generation of the running keepalive task, if any.
    ticker: Option<(u64, tokio::task::JoinHandle<()>)>,
}

impl HubState {
    fn ensure_feed(&mut self, store: &Store, core: &Core) {
        if self.subscription.is_none() {
            let sink = core.clone();
            self.subscription =
                Some(store.subscribe_log_activity(Arc::new(move |row| sink.broadcast(row))));
        }
    }

    fn ensure_ticker(&mut self, core: &Core) {
        if self.ticker.is_none() {
            let generation = self.next_generation();
            let sink = core.clone();
            self.ticker = Some((
                generation,
                tokio::spawn(async move { sink.keepalive_loop(generation).await }),
            ));
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Push one frame to every healthy subscriber; drop the dead and the slow.
    fn fan_out(&mut self, payload: Arc<Frame>) {
        self.subscribers.retain(|_, tx| {
            matches!(
                tx.try_send(payload.clone()),
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_))
            )
        });
    }

    fn stop_feed(&mut self, store: &Store) {
        if let Some(subscription) = self.subscription.take() {
            store.unsubscribe_log_activity(subscription);
        }
    }
}

/// Shared hub internals; cloned into the feed handler and the keepalive task.
#[derive(Clone)]
struct Core {
    store: Arc<Store>,
    keepalive: Duration,
    capacity: usize,
    state: Arc<Mutex<HubState>>,
}

impl Core {
    fn lock(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state.lock().expect("event hub lock")
    }

    fn high_water(&self) -> i64 {
        self.store.log_high_water().unwrap_or(0)
    }

    fn broadcast(&self, row: &LogActivity) {
        self.lock().fan_out(row_frame(row));
    }

    async fn keepalive_loop(self, generation: u64) {
        loop {
            tokio::time::sleep(self.keepalive).await;
            let payload = keepalive_frame(self.high_water());
            self.lock().fan_out(payload);
            // Nobody left to serve: tear this ticker and the feed down. A
            // newer generation (a subscriber connected since) owns them then.
            let mut state = self.lock();
            let latest = state
                .ticker
                .as_ref()
                .is_some_and(|(id, _)| *id == generation);
            if latest && state.subscribers.is_empty() {
                state.ticker = None;
                state.stop_feed(&self.store);
                return;
            }
        }
    }
}

/// Fan one store's activity feed out to connected event streams.
pub struct EventHub {
    core: Core,
    shutdown_lock: Mutex<()>,
}

impl EventHub {
    pub fn new(store: Arc<Store>) -> Self {
        EventHub::with_options(store, Duration::from_millis(KEEPALIVE_MS), CHANNEL_CAPACITY)
    }

    /// The production hub with a custom keepalive cadence and buffer size.
    pub fn with_options(store: Arc<Store>, keepalive: Duration, capacity: usize) -> Self {
        EventHub {
            core: Core {
                store,
                keepalive,
                capacity,
                state: Arc::new(Mutex::new(HubState {
                    subscribers: HashMap::new(),
                    next_id: 0,
                    subscription: None,
                    ticker: None,
                })),
            },
            shutdown_lock: Mutex::new(()),
        }
    }

    /// How many streams are currently attached (host diagnostics).
    pub fn active_subscribers(&self) -> usize {
        self.core.lock().subscribers.len()
    }

    /// Attach a client connecting with cursor `after`. Returns the frames to
    /// emit immediately — exactly the replayed rows plus the ready frame — and
    /// the receiver carrying everything from now on. Subscribing before
    /// reading means a row written during replay arrives once from the feed:
    /// never lost, never duplicated (row ids are monotonic).
    pub(crate) fn attach(&self, after: i64) -> (Vec<Arc<Frame>>, mpsc::Receiver<Arc<Frame>>) {
        let mut replay: Vec<Arc<Frame>> = Vec::new();
        if let Ok(rows) = self.core.store.log_rows_after(after) {
            for row in rows {
                replay.push(row_frame(&to_activity(row)));
            }
        }
        replay.push(ready_frame(self.core.high_water()));

        let (tx, rx) = mpsc::channel(self.core.capacity);
        {
            let mut state = self.core.lock();
            let id = state.next_id;
            state.next_id += 1;
            state.subscribers.insert(id, tx);
            state.ensure_feed(&self.core.store, &self.core);
            state.ensure_ticker(&self.core);
        }
        (replay, rx)
    }

    /// End every stream. Used on graceful shutdown so held-open responses do
    /// not stall the drain.
    pub async fn shutdown(&self) {
        let _quiesce = self.shutdown_lock.lock().expect("shutdown lock");
        let mut state = self.core.lock();
        state.subscribers.clear();
        state.stop_feed(&self.core.store);
        if let Some((_, handle)) = state.ticker.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_after_accepts_only_non_negative_integers() {
        assert_eq!(parse_after(None), Some(0));
        assert_eq!(parse_after(Some("42")), Some(42));
        assert_eq!(parse_after(Some("")), None);
        assert_eq!(parse_after(Some("abc")), None);
        assert_eq!(parse_after(Some("-1")), None);
        assert_eq!(parse_after(Some("1.5")), None);
        assert_eq!(parse_after(Some("12abc")), None);
        assert_eq!(parse_after(Some("+2")), None);
        assert_eq!(
            parse_after(Some("99999999999999999999")),
            None,
            "must stay in i64"
        );
    }
}
