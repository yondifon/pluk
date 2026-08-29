use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::error::AdapterError;

pub const TIMEOUT_MS: u64 = 20_000;

#[derive(Debug, Clone)]
pub struct SentryConfig {
    pub base_url: String,
    pub token: String,
    pub org: String,
    pub project: Option<String>,
}

pub fn sentry_config_from(conn: &pluk_store::Integration) -> SentryConfig {
    let base_url = conn
        .config
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://sentry.io")
        .trim_end_matches('/')
        .to_string();
    let token = conn
        .config
        .get("auth_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let org = conn
        .config
        .get("org_slug")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let project = conn
        .config
        .get("project_slug")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    SentryConfig {
        base_url,
        token,
        org,
        project,
    }
}

/// Test hook
pub type SentryRunner = Arc<
    dyn Fn(
            String,
            String,
            Value,
            Option<Value>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<SentryRawResponse, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

#[derive(Debug, Clone)]
pub struct SentryRawResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: std::collections::HashMap<String, String>,
}

static RUNNER: OnceLock<Mutex<Option<SentryRunner>>> = OnceLock::new();
fn slot() -> &'static Mutex<Option<SentryRunner>> {
    RUNNER.get_or_init(|| Mutex::new(None))
}
pub fn set_sentry_runner(r: Option<SentryRunner>) {
    *slot().lock().unwrap() = r;
}
fn get_runner() -> Option<SentryRunner> {
    slot().lock().unwrap().clone()
}

async fn request(
    cfg: &SentryConfig,
    method: &str,
    path: &str,
    query: Option<Value>,
    body: Option<Value>,
) -> Result<SentryRawResponse, AdapterError> {
    if cfg.token.is_empty() {
        return Err(AdapterError::new(
            "Sentry auth token is missing. Set it in the integration config.",
        ));
    }
    if cfg.org.is_empty() {
        return Err(AdapterError::new(
            "Sentry organization slug is missing. Set it in the integration config.",
        ));
    }
    // build url
    let mut url = format!("{}/api/0{}", cfg.base_url, path);
    if let Some(q) = &query {
        let mut params: Vec<String> = Vec::new();
        if let Some(obj) = q.as_object() {
            for (k, v) in obj {
                if v.is_null() {
                    continue;
                }
                if *v == Value::String(String::new()) {
                    continue;
                }
                if let Some(arr) = v.as_array() {
                    for item in arr {
                        params.push(format!(
                            "{}={}",
                            urlencoding::encode(k),
                            urlencoding::encode(item.to_string().trim_matches('"'))
                        ));
                    }
                } else if let Some(s) = v.as_str() {
                    if s.is_empty() {
                        continue;
                    }
                    params.push(format!(
                        "{}={}",
                        urlencoding::encode(k),
                        urlencoding::encode(s)
                    ));
                } else {
                    params.push(format!(
                        "{}={}",
                        urlencoding::encode(k),
                        urlencoding::encode(v.to_string().trim_matches('"'))
                    ));
                }
            }
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
    }
    if let Some(r) = get_runner() {
        return r(method.to_string(), url, query.unwrap_or(Value::Null), body).await;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(TIMEOUT_MS))
        .build()
        .map_err(|e| AdapterError::new(e.to_string()))?;
    let mut req = client.request(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
        &url,
    );
    req = req
        .header("Authorization", format!("Bearer {}", cfg.token))
        .header("Content-Type", "application/json");
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| {
        if e.is_timeout() {
            AdapterError::new(format!("Sentry API timed out after {}s", TIMEOUT_MS / 1000))
        } else {
            AdapterError::new(format!("Sentry API request failed: {e}"))
        }
    })?;
    let status = res.status().as_u16();
    let headers: std::collections::HashMap<String, String> = res
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string().to_lowercase(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let bytes = res
        .bytes()
        .await
        .map_err(|e| AdapterError::new(format!("Sentry API request failed: {e}")))?
        .to_vec();
    // error handling
    if status >= 400 {
        let detail = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|j| {
                j.get("detail")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(" — {detail}")
        };
        if status == 401 {
            return Err(AdapterError::new(format!(
                "Sentry: unauthorized (401) — check the auth token{suffix}"
            )));
        }
        if status == 404 {
            return Err(AdapterError::new(format!(
                "Sentry: not found (404) — check the org/project/issue id{suffix}"
            )));
        }
        return Err(AdapterError::new(format!("Sentry API {status}{suffix}")));
    }
    Ok(SentryRawResponse {
        status,
        body: bytes,
        headers,
    })
}

pub async fn sentry_request(
    cfg: &SentryConfig,
    method: &str,
    path: &str,
    query: Option<Value>,
    body: Option<Value>,
) -> Result<Value, AdapterError> {
    let res = request(cfg, method, path, query, body).await?;
    if res.status == 204 {
        return Ok(Value::Null);
    }
    if res.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice::<Value>(&res.body)
        .map_err(|e| AdapterError::new(format!("Sentry API response parse failed: {e}")))
}

pub struct RawBytes {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub content_length: Option<String>,
}

pub async fn sentry_request_bytes(
    cfg: &SentryConfig,
    method: &str,
    path: &str,
    query: Option<Value>,
) -> Result<RawBytes, AdapterError> {
    let res = request(cfg, method, path, query, None).await?;
    Ok(RawBytes {
        bytes: res.body,
        content_type: res.headers.get("content-type").cloned(),
        content_length: res.headers.get("content-length").cloned(),
    })
}
