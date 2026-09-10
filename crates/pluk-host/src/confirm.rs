//! The window that asks about a call the policy refused.
//!
//! One question at a time, in a window of its own that stays above other
//! apps — the answer is needed while the agent is waiting, so it cannot sit
//! behind the main window or wait for Pluk to be opened. The window fetches
//! its own contents by id, sends back the answer, and closes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pluk_adapters::{ConfirmChoice, ConfirmPrompter, ConfirmRequest};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

const WINDOW_LABEL: &str = "confirm";
const WIDTH: f64 = 460.0;
const HEIGHT: f64 = 460.0;

/// One unanswered question.
struct Pending {
    request: ConfirmRequest,
    answer: oneshot::Sender<ConfirmChoice>,
}

#[derive(Default)]
pub struct ConfirmState {
    pending: Mutex<HashMap<String, Pending>>,
}

impl ConfirmState {
    fn take(&self, id: &str) -> Option<Pending> {
        self.pending.lock().expect("pending questions").remove(id)
    }
}

/// Asks by opening the confirm window.
pub struct WindowPrompter {
    app: AppHandle,
    registry: Arc<pluk_adapters::AdapterRegistry>,
}

impl WindowPrompter {
    pub fn new(app: AppHandle, registry: Arc<pluk_adapters::AdapterRegistry>) -> Self {
        WindowPrompter { app, registry }
    }

    /// The integration's kind as the app names it elsewhere.
    fn kind_label(&self, kind: &str) -> String {
        self.registry
            .get(kind)
            .map(|adapter| adapter.label().to_string())
            .unwrap_or_else(|| kind.to_string())
    }
}

#[async_trait]
impl ConfirmPrompter for WindowPrompter {
    async fn ask(&self, request: ConfirmRequest) -> ConfirmChoice {
        let state = self.app.state::<ConfirmState>();
        let id = pluk_store::new_id();
        let (answer, wait) = oneshot::channel();
        let request = ConfirmRequest {
            integration_kind: self.kind_label(&request.integration_kind),
            ..request
        };
        state
            .pending
            .lock()
            .expect("pending questions")
            .insert(id.clone(), Pending { request, answer });

        // Whoever asked can give up waiting — the question and its window go
        // with them.
        let _cleanup = Cleanup {
            app: self.app.clone(),
            id: id.clone(),
        };

        let app = self.app.clone();
        let window_id = id.clone();
        if self
            .app
            .run_on_main_thread(move || open_window(&app, &window_id))
            .is_err()
        {
            return ConfirmChoice::Deny;
        }
        wait.await.unwrap_or(ConfirmChoice::Deny)
    }
}

/// Drops the question and closes its window however the wait ends.
struct Cleanup {
    app: AppHandle,
    id: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.app.state::<ConfirmState>().take(&self.id);
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || close_window(&app));
    }
}

/// Show the window for question `id`. An open window is reused: only one
/// question is ever outstanding, so a leftover window just gets new contents.
fn open_window(app: &AppHandle, id: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("pluk://confirm-question", id);
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let url = format!("confirm.html?id={id}");
    let built = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("Pluk")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .center()
        .focused(true)
        .build();
    if let Ok(window) = built {
        let app = app.clone();
        // Closing the window is an answer of its own: no.
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                answer_all(&app, ConfirmChoice::Deny);
            }
        });
    }
}

fn close_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

/// Settle every outstanding question with `choice`.
fn answer_all(app: &AppHandle, choice: ConfirmChoice) {
    let state = app.state::<ConfirmState>();
    let pending: Vec<Pending> = state
        .pending
        .lock()
        .expect("pending questions")
        .drain()
        .map(|(_, pending)| pending)
        .collect();
    for entry in pending {
        let _ = entry.answer.send(choice);
    }
}

/// What the confirm window shows. `None` once the question has been answered
/// or has run out of time.
#[tauri::command]
pub fn confirm_question(
    state: tauri::State<'_, ConfirmState>,
    id: String,
) -> Option<ConfirmRequest> {
    state
        .pending
        .lock()
        .expect("pending questions")
        .get(&id)
        .map(|pending| pending.request.clone())
}

/// How long an unanswered question stays open, in seconds — the window counts
/// down with it.
#[tauri::command]
pub fn confirm_answer_window() -> u64 {
    pluk_adapters::ANSWER_WINDOW.as_secs()
}

#[tauri::command]
pub fn confirm_answer(state: tauri::State<'_, ConfirmState>, id: String, choice: ConfirmChoice) {
    if let Some(pending) = state.take(&id) {
        let _ = pending.answer.send(choice);
    }
}
