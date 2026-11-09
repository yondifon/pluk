//! 1Password shows its SSH approval only for the app in front, so a connect
//! waiting on the agent brings Pluk forward until it ends. It does that with a
//! window too small and far off screen to see, so the only thing the person
//! notices is 1Password's own prompt.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "signin";
const OFF_SCREEN: f64 = -10_000.0;

/// Connects running at the same time share one window.
static HOLDERS: Mutex<usize> = Mutex::new(0);

pub struct SignInWindow {
    app: AppHandle,
}

impl Drop for SignInWindow {
    fn drop(&mut self) {
        let last = {
            let mut holders = HOLDERS.lock().unwrap_or_else(|p| p.into_inner());
            *holders = holders.saturating_sub(1);
            *holders == 0
        };
        if last {
            let app = self.app.clone();
            let _ = self.app.run_on_main_thread(move || close(&app));
        }
    }
}

pub fn open(app: &AppHandle) -> SignInWindow {
    *HOLDERS.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show(&handle));
    SignInWindow { app: app.clone() }
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        return;
    }
    let Ok(blank) = "about:blank".parse() else {
        return;
    };
    let _ = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::External(blank))
        .title("Pluk")
        .inner_size(1.0, 1.0)
        .position(OFF_SCREEN, OFF_SCREEN)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(true)
        .build();
}

/// Close the window and, when Pluk has nothing else open, give focus back to
/// the app the user was in.
fn close(app: &AppHandle) {
    if *HOLDERS.lock().unwrap_or_else(|p| p.into_inner()) > 0 {
        return;
    }
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.destroy();
    }
    #[cfg(target_os = "macos")]
    {
        let main_visible = app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        if !main_visible && app.get_webview_window("confirm").is_none() {
            let _ = app.hide();
        }
    }
}
