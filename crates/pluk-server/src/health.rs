//! Per-connection health, surfaced to the UI so a failing connection shows red
//! instead of silently looking fine.
//!
//! Ported from `pluk/src/mcp/health.ts`. Updated wherever a connection is
//! actually exercised — driver/tunnel setup (adapters) and the manual test
//! endpoint — so connect/auth/tunnel failures (the silent ones) are visible
//! without the user clicking Test. An id **absent** from the map means "not
//! checked yet", a third state the frontend renders distinctly from healthy.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Error,
}

/// One observation of one integration's reachability.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnHealth {
    pub status: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Epoch milliseconds of the last observation.
    pub at: i64,
}

#[derive(Default)]
pub struct HealthMap {
    entries: Mutex<BTreeMap<String, ConnHealth>>,
}

impl HealthMap {
    pub fn record(&self, id: &str, status: HealthStatus, error: Option<String>) {
        let entry = ConnHealth { status, error, at: now_millis() };
        self.entries.lock().expect("health lock").insert(id.to_string(), entry);
    }

    /// Every recorded observation keyed by integration id.
    ///
    /// A `BTreeMap` so the JSON is stable for humans diffing responses; the
    /// TypeScript object iterated insertion order, which nothing relied on.
    pub fn all(&self) -> BTreeMap<String, ConnHealth> {
        self.entries.lock().expect("health lock").clone()
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_is_distinguishable_from_ok_and_error() {
        let health = HealthMap::default();
        assert!(health.all().get("i1").is_none(), "unobserved ids stay absent");

        health.record("i1", HealthStatus::Ok, None);
        health.record("i2", HealthStatus::Error, Some("connection refused".into()));

        let all = health.all();
        assert_eq!(all["i1"].status, HealthStatus::Ok);
        assert_eq!(all["i1"].error, None);
        assert_eq!(all["i2"].status, HealthStatus::Error);
        assert_eq!(all["i2"].error.as_deref(), Some("connection refused"));
        // The wire shape keeps `status`, optional `error`, and `at`.
        let value = serde_json::to_value(all).unwrap();
        assert_eq!(value["i1"], serde_json::json!({ "status": "ok", "at": value["i1"]["at"] }));
        assert_eq!(value["i2"]["status"], "error");
    }

    #[test]
    fn later_observations_replace_earlier_ones() {
        let health = HealthMap::default();
        health.record("i1", HealthStatus::Error, Some("down".into()));
        health.record("i1", HealthStatus::Ok, None);
        assert_eq!(health.all()["i1"].status, HealthStatus::Ok);
    }
}
