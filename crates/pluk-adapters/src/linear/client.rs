use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::error::AdapterError;

const ENDPOINT: &str = "https://api.linear.app/graphql";
pub const TIMEOUT_MS: u64 = 20_000;

/// Hook for tests: if set, used instead of real HTTP.
pub type LinearRunner = Arc<
    dyn Fn(
            String,
            Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

static RUNNER: OnceLock<Mutex<Option<LinearRunner>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<LinearRunner>> {
    RUNNER.get_or_init(|| Mutex::new(None))
}

pub fn set_linear_runner(runner: Option<LinearRunner>) {
    *slot().lock().unwrap() = runner;
}

fn runner() -> Option<LinearRunner> {
    slot().lock().unwrap().clone()
}

pub async fn linear_graphql(
    api_key: &str,
    query: &str,
    variables: Value,
) -> Result<Value, AdapterError> {
    if api_key.is_empty() {
        return Err(AdapterError::new(
            "Linear API key is missing. Set it in the integration config.",
        ));
    }
    if let Some(r) = runner() {
        return r(query.to_string(), variables).await;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(TIMEOUT_MS))
        .build()
        .map_err(|e| AdapterError::new(e.to_string()))?;
    let body = serde_json::json!({ "query": query, "variables": variables });
    let res = client
        .post(ENDPOINT)
        .header("Content-Type", "application/json")
        .header("Authorization", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AdapterError::new(format!("Linear API timed out after {}s", TIMEOUT_MS / 1000))
            } else {
                AdapterError::new(format!("Linear API request failed: {e}"))
            }
        })?;
    let status = res.status().as_u16();
    let text = res
        .text()
        .await
        .map_err(|e| AdapterError::new(format!("Linear API request failed: {e}")))?;
    let json: Value = serde_json::from_str(&text)
        .map_err(|_| AdapterError::new(format!("Linear API {status}: non-JSON response")))?;
    if let Some(errors) = json.get("errors").and_then(|v| v.as_array())
        && !errors.is_empty()
    {
        let msgs: Vec<String> = errors
            .iter()
            .filter_map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        return Err(AdapterError::new(format!("Linear: {}", msgs.join("; "))));
    }
    if status >= 400 {
        return Err(AdapterError::new(format!("Linear API {status}")));
    }
    json.get("data")
        .cloned()
        .ok_or_else(|| AdapterError::new("Linear API returned no data"))
}
