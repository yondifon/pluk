//! Browser control for Pluk.
//!
//! An agent asks for a page on X; a paired Chrome extension, signed in as the
//! user, drives it. Invoking a tool creates a **job**, the extension picks it
//! up over the WebSocket at `/wande/extension/ws`, and posts back a result
//! this crate validates against the command it issued.
//!
//! Publishing stays two steps on purpose: `compose_post` types the exact text
//! and produces a **draft** — nothing is public — and only
//! `POST /wande/drafts/{id}/confirm` publishes it. That boundary is the safety
//! model; `prepare_reply` and `submit_reply` mirror it for replies.
//!
//! The TypeScript half of the protocol lives in `extension/src/protocol.ts`;
//! both sides validate every envelope independently.

mod catalog;
mod protocol;
mod service;

pub use catalog::{ToolSpec as CatalogTool, tools as catalog_tools};
pub use service::{BrowserState, INTEGRATION_TYPE, pairing_key, router};
