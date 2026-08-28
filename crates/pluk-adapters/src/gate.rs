//! The gated call lifecycle — the audit backbone every adapter shares.
//!
//! [`run_gated`] is the single audited place for the log lifecycle:
//!
//! 1. **precheck** — a returned reason blocks the call *before* any pending
//!    entry is written (matching how policy denials are logged today).
//! 2. **pending** — a log row is created so long-running calls are visible
//!    while they run. The row id flows into `run`, which can register a
//!    per-call abort against it.
//! 3. **run** — the adapter body executes.
//! 4. **finalize** — the row gets its verdict: `allowed`, `blocked` (a
//!    post-pending block, e.g. a SQL cost gate), or a terminal failure.
//!
//! Cancellation is a distinct verdict from error: a thrown error the
//! classifier maps to cancellation never triggers the error hook, so a
//! cancelled SQL query does not evict its pooled connection. SSH
//! pending-approval errors get the same protection even though they finalize
//! as errors.

use std::future::Future;

use serde::Serialize;

use pluk_store::{LogDraft, LogGroup, LogUpdate, QueryResult, Store, Verdict};

use crate::error::AdapterError;

// ── MCP response shaping ─────────────────────────────────────────────────────

/// One text block of an MCP tool response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub text: String,
}

/// The shaped MCP tool result: text content plus an optional error flag.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<TextContent>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

/// A successful response carrying `text`.
pub fn ok(text: impl Into<String>) -> ToolResult {
    ToolResult {
        content: vec![TextContent {
            content_type: "text",
            text: text.into(),
        }],
        is_error: false,
    }
}

/// An error-flagged response carrying `text`.
pub fn err(text: impl Into<String>) -> ToolResult {
    ToolResult {
        content: vec![TextContent {
            content_type: "text",
            text: text.into(),
        }],
        is_error: true,
    }
}

impl ToolResult {
    /// The first text block's contents.
    pub fn text(&self) -> &str {
        self.content
            .first()
            .map(|c| c.text.as_str())
            .unwrap_or_default()
    }
}

// ── Gate inputs ──────────────────────────────────────────────────────────────

/// Structured metadata describing one gated call.
#[derive(Debug, Clone)]
pub struct GateMeta {
    /// Log category (an action category, SQL statement categories, …).
    pub category: String,
    /// Originating tool / operation — the log's `source`.
    pub action: String,
    /// Human-readable line stored in the log (`sql` column).
    pub detail: String,
    /// Target database when the call selected one (multi-db SQL connections).
    pub database: Option<String>,
    /// Exact CLI command when the adapter shells out; replaces `detail` in
    /// the log row and is re-recorded on finalization.
    pub command: Option<String>,
}

impl GateMeta {
    pub fn new(
        category: impl Into<String>,
        action: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        GateMeta {
            category: category.into(),
            action: action.into(),
            detail: detail.into(),
            database: None,
            command: None,
        }
    }

    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }
}

/// The connection a gated call belongs to: what the audit row names.
#[derive(Debug, Clone)]
pub struct CallTarget {
    pub connection_id: String,
    pub connection_name: String,
    /// Set when a group endpoint fronted the member integration.
    pub group: Option<LogGroup>,
}

impl CallTarget {
    pub fn new(connection_id: impl Into<String>, connection_name: impl Into<String>) -> Self {
        CallTarget {
            connection_id: connection_id.into(),
            connection_name: connection_name.into(),
            group: None,
        }
    }
}

impl From<&pluk_store::Integration> for CallTarget {
    fn from(conn: &pluk_store::Integration) -> Self {
        CallTarget {
            connection_id: conn.id.clone(),
            connection_name: conn.name.clone(),
            group: None,
        }
    }
}

/// What a gated tool's body produced.
///
/// - [`Outcome::Blocked`]: a post-pending block (e.g. a SQL cost gate) —
///   logged as blocked.
/// - [`Outcome::Ran`]: the call executed. `is_error` marks ran-but-failed
///   results (e.g. a non-zero SSH exit) so they are logged as errors yet
///   still return their text to the agent.
#[derive(Debug, Clone)]
pub enum Outcome {
    Blocked(String),
    Ran(RunOutcome),
}

/// A completed run: agent-visible text plus everything the log row records.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub text: String,
    pub is_error: bool,
    /// Why the call failed, when known (stored as the log reason).
    pub reason: Option<String>,
    /// Structured snapshot stored in the log (`result_json`).
    pub result: Option<QueryResult>,
    /// Raw agent-visible response text, shown in full by the log viewer.
    pub response_text: Option<String>,
    /// Exact CLI command executed, replacing the recorded SQL when set.
    pub command: Option<String>,
}

impl Outcome {
    pub fn ran(text: impl Into<String>) -> Self {
        Outcome::Ran(RunOutcome {
            text: text.into(),
            ..Default::default()
        })
    }

    /// A ran-but-failed outcome: logged as an error, still returns its text.
    pub fn failed(text: impl Into<String>, reason: impl Into<String>) -> Self {
        Outcome::Ran(RunOutcome {
            text: text.into(),
            is_error: true,
            reason: Some(reason.into()),
            ..Default::default()
        })
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        Outcome::Blocked(reason.into())
    }
}

/// Optional hooks around the gated lifecycle.
pub type PrecheckFn = Box<dyn FnOnce() -> Option<String> + Send>;
pub type ClassifyErrorFn = Box<dyn Fn(&AdapterError) -> Verdict + Send>;
pub type OnErrorFn = Box<dyn Fn(&AdapterError) + Send>;
pub type FormatErrorFn = Box<dyn Fn(&AdapterError, Verdict) -> String + Send>;

#[derive(Default)]
pub struct GateOpts {
    /// Pre-flight permission check. A returned reason blocks the call before
    /// any pending entry is written.
    pub precheck: Option<PrecheckFn>,
    /// Map a thrown error to a terminal verdict. Must yield
    /// [`Verdict::Cancelled`] or [`Verdict::Error`]; anything else is coerced
    /// to [`Verdict::Error`]. Used by SQL to record aborted queries as
    /// cancelled.
    pub classify_error: Option<ClassifyErrorFn>,
    /// Side effects to run only on a true error — never on cancellation, and
    /// never for SSH pending-approval errors (e.g. evicting a pooled driver).
    pub on_error: Option<OnErrorFn>,
    /// Render the agent-facing text of a failure (defaults to
    /// `Cancelled: …` / `Error: …`).
    pub format_error: Option<FormatErrorFn>,
}

impl GateOpts {
    pub fn new() -> Self {
        GateOpts::default()
    }

    pub fn precheck(mut self, precheck: impl FnOnce() -> Option<String> + Send + 'static) -> Self {
        self.precheck = Some(Box::new(precheck));
        self
    }

    pub fn classify_error(
        mut self,
        classify: impl Fn(&AdapterError) -> Verdict + Send + 'static,
    ) -> Self {
        self.classify_error = Some(Box::new(classify));
        self
    }

    pub fn on_error(mut self, on_error: impl Fn(&AdapterError) + Send + 'static) -> Self {
        self.on_error = Some(Box::new(on_error));
        self
    }

    pub fn format_error(
        mut self,
        format: impl Fn(&AdapterError, Verdict) -> String + Send + 'static,
    ) -> Self {
        self.format_error = Some(Box::new(format));
        self
    }
}

pub fn cancelled_when_message_contains(
    needle: &'static str,
) -> impl Fn(&AdapterError) -> Verdict + Send {
    let needle = needle.to_ascii_lowercase();
    move |error: &AdapterError| {
        let lower = error.message.to_ascii_lowercase();
        if lower.contains(&needle)
            || lower.contains("cancel")
            || lower.contains("interrupted")
            || lower.contains("killed")
        {
            Verdict::Cancelled
        } else {
            Verdict::Error
        }
    }
}

// ── The lifecycle ────────────────────────────────────────────────────────────

/// Assemble a draft carrying everything one call records up front.
fn draft_for(
    target: &CallTarget,
    recorded_sql: &str,
    meta: &GateMeta,
    verdict: Verdict,
    reason: Option<String>,
) -> LogDraft {
    let mut draft = LogDraft::new(
        target.connection_id.as_str(),
        target.connection_name.as_str(),
        recorded_sql,
    );
    draft.verdict = verdict;
    draft.reason = reason;
    draft.categories = Some(meta.category.clone());
    draft.source = Some(meta.action.clone());
    draft.database = meta.database.clone();
    draft.group = target.group.clone();
    draft
}

/// Run a tool body through the policy gate + activity log, returning a shaped
/// MCP response. See the module docs for the flow.
///
/// `run` receives the log row id (`None` only when the row could not be
/// created, which must not fail the call). Log-write failures are swallowed
/// exactly like the TypeScript server's try/catch: auditing degrades, the
/// call does not.
pub async fn run_gated<F, Fut>(
    store: &Store,
    target: &CallTarget,
    meta: GateMeta,
    run: F,
    opts: GateOpts,
) -> ToolResult
where
    F: FnOnce(Option<i64>) -> Fut,
    Fut: Future<Output = Result<Outcome, AdapterError>>,
{
    let recorded_sql = meta.command.clone().unwrap_or_else(|| meta.detail.clone());

    // A precheck block never writes a pending row.
    if let Some(block) = opts.precheck.and_then(|precheck| precheck()) {
        let draft = draft_for(
            target,
            &recorded_sql,
            &meta,
            Verdict::Blocked,
            Some(block.clone()),
        );
        let _ = store.create_log_entry(draft);
        return err(format!("Blocked: {block}"));
    }

    let log_id = store
        .create_log_entry(draft_for(
            target,
            &recorded_sql,
            &meta,
            Verdict::Pending,
            None,
        ))
        .ok();

    let finalized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct PendingGuard {
        store: *const Store,
        id: Option<i64>,
        finalized: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    unsafe impl Send for PendingGuard {}
    unsafe impl Sync for PendingGuard {}
    impl Drop for PendingGuard {
        fn drop(&mut self) {
            if !self.finalized.load(std::sync::atomic::Ordering::SeqCst)
                && let Some(id) = self.id
                && let Some(s) = unsafe { self.store.as_ref() }
            {
                let _ = s.update_log_entry(
                    id,
                    LogUpdate {
                        verdict: Verdict::Error,
                        reason: Some("Query was interrupted (dropped or panicked)".into()),
                        response_text: Some(
                            "Error: Query was interrupted (dropped or panicked)".into(),
                        ),
                        ..Default::default()
                    },
                );
            }
        }
    }
    let _guard = PendingGuard {
        store: store as *const Store,
        id: log_id,
        finalized: finalized.clone(),
    };

    use futures::FutureExt as _;
    let run_result: Result<Outcome, AdapterError> = match std::panic::AssertUnwindSafe(run(log_id))
        .catch_unwind()
        .await
    {
        Ok(r) => r,
        Err(_) => Err(crate::error::AdapterError::new("Query panicked")),
    };
    finalized.store(true, std::sync::atomic::Ordering::SeqCst);

    match run_result {
        Ok(Outcome::Blocked(block)) => {
            if let Some(id) = log_id {
                let update = LogUpdate {
                    verdict: Verdict::Blocked,
                    reason: Some(block.clone()),
                    ..Default::default()
                };
                let _ = store.update_log_entry(id, update);
            }
            err(format!("Blocked: {block}"))
        }
        Ok(Outcome::Ran(ran)) => {
            let status = if ran.is_error {
                Verdict::Error
            } else {
                Verdict::Allowed
            };
            if let Some(id) = log_id {
                let update = LogUpdate {
                    sql: ran.command.clone(),
                    verdict: status,
                    reason: ran.reason.clone(),
                    result: ran.result.clone(),
                    response_text: ran.response_text.clone(),
                };
                let _ = store.update_log_entry(id, update);
            }
            if ran.is_error {
                err(ran.text)
            } else {
                ok(ran.text)
            }
        }
        Err(error) => {
            let status = match opts
                .classify_error
                .as_ref()
                .map(|classify| classify(&error))
            {
                Some(Verdict::Cancelled) => Verdict::Cancelled,
                _ => Verdict::Error,
            };
            let text = opts
                .format_error
                .as_ref()
                .map(|format| format(&error, status))
                .unwrap_or_else(|| {
                    format!(
                        "{}: {}",
                        if status == Verdict::Cancelled {
                            "Cancelled"
                        } else {
                            "Error"
                        },
                        error.message
                    )
                });
            if let Some(id) = log_id {
                let update = LogUpdate {
                    sql: meta.command.clone(),
                    verdict: status,
                    reason: Some(error.message.clone()),
                    response_text: Some(text.clone()),
                    ..Default::default()
                };
                let _ = store.update_log_entry(id, update);
            }
            // Cancellations and SSH pending approvals keep their resources:
            // neither is the driver's fault.
            if status == Verdict::Error
                && !error.is_ssh_pending()
                && let Some(on_error) = opts.on_error.as_ref()
            {
                on_error(&error);
            }
            err(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SSH_CONNECT_PENDING_CODE;
    use pluk_store::{LogEntry, LogRange, LogScope};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pluk.db");
        let store = Store::open(&path).expect("open");
        (dir, store)
    }

    fn entries(store: &Store) -> Vec<LogEntry> {
        store
            .read_log_page(&LogScope::Connection("c1".into()), LogRange::All, None)
            .expect("page")
            .entries
    }

    fn target() -> CallTarget {
        CallTarget::new("c1", "Main DB")
    }

    async fn single_entry(store: &Store) -> LogEntry {
        let all = entries(store);
        assert_eq!(all.len(), 1, "expected exactly one log row");
        all.into_iter().next().expect("row")
    }

    #[tokio::test]
    async fn allowed_calls_finalize_with_text_and_snapshot() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "query", "SELECT 1"),
            |_| async {
                Ok(Outcome::Ran(RunOutcome {
                    text: "[]".into(),
                    result: Some(QueryResult {
                        fields: vec!["id".into()],
                        rows: vec![json!(1)],
                    }),
                    response_text: Some("[]".into()),
                    ..Default::default()
                }))
            },
            GateOpts::default(),
        )
        .await;

        assert!(!result.is_error);
        assert_eq!(result.text(), "[]");
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "allowed");
        assert_eq!(row.categories.as_deref(), Some("read"));
        assert_eq!(row.source.as_deref(), Some("query"));
        assert_eq!(row.sql, "SELECT 1");
        assert_eq!(row.response_text.as_deref(), Some("[]"));
        assert_eq!(row.row_count, Some(1));
        let snapshot: serde_json::Value =
            serde_json::from_str(row.result_json.as_deref().expect("result json")).unwrap();
        assert_eq!(snapshot["fields"], json!(["id"]));
        assert_eq!(snapshot["rows"], json!([1]));
    }

    #[tokio::test]
    async fn a_precheck_block_writes_no_pending_row() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("write", "del", "DEL key"),
            |_| async { unreachable!("a blocked call never runs") },
            GateOpts::default().precheck(|| Some("deletes are off".into())),
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.text(), "Blocked: deletes are off");
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "blocked");
        assert_eq!(row.reason.as_deref(), Some("deletes are off"));
        assert_eq!(row.sql, "DEL key");
    }

    #[tokio::test]
    async fn a_post_pending_block_finalizes_as_blocked() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "query", "SELECT * FROM big"),
            |_| async { Ok(Outcome::blocked("estimated cost too high")) },
            GateOpts::default(),
        )
        .await;

        assert_eq!(result.text(), "Blocked: estimated cost too high");
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "blocked");
        assert_eq!(row.reason.as_deref(), Some("estimated cost too high"));
    }

    #[tokio::test]
    async fn cancellation_is_a_verdict_of_its_own_and_skips_the_error_hook() {
        let (_dir, store) = temp_store();
        let evicted = Arc::new(Mutex::new(false));
        let flag = evicted.clone();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "query", "SELECT pg_sleep(100)"),
            |_| async { Err(AdapterError::new("query cancelled: terminating connection")) },
            GateOpts::default()
                .classify_error(cancelled_when_message_contains("cancelled"))
                .on_error(move |_| *flag.lock().unwrap() = true),
        )
        .await;

        assert!(result.is_error);
        assert_eq!(
            result.text(),
            "Cancelled: query cancelled: terminating connection"
        );
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "cancelled");
        assert_eq!(
            row.reason.as_deref(),
            Some("query cancelled: terminating connection")
        );
        assert!(
            !*evicted.lock().unwrap(),
            "a cancelled query must not evict its pooled connection"
        );
    }

    #[tokio::test]
    async fn errors_run_the_hook_and_use_the_default_format() {
        let (_dir, store) = temp_store();
        let seen = Arc::new(Mutex::new(None::<String>));
        let capture = seen.clone();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "get", "GET key"),
            |_| async { Err(AdapterError::new("connection refused")) },
            GateOpts::default()
                .on_error(move |e| *capture.lock().unwrap() = Some(e.message.clone())),
        )
        .await;

        assert_eq!(result.text(), "Error: connection refused");
        assert_eq!(seen.lock().unwrap().as_deref(), Some("connection refused"));
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "error");
        assert_eq!(
            row.response_text.as_deref(),
            Some("Error: connection refused")
        );
    }

    #[tokio::test]
    async fn ssh_pending_approvals_suppress_the_error_hook() {
        let (_dir, store) = temp_store();
        let called = Arc::new(Mutex::new(false));
        let flag = called.clone();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "list_tables", "introspect"),
            |_| async {
                Err(
                    AdapterError::new("SSH connection is waiting on an approval.")
                        .with_code(SSH_CONNECT_PENDING_CODE),
                )
            },
            GateOpts::default().on_error(move |_| *flag.lock().unwrap() = true),
        )
        .await;

        assert_eq!(
            result.text(),
            "Error: SSH connection is waiting on an approval."
        );
        assert!(
            !*called.lock().unwrap(),
            "pending approvals must not trigger eviction"
        );
        assert_eq!(single_entry(&store).await.verdict, "error");
    }

    #[tokio::test]
    async fn format_error_replaces_the_default_text() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "query", "SELECT nope"),
            |_| async { Err(AdapterError::new("relation \"nope\" does not exist")) },
            GateOpts::default().format_error(|e, _| format!("That table isn't there ({e}).")),
        )
        .await;

        assert_eq!(
            result.text(),
            "That table isn't there (relation \"nope\" does not exist)."
        );
    }

    #[tokio::test]
    async fn non_cancelling_classifiers_are_coerced_to_error() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("read", "query", "SELECT 1"),
            |_| async { Err(AdapterError::new("boom")) },
            GateOpts::default().classify_error(|_| Verdict::Allowed),
        )
        .await;

        assert_eq!(result.text(), "Error: boom");
        assert_eq!(single_entry(&store).await.verdict, "error");
    }

    #[tokio::test]
    async fn the_exact_command_replaces_the_recorded_sql() {
        let (_dir, store) = temp_store();

        let failed = run_gated(
            &store,
            &target(),
            GateMeta::new("admin", "gh", "gh pr list").with_command("gh pr list --limit 30"),
            |_| async { Err(AdapterError::new("exit status 1")) },
            GateOpts::default(),
        )
        .await;
        assert!(failed.is_error);

        let succeeded = run_gated(
            &store,
            &target(),
            GateMeta::new("admin", "gh", "gh repo view"),
            |_| async {
                Ok(Outcome::Ran(RunOutcome {
                    text: "pluk".into(),
                    response_text: Some("pluk".into()),
                    command: Some("gh repo view pluk".into()),
                    ..Default::default()
                }))
            },
            GateOpts::default(),
        )
        .await;
        assert!(!succeeded.is_error);

        let rows = entries(&store);
        assert_eq!(rows.len(), 2);
        // Newest first: the succeeded call replaced its SQL on finalization.
        assert_eq!(rows[0].sql, "gh repo view pluk");
        assert_eq!(rows[1].sql, "gh pr list --limit 30");
    }

    #[tokio::test]
    async fn ran_but_failed_outcomes_are_logged_errors_that_still_return_text() {
        let (_dir, store) = temp_store();
        let result = run_gated(
            &store,
            &target(),
            GateMeta::new("admin", "run_command", "deploy.sh"),
            |_| async { Ok(Outcome::failed("exit status 3", "non-zero exit")) },
            GateOpts::default(),
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.text(), "exit status 3");
        let row = single_entry(&store).await;
        assert_eq!(row.verdict, "error");
        assert_eq!(row.reason.as_deref(), Some("non-zero exit"));
    }

    #[test]
    fn tool_results_serialize_in_the_mcp_wire_shape() {
        assert_eq!(
            serde_json::to_value(ok("hello")).unwrap(),
            serde_json::json!({ "content": [{ "type": "text", "text": "hello" }] })
        );
        assert_eq!(
            serde_json::to_value(err("bad")).unwrap(),
            serde_json::json!({ "content": [{ "type": "text", "text": "bad" }], "isError": true })
        );
    }
}
