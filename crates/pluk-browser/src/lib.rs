//! Browser control for Pluk.
//!
//! An agent asks for a page on X; a paired Chrome extension, signed in as the
//! user, drives it. Invoking a tool creates a **job**, the extension picks it
//! up over the WebSocket at `/wande/extension/ws`, and posts back a result
//! this crate validates against the command it issued.
//!
//! Publishing never happens because an agent asked. `post` and `reply` write
//! a draft that waits in Pluk and touch no page; the owner confirms it there,
//! and only that confirmation sends the one command that fills the composer
//! and submits. That boundary is the safety model. No route reachable with
//! the Pluk ID publishes anything.
//!
//! The TypeScript half of the protocol lives in `extension/src/protocol.ts`;
//! both sides validate every envelope independently.

mod catalog;
mod prompt;
mod protocol;
mod service;
mod thread;

pub use catalog::{ToolSpec as CatalogTool, tools as catalog_tools};
pub use prompt::{PostAnswer, PostChoice, PostPrompt, PostPrompter, set_post_prompter};
pub use thread::{X_POST_LIMIT, weighted_length};
pub use service::{
    BridgeError, BrowserState, INTEGRATION_TYPE, pairing_key, pairing_key_for, router,
};
