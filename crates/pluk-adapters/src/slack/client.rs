use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::error::AdapterError;

pub const TIMEOUT_MS: u64 = 20_000;
const BASE_URL: &str = "https://slack.com/api";

#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub token: String,
    pub default_channel: Option<String>,
}

pub fn slack_config_from(conn: &pluk_store::Integration) -> Result<SlackConfig, AdapterError> {
    let token = conn
        .config
        .get("bot_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if token.trim().is_empty() {
        return Err(AdapterError::new(
            "Slack bot token is missing. Set it in the integration config.",
        ));
    }
    let default_channel = conn
        .config
        .get("default_channel")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(SlackConfig {
        token,
        default_channel,
    })
}

pub fn resolve_channel(cfg: &SlackConfig, arg: Option<&str>) -> Result<String, AdapterError> {
    let channel = arg
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.default_channel.clone());
    match channel {
        Some(c) if !c.trim().is_empty() => Ok(c),
        _ => Err(AdapterError::new(
            "No channel given. Pass a channel id/name or set a default channel in the integration config.",
        )),
    }
}

// Test hook: intercept slack requests
pub type SlackRunner = Arc<
    dyn Fn(
            String,
            Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

static RUNNER: OnceLock<Mutex<Option<SlackRunner>>> = OnceLock::new();
fn slot() -> &'static Mutex<Option<SlackRunner>> {
    RUNNER.get_or_init(|| Mutex::new(None))
}
pub fn set_slack_runner(r: Option<SlackRunner>) {
    *slot().lock().unwrap() = r;
}
fn get_runner() -> Option<SlackRunner> {
    slot().lock().unwrap().clone()
}

pub async fn slack_request(
    cfg: &SlackConfig,
    method: &str,
    params: Value,
) -> Result<Value, AdapterError> {
    if let Some(runner) = get_runner() {
        return runner(method.to_string(), params).await;
    }
    // Build form body from params
    let mut form: Vec<(String, String)> = Vec::new();
    if let Some(obj) = params.as_object() {
        for (k, v) in obj {
            if v.is_null() {
                continue;
            }
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => v.to_string(),
            };
            if !s.is_empty() {
                form.push((k.clone(), s));
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(TIMEOUT_MS))
        .build()
        .map_err(|e| AdapterError::new(e.to_string()))?;

    let url = format!("{BASE_URL}/{method}");
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.token))
        .form(&form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AdapterError::new(format!("Slack API timed out after {}s", TIMEOUT_MS / 1000))
            } else {
                AdapterError::new(format!("Slack API request failed: {e}"))
            }
        })?;

    if !res.status().is_success() {
        return Err(AdapterError::new(format!(
            "Slack API {method}: HTTP {}",
            res.status().as_u16()
        )));
    }
    let json: Value = res
        .json()
        .await
        .map_err(|e| AdapterError::new(format!("Slack API response parse failed: {e}")))?;
    if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::new(format!("Slack API {method}: {err}")));
    }
    Ok(json)
}
