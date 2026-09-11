//! Browser control for Pluk.
//!
//! An agent asks for a page on X; a paired Chrome extension, signed in as the
//! user, drives it. Invoking a tool creates a **job**, the extension picks it
//! up over the WebSocket at `/wande/extension/ws`, and posts back a result
//! this crate validates against the command it issued.
//!
//! Publishing never happens because an agent asked. `post` types the
//! exact text and stops there; the owner is then shown what was written and
//! answers, and only their answer submits it. That boundary is the safety
//! model, and `reply` mirrors it for replies. No route reachable with
//! the Pluk ID publishes anything.
//!
//! The TypeScript half of the protocol lives in `extension/src/protocol.ts`;
//! both sides validate every envelope independently.

mod catalog;
mod prompt;
mod protocol;
mod service;

pub use catalog::{ToolSpec as CatalogTool, tools as catalog_tools};
pub use prompt::{PostChoice, PostPrompt};
pub use service::{BridgeError, BrowserState, INTEGRATION_TYPE, pairing_key, router};
