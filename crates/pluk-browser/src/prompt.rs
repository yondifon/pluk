//! What the owner is asked about a post that has just been written, and what
//! they can say back.
//!
//! A written post is never published by whoever asked for it. The moment the
//! composer is filled, the extension draws this question over the page the
//! text was typed into — where the owner is already looking — and their
//! answer travels back over the socket Pluk and Chrome already share.
//!
//! Nobody to ask and nobody answering mean the same thing: the post stays
//! waiting, and the panel in the app is where it gets picked up instead.
//! Silence never publishes.

use pluk_store::browser::Draft;

/// One written post, as the owner needs to see it before deciding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostPrompt {
    /// The account the post will appear under, exactly as the page shows it.
    pub account: String,
    /// The full text, verbatim — never an excerpt.
    pub text: String,
    /// What this replies to, when it is a reply.
    pub replying_to: Option<String>,
    /// Whether taking a later slot is an option for this one.
    pub can_queue: bool,
}

impl PostPrompt {
    pub fn from_draft(draft: &Draft) -> Self {
        PostPrompt {
            account: draft.visible_account_identity.clone(),
            text: draft.text.clone(),
            replying_to: (!draft.target_excerpt.is_empty()).then(|| draft.target_excerpt.clone()),
            can_queue: draft.can_queue(),
        }
    }
}

/// The owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl PostChoice {
    /// Read the word the overlay sent back. Anything unrecognised is no
    /// answer, so a malformed reply can only ever leave the post waiting.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "postNow" => PostChoice::PostNow,
            "queue" => PostChoice::Queue,
            "discard" => PostChoice::Discard,
            _ => PostChoice::Later,
        }
    }
}
