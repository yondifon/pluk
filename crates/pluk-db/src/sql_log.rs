//! Introspection instrumentation.
//!
//! JS used `AsyncLocalStorage<SqlLogContext>` in `pluk/src/db/sqlLog.ts` so
//! driver calls could log without threading a context param. Rust equivalent
//! is a `task_local!` — same semantics (scoped to the async task tree), but
//! explicit about being Tokio-task-local rather than thread-local. We chose
//! task-local over an explicit `ctx` parameter because it preserves the same
//! call-site ergonomics as the TS driver (no extra arg on every method) and
//! avoids polluting the `Driver` trait. An explicit parameter would have been
//! more visible but would have required R09 to thread context through every
//! tool call; task-local lets `instrument_driver` wrap methods identically to
//! the JS `instrumentDriver`.

use std::cell::RefCell;

#[derive(Debug, Clone)]
pub struct SqlLogContext {
    pub conn_id: String,
    pub conn_name: String,
    pub source: String,
    pub group: Option<LogGroup>,
    pub database: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LogGroup {
    pub id: String,
    pub name: String,
}

thread_local! {
    static SQL_LOG_CTX: RefCell<Option<SqlLogContext>> = const { RefCell::new(None) };
}

tokio::task_local! {
    static TASK_SQL_LOG_CTX: SqlLogContext;
}

/// Run `f` with the given log context set for synchronous introspection recording.
/// Uses thread-local for non-async contexts; async callers should use `run_with_sql_log_async`.
pub fn run_with_sql_log<R>(ctx: SqlLogContext, f: impl FnOnce() -> R) -> R {
    SQL_LOG_CTX.with(|c| {
        let prev = c.replace(Some(ctx));
        let r = f();
        c.replace(prev);
        r
    })
}

/// Async version using Tokio task-local when inside a Tokio task, falling back to thread-local.
pub async fn run_with_sql_log_async<F, T>(ctx: SqlLogContext, f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // Use task-local if we are inside a Tokio runtime; otherwise thread-local fallback is handled by caller.
    // We set both so record hooks can read either.
    SQL_LOG_CTX.with(|c| { c.replace(Some(ctx.clone())); });
    let res = TASK_SQL_LOG_CTX.scope(ctx.clone(), f).await;
    SQL_LOG_CTX.with(|c| { c.replace(None); });
    res
}

/// Record one executed statement. No-op outside a logging context.
pub fn record_executed_sql(sql: &str, row_count: Option<i64>, error: Option<&str>) {
    // Try task-local first, then thread-local.
    let ctx = TASK_SQL_LOG_CTX.try_with(|c| c.clone()).ok()
        .or_else(|| SQL_LOG_CTX.with(|c| c.borrow().clone()));
    if let Some(ctx) = ctx {
        // In real integration this would call Store::logExecutedStatement via callback.
        // We dispatch through a global hook so tests can observe without a DB.
        dispatch_log(&ctx, sql, row_count, error);
    }
}

type LogHook = Box<dyn Fn(&SqlLogContext, &str, Option<i64>, Option<&str>) + Send + Sync>;

static LOG_HOOK: std::sync::OnceLock<std::sync::Mutex<Option<LogHook>>> = std::sync::OnceLock::new();

fn hook_slot() -> &'static std::sync::Mutex<Option<LogHook>> {
    LOG_HOOK.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn set_log_hook(hook: LogHook) {
    *hook_slot().lock().unwrap() = Some(hook);
}

pub fn clear_log_hook() {
    *hook_slot().lock().unwrap() = None;
}

fn dispatch_log(ctx: &SqlLogContext, sql: &str, row_count: Option<i64>, error: Option<&str>) {
    if let Some(hook) = hook_slot().lock().unwrap().as_ref() {
        hook(ctx, sql, row_count, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn task_local_records_with_context() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        set_log_hook(Box::new(move |ctx, sql, row_count, _| {
            seen2.lock().unwrap().push((ctx.source.clone(), sql.to_string(), row_count));
        }));
        let ctx = SqlLogContext { conn_id: "c1".into(), conn_name: "db".into(), source: "list_tables".into(), group: None, database: None };
        run_with_sql_log_async(ctx, async {
            record_executed_sql("SELECT 1", Some(1), None);
        }).await;
        let v = seen.lock().unwrap().clone();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "list_tables");
        clear_log_hook();
    }

    #[test]
    fn no_context_is_noop() {
        clear_log_hook();
        // Should not panic even with hook set to observe nothing
        let called = std::sync::Arc::new(std::sync::Mutex::new(false));
        let c2 = called.clone();
        set_log_hook(Box::new(move |_, _, _, _| { *c2.lock().unwrap() = true; }));
        record_executed_sql("SELECT 1", None, None);
        assert!(!*called.lock().unwrap());
        clear_log_hook();
    }
}
