//! Putting a requested post to the owner the moment it arrives.
//!
//! An agent asking for a post writes a draft and nothing else. Whoever is
//! attached here (the desktop app, with a window that stays above other
//! apps) is asked straight away, and their answer is what sends, queues, or
//! drops it. Nobody attached, nobody answering, or the window dismissed all
//! mean the same thing: the post keeps waiting in the app's panel. Silence
//! never publishes.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pluk_store::browser::{DRAFT_TTL_MS, Draft, DraftImage, QuotedPost};
use serde::{Deserialize, Serialize};

/// One requested post, as the owner needs to see it before deciding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostPrompt {
    /// The integration this draft belongs to — needed to fetch an image
    /// preview, which is scoped to one integration's own drafts.
    pub integration_id: String,
    /// The draft this decides; the answer is applied to this one only.
    pub draft_id: String,
    /// The full text, verbatim, never an excerpt.
    pub text: String,
    /// The posts of a thread, in order. Empty for a plain post.
    pub parts: Vec<String>,
    /// The images this post was asked for, in order. Never a path — a
    /// preview is fetched by id, scoped to this one draft.
    pub images: Vec<DraftImage>,
    /// The post this replies to, when it is a reply.
    pub replying_to: Option<String>,
    /// The post this shares as it stands, when it is a repost. Nothing is
    /// read off the page before the answer, so this is the post's own
    /// address and nothing more.
    pub reposting: Option<String>,
    /// The post this quotes, when it is a quote.
    pub quoting: Option<QuotedPost>,
    /// Whether taking a later slot is an option for this one.
    pub can_queue: bool,
    /// When the post stops being sendable, in epoch milliseconds.
    pub closes_at: i64,
}

impl PostPrompt {
    pub fn from_draft(draft: &Draft) -> Self {
        PostPrompt {
            integration_id: draft.integration_id.clone(),
            draft_id: draft.id.clone(),
            text: draft.text.clone(),
            parts: draft.parts.clone(),
            images: draft.images.clone(),
            replying_to: (draft.kind == "reply").then(|| draft.target_url.clone()),
            reposting: (draft.kind == "repost").then(|| draft.target_url.clone()),
            quoting: draft.quoted.clone(),
            can_queue: draft.can_queue(),
            closes_at: draft.created_at + DRAFT_TTL_MS,
        }
    }
}

/// The owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PostChoice {
    /// Send it now.
    PostNow,
    /// Give it the next free slot instead.
    Queue,
    /// Never send it.
    Discard,
    /// No answer. The post keeps waiting.
    Later,
}

pub type PostAnswer<'a> = Pin<Box<dyn Future<Output = PostChoice> + Send + 'a>>;

/// Whoever can put a post to the owner.
pub trait PostPrompter: Send + Sync {
    fn ask(&self, prompt: PostPrompt) -> PostAnswer<'_>;
}

type PrompterSlot = Mutex<Option<Arc<dyn PostPrompter>>>;

fn prompter_slot() -> &'static PrompterSlot {
    static PROMPTER: OnceLock<PrompterSlot> = OnceLock::new();
    PROMPTER.get_or_init(|| Mutex::new(None))
}

/// Attach the thing that asks. The desktop host calls this once at startup;
/// without it every post waits in the panel.
pub fn set_post_prompter(prompter: Arc<dyn PostPrompter>) {
    *prompter_slot().lock().expect("post prompter") = Some(prompter);
}

/// Ask about one post, one at a time, for as long as it stays sendable.
pub(crate) async fn ask(prompt: PostPrompt) -> PostChoice {
    let Some(prompter) = prompter_slot().lock().expect("post prompter").clone() else {
        return PostChoice::Later;
    };
    static QUEUE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _turn = QUEUE
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    tokio::time::timeout(
        Duration::from_millis(DRAFT_TTL_MS.max(0) as u64),
        prompter.ask(prompt),
    )
    .await
    .unwrap_or(PostChoice::Later)
}
