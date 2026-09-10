//! Asking the owner about a call their policy refused.
//!
//! Every adapter shares this path. When a call is refused and asking is on,
//! [`decide`] puts the question to whoever is attached — the desktop app opens
//! a window; a headless server has nobody to ask, so the refusal stands.
//!
//! One question is asked at a time: a second refused call waits for the first
//! to be answered rather than opening a window behind it. An unanswered
//! question is refused after [`ANSWER_WINDOW`].

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;

/// How long a question stays open before the call is refused.
pub const ANSWER_WINDOW: Duration = Duration::from_secs(60);

/// What one refused call needs the owner to see.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmRequest {
    pub integration_id: String,
    /// The integration's name, as the owner named it.
    pub integration_name: String,
    /// The kind of integration, for the line above the command ("SSH", "Postgres").
    pub integration_kind: String,
    /// The tool the agent called.
    pub tool: String,
    /// The exact command or statement, shown verbatim.
    pub command: String,
    /// Why it was refused.
    pub reason: String,
}

/// The owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfirmChoice {
    /// Run it this once.
    Once,
    /// Run it, and stop asking about it until Pluk quits.
    Session,
    /// Run it, and keep it allowed.
    Always,
    /// Do not run it.
    Deny,
}

impl ConfirmChoice {
    pub fn allows(self) -> bool {
        !matches!(self, ConfirmChoice::Deny)
    }
}

/// Whoever can put the question to the owner.
#[async_trait]
pub trait ConfirmPrompter: Send + Sync {
    async fn ask(&self, request: ConfirmRequest) -> ConfirmChoice;
}

type PrompterSlot = Mutex<Option<Arc<dyn ConfirmPrompter>>>;

fn prompter_slot() -> &'static PrompterSlot {
    static PROMPTER: OnceLock<PrompterSlot> = OnceLock::new();
    PROMPTER.get_or_init(|| Mutex::new(None))
}

/// Attach the thing that asks. The desktop host calls this once at startup;
/// without it every refused call stays refused.
pub fn set_prompter(prompter: Arc<dyn ConfirmPrompter>) {
    *prompter_slot().lock().expect("prompter") = Some(prompter);
}

pub fn has_prompter() -> bool {
    prompter_slot().lock().expect("prompter").is_some()
}

#[cfg(test)]
pub fn clear_prompter() {
    *prompter_slot().lock().expect("prompter") = None;
}

/// Commands allowed for the rest of this run. Deliberately in memory only:
/// quitting Pluk clears them.
fn session_allowances() -> &'static Mutex<HashSet<(String, String)>> {
    static ALLOWED: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    ALLOWED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn allowed_for_session(integration_id: &str, command: &str) -> bool {
    let key = (
        integration_id.to_string(),
        pluk_policy::approval::normalize(command),
    );
    session_allowances()
        .lock()
        .expect("session allowances")
        .contains(&key)
}

pub fn allow_for_session(integration_id: &str, command: &str) {
    session_allowances()
        .lock()
        .expect("session allowances")
        .insert((
            integration_id.to_string(),
            pluk_policy::approval::normalize(command),
        ));
}

#[cfg(test)]
pub fn clear_session_allowances() {
    session_allowances()
        .lock()
        .expect("session allowances")
        .clear();
}

/// Ask about one refused call, one question at a time.
///
/// Returns [`ConfirmChoice::Deny`] when nobody is attached to ask, and when the
/// question goes unanswered for [`ANSWER_WINDOW`].
pub async fn ask(request: ConfirmRequest) -> ConfirmChoice {
    let Some(prompter) = prompter_slot().lock().expect("prompter").clone() else {
        return ConfirmChoice::Deny;
    };
    static QUEUE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _turn = QUEUE
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    tokio::time::timeout(ANSWER_WINDOW, prompter.ask(request))
        .await
        .unwrap_or(ConfirmChoice::Deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_allowance_ignores_spacing_and_is_scoped_to_one_integration() {
        clear_session_allowances();
        allow_for_session("i1", "ls  -la");
        assert!(allowed_for_session("i1", "ls -la"));
        assert!(!allowed_for_session("i2", "ls -la"));
        assert!(!allowed_for_session("i1", "ls -l"));
        clear_session_allowances();
    }

    #[tokio::test]
    async fn with_nobody_attached_the_refusal_stands() {
        let choice = ask(ConfirmRequest {
            integration_id: "i1".into(),
            integration_name: "Prod".into(),
            integration_kind: "SSH".into(),
            tool: "run_command".into(),
            command: "rm -rf /".into(),
            reason: "command not allowed".into(),
        })
        .await;
        assert_eq!(choice, ConfirmChoice::Deny);
    }
}
