//! Auto-update.
//!
//! `tauri-plugin-updater` owns the network half: it fetches the manifest named
//! in `tauri.conf.json > plugins.updater.endpoints`, verifies the Minisign
//! signature against the public key baked into the binary, downloads the
//! platform artifact and swaps the bundle in place.
//!
//! This module owns the state half — a pure, testable machine plus the Tauri
//! command and event surface the window renders from. A build that ships
//! without a public key or with the placeholder endpoint stays `Disabled`: no
//! network, no banner, no toast.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// How often the app checks for updates in the background.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Endpoint the repo ships with. It marks the updater as unconfigured so a
/// source checkout never reaches the network.
pub const PLACEHOLDER_ENDPOINT: &str = "https://example.com/updates/latest.json";

/// Every state change reaches the window here.
pub const STATE_EVENT: &str = "pluk://update-state";

/// Emitted only for a check the person asked for, and only when they are
/// already on the newest version — a background check stays silent.
pub const NO_UPDATE_EVENT: &str = "pluk://update-none";

/// Current app version — filled from `CARGO_PKG_VERSION` via tauri.conf.json's
/// `version` field at build time. Used only for display; the updater plugin
/// compares against the manifest's `version` itself.
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}


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

    /// Read `plugins.updater` out of the resolved Tauri config. A missing or
    /// malformed block reads as the placeholder, which keeps the updater off.
    pub fn from_plugins(plugins: &tauri::utils::config::PluginConfig) -> Self {
        let Some(block) = plugins.0.get("updater") else {
            return Self::placeholder();
        };
        Self {
            pubkey: block
                .get("pubkey")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            endpoints: block
                .get("endpoints")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Whether the updater is configured: pubkey non-empty and at least one real endpoint.
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
    /// No banner, no toast, no crash.
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


fn emit_state<R: Runtime>(app: &AppHandle<R>, updater: &Updater) {
    if let Ok(payload) = serde_json::to_value(updater.state()) {
        let _ = app.emit(STATE_EVENT, payload);
    }
}

/// A mismatched signature means the artifact was not built with the key this
/// binary trusts — worth naming apart from any other transport failure.
fn is_signature_failure(error: &tauri_plugin_updater::Error) -> bool {
    use tauri_plugin_updater::Error as E;
    matches!(
        error,
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) | E::InvalidUpdaterFormat
    )
}

fn check_failure(error: &tauri_plugin_updater::Error) -> FailureKind {
    use tauri_plugin_updater::Error as E;
    if is_signature_failure(error) {
        FailureKind::Signature
    } else if matches!(
        error,
        E::Reqwest(_) | E::Network(_) | E::ReleaseNotFound | E::Io(_)
    ) {
        FailureKind::Unreachable
    } else {
        FailureKind::Other
    }
}

fn download_failure(error: &tauri_plugin_updater::Error) -> FailureKind {
    use tauri_plugin_updater::Error as E;
    if is_signature_failure(error) {
        FailureKind::Signature
    } else if matches!(error, E::Reqwest(_) | E::Network(_) | E::Io(_)) {
        FailureKind::Download
    } else {
        FailureKind::Other
    }
}

fn handle<R: Runtime>(app: &AppHandle<R>) -> Option<Updater> {
    app.try_state::<Updater>().map(|s| s.inner().clone())
}

async fn fetch_update<R: Runtime>(
    app: &AppHandle<R>,
) -> tauri_plugin_updater::Result<Option<tauri_plugin_updater::Update>> {
    app.updater()?.check().await
}

/// Ask the endpoint whether a newer version exists. `user_initiated` decides
/// whether "you are already up to date" is worth saying out loud.
pub async fn run_check<R: Runtime>(app: AppHandle<R>, user_initiated: bool) {
    let Some(updater) = handle(&app) else { return };
    if !updater.is_configured() || !updater.begin_check() {
        return;
    }
    emit_state(&app, &updater);

    match fetch_update(&app).await {
        Ok(found) => {
            let is_none = found.is_none();
            updater.finish_check(found.map(|update| UpdateInfo {
                version: update.version.clone(),
                notes: update.body.clone(),
                pub_date: update.date.map(|d| d.to_string()),
            }));
            emit_state(&app, &updater);
            if is_none && user_initiated {
                let _ = app.emit(NO_UPDATE_EVENT, current_version());
            }
        }
        Err(error) => {
            updater.fail(check_failure(&error), error.to_string());
            emit_state(&app, &updater);
        }
    }
}

/// Download the announced update, verify it, swap the bundle, relaunch.
pub async fn run_install<R: Runtime>(app: AppHandle<R>) {
    let Some(updater) = handle(&app) else { return };
    if !updater.begin_download() {
        return;
    }
    emit_state(&app, &updater);

    let update = match fetch_update(&app).await {
        Ok(Some(update)) => update,
        Ok(None) => {
            updater.finish_check(None);
            emit_state(&app, &updater);
            return;
        }
        Err(error) => {
            updater.fail_loud(download_failure(&error), error.to_string());
            emit_state(&app, &updater);
            return;
        }
    };

    let version = update.version.clone();
    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;
    let outcome = update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let Some(total) = total.filter(|t| *t > 0) else {
                    return;
                };
                let percent = ((downloaded * 100) / total).min(100) as u8;
                if percent == last_percent {
                    return;
                }
                last_percent = percent;
                updater.update_progress(percent);
                emit_state(&app, &updater);
            },
            || {},
        )
        .await;

    match outcome {
        Ok(()) => {
            updater.mark_ready(version);
            emit_state(&app, &updater);
            app.restart();
        }
        Err(error) => {
            updater.fail_loud(download_failure(&error), error.to_string());
            emit_state(&app, &updater);
        }
    }
}


#[tauri::command]
pub fn get_update_state(updater: tauri::State<'_, Updater>) -> serde_json::Value {
    serde_json::to_value(updater.state()).unwrap_or(serde_json::Value::Null)
}

#[tauri::command]
pub async fn check_for_updates<R: Runtime>(app: AppHandle<R>) {
    run_check(app, true).await;
}

#[tauri::command]
pub async fn install_update<R: Runtime>(app: AppHandle<R>) {
    run_install(app).await;
}


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
    fn config_reads_pubkey_and_endpoints_from_plugins_block() {
        let plugins = tauri::utils::config::PluginConfig(
            [(
                "updater".to_string(),
                serde_json::json!({
                    "pubkey": "dW50cnVzdGVk",
                    "endpoints": ["https://updates.pluk.example/latest.json"],
                }),
            )]
            .into_iter()
            .collect(),
        );
        let cfg = UpdaterConfig::from_plugins(&plugins);
        assert_eq!(cfg.pubkey, "dW50cnVzdGVk");
        assert_eq!(cfg.endpoints, ["https://updates.pluk.example/latest.json"]);
        assert!(cfg.is_configured());
    }

    #[test]
    fn config_without_updater_block_stays_unconfigured() {
        let plugins = tauri::utils::config::PluginConfig(Default::default());
        assert!(!UpdaterConfig::from_plugins(&plugins).is_configured());
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
