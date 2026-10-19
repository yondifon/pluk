use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;
use tauri_plugin_notification::{NotificationExt, PermissionState};

static PERMISSION_ASKED: AtomicBool = AtomicBool::new(false);

/// The agent is only named when it is known; otherwise the copy points at every
/// place an approval can come from.
pub fn copy(name: &str, agent: Option<&str>) -> (String, String) {
    match agent {
        Some(agent) => (
            format!("Approve in {agent}"),
            format!("{name} is waiting for your SSH key."),
        ),
        None => (
            "Approve your SSH sign-in".to_string(),
            format!(
                "{name} is waiting for you to approve the SSH connection. Check your key agent, security key, or browser."
            ),
        ),
    }
}

/// Shows the notice for a connect waiting on an approval. Failures are logged and
/// dropped so the connect itself is never held up.
pub fn show(app: &AppHandle, name: &str, agent: Option<&str>) {
    let notification = app.notification();
    // macOS shows nothing until the user has allowed notifications, and asking
    // twice in one run only repeats a prompt they already answered.
    if !matches!(notification.permission_state(), Ok(PermissionState::Granted))
        && !PERMISSION_ASKED.swap(true, Ordering::SeqCst)
    {
        match notification.request_permission() {
            Ok(PermissionState::Granted) => {}
            Ok(_) => return,
            Err(reason) => {
                eprintln!("Pluk couldn't ask to show notifications: {reason}");
                return;
            }
        }
    }
    let (title, body) = copy(name, agent);
    if let Err(reason) = notification.builder().title(title).body(body).show() {
        eprintln!("Pluk couldn't show the approval notice: {reason}");
    }
}
