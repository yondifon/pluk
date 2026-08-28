//! Tauri updater integration — replaces the source-checkout `git pull` path.
//!
//! The Swift `UpdateChecker` required a baked `PlukBuildCommit` + `PlukRepoPath`
//! and polled `git ls-remote` every 6h. The Rust replacement uses
//! `tauri-plugin-updater`: the app ships a public key in `tauri.conf.json`,
//! polls a versioned JSON manifest on a schedule and on demand, Tauri
//! verifies the Minisign signature, downloads the platform artifact, and
//! restarts.
//!
//! This module owns the **state machine** and its Tauri surface. The network
//! half is delegated to `tauri-plugin-updater`; the state half is pure and
//! tested without touching the net.
//!
//! See `docs/updater-r23.md` for the packaging contract R23 must fulfil.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json;

/// How often the app checks for updates in the background.
/// Kept at 6h to match the Swift checker unless benchmarks show otherwise.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Placeholder endpoint — R23 replaces this at packaging time.
/// Any endpoint containing `example.com` is treated as unconfigured and the
/// updater degrades quietly (same as Swift's `isConfigured == false` in dev).
pub const PLACEHOLDER_ENDPOINT: &str = "https://example.com/updates/latest.json";

/// Current app version — filled from `CARGO_PKG_VERSION` via tauri.conf.json's
/// `version` field at build time. Used only for display; the updater plugin
/// compares against the manifest's `version` itself.
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ── Configuration ─────────────────────────────────────────────────────────

/// Runtime view of whether the updater is configured.
///
/// Mirrors `tauri.conf.json > plugins.updater`. Unconfigured values cause the
/// updater to stay in `Disabled` and never emit a banner or toast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdaterConfig {
    /// Minisign public key (base64). Empty means unconfigured.
    pub pubkey: String,
    /// Manifest endpoints. Empty or containing the placeholder means unconfigured.
    pub endpoints: Vec<String>,
}

impl UpdaterConfig {
    pub fn placeholder() -> Self {
        Self {
            pubkey: String::new(),
            endpoints: vec![PLACEHOLDER_ENDPOINT.to_string()],
        }
    }

    /// Whether Tauri's updater has something to do. Mirrors Swift's
    /// `isConfigured` — baked commit + repo path both present and repo exists.
    /// Here: pubkey non-empty and at least one real endpoint.
    pub fn is_configured(&self) -> bool {
        if self.pubkey.trim().is_empty() {
            return false;
        }
        if self.endpoints.is_empty() {
            return false;
        }
        // Any non-placeholder endpoint counts as configured.
        self.endpoints
            .iter()
            .any(|e| !e.contains("example.com") && !e.trim().is_empty())
    }

    /// Whether the config still holds the placeholder (failing-loudly signal
    /// for R23/packaging: the binary was shipped without manifest wiring).
    pub fn is_placeholder(&self) -> bool {
        self.pubkey.trim().is_empty() || self.endpoints.iter().any(|e| e.contains("example.com"))
    }
}

// ── State machine ─────────────────────────────────────────────────────────

/// Why an update check or install failed. The frontend uses `kind` to decide
/// whether to show a toast (`Download`, `Signature`, `Other`) or degrade
/// quietly (`Unreachable`, and any failure while `Disabled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// Manifest endpoint unreachable or timed out.
    Unreachable,
    /// Artifact download failed (network, 404, truncated).
    Download,
    /// Minisign signature verification failed.
    Signature,
    /// Anything else (serde, io, Tauri error).
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UpdateState {
    /// Updater not configured (dev run, placeholder endpoint/pubkey).
    /// No banner, no toast, no crash — matches Swift dev-run disabled path.
    Disabled {
        reason: String,
    },
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes: Option<String>,
    },
    Downloading {
        progress: u8,
    },
    Ready {
        version: String,
    },
    Failed {
        #[serde(rename = "kind")]
        kind: FailureKind,
        message: String,
    },
}

impl UpdateState {
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled { .. })
    }

    pub fn should_show_banner(&self) -> bool {
        matches!(self, Self::Available { .. } | Self::Ready { .. })
    }

    pub fn should_show_toast(&self) -> bool {
        match self {
            Self::Failed { kind, .. } => !matches!(kind, FailureKind::Unreachable),
            _ => false,
        }
    }

    /// Human label for menu / banner.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled { .. } => "Updates unavailable",
            Self::Idle => "Idle",
            Self::Checking => "Checking…",
            Self::UpToDate => "Up to date",
            Self::Available { .. } => "Update available",
            Self::Downloading { .. } => "Downloading…",
            Self::Ready { .. } => "Ready to restart",
            Self::Failed { .. } => "Update failed",
        }
    }
}

/// Info parsed from the update manifest for an available version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

// ── Updater handle (shared state) ─────────────────────────────────────────

/// Thread-safe updater handle owned by Tauri's managed state.
/// Frontend reads via `get_update_state`; backend drives transitions and
/// emits `pluk://update-state` on each change.
#[derive(Debug, Clone)]
pub struct Updater {
    config: UpdaterConfig,
    state: Arc<Mutex<UpdateState>>,
}

impl Updater {
    pub fn new(config: UpdaterConfig) -> Self {
        let initial = if config.is_configured() {
            UpdateState::Idle
        } else {
            UpdateState::Disabled {
                reason: "updater not configured".to_string(),
            }
        };
        Self {
            config,
            state: Arc::new(Mutex::new(initial)),
        }
    }

    pub fn config(&self) -> &UpdaterConfig {
        &self.config
    }

    pub fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().expect("updater state").clone()
    }

    pub fn set_state(&self, next: UpdateState) {
        *self.state.lock().expect("updater state") = next;
    }

    // ── Pure transitions — no I/O, fully testable ─────────────────────

    /// Begin a check. No-op when disabled or already checking/updating.
    /// Returns true if a check should actually be performed.
    pub fn begin_check(&self) -> bool {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return false;
        }
        match &*s {
            UpdateState::Checking | UpdateState::Downloading { .. } => false,
            _ => {
                *s = UpdateState::Checking;
                true
            }
        }
    }

    /// Record a successful check result.
    pub fn finish_check(&self, info: Option<UpdateInfo>) {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return;
        }
        *s = match info {
            None => UpdateState::UpToDate,
            Some(i) => UpdateState::Available {
                version: i.version,
                notes: i.notes,
            },
        };
    }

    /// Record a check/install failure.
    pub fn fail(&self, kind: FailureKind, message: impl Into<String>) {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return;
        }
        // Unreachable during a check degrades quietly back to Idle rather than
        // surfacing a toast — matches "graceful absence" for flaky endpoints.
        // Callers that explicitly want a louder unreachable should use
        // `fail_loud`.
        if kind == FailureKind::Unreachable {
            *s = UpdateState::Idle;
            return;
        }
        *s = UpdateState::Failed {
            kind,
            message: message.into(),
        };
    }

    /// Like `fail` but surfaces `Unreachable` as a `Failed` state (for tests
    /// that need to observe it, or for download-phase unreachable).
    pub fn fail_loud(&self, kind: FailureKind, message: impl Into<String>) {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return;
        }
        *s = UpdateState::Failed {
            kind,
            message: message.into(),
        };
    }

    pub fn begin_download(&self) -> bool {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return false;
        }
        match &*s {
            UpdateState::Available { .. } => {
                *s = UpdateState::Downloading { progress: 0 };
                true
            }
            _ => false,
        }
    }

    pub fn update_progress(&self, progress: u8) {
        let mut s = self.state.lock().expect("updater state");
        if let UpdateState::Downloading { .. } = &*s {
            *s = UpdateState::Downloading {
                progress: progress.min(100),
            };
        }
    }

    pub fn mark_ready(&self, version: String) {
        let mut s = self.state.lock().expect("updater state");
        if s.is_disabled() {
            return;
        }
        *s = UpdateState::Ready { version };
    }

    pub fn reset_after_failure(&self) {
        let mut s = self.state.lock().expect("updater state");
        if let UpdateState::Failed { .. } = &*s {
            *s = UpdateState::Idle;
        }
    }

    /// Reset `UpToDate` back to `Idle` so the next periodic tick can fire
    /// without the UI lingering on "up to date".
    pub fn reset_up_to_date(&self) {
        let mut s = self.state.lock().expect("updater state");
        if matches!(&*s, UpdateState::UpToDate) {
            *s = UpdateState::Idle;
        }
    }
}

// ── Tauri command surface (thin wrappers over the handle) ─────────────────

#[tauri::command]
pub fn get_update_state(updater: tauri::State<'_, Updater>) -> serde_json::Value {
    serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null)
}

#[tauri::command]
pub async fn check_for_updates(
    updater: tauri::State<'_, Updater>,
) -> Result<serde_json::Value, String> {
    if !updater.is_configured() {
        return Ok(serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null));
    }
    if !updater.begin_check() {
        return Ok(serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null));
    }
    Ok(serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null))
}

#[tauri::command]
pub fn install_update(updater: tauri::State<'_, Updater>) -> Result<serde_json::Value, String> {
    if !updater.is_configured() {
        return Err("updater not configured".to_string());
    }
    if !updater.begin_download() {
        return Err(format!("cannot install from state {:?}", updater.state()));
    }
    Ok(serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> UpdaterConfig {
        UpdaterConfig {
            pubkey: "dW50cnVzdGVkIGtleSBmb3IgdGVzdHM=".to_string(),
            endpoints: vec!["https://updates.pluk.example/latest.json".to_string()],
        }
    }

    fn placeholder() -> UpdaterConfig {
        UpdaterConfig::placeholder()
    }

    #[test]
    fn no_update_available_ends_up_to_date() {
        let u = Updater::new(configured());
        assert_eq!(u.state(), UpdateState::Idle);
        assert!(u.begin_check());
        assert_eq!(u.state(), UpdateState::Checking);
        u.finish_check(None);
        assert_eq!(u.state(), UpdateState::UpToDate);
        assert!(!u.state().should_show_banner());
        // UpToDate can be reset to Idle for the next tick.
        u.reset_up_to_date();
        assert_eq!(u.state(), UpdateState::Idle);
    }

    #[test]
    fn update_available_shows_banner() {
        let u = Updater::new(configured());
        u.begin_check();
        u.finish_check(Some(UpdateInfo {
            version: "0.2.0".to_string(),
            notes: Some("Fixes".to_string()),
            pub_date: None,
        }));
        assert_eq!(
            u.state(),
            UpdateState::Available {
                version: "0.2.0".to_string(),
                notes: Some("Fixes".to_string())
            }
        );
        assert!(u.state().should_show_banner());
        // Download flow
        assert!(u.begin_download());
        assert_eq!(u.state(), UpdateState::Downloading { progress: 0 });
        u.update_progress(42);
        assert_eq!(u.state(), UpdateState::Downloading { progress: 42 });
        u.mark_ready("0.2.0".to_string());
        assert_eq!(
            u.state(),
            UpdateState::Ready {
                version: "0.2.0".to_string()
            }
        );
        assert!(u.state().should_show_banner());
    }

    #[test]
    fn download_failure_surfaces_failed_with_toast() {
        let u = Updater::new(configured());
        u.begin_check();
        u.finish_check(Some(UpdateInfo {
            version: "0.2.0".to_string(),
            notes: None,
            pub_date: None,
        }));
        u.begin_download();
        u.fail_loud(FailureKind::Download, "network reset");
        assert_eq!(
            u.state(),
            UpdateState::Failed {
                kind: FailureKind::Download,
                message: "network reset".to_string()
            }
        );
        assert!(u.state().should_show_toast());
        u.reset_after_failure();
        assert_eq!(u.state(), UpdateState::Idle);
    }

    #[test]
    fn signature_verification_failure_surfaces_failed() {
        let u = Updater::new(configured());
        u.begin_check();
        u.finish_check(Some(UpdateInfo {
            version: "0.2.0".to_string(),
            notes: None,
            pub_date: None,
        }));
        u.begin_download();
        u.fail_loud(FailureKind::Signature, "signature mismatch: expected …");
        assert_eq!(
            u.state(),
            UpdateState::Failed {
                kind: FailureKind::Signature,
                message: "signature mismatch: expected …".to_string()
            }
        );
        assert!(u.state().should_show_toast());
        assert!(!u.state().should_show_banner());
    }

    #[test]
    fn unconfigured_degrades_quietly_no_banner_no_toast() {
        let u = Updater::new(placeholder());
        assert_eq!(
            u.state(),
            UpdateState::Disabled {
                reason: "updater not configured".to_string()
            }
        );
        assert!(!u.is_configured());
        assert!(u.config().is_placeholder());
        assert!(!u.state().should_show_banner());
        assert!(!u.state().should_show_toast());
        // begin_check is a no-op
        assert!(!u.begin_check());
        assert_eq!(
            u.state(),
            UpdateState::Disabled {
                reason: "updater not configured".to_string()
            }
        );
        // fail is a no-op while disabled
        u.fail(FailureKind::Download, "should be ignored");
        assert!(matches!(u.state(), UpdateState::Disabled { .. }));
        // Even loud fail is swallowed when disabled
        u.fail_loud(FailureKind::Signature, "ignored");
        assert!(matches!(u.state(), UpdateState::Disabled { .. }));
    }

    #[test]
    fn unreachable_during_check_degrades_quietly_to_idle() {
        let u = Updater::new(configured());
        u.begin_check();
        // Endpoint unreachable — quiet path folds back to Idle, no toast.
        u.fail(FailureKind::Unreachable, "dns failure");
        assert_eq!(u.state(), UpdateState::Idle);
        assert!(!u.state().should_show_toast());
        assert!(!u.state().should_show_banner());
    }

    #[test]
    fn placeholder_endpoint_is_not_configured() {
        let cfg = UpdaterConfig {
            pubkey: "somekey".to_string(),
            endpoints: vec![PLACEHOLDER_ENDPOINT.to_string()],
        };
        assert!(!cfg.is_configured());
        let cfg2 = UpdaterConfig {
            pubkey: "".to_string(),
            endpoints: vec!["https://real.example/latest.json".to_string()],
        };
        assert!(!cfg2.is_configured());
        let cfg3 = configured();
        assert!(cfg3.is_configured());
    }

    #[test]
    fn checking_guard_prevents_concurrent_checks() {
        let u = Updater::new(configured());
        assert!(u.begin_check());
        assert!(!u.begin_check());
        u.finish_check(None);
        assert_eq!(u.state(), UpdateState::UpToDate);
    }

    #[test]
    fn disabled_label_and_idle_label() {
        assert_eq!(UpdateState::Idle.label(), "Idle");
        assert_eq!(
            UpdateState::Disabled { reason: "x".into() }.label(),
            "Updates unavailable"
        );
    }
}
