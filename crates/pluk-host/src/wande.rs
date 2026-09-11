//! The Wande surface the window calls: the Pluk ID Chrome pairs with, and the
//! posts waiting on the person at the keyboard.
//!
//! Nothing an agent can reach sends a post. These commands are the other
//! side of that: the list a person sees and the actions they take on it.
//! Sending one is what puts the text into the page and submits. They read and
//! write the browser state in process, so no route carrying the Pluk ID has
//! to be able to publish.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

use pluk_browser::{BrowserState, PostAnswer, PostChoice, PostPrompt, PostPrompter};
use pluk_store::browser::{DRAFT_TTL_MS, Draft, ScheduleReservation};

use crate::commands::HostState;

const WINDOW_LABEL: &str = "wande-post";
const WIDTH: f64 = 460.0;
const HEIGHT: f64 = 420.0;

type CmdResult<T> = Result<T, String>;

/// The one value Wande asks for, read from the running browser surface so it
/// is always the one Chrome can actually connect with.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlukId {
    pub id: String,
}

/// Both lists the Wande panel shows, plus whether Chrome is there to run them
/// and whether a post is going out right now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WandePosts {
    pub chrome_connected: bool,
    /// One post goes out at a time; while this is set, another can only be
    /// queued.
    pub sending: bool,
    pub waiting: Vec<WaitingPost>,
    pub queued: Vec<QueuedPost>,
}

/// A post that has been asked for and is waiting for someone to send it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingPost {
    pub id: String,
    pub text: String,
    /// The posts of a thread, in order. Empty for a plain post.
    pub parts: Vec<String>,
    /// The post this one replies to, when it is a reply.
    pub replying_to: Option<String>,
    /// When it stops being sendable, in epoch milliseconds.
    pub expires_at: i64,
    pub can_queue: bool,
}

/// A post holding a slot in the queue, and how that slot ended up.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedPost {
    pub id: String,
    pub text: String,
    pub scheduled_at: i64,
    /// `reserved`, `committed`, `unknown` or `released`.
    pub status: String,
}

impl From<Draft> for WaitingPost {
    fn from(draft: Draft) -> Self {
        WaitingPost {
            can_queue: draft.can_queue(),
            expires_at: draft.created_at + DRAFT_TTL_MS,
            replying_to: draft.post_id.is_some().then_some(draft.target_url),
            parts: draft.parts,
            text: draft.text,
            id: draft.id,
        }
    }
}

impl From<ScheduleReservation> for QueuedPost {
    fn from(reservation: ScheduleReservation) -> Self {
        QueuedPost {
            id: reservation.draft_id,
            text: reservation.text,
            scheduled_at: reservation.scheduled_at,
            status: reservation.status,
        }
    }
}

fn browser<'a>(state: &'a State<'_, HostState>) -> CmdResult<&'a BrowserState> {
    state
        .shared
        .browser
        .as_deref()
        .ok_or_else(|| "Browser control is not running.".to_string())
}

/// Turn a refusal into something the window can show, keeping the one case a
/// person can act on — the post is gone — in this layer's own words.
fn refusal(error: pluk_browser::BridgeError, gone: &str) -> String {
    if error.code == "already_consumed" {
        return gone.to_owned();
    }
    error.message
}

#[tauri::command]
pub fn get_pluk_id(state: State<'_, HostState>) -> CmdResult<PlukId> {
    Ok(PlukId {
        id: browser(&state)?.pairing_key().to_string(),
    })
}

#[tauri::command]
pub fn list_wande_posts(state: State<'_, HostState>) -> CmdResult<WandePosts> {
    let browser = browser(&state)?;
    let waiting = browser
        .pending_drafts()
        .map_err(|error| error.message)?
        .into_iter()
        .map(WaitingPost::from)
        .collect();
    let queued = browser
        .scheduled_posts()
        .map_err(|error| error.message)?
        .into_iter()
        .map(QueuedPost::from)
        .collect();
    Ok(WandePosts {
        chrome_connected: browser.extension_connected(),
        sending: browser.sending().map_err(|error| error.message)?,
        waiting,
        queued,
    })
}

#[tauri::command]
pub fn send_wande_post(
    state: State<'_, HostState>,
    draft_id: String,
    queue: bool,
) -> CmdResult<()> {
    browser(&state)?
        .confirm_draft(&draft_id, queue)
        .map_err(|error| refusal(error, "This post is no longer waiting. Its time ran out."))
}

#[tauri::command]
pub fn discard_wande_post(state: State<'_, HostState>, draft_id: String) -> CmdResult<()> {
    browser(&state)?
        .discard_draft(&draft_id)
        .map_err(|error| refusal(error, "This post is no longer waiting. Its time ran out."))
}

#[tauri::command]
pub fn cancel_queued_wande_post(state: State<'_, HostState>, draft_id: String) -> CmdResult<()> {
    browser(&state)?
        .cancel_scheduled(&draft_id)
        .map_err(|error| refusal(error, "This post is already on its way out."))
}

/// One post the owner has not answered yet.
struct PendingPost {
    prompt: PostPrompt,
    answer: oneshot::Sender<PostChoice>,
}

#[derive(Default)]
pub struct PostQuestions {
    pending: Mutex<HashMap<String, PendingPost>>,
}

impl PostQuestions {
    fn take(&self, id: &str) -> Option<PendingPost> {
        self.pending.lock().expect("pending posts").remove(id)
    }
}

/// Asks about a post by opening a window of its own above other apps, the
/// way a refused call is asked about. Dismissing the window is not an answer:
/// the post keeps waiting in the panel.
pub struct WindowPostPrompter {
    app: AppHandle,
}

impl WindowPostPrompter {
    pub fn new(app: AppHandle) -> Self {
        WindowPostPrompter { app }
    }
}

impl PostPrompter for WindowPostPrompter {
    fn ask(&self, prompt: PostPrompt) -> PostAnswer<'_> {
        Box::pin(async move {
            let id = prompt.draft_id.clone();
            let (answer, wait) = oneshot::channel();
            self.app
                .state::<PostQuestions>()
                .pending
                .lock()
                .expect("pending posts")
                .insert(id.clone(), PendingPost { prompt, answer });
            let _cleanup = Cleanup {
                app: self.app.clone(),
                id: id.clone(),
            };
            let app = self.app.clone();
            if self
                .app
                .run_on_main_thread(move || open_window(&app, &id))
                .is_err()
            {
                return PostChoice::Later;
            }
            wait.await.unwrap_or(PostChoice::Later)
        })
    }
}

/// Drops the question and closes its window however the wait ends.
struct Cleanup {
    app: AppHandle,
    id: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.app.state::<PostQuestions>().take(&self.id);
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || close_window(&app));
    }
}

/// Show the window for post `id`. Posts are asked about one at a time, so an
/// open window just gets the next one.
fn open_window(app: &AppHandle, id: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("pluk://wande-question", id);
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let url = format!("confirm.html?post={id}");
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
        // Closing the window decides nothing: the post waits in the panel.
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                answer_all(&app, PostChoice::Later);
            }
        });
    }
}

fn close_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

fn answer_all(app: &AppHandle, choice: PostChoice) {
    let pending: Vec<PendingPost> = app
        .state::<PostQuestions>()
        .pending
        .lock()
        .expect("pending posts")
        .drain()
        .map(|(_, pending)| pending)
        .collect();
    for entry in pending {
        let _ = entry.answer.send(choice);
    }
}

/// What the post window shows. `None` once the post has been answered or has
/// run out of time.
#[tauri::command]
pub fn wande_question(state: State<'_, PostQuestions>, id: String) -> Option<PostPrompt> {
    state
        .pending
        .lock()
        .expect("pending posts")
        .get(&id)
        .map(|pending| pending.prompt.clone())
}

#[tauri::command]
pub fn wande_answer(state: State<'_, PostQuestions>, id: String, choice: PostChoice) {
    if let Some(pending) = state.take(&id) {
        let _ = pending.answer.send(choice);
    }
}
