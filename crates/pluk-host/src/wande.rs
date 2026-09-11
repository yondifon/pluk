//! The Wande surface the window calls: the Pluk ID Chrome pairs with, and the
//! posts waiting on the person at the keyboard.
//!
//! Nothing an agent can reach sends a post. These commands are the other
//! side of that: the list a person sees and the actions they take on it, for
//! the posts that were not answered when they were written. They read and
//! write the browser state in process, so no route carrying the Pluk ID has
//! to be able to publish.

use serde::Serialize;
use tauri::State;

use pluk_browser::BrowserState;
use pluk_store::browser::{DRAFT_TTL_MS, Draft, ScheduleReservation};

use crate::commands::HostState;

type CmdResult<T> = Result<T, String>;

/// The one value Wande asks for, read from the running browser surface so it
/// is always the one Chrome can actually connect with.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlukId {
    pub id: String,
}

/// Both lists the Wande panel shows, plus whether Chrome is there to run them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WandePosts {
    pub chrome_connected: bool,
    pub waiting: Vec<WaitingPost>,
    pub queued: Vec<QueuedPost>,
}

/// A post that has been written and is waiting for someone to send it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingPost {
    pub id: String,
    pub account: String,
    pub text: String,
    /// What this one replies to, when it is a reply.
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
    pub account: String,
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
            replying_to: (!draft.target_excerpt.is_empty()).then_some(draft.target_excerpt),
            account: draft.visible_account_identity,
            text: draft.text,
            id: draft.id,
        }
    }
}

impl From<ScheduleReservation> for QueuedPost {
    fn from(reservation: ScheduleReservation) -> Self {
        QueuedPost {
            id: reservation.draft_id,
            account: reservation.account_identity,
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
        .map_err(|error| refusal(error, "This post is no longer waiting — its time ran out."))
}

#[tauri::command]
pub fn discard_wande_post(state: State<'_, HostState>, draft_id: String) -> CmdResult<()> {
    browser(&state)?
        .discard_draft(&draft_id)
        .map_err(|error| refusal(error, "This post is no longer waiting — its time ran out."))
}

#[tauri::command]
pub fn cancel_queued_wande_post(state: State<'_, HostState>, draft_id: String) -> CmdResult<()> {
    browser(&state)?
        .cancel_scheduled(&draft_id)
        .map_err(|error| refusal(error, "This post is already on its way out."))
}
