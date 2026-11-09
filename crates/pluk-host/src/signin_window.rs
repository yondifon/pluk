//! 1Password shows its SSH approval only for the app in front, so a connect
//! waiting on the agent brings Pluk forward with this window until it ends.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "signin";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 180.0;

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

pub fn open(app: &AppHandle, name: &str, agent: Option<&str>) -> SignInWindow {
    *HOLDERS.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    let url = signin_url(name, agent);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show(&handle, &url));
    SignInWindow { app: app.clone() }
}

fn signin_url(name: &str, agent: Option<&str>) -> String {
    let mut url = format!("signin.html?name={}", encode(name));
    if let Some(agent) = agent {
        url.push_str(&format!("&agent={}", encode(agent)));
    }
    url
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn show(app: &AppHandle, url: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("Pluk")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .always_on_top(true)
        .center()
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
