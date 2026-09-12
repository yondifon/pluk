//! The browser queue, the loopback routes under `/wande`, and the extension
//! WebSocket.
//!
//! One job runs at a time. [`BrowserState::pump`] claims the next job the
//! paired extension can handle, sends it as a validated command envelope, and
//! arms a deadline; the result that comes back is checked against the command
//! that was issued before anything is stored. A queued post waits for its
//! reserved slot, so the pump also arms a wake-up timer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};
use url::Url;
use uuid::Uuid;

use pluk_store::browser::schedule::ScheduleSettings;
use pluk_store::browser::{
    ArtifactBody, BrowserError, Completion, Draft, DraftInput, Job, JobCompletion, JobInput,
    ProtocolErrorView, ScheduleReservation,
};
use pluk_store::{LogDraft, LogUpdate, Store, Verdict};

use crate::catalog::{catalog_value, find_tool};
use crate::prompt::{PostChoice, PostPrompt};
use crate::protocol::{
    Action, CommandInput, CreateJobRequest, ExtensionCapability, ExtensionMessage,
    HEARTBEAT_INTERVAL_MS, MAX_BODY_BYTES, MAX_CLOCK_SKEW_MS, MAX_EXTRACT_BYTES, MAX_HTML_BYTES,
    MAX_ID_LENGTH, MAX_MESSAGE_BYTES, MAX_SCREENSHOT_BYTES, MAX_URL_LENGTH, PROTOCOL_VERSION, Platform,
    ProtocolError, ResultMessage, canonicalize_target_url, is_allowed_extension_origin,
    make_command, make_heartbeat, make_heartbeat_ack, make_ready_envelope, parse_command_envelope,
    parse_create_job_request, parse_extension_message,
};

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LIST_LIMIT: i64 = 100;
const MAX_TOKEN_LENGTH: usize = 256;

/// The integration type browser control is configured under. One integration
/// holds every platform in [`catalog`](crate::catalog), so this never names
/// one. Activity is recorded against it so it reads like any other
/// connection's.
pub const INTEGRATION_TYPE: &str = "wande";

/// What the activity log calls browser work before the user has added the
/// integration. Rows written under it are outside every connection's log.
const LOG_CONNECTION_ID: &str = "browser";
const LOG_CONNECTION_NAME: &str = "Browser";

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}

/// Everything the `/wande` routes and the extension socket share. Cheap to
/// clone.
#[derive(Clone)]
pub struct BrowserState {
    queues: Arc<Mutex<HashMap<String, QueueState>>>,
    store: Arc<Store>,
    legacy_token: Arc<String>,
    port: u16,
    extension_origin: Option<String>,
    /// Timers are armed from Tauri command handlers too, and those run on a
    /// blocking thread with no runtime of their own, so the one this state
    /// was built on is carried along rather than looked up at spawn time.
    runtime: Handle,
    #[cfg(test)]
    test_now: Arc<Mutex<Option<i64>>>,
}

#[derive(Default)]
struct QueueState {
    connection: Option<ExtensionConnection>,
    active: Option<ActiveJob>,
    last_rejected_at: Option<i64>,
    wake_at: Option<i64>,
    wake_timer: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct ExtensionConnection {
    id: String,
    capabilities: Vec<ExtensionCapability>,
    sender: UnboundedSender<String>,
}

struct ActiveJob {
    job: Job,
    connection_id: String,
    timer: JoinHandle<()>,
}

impl BrowserState {
    /// Recover jobs the last run left mid-flight and mint the pairing key if
    /// this is the first start. `port` is the loopback port the server binds,
    /// which the Host and Origin checks pin requests to.
    pub fn new(store: Arc<Store>, port: u16) -> Result<Self, String> {
        let runtime = Handle::try_current()
            .map_err(|_| "Browser control has to start inside a Tokio runtime.".to_owned())?;
        let legacy_token = pairing_key(&store)?;
        let extension_origin = configured_extension_origin()?;
        store
            .browser()
            .recover_in_flight(now_millis())
            .map_err(|error| error.to_string())?;
        for integration in store
            .list_integrations()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|integration| integration.r#type == INTEGRATION_TYPE)
        {
            store
                .browser_for(&integration.id)
                .recover_in_flight(now_millis())
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            store,
            legacy_token: Arc::new(legacy_token),
            port,
            extension_origin,
            runtime,
            #[cfg(test)]
            test_now: Arc::new(Mutex::new(None)),
        })
    }

    /// The value the extension presents when it pairs — the Pluk ID the app
    /// shows. Minted on first start and kept in the store, so it survives
    /// restarts.
    pub fn pairing_key(&self) -> &str {
        &self.legacy_token
    }

    pub fn pairing_key_for(&self, integration_id: &str) -> Result<String, String> {
        pairing_key_for(&self.store, integration_id)
    }

    /// Posts waiting on a person, and the posts already holding a queue slot.
    ///
    /// Pluk's own window reads these in process rather than over the loopback
    /// routes, so nothing in the app has to carry the pairing key.
    pub fn pending_drafts(&self, integration_id: &str) -> Result<Vec<Draft>, BridgeError> {
        self.store
            .browser_for(integration_id)
            .list_pending_drafts(self.now())
            .map_err(BridgeError::from)
    }

    pub fn scheduled_posts(
        &self,
        integration_id: &str,
    ) -> Result<Vec<ScheduleReservation>, BridgeError> {
        self.store
            .browser_for(integration_id)
            .list_schedule_reservations(MAX_LIST_LIMIT, self.now())
            .map_err(BridgeError::from)
    }

    /// Publish a waiting draft, or give it the next queue slot.
    ///
    /// Only one post goes out at a time: while one is on its way into the
    /// page, another can take a queue slot but not go now.
    pub fn confirm_draft(
        &self,
        integration_id: &str,
        draft_id: &str,
        schedule: bool,
    ) -> Result<(), BridgeError> {
        if !schedule && self.sending(integration_id)? {
            return Err(BridgeError::with_status(
                "busy",
                "Another post is going out. Add this one to the queue, or wait for it to land.",
                409,
            ));
        }
        confirm_draft_value(self, integration_id, draft_id, schedule).map(|_| ())
    }

    /// Whether a post is on its way into the page right now.
    pub fn sending(&self, integration_id: &str) -> Result<bool, BridgeError> {
        self.store
            .browser_for(integration_id)
            .submission_in_flight(self.now())
            .map_err(BridgeError::from)
    }

    /// Drop a draft before it is confirmed.
    pub fn discard_draft(&self, integration_id: &str, draft_id: &str) -> Result<(), BridgeError> {
        let dropped = self
            .store
            .browser_for(integration_id)
            .cancel_draft(draft_id, self.now())
            .map_err(BridgeError::from)?;
        dropped.map(|_| ()).ok_or_else(already_consumed)
    }

    /// Release a queue slot so the post it holds never goes out.
    pub fn cancel_scheduled(
        &self,
        integration_id: &str,
        draft_id: &str,
    ) -> Result<(), BridgeError> {
        let released = self
            .store
            .browser_for(integration_id)
            .cancel_scheduled(draft_id, self.now())
            .map_err(BridgeError::from)?;
        released.map(|_| ()).ok_or_else(already_consumed)
    }

    /// Whether the paired extension is connected right now.
    pub fn extension_connected(&self, integration_id: &str) -> bool {
        self.queues
            .lock()
            .ok()
            .and_then(|queues| queues.get(integration_id).map(|queue| queue.connection.is_some()))
            .unwrap_or(false)
    }

    fn now(&self) -> i64 {
        #[cfg(test)]
        if let Ok(value) = self.test_now.lock()
            && let Some(now) = *value
        {
            return now;
        }
        now_millis()
    }

    #[cfg(test)]
    fn set_test_now(&self, now: i64) {
        *self
            .test_now
            .lock()
            .expect("test clock should be available") = Some(now);
    }

    /// Record what a finished job did to the page.
    ///
    /// One row per job, written once it reaches a terminal state — the live
    /// queue is already readable at `GET /wande/jobs`, so the audit trail
    /// carries the outcome rather than a second copy of the queue.
    ///
    /// Takes the store lock, so never call it while a
    /// [`pluk_store::browser::BrowserStore`] guard is alive.
    fn log_job(&self, integration_id: &str, job_id: &str) {
        let Ok(Some(job)) = self
            .store
            .browser_for(integration_id)
            .get_job(job_id, self.now())
        else {
            return;
        };
        let (verdict, reason) = match job.status.as_str() {
            "succeeded" => (Verdict::Allowed, None),
            "queued" | "running" => return,
            _ => (
                Verdict::Error,
                job.error
                    .map(|error| format!("{}: {}", error.code, error.message)),
            ),
        };
        let response = job.result.as_ref().map(|result| {
            serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
        });
        let (connection_id, connection_name) = self.log_connection(&job.integration_id);
        let mut draft = LogDraft::new(
            connection_id,
            connection_name,
            format!("{}.{} {}", job.platform, job.action, job.target_url),
        )
        .with_verdict(verdict)
        .with_source(job.action);
        draft.reason = reason;
        let Ok(entry_id) = self.store.create_log_entry(draft) else {
            return;
        };
        if let Some(response_text) = response {
            let _ = self.store.update_log_entry(
                entry_id,
                LogUpdate {
                    verdict,
                    response_text: Some(response_text),
                    ..LogUpdate::default()
                },
            );
        }
    }

    /// Whose activity log a finished job belongs in: the integration the user
    /// added, or [`LOG_CONNECTION_ID`] while there is none.
    fn log_connection(&self, integration_id: &str) -> (String, String) {
        match self.store.integration_by_id(integration_id) {
            Ok(Some(integration)) if integration.r#type == INTEGRATION_TYPE => {
                (integration.id, integration.name)
            }
            _ => (LOG_CONNECTION_ID.to_owned(), LOG_CONNECTION_NAME.to_owned()),
        }
    }

    /// Start what a caller asked for. A post or reply becomes a draft waiting
    /// on a person in Pluk and touches no page; everything else becomes a job
    /// for the browser.
    ///
    /// The same post asked for twice is the one draft already waiting, so a
    /// caller that lost the first answer cannot line up a duplicate.
    fn start(
        &self,
        integration_id: &str,
        request: &CreateJobRequest,
    ) -> Result<Value, BridgeError> {
        if !matches!(request.action, Action::Post | Action::Reply) {
            return self
                .create_job(integration_id, request)
                .map(|job| json!({ "job": job }));
        }
        let parts: Vec<String> = request
            .payload
            .get("parts")
            .and_then(Value::as_array)
            .filter(|parts| parts.len() > 1)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let input = DraftInput {
            platform: request.platform.as_str(),
            target_url: &request.target_url,
            post_id: request.payload.get("postId").and_then(Value::as_str),
            text: request
                .payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            parts: &parts,
            debug: request
                .payload
                .get("debug")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let now = self.now();
        let mut browser = self.store.browser_for(integration_id);
        if let Some(waiting) = browser
            .find_pending_draft(&input, now)
            .map_err(BridgeError::from)?
        {
            return Ok(json!({ "draft": waiting }));
        }
        let draft = browser
            .create_draft(&input, now)
            .map_err(BridgeError::from)?;
        drop(browser);
        self.ask_about(draft.clone());
        Ok(json!({ "draft": draft }))
    }

    /// Put a new post to the owner straight away, in its own task so the
    /// caller gets its draft back at once.
    ///
    /// Their answer is what publishes. No answer leaves it pending, which is
    /// exactly where the app's Waiting list picks it up.
    fn ask_about(&self, draft: Draft) {
        let state = self.clone();
        self.runtime.spawn(async move {
            let outcome = match crate::prompt::ask(PostPrompt::from_draft(&draft)).await {
                PostChoice::PostNow => state.confirm_draft(&draft.integration_id, &draft.id, false),
                PostChoice::Queue => state.confirm_draft(&draft.integration_id, &draft.id, true),
                PostChoice::Discard => state.discard_draft(&draft.integration_id, &draft.id),
                PostChoice::Later => return,
            };
            if let Err(error) = outcome {
                state.log_answer_failure(&draft, &error);
            }
        });
    }

    /// An answer that could not be carried out is the one thing the owner
    /// cannot see from the window, so it goes in the activity log.
    fn log_answer_failure(&self, draft: &Draft, error: &BridgeError) {
        let (connection_id, connection_name) = self.log_connection(&draft.integration_id);
        let mut entry = LogDraft::new(
            connection_id,
            connection_name,
            format!("{}.{} {}", draft.platform, draft.kind, draft.target_url),
        )
        .with_verdict(Verdict::Error)
        .with_source("post_answer");
        entry.reason = Some(error.message.clone());
        let _ = self.store.create_log_entry(entry);
    }

    fn create_job(
        &self,
        integration_id: &str,
        request: &CreateJobRequest,
    ) -> Result<Job, BridgeError> {
        let input = JobInput {
            platform: request.platform.as_str(),
            action: request.action.as_str(),
            target_url: &request.target_url,
            payload: &request.payload,
            ttl_ms: request.ttl_ms,
        };
        let job = self
            .store
            .browser_for(integration_id)
            .create_job(&input, self.now())
            .map_err(BridgeError::from)?;
        self.pump(integration_id);
        self.get_job(integration_id, &job.id)
            .map(|value| value.unwrap_or(job))
    }

    fn get_job(&self, integration_id: &str, job_id: &str) -> Result<Option<Job>, BridgeError> {
        self.store
            .browser_for(integration_id)
            .get_job(job_id, self.now())
            .map_err(BridgeError::from)
    }

    fn pump(&self, integration_id: &str) {
        loop {
            let mut queues = match self.queues.lock() {
                Ok(queues) => queues,
                Err(_) => return,
            };
            let queue = queues.entry(integration_id.to_owned()).or_default();
            if queue.active.is_some() || queue.connection.is_none() {
                return;
            }
            let now = self.now();
            let Some(job) = (match self.store.browser_for(integration_id).claim_next(now) {
                Ok(job) => job,
                Err(_) => return,
            }) else {
                let next_wakeup = match self
                    .store
                    .browser_for(integration_id)
                    .next_scheduled_job_at(now)
                {
                    Ok(value) => value,
                    Err(_) => return,
                };
                if let Some(wake_at) = next_wakeup {
                    self.arm_wakeup(queue, integration_id, wake_at);
                } else {
                    self.clear_wakeup(queue);
                }
                return;
            };
            self.clear_wakeup(queue);
            let Some(platform) = Platform::from_str(&job.platform) else {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job.id,
                    &job.command_id,
                    "failed",
                    &ProtocolErrorView {
                        code: "invalid_job".to_owned(),
                        message: "The stored job uses an unsupported platform.".to_owned(),
                    },
                    self.now(),
                );
                continue;
            };
            let Some(action) = Action::from_str(&job.action) else {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job.id,
                    &job.command_id,
                    "failed",
                    &ProtocolErrorView {
                        code: "invalid_job".to_owned(),
                        message: "The stored job uses an unsupported action.".to_owned(),
                    },
                    self.now(),
                );
                continue;
            };
            let connection = queue.connection.clone().expect("connection checked");
            if !connection
                .capabilities
                .iter()
                .any(|value| value.platform == platform && value.capabilities.contains(&action))
            {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job.id,
                    &job.command_id,
                    "failed",
                    &ProtocolErrorView {
                        code: "unsupported_capability".to_owned(),
                        message: "The paired browser extension does not support this action."
                            .to_owned(),
                    },
                    self.now(),
                );
                continue;
            }
            let command = make_command(CommandInput {
                job_id: &job.id,
                command_id: &job.command_id,
                platform,
                action,
                target_url: &job.target_url,
                issued_at: now,
                expires_at: job.expires_at,
                payload: &job.payload,
            });
            if parse_command_envelope(&command).is_err() {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job.id,
                    &job.command_id,
                    "failed",
                    &ProtocolErrorView {
                        code: "invalid_job".to_owned(),
                        message: "The stored job could not be validated for the browser bridge."
                            .to_owned(),
                    },
                    self.now(),
                );
                continue;
            }
            let encoded = command.to_string();
            if connection.sender.send(encoded).is_err() {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job.id,
                    &job.command_id,
                    "unknown",
                    &ProtocolErrorView {
                        code: "disconnected".to_owned(),
                        message: "The browser extension disconnected before this job completed."
                            .to_owned(),
                    },
                    self.now(),
                );
                queue.connection = None;
                continue;
            }
            let service = self.clone();
            let integration_id = integration_id.to_owned();
            let job_id = job.id.clone();
            let command_id = job.command_id.clone();
            let delay = Duration::from_millis((job.expires_at - self.now()).max(1) as u64);
            let timer = self.runtime.spawn(async move {
                sleep(delay).await;
                service.expire_active(&integration_id, &job_id, &command_id);
            });
            queue.active = Some(ActiveJob {
                job: job.clone(),
                connection_id: connection.id.clone(),
                timer,
            });
            return;
        }
    }

    fn arm_wakeup(&self, queue: &mut QueueState, integration_id: &str, wake_at: i64) {
        if queue.wake_at == Some(wake_at) && queue.wake_timer.is_some() {
            return;
        }
        self.clear_wakeup(queue);
        let service = self.clone();
        let integration_id = integration_id.to_owned();
        let delay = Duration::from_millis((wake_at - self.now()).max(1) as u64);
        queue.wake_at = Some(wake_at);
        queue.wake_timer = Some(self.runtime.spawn(async move {
            sleep(delay).await;
            service.wake_pump(&integration_id, wake_at);
        }));
    }

    fn clear_wakeup(&self, queue: &mut QueueState) {
        queue.wake_at = None;
        if let Some(timer) = queue.wake_timer.take() {
            timer.abort();
        }
    }

    fn wake_pump(&self, integration_id: &str, wake_at: i64) {
        let should_pump = match self.queues.lock() {
            Ok(mut queues) => {
                let Some(queue) = queues.get_mut(integration_id) else {
                    return;
                };
                if queue.wake_at != Some(wake_at) {
                    false
                } else {
                    queue.wake_at = None;
                    queue.wake_timer = None;
                    true
                }
            }
            Err(_) => false,
        };
        if should_pump {
            self.pump(integration_id);
        }
    }

    fn expire_active(&self, integration_id: &str, job_id: &str, command_id: &str) {
        let mut queues = match self.queues.lock() {
            Ok(queues) => queues,
            Err(_) => return,
        };
        let Some(queue) = queues.get_mut(integration_id) else {
            return;
        };
        let matches = queue
            .active
            .as_ref()
            .is_some_and(|active| active.job.id == job_id && active.job.command_id == command_id);
        if !matches {
            return;
        }
        queue.active = None;
        let _ = self.store.browser_for(integration_id).mark_in_flight(
            job_id,
            command_id,
            "unknown",
            &ProtocolErrorView {
                code: "deadline_exceeded".to_owned(),
                message: "The browser extension did not complete this job before its deadline."
                    .to_owned(),
            },
            self.now(),
        );
        drop(queues);
        self.log_job(integration_id, job_id);
        self.pump(integration_id);
    }

    fn connect_extension(
        &self,
        integration_id: &str,
        connection_id: String,
        capabilities: Vec<ExtensionCapability>,
        sender: UnboundedSender<String>,
    ) {
        if let Ok(mut queues) = self.queues.lock() {
            let queue = queues.entry(integration_id.to_owned()).or_default();
            if queue
                .connection
                .as_ref()
                .is_some_and(|value| value.id != connection_id)
            {
                let _ = self.disconnect_locked(integration_id, queue, None);
            }
            queue.connection = Some(ExtensionConnection {
                id: connection_id,
                capabilities,
                sender,
            });
            queue.last_rejected_at = None;
        }
        self.pump(integration_id);
    }

    fn record_pairing_rejection(&self) {
        let integration_id = match self.store.list_integrations() {
            Ok(integrations) => {
                let ids: Vec<String> = integrations
                    .into_iter()
                    .filter(|integration| integration.r#type == INTEGRATION_TYPE)
                    .map(|integration| integration.id)
                    .collect();
                match ids.as_slice() {
                    [] => Some(pluk_store::browser::LEGACY_INTEGRATION_ID.to_owned()),
                    [integration_id] => Some(integration_id.clone()),
                    _ => None,
                }
            }
            Err(_) => None,
        };
        let Some(integration_id) = integration_id else {
            return;
        };
        if let Ok(mut queues) = self.queues.lock() {
            queues.entry(integration_id).or_default().last_rejected_at = Some(self.now());
        }
    }

    fn disconnect_extension(&self, integration_id: &str, connection_id: &str) {
        let abandoned = self
            .queues
            .lock()
            .ok()
            .and_then(|mut queues| {
                queues
                    .get_mut(integration_id)
                    .and_then(|queue| {
                        self.disconnect_locked(integration_id, queue, Some(connection_id))
                            .ok()
                    })
            })
            .flatten();
        if let Some(job_id) = abandoned {
            self.log_job(integration_id, &job_id);
        }
    }

    pub fn disconnect_integration(&self, integration_id: &str) -> Result<(), BridgeError> {
        let abandoned = self
            .queues
            .lock()
            .map_err(|_| BridgeError::new("service_unavailable", "Browser control is unavailable."))
            .and_then(|mut queues| {
                queues
                    .get_mut(integration_id)
                    .map(|queue| self.disconnect_locked(integration_id, queue, None))
                    .transpose()
                    .map(|value| value.flatten())
                    .map_err(BridgeError::from)
            })?;
        if let Some(job_id) = abandoned {
            self.log_job(integration_id, &job_id);
        }
        Ok(())
    }

    /// Drop the paired connection and fail whatever it was running. Returns
    /// the abandoned job's id so the caller can record it once the queue lock
    /// is free.
    fn disconnect_locked(
        &self,
        integration_id: &str,
        queue: &mut QueueState,
        expected_id: Option<&str>,
    ) -> Result<Option<String>, BrowserError> {
        if expected_id
            .is_some_and(|id| queue.connection.as_ref().is_none_or(|value| value.id != id))
        {
            return Ok(None);
        }
        queue.connection = None;
        self.clear_wakeup(queue);
        let Some(active) = queue.active.take() else {
            return Ok(None);
        };
        active.timer.abort();
        self.store.browser_for(integration_id).mark_in_flight(
            &active.job.id,
            &active.job.command_id,
            "unknown",
            &ProtocolErrorView {
                code: "disconnected".to_owned(),
                message: "The browser extension disconnected before this job completed.".to_owned(),
            },
            self.now(),
        )?;
        Ok(Some(active.job.id))
    }

    fn handle_result(&self, integration_id: &str, connection_id: &str, result: ResultMessage) {
        let mut queues = match self.queues.lock() {
            Ok(queues) => queues,
            Err(_) => return,
        };
        let Some(queue) = queues.get_mut(integration_id) else {
            return;
        };
        let Some(active) = queue.active.take() else {
            return;
        };
        if active.connection_id != connection_id
            || result.job_id != active.job.id
            || result.command_id != active.job.command_id
            || result.expires_at != active.job.expires_at
            || result.expires_at <= self.now()
            || result.issued_at < active.job.created_at
            || result.issued_at > self.now() + MAX_CLOCK_SKEW_MS
        {
            queue.active = Some(active);
            return;
        }
        active.timer.abort();
        let (status, result_data, error) = self.validate_result(&active.job, &result);
        let job_id = active.job.id.clone();
        let command_id = active.job.command_id.clone();
        let completion = self.store.browser_for(integration_id).complete_job(JobCompletion {
            id: &job_id,
            command_id: &command_id,
            outcome: status,
            result: result_data.as_ref(),
            error: error.as_ref(),
            now: self.now(),
        });
        match completion {
            Ok(Completion { accepted: true }) => {}
            Ok(Completion { accepted: false }) | Err(_) => {
                let _ = self.store.browser_for(integration_id).mark_in_flight(
                    &job_id,
                    &command_id,
                    "unknown",
                    &ProtocolErrorView {
                        code: "result_rejected".to_owned(),
                        message: "The browser result could not be saved safely. Check the page before trying again.".to_owned(),
                    },
                    self.now(),
                );
            }
        }
        drop(queues);
        self.log_job(integration_id, &job_id);
        self.pump(integration_id);
    }

    fn validate_result(
        &self,
        job: &Job,
        result: &ResultMessage,
    ) -> (&'static str, Option<Value>, Option<ProtocolErrorView>) {
        if !result.succeeded {
            let error = result.error.clone().unwrap_or(ProtocolError {
                code: "invalid_result".to_owned(),
                message: "The browser extension returned an invalid result.".to_owned(),
            });
            let status = if matches!(
                error.code.as_str(),
                "submission_unknown" | "uncertain_side_effect" | "uncertain_execution"
            ) {
                "unknown"
            } else {
                "failed"
            };
            return (
                status,
                None,
                Some(ProtocolErrorView {
                    code: error.code,
                    message: error.message,
                }),
            );
        }
        if job.action == Action::SubmitPost.as_str() {
            let valid = result
                .data
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|data| {
                    let Some(posted_id) = data.get("postedId").and_then(Value::as_str) else {
                        return false;
                    };
                    let Some(posted_url) = data.get("postedUrl").and_then(Value::as_str) else {
                        return false;
                    };
                    let Some(path) = Url::parse(posted_url)
                        .ok()
                        .map(|value| value.path().trim_end_matches('/').to_owned())
                    else {
                        return false;
                    };
                    data.get("kind").and_then(Value::as_str) == Some("submission")
                        && data.get("platform").and_then(Value::as_str) == Some("x")
                        && !posted_id.is_empty()
                        && posted_id.len() <= MAX_ID_LENGTH
                        && posted_id.chars().all(|value| value.is_ascii_digit())
                        && posted_url.len() <= MAX_URL_LENGTH
                        && canonicalize_target_url(posted_url, Platform::X).is_ok()
                        && path
                            .rsplit_once("/status/")
                            .is_some_and(|(_, value)| value == posted_id)
                });
            if !valid {
                return ("unknown", None, Some(invalid_result()));
            }
        }
        ("succeeded", result.data.clone(), None)
    }

    fn queue_snapshot(&self, integration_id: &str) -> Result<Value, BridgeError> {
        let mut browser = self.store.browser_for(integration_id);
        let queued = browser
            .count_queued(self.now())
            .map_err(BridgeError::from)?;
        let schedule = browser.schedule_summary().map_err(BridgeError::from)?;
        drop(browser);
        let queues = self
            .queues
            .lock()
            .map_err(|_| BridgeError::new("service_unavailable", "Browser control is unavailable."))?;
        let queue = queues.get(integration_id);
        Ok(json!({
            "inFlightJobId": queue.and_then(|value| value.active.as_ref().map(|active| active.job.id.clone())),
            "inFlightCommandId": queue.and_then(|value| value.active.as_ref().map(|active| active.job.command_id.clone())),
            "queuedJobs": queued,
            "pendingReservations": schedule.pending_reservations,
            "uncertainReservations": schedule.uncertain_reservations,
        }))
    }

    /// Pin the request to this loopback port. A page on any other origin, and
    /// any request that reached the server under a different Host, is refused
    /// before it can read or queue anything.
    fn check_boundary(&self, headers: &HeaderMap) -> Result<(), Box<Response>> {
        let port = self.port;
        let expected_host = if port == 80 {
            "127.0.0.1".to_owned()
        } else {
            format!("127.0.0.1:{port}")
        };
        if headers.get("host").and_then(|value| value.to_str().ok()) != Some(expected_host.as_str())
        {
            return Err(Box::new(api_error(
                StatusCode::FORBIDDEN,
                "forbidden_host",
                "Only the loopback host is accepted.",
            )));
        }
        if let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok())
            && !self.allowed_origin(origin, port)
        {
            return Err(Box::new(api_error(
                StatusCode::FORBIDDEN,
                "forbidden_origin",
                "This origin is not allowed.",
            )));
        }
        Ok(())
    }

    fn allowed_origin(&self, origin: &str, port: u16) -> bool {
        if origin.starts_with("chrome-extension://") {
            return self.extension_origin.as_deref().map_or_else(
                || is_allowed_extension_origin(origin),
                |configured| configured == origin,
            );
        }
        let Ok(url) = Url::parse(origin) else {
            return false;
        };
        matches!(url.scheme(), "http")
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"))
            && url.port_or_known_default() == Some(port)
    }

    fn authenticated(&self, headers: &HeaderMap) -> Option<String> {
        let value = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())?;
        let token = value.strip_prefix("Bearer ")?;
        self.integration_id_for_token(token)
    }

    fn integration_id_for_token(&self, token: &str) -> Option<String> {
        if !valid_token(token) {
            return None;
        }
        if let Ok(Some(integration_id)) = self.store.browser_integration_id_by_token(token) {
            return Some(integration_id);
        }
        if !token_equals(token, self.legacy_token.as_str()) {
            return None;
        }
        let integrations: Vec<String> = self
            .store
            .list_integrations()
            .ok()?
            .into_iter()
            .filter(|integration| integration.r#type == INTEGRATION_TYPE)
            .map(|integration| integration.id)
            .collect();
        match integrations.as_slice() {
            [] => Some(pluk_store::browser::LEGACY_INTEGRATION_ID.to_owned()),
            [integration_id] => Some(integration_id.clone()),
            _ => None,
        }
    }

    fn extension_token(&self, headers: &HeaderMap, uri: &Uri) -> Option<String> {
        let mut query_token = None;
        let mut has_query_token = false;
        for pair in uri.query().unwrap_or_default().split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key != "token" {
                continue;
            }
            if has_query_token {
                return None;
            }
            has_query_token = true;
            query_token = Some(
                percent_encoding::percent_decode_str(value)
                    .decode_utf8()
                    .ok()?
                    .into_owned(),
            );
        }
        if has_query_token {
            return query_token;
        }
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_owned)
    }
}

/// The `/wande` surface, ready to nest into Pluk's loopback router.
///
/// Everything but `/wande/healthz` needs the Pluk ID as a Bearer token, and
/// every request is pinned to the loopback host and an allowed origin.
///
/// Nothing here publishes. A requested post is confirmed by the owner in
/// Pluk's own window, so no route reachable with the Pluk ID can send one.
pub fn router(state: BrowserState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/status", get(http_status))
        .route("/tools", get(http_list_tools))
        .route("/tools/{tool_id}", axum::routing::post(http_invoke_tool))
        .route("/jobs", get(http_list_jobs).post(http_create_job))
        .route("/jobs/{job_id}", get(http_get_job))
        .route(
            "/jobs/{job_id}/artifacts",
            axum::routing::post(http_create_artifact),
        )
        .route(
            "/jobs/{job_id}/artifacts/{artifact_id}",
            get(http_get_job_artifact),
        )
        .route("/artifacts/{artifact_id}", get(http_get_artifact))
        .route("/drafts/{draft_id}", get(http_get_draft))
        .route(
            "/drafts/{draft_id}/cancel",
            axum::routing::post(http_cancel_draft),
        )
        .route("/schedule", get(http_schedule).post(http_update_schedule))
        .route(
            "/schedule/{draft_id}/resolve",
            axum::routing::post(http_resolve_schedule),
        )
        .route("/extension/ws", get(http_extension_ws))
        .layer(DefaultBodyLimit::max(MAX_SCREENSHOT_BYTES))
        .with_state(state)
}

fn schedule_snapshot(
    state: &BrowserState,
    integration_id: &str,
) -> Result<Value, BridgeError> {
    let browser = state.store.browser_for(integration_id);
    let settings = browser.get_schedule_settings().map_err(BridgeError::from)?;
    let reservations = browser
        .list_schedule_reservations(MAX_LIST_LIMIT, state.now())
        .map_err(BridgeError::from)?;
    let summary = browser.schedule_summary().map_err(BridgeError::from)?;
    Ok(json!({
        "settings": settings,
        "reservations": reservations,
        "summary": summary,
    }))
}

fn service_status_value(
    state: &BrowserState,
    integration_id: &str,
) -> Result<Value, BridgeError> {
    let snapshot = state.queue_snapshot(integration_id)?;
    let queues = state
        .queues
        .lock()
        .map_err(|_| BridgeError::new("service_unavailable", "Browser control is unavailable."))?;
    let queue = queues.get(integration_id);
    Ok(json!({
        "status": "ok",
        "extension": {
            "connected": queue.is_some_and(|value| value.connection.is_some()),
            "ready": queue.is_some_and(|value| value.connection.is_some()),
            "pairingRejectedAt": queue.and_then(|value| value.last_rejected_at),
        },
        "queue": snapshot,
    }))
}

async fn healthz(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    api_json(StatusCode::OK, json!({ "status": "ok" }))
}

async fn http_status(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    match service_status_value(&state, &integration_id) {
        Ok(value) => api_json(StatusCode::OK, value),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_list_jobs(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    let limit = query_value(uri.query(), "limit")
        .map(|value| value.parse::<i64>())
        .transpose();
    let Ok(limit) = limit else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Limit must be a positive integer.",
        );
    };
    let limit = limit.unwrap_or(MAX_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    match state
        .store
        .browser_for(&integration_id)
        .list_jobs(limit, state.now())
        .map_err(BridgeError::from)
    {
        Ok(jobs) => api_json(StatusCode::OK, json!({ "jobs": jobs })),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_schedule(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    match schedule_snapshot(&state, &integration_id) {
        Ok(value) => api_json(StatusCode::OK, value),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_update_schedule(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    let value = match parse_json_body(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let settings: ScheduleSettings = match serde_json::from_value(value) {
        Ok(settings) => settings,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_schedule",
                "Schedule settings are invalid.",
            );
        }
    };
    if let Err(error) = settings.to_config() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schedule",
            &error.to_string(),
        );
    }
    match state
        .store
        .browser_for(&integration_id)
        .update_schedule_settings(&settings)
    {
        Ok(_) => match schedule_snapshot(&state, &integration_id) {
            Ok(value) => api_json(StatusCode::OK, value),
            Err(error) => bridge_error_response(error),
        },
        Err(error) => bridge_error_response(error.into()),
    }
}

async fn http_resolve_schedule(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    if !is_uuid(&draft_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Schedule not found.");
    }
    let value = match parse_json_body(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(published) = parse_published_resolution(&value) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Resolution needs a published boolean.",
        );
    };
    let reservation =
        match state
            .store
            .browser_for(&integration_id)
            .resolve_schedule(&draft_id, published, state.now())
        {
            Ok(reservation) => reservation,
            Err(error) => return bridge_error_response(error.into()),
        };
    if reservation.is_none() {
        return api_error(
            StatusCode::CONFLICT,
            "already_resolved",
            "This schedule has already been resolved.",
        );
    }
    match schedule_snapshot(&state, &integration_id) {
        Ok(value) => api_json(StatusCode::OK, value),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_create_job(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    let value = match parse_json_body(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = match parse_create_job_request(&value) {
        Ok(request) => request,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error.code, &error.message),
    };
    match state.start(&integration_id, &request) {
        Ok(started) => api_json(StatusCode::ACCEPTED, started),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_list_tools(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(_integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    api_json(StatusCode::OK, json!({ "tools": catalog_value() }))
}

async fn http_invoke_tool(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(tool_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    let Some((platform, action)) = find_tool(&tool_id) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "unknown_tool",
            "This tool is not in the catalog.",
        );
    };
    let mut value = match parse_json_body(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(object) = value.as_object_mut() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Request body must contain valid JSON.",
        );
    };
    if object.contains_key("platform") || object.contains_key("action") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Tool routes take their platform and action from the URL, not the body.",
        );
    }
    object.insert("platform".to_owned(), json!(platform.as_str()));
    object.insert("action".to_owned(), json!(action.as_str()));
    object.entry("payload").or_insert_with(|| json!({}));
    let request = match parse_create_job_request(&value) {
        Ok(request) => request,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error.code, &error.message),
    };
    match state.start(&integration_id, &request) {
        Ok(started) => api_json(StatusCode::ACCEPTED, started),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_get_job(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    if !is_uuid(&job_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Job not found.");
    }
    match state.get_job(&integration_id, &job_id) {
        Ok(Some(job)) => api_json(StatusCode::OK, json!({ "job": job })),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "Job not found."),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_get_draft(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    if !is_uuid(&draft_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Draft not found.");
    }
    match state
        .store
        .browser_for(&integration_id)
        .get_draft(&draft_id)
        .map_err(BridgeError::from)
    {
        Ok(Some(draft)) => api_json(StatusCode::OK, json!({ "draft": draft })),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "Draft not found."),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_cancel_draft(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(draft_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    if !is_uuid(&draft_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Draft not found.");
    }
    if !is_empty_json_object(&body) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Cancellation accepts an empty JSON object.",
        );
    }
    match state
        .store
        .browser_for(&integration_id)
        .cancel_draft(&draft_id, state.now())
        .map_err(BridgeError::from)
    {
        Ok(Some(draft)) => api_json(StatusCode::OK, json!({ "draft": draft })),
        Ok(None) => api_error(
            StatusCode::CONFLICT,
            "already_consumed",
            "This draft was already confirmed or is no longer available.",
        ),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_create_artifact(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(&headers) else {
        return unauthorized();
    };
    if !is_uuid(&job_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Job not found.");
    }
    let job = match state.get_job(&integration_id, &job_id) {
        Ok(Some(job)) => job,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "Job not found."),
        Err(error) => return bridge_error_response(error),
    };
    if job.status != "running" {
        return api_error(
            StatusCode::CONFLICT,
            "job_not_running",
            "Snapshots can only be attached while the browser job is running.",
        );
    }
    let kind = match headers
        .get("x-wande-artifact-kind")
        .and_then(|value| value.to_str().ok())
    {
        Some("screenshot") | Some("extract") => headers
            .get("x-wande-artifact-kind")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_schema",
                "Artifact kind must be screenshot or extract.",
            );
        }
    };
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    let valid_content_type = match kind {
        "screenshot" => content_type == "image/png" && body.len() <= MAX_SCREENSHOT_BYTES,
        "extract" => match content_type {
            "application/json" | "text/plain" => body.len() <= MAX_EXTRACT_BYTES,
            "text/html" => body.len() <= MAX_HTML_BYTES,
            _ => false,
        },
        _ => false,
    };
    if !valid_content_type {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Artifact content type or size is not allowed for this kind.",
        );
    }
    if body.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Artifact body cannot be empty.",
        );
    }
    match state
        .store
        .browser_for(&integration_id)
        .create_artifact(&job_id, kind, content_type, &body, state.now())
        .map_err(BridgeError::from)
    {
        Ok(artifact) => api_json(StatusCode::CREATED, json!({ "artifact": artifact })),
        Err(error) => bridge_error_response(error),
    }
}

async fn http_get_job_artifact(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath((job_id, artifact_id)): AxumPath<(String, String)>,
) -> Response {
    get_artifact_response(&state, &headers, &artifact_id, Some(&job_id)).await
}

async fn http_get_artifact(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    AxumPath(artifact_id): AxumPath<String>,
) -> Response {
    get_artifact_response(&state, &headers, &artifact_id, None).await
}

async fn get_artifact_response(
    state: &BrowserState,
    headers: &HeaderMap,
    artifact_id: &str,
    job_id: Option<&str>,
) -> Response {
    if let Err(response) = state.check_boundary(headers) {
        return *response;
    }
    let Some(integration_id) = state.authenticated(headers) else {
        return unauthorized();
    };
    if !is_uuid(artifact_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Artifact not found.");
    }
    let body = match state
        .store
        .browser_for(&integration_id)
        .get_artifact_body(artifact_id)
        .map_err(BridgeError::from)
    {
        Ok(body) => body,
        Err(error) => return bridge_error_response(error),
    };
    let Some(ArtifactBody { metadata, data }) = body else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Artifact not found.");
    };
    if job_id.is_some_and(|job_id| job_id != metadata.job_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "Artifact not found.");
    }
    let mut response = Response::new(axum::body::Body::from(data));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_str(&metadata.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        "x-wande-artifact-id",
        HeaderValue::from_str(&metadata.id).unwrap_or_else(|_| HeaderValue::from_static("unknown")),
    );
    response
}

async fn http_extension_ws(
    State(state): State<BrowserState>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Err(response) = state.check_boundary(&headers) {
        return *response;
    }
    let Some(token) = state.extension_token(&headers, &uri) else {
        state.record_pairing_rejection();
        return unauthorized_with_message("Extension pairing requires a valid token.");
    };
    let Some(integration_id) = state.integration_id_for_token(&token) else {
        state.record_pairing_rejection();
        return unauthorized_with_message("Extension pairing requires a valid token.");
    };
    let connection_id = Uuid::new_v4().to_string();
    upgrade
        .on_upgrade(move |socket| handle_socket(state, socket, connection_id, integration_id))
        .into_response()
}

async fn handle_socket(
    state: BrowserState,
    mut socket: WebSocket,
    connection_id: String,
    integration_id: String,
) {
    if socket
        .send(Message::Text(Utf8Bytes::from(
            make_ready_envelope(&connection_id, state.now()).to_string(),
        )))
        .await
        .is_err()
    {
        return;
    }
    let first_message = tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await;
    let Some(Ok(Message::Text(text))) = first_message.ok().flatten() else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    if text.len() > MAX_MESSAGE_BYTES {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    let Ok(ExtensionMessage::Hello(hello)) = parse_extension_message(&value) else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    let now = state.now();
    if hello.expires_at <= now
        || hello.issued_at < now - MAX_CLOCK_SKEW_MS
        || hello.issued_at > now + MAX_CLOCK_SKEW_MS
    {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    let (sender, mut receiver): (UnboundedSender<String>, UnboundedReceiver<String>) =
        mpsc::unbounded_channel();
    state.connect_extension(
        &integration_id,
        connection_id.clone(),
        hello.capabilities,
        sender,
    );
    let mut ticker = interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS as u64));
    let mut last_seen = now;
    loop {
        tokio::select! {
            outbound = receiver.recv() => {
                let Some(outbound) = outbound else { break; };
                if socket.send(Message::Text(Utf8Bytes::from(outbound))).await.is_err() { break; }
            }
            inbound = socket.recv() => {
                let Some(Ok(message)) = inbound else { break; };
                last_seen = state.now();
                let Message::Text(text) = message else { break; };
                if text.len() > MAX_MESSAGE_BYTES { break; }
                let Ok(value) = serde_json::from_str::<Value>(&text) else { break; };
                let Ok(parsed) = parse_extension_message(&value) else { break; };
                match parsed {
                    ExtensionMessage::Heartbeat(heartbeat) => {
                        let current = state.now();
                        if heartbeat.expires_at <= current || heartbeat.issued_at < current - MAX_CLOCK_SKEW_MS || heartbeat.issued_at > current + MAX_CLOCK_SKEW_MS { break; }
                        let _ = socket.send(Message::Text(Utf8Bytes::from(make_heartbeat_ack(&heartbeat.nonce, current).to_string()))).await;
                    }
                    ExtensionMessage::Result(result) => state.handle_result(&integration_id, &connection_id, result),
                    ExtensionMessage::Hello(_) => break,
                }
            }
            _ = ticker.tick() => {
                if state.now() - last_seen > HEARTBEAT_INTERVAL_MS * 2 { break; }
                if socket.send(Message::Text(Utf8Bytes::from(make_heartbeat(state.now()).to_string()))).await.is_err() { break; }
            }
        }
    }
    state.disconnect_extension(&integration_id, &connection_id);
    let _ = socket.send(Message::Close(None)).await;
}

/// The one refusal every draft action shares: it is no longer the caller's to
/// act on.
fn already_consumed() -> BridgeError {
    BridgeError::with_status(
        "already_consumed",
        "This draft was already confirmed or is no longer available.",
        409,
    )
}

fn confirm_draft_value(
    state: &BrowserState,
    integration_id: &str,
    draft_id: &str,
    schedule: bool,
) -> Result<Value, BridgeError> {
    let creation = state
        .store
        .browser_for(integration_id)
        .consume_draft(draft_id, state.now(), schedule)
        .map_err(BridgeError::from)?;
    let Some(creation) = creation else {
        return Err(already_consumed());
    };
    state.pump(integration_id);
    Ok(json!({ "draft": creation.draft, "job": creation.job }))
}

fn api_json(status: StatusCode, value: Value) -> Response {
    let mut object = match value {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.insert("version".to_owned(), json!(PROTOCOL_VERSION));
    let body = serde_json::to_vec(&Value::Object(object)).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = Response::new(axum::body::Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    api_json(
        status,
        json!({ "error": { "code": code, "message": message } }),
    )
}

fn parse_json_body(body: &Bytes) -> Result<Value, Box<Response>> {
    if body.len() > MAX_BODY_BYTES {
        return Err(Box::new(api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request body exceeds the configured limit.",
        )));
    }
    serde_json::from_slice(body).map_err(|_| {
        Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_schema",
            "Request body must contain valid JSON.",
        ))
    })
}

fn parse_published_resolution(value: &Value) -> Option<bool> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (key, value) = object.iter().next()?;
    if !matches!(key.as_str(), "published" | "scheduled") {
        return None;
    }
    value.as_bool()
}

fn unauthorized() -> Response {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Authentication is required.",
    );
    response
        .headers_mut()
        .insert("www-authenticate", HeaderValue::from_static("Bearer"));
    response
}

fn unauthorized_with_message(message: &str) -> Response {
    api_error(StatusCode::UNAUTHORIZED, "unauthorized", message)
}

fn bridge_error_response(error: BridgeError) -> Response {
    let status = error
        .status
        .map(StatusCode::from_u16)
        .transpose()
        .ok()
        .flatten()
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    api_error(status, &error.code, &error.message)
}

fn is_empty_json_object(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .is_some_and(|value| value.as_object().is_some_and(Map::is_empty))
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.to_owned())
    })
}

fn invalid_result() -> ProtocolErrorView {
    ProtocolErrorView {
        code: "invalid_result".to_owned(),
        message: "The browser extension returned a result that did not match the command."
            .to_owned(),
    }
}

impl BridgeError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            status: None,
        }
    }

    fn with_status(code: &str, message: &str, status: u16) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            status: Some(status),
        }
    }
}

impl From<BrowserError> for BridgeError {
    fn from(error: BrowserError) -> Self {
        match error {
            BrowserError::Full => Self::with_status(
                "history_full",
                "Job history is full; no completed job is available for automatic cleanup.",
                503,
            ),
            BrowserError::ScheduleBlocked => Self::with_status(
                "schedule_blocked",
                "Check X for the post Pluk could not confirm, then queue another one.",
                409,
            ),
            BrowserError::Sqlite(_) | BrowserError::InvalidData(_) => Self::new(
                "service_error",
                "Pluk could not safely record what the browser did.",
            ),
        }
    }
}

fn is_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn token_equals(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn valid_token(value: &str) -> bool {
    value.len() >= 16
        && value.len() <= MAX_TOKEN_LENGTH
        && value.trim() == value
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(|character| character.is_control())
}

fn configured_extension_origin() -> Result<Option<String>, String> {
    let value = std::env::var("PLUK_BROWSER_EXTENSION_ORIGIN").unwrap_or_default();
    if value.is_empty() {
        return Ok(None);
    }
    if !is_allowed_extension_origin(&value) {
        return Err(
            "PLUK_BROWSER_EXTENSION_ORIGIN must be an exact chrome-extension origin".to_owned(),
        );
    }
    Ok(Some(value))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// The extension's pairing key. `PLUK_BROWSER_TOKEN` wins when set; otherwise
/// the key is minted once and kept in the store beside everything else Pluk
/// remembers.
pub fn pairing_key(store: &Store) -> Result<String, String> {
    if let Ok(configured) = std::env::var("PLUK_BROWSER_TOKEN") {
        if !valid_token(&configured) {
            return Err("PLUK_BROWSER_TOKEN must be 16-256 non-whitespace characters".to_owned());
        }
        return Ok(configured);
    }
    store
        .browser_pairing_token()
        .map_err(|error| format!("Could not read the pairing key: {error}"))
}

pub fn pairing_key_for(store: &Store, integration_id: &str) -> Result<String, String> {
    if integration_id == pluk_store::browser::LEGACY_INTEGRATION_ID {
        return pairing_key(store);
    }
    if let Ok(configured) = std::env::var("PLUK_BROWSER_TOKEN") {
        if !valid_token(&configured) {
            return Err("PLUK_BROWSER_TOKEN must be 16-256 non-whitespace characters".to_owned());
        }
        let wande_count = store
            .list_integrations()
            .map_err(|error| format!("Could not read integrations: {error}"))?
            .into_iter()
            .filter(|integration| integration.r#type == INTEGRATION_TYPE)
            .count();
        if wande_count <= 1 {
            return Ok(configured);
        }
    }
    store
        .browser_pairing_token_for(integration_id)
        .map_err(|error| format!("Could not read the pairing key: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use pluk_store::BROWSER_PAIRING_TOKEN_KEY;
    use reqwest::header::ORIGIN;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    use tempfile::{TempDir, tempdir};
    use tokio_tungstenite::connect_async;

    const TOKEN: &str = "test-control-token-123456";
    const LEGACY: &str = pluk_store::browser::LEGACY_INTEGRATION_ID;

    struct Fixture {
        state: BrowserState,
        base: String,
        _directory: TempDir,
    }

    impl Fixture {
        /// Serve the `/wande` routes on an ephemeral loopback port, nested
        /// exactly the way the host nests them.
        async fn start() -> Fixture {
            Fixture::with_token(TOKEN).await
        }

        async fn with_token(token: &str) -> Fixture {
            let directory = tempdir().unwrap();
            let store = Store::open(&directory.path().join("pluk.db")).unwrap();
            store.set_setting(BROWSER_PAIRING_TOKEN_KEY, token).unwrap();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let state = BrowserState::new(Arc::new(store), port).unwrap();
            let app = Router::new().nest("/wande", router(state.clone()));
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Fixture {
                state,
                base: format!("http://127.0.0.1:{port}"),
                _directory: directory,
            }
        }

        async fn with_integrations() -> Fixture {
            let directory = tempdir().unwrap();
            let store = Store::open(&directory.path().join("pluk.db")).unwrap();
            store
                .create_integration(&pluk_store::IntegrationInput::new("First", INTEGRATION_TYPE))
                .unwrap();
            store
                .create_integration(&pluk_store::IntegrationInput::new("Second", INTEGRATION_TYPE))
                .unwrap();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let state = BrowserState::new(Arc::new(store), port).unwrap();
            let app = Router::new().nest("/wande", router(state.clone()));
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Fixture {
                state,
                base: format!("http://127.0.0.1:{port}"),
                _directory: directory,
            }
        }

        fn socket_url(&self, token: &str) -> String {
            format!(
                "{}/wande/extension/ws?token={token}",
                self.base.replace("http", "ws")
            )
        }
    }

    /// Browser activity reaches the integration's own log once the user has
    /// added it, and falls back to a standalone pair before that.
    #[tokio::test]
    async fn activity_is_logged_against_the_integration_once_it_exists() {
        let fixture = Fixture::start().await;
        let store = fixture.state.store.clone();

        let (id, name) = fixture.state.log_connection(LEGACY);
        assert_eq!(id, LOG_CONNECTION_ID);
        assert_eq!(name, LOG_CONNECTION_NAME);

        let integration = store
            .create_integration(&pluk_store::IntegrationInput::new(
                "My browser",
                INTEGRATION_TYPE,
            ))
            .unwrap();
        let (id, name) = fixture.state.log_connection(&integration.id);
        assert_eq!(id, integration.id);
        assert_eq!(name, "My browser");

        store
            .create_log_entry(LogDraft::new(id, name, "x.read_feed https://x.com/home"))
            .unwrap();
        let page = store
            .read_log_page(
                &pluk_store::LogScope::Connection(integration.id),
                pluk_store::LogRange::All,
                None,
            )
            .unwrap();
        let details: Vec<&str> = page
            .entries
            .iter()
            .map(|entry| entry.sql.as_str())
            .collect();
        assert_eq!(details, ["x.read_feed https://x.com/home"]);
    }

    type TestSocket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn send(socket: &mut TestSocket, value: Value) {
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                value.to_string(),
            ))
            .await
            .unwrap();
    }

    async fn next_command(socket: &mut TestSocket) -> Value {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            if value["type"] == "command" {
                return value;
            }
        }
    }

    async fn pair(fixture: &Fixture, capabilities: &[&str]) -> TestSocket {
        pair_for(fixture, LEGACY, TOKEN, capabilities).await
    }

    async fn pair_for(
        fixture: &Fixture,
        integration_id: &str,
        token: &str,
        capabilities: &[&str],
    ) -> TestSocket {
        let (mut socket, _) = connect_async(fixture.socket_url(token)).await.unwrap();
        let ready = socket.next().await.unwrap().unwrap();
        assert!(ready.to_text().unwrap().contains("\"type\":\"ready\""));
        let now = now_millis();
        send(
            &mut socket,
            json!({
                "version": 1,
                "type": "hello",
                "extensionVersion": "fixture",
                "capabilities": [{ "platform": "x", "capabilities": capabilities }],
                "issuedAt": now,
                "expiresAt": now + 60_000
            }),
        )
        .await;
        // Paired means registered: a test that moves the clock before the
        // hello lands would see it refused as expired.
        for _ in 0..200 {
            if fixture.state.extension_connected(integration_id) {
                return socket;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the extension never paired");
    }

    async fn wait_for_status(state: &BrowserState, job_id: &str, status: &str) -> Job {
        wait_for_status_for(state, LEGACY, job_id, status).await
    }

    async fn wait_for_status_for(
        state: &BrowserState,
        integration_id: &str,
        job_id: &str,
        status: &str,
    ) -> Job {
        for _ in 0..40 {
            if let Some(job) = state.get_job(integration_id, job_id).unwrap()
                && job.status == status
            {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("job {job_id} never reached {status}");
    }

    fn job_request(value: Value) -> CreateJobRequest {
        parse_create_job_request(&value).unwrap()
    }

    #[tokio::test]
    async fn server_fixture_completes_correlated_job() {
        let fixture = Fixture::start().await;
        let mut socket = pair(&fixture, &["inspect"]).await;
        let job = fixture
            .state
            .create_job(LEGACY, &job_request(json!({
                "platform":"x","action":"inspect","targetUrl":"https://x.com/status/42","payload":{}
            })))
            .unwrap();
        let command = next_command(&mut socket).await;
        send(
            &mut socket,
            json!({
                "version":1,"type":"result","jobId":job.id,"commandId":job.command_id,
                "issuedAt":now_millis(),"expiresAt":command["expiresAt"],"outcome":"succeeded",
                "data":{"kind":"page","title":"Fixture"}
            }),
        )
        .await;
        wait_for_status(&fixture.state, &job.id, "succeeded").await;
    }

    /// The activity log carries what the page answered, not just that it was
    /// asked — a row with the call and no result reads as "No response".
    #[tokio::test]
    async fn the_log_carries_what_the_page_answered() {
        let fixture = Fixture::start().await;
        let mut socket = pair(&fixture, &["inspect"]).await;
        let job = fixture
            .state
            .create_job(LEGACY, &job_request(json!({
                "platform":"x","action":"inspect","targetUrl":"https://x.com/status/42","payload":{}
            })))
            .unwrap();
        let command = next_command(&mut socket).await;
        send(
            &mut socket,
            json!({
                "version":1,"type":"result","jobId":job.id,"commandId":job.command_id,
                "issuedAt":now_millis(),"expiresAt":command["expiresAt"],"outcome":"succeeded",
                "data":{"kind":"page","title":"Fixture"}
            }),
        )
        .await;
        wait_for_status(&fixture.state, &job.id, "succeeded").await;

        let page = fixture
            .state
            .store
            .read_log_page(
                &pluk_store::LogScope::Connection(LOG_CONNECTION_ID.to_owned()),
                pluk_store::LogRange::All,
                None,
            )
            .unwrap();
        let response = page
            .entries
            .iter()
            .find(|entry| entry.sql.starts_with("x.inspect"))
            .and_then(|entry| entry.response_text.clone())
            .expect("the inspect row carries a response");
        assert!(response.contains("Fixture"), "response was {response}");
    }

    // Every advertised X action must reach a paired extension through the real
    // dispatch loop, not just pass job-creation validation.
    #[tokio::test]
    async fn server_fixture_dispatches_read_profile_and_read_post() {
        let fixture = Fixture::start().await;
        let mut socket = pair(&fixture, &["read_profile", "read_post"]).await;

        let profile_job = fixture
            .state
            .create_job(LEGACY, &job_request(json!({
                "platform": "x", "action": "read_profile", "payload": { "username": "yondifon" }
            })))
            .unwrap();
        let profile_command = next_command(&mut socket).await;
        assert_eq!(profile_command["action"], "read_profile");
        assert_eq!(profile_command["targetUrl"], "https://x.com/yondifon");
        send(&mut socket, json!({
            "version":1,"type":"result","jobId":profile_job.id,"commandId":profile_job.command_id,
            "issuedAt":now_millis(),"expiresAt":profile_command["expiresAt"],"outcome":"succeeded",
            "data":{"kind":"x_profile","handle":"yondifon"}
        })).await;
        wait_for_status(&fixture.state, &profile_job.id, "succeeded").await;

        let post_job = fixture
            .state
            .create_job(LEGACY, &job_request(json!({
                "platform": "x", "action": "read_post", "payload": { "postId": "42" }
            })))
            .unwrap();
        let post_command = next_command(&mut socket).await;
        assert_eq!(post_command["action"], "read_post");
        assert_eq!(post_command["targetUrl"], "https://x.com/i/status/42");
        assert_eq!(post_command["payload"]["postId"], "42");
        send(
            &mut socket,
            json!({
                "version":1,"type":"result","jobId":post_job.id,"commandId":post_job.command_id,
                "issuedAt":now_millis(),"expiresAt":post_command["expiresAt"],"outcome":"succeeded",
                "data":{"kind":"x_post","postId":"42"}
            }),
        )
        .await;
        wait_for_status(&fixture.state, &post_job.id, "succeeded").await;
    }

    #[tokio::test]
    async fn integrations_keep_jobs_and_connections_isolated() {
        let fixture = Fixture::with_integrations().await;
        let integrations = fixture
            .state
            .store
            .list_integrations()
            .unwrap()
            .into_iter()
            .filter(|integration| integration.r#type == INTEGRATION_TYPE)
            .collect::<Vec<_>>();
        let first_id = integrations[0].id.clone();
        let second_id = integrations[1].id.clone();
        let first_token = fixture.state.pairing_key_for(&first_id).unwrap();
        let second_token = fixture.state.pairing_key_for(&second_id).unwrap();
        let mut first_socket = pair_for(&fixture, &first_id, &first_token, &["inspect"]).await;
        let mut second_socket =
            pair_for(&fixture, &second_id, &second_token, &["inspect"]).await;

        assert!(fixture.state.extension_connected(&first_id));
        assert!(fixture.state.extension_connected(&second_id));

        let first_job = fixture
            .state
            .create_job(
                &first_id,
                &job_request(json!({
                    "platform": "x",
                    "action": "inspect",
                    "targetUrl": "https://x.com/status/1",
                    "payload": {}
                })),
            )
            .unwrap();
        let first_command = next_command(&mut first_socket).await;
        assert_eq!(first_command["jobId"], first_job.id);

        let second_job = fixture
            .state
            .create_job(
                &second_id,
                &job_request(json!({
                    "platform": "x",
                    "action": "inspect",
                    "targetUrl": "https://x.com/status/2",
                    "payload": {}
                })),
            )
            .unwrap();
        let second_command = next_command(&mut second_socket).await;
        assert_eq!(second_command["jobId"], second_job.id);

        send(
            &mut first_socket,
            json!({
                "version": 1,
                "type": "result",
                "jobId": first_job.id.clone(),
                "commandId": first_job.command_id.clone(),
                "issuedAt": now_millis(),
                "expiresAt": first_command["expiresAt"],
                "outcome": "succeeded",
                "data": {"kind": "page", "title": "First"}
            }),
        )
        .await;
        send(
            &mut second_socket,
            json!({
                "version": 1,
                "type": "result",
                "jobId": second_job.id.clone(),
                "commandId": second_job.command_id.clone(),
                "issuedAt": now_millis(),
                "expiresAt": second_command["expiresAt"],
                "outcome": "succeeded",
                "data": {"kind": "page", "title": "Second"}
            }),
        )
        .await;
        wait_for_status_for(&fixture.state, &first_id, &first_job.id, "succeeded").await;
        wait_for_status_for(&fixture.state, &second_id, &second_job.id, "succeeded").await;

        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/wande/jobs/{}", fixture.base, first_job.id))
            .bearer_auth(second_token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn accepts_plain_post_submission_results_without_a_schedule() {
        let fixture = Fixture::start().await;
        let job = Job {
            integration_id: LEGACY.to_owned(),
            id: "job-1".to_owned(),
            command_id: "command-1".to_owned(),
            platform: "x".to_owned(),
            action: "submit_post".to_owned(),
            target_url: "https://x.com/compose/post".to_owned(),
            payload: json!({
                "kind": "post_submission",
                "draftId": "draft-1",
                "text": "Exact post text"
            }),
            status: "running".to_owned(),
            created_at: 1_000,
            expires_at: 2_000,
            started_at: Some(1_000),
            finished_at: None,
            error: None,
            result: None,
            dispatch_count: 1,
            draft_id: Some("draft-1".to_owned()),
            artifacts: Vec::new(),
        };
        let result = ResultMessage {
            job_id: job.id.clone(),
            command_id: job.command_id.clone(),
            issued_at: 1_000,
            expires_at: 2_000,
            succeeded: true,
            data: Some(json!({
                "kind": "submission",
                "platform": "x",
                "postedId": "999",
                "postedUrl": "https://x.com/owner/status/999"
            })),
            error: None,
        };

        let (status, data, error) = fixture.state.validate_result(&job, &result);
        assert_eq!(status, "succeeded");
        assert_eq!(
            data.as_ref().and_then(|value| value.get("kind")),
            Some(&json!("submission"))
        );
        assert!(error.is_none());
    }

    #[test]
    fn accepts_current_and_legacy_schedule_resolution_fields() {
        assert_eq!(
            parse_published_resolution(&json!({"published": true})),
            Some(true)
        );
        assert_eq!(
            parse_published_resolution(&json!({"scheduled": false})),
            Some(false)
        );
        assert!(
            parse_published_resolution(&json!({
                "published": true,
                "scheduled": true
            }))
            .is_none()
        );
        assert!(parse_published_resolution(&json!({"published": "true"})).is_none());
    }

    /// Asking for a post writes it down in Pluk and sends nothing to the
    /// browser. Only a person confirming it produces the one command that
    /// fills the composer and submits.
    #[tokio::test]
    async fn a_requested_post_waits_in_pluk_until_someone_sends_it() {
        let fixture = Fixture::start().await;
        let state = &fixture.state;
        let mut socket = pair(&fixture, &["submit_post"]).await;

        let started = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": "Only if you say so" }
            })))
            .unwrap();
        assert!(
            started.get("job").is_none(),
            "a post request is not a browser job"
        );
        let draft_id = started["draft"]["id"].as_str().unwrap().to_owned();
        assert_eq!(started["draft"]["status"], "pending");
        assert_eq!(started["draft"]["text"], "Only if you say so");

        let waiting = || -> Vec<String> {
            state
                .pending_drafts(LEGACY)
                .unwrap()
                .into_iter()
                .map(|draft| draft.id)
                .collect()
        };
        assert!(waiting().contains(&draft_id));
        assert!(
            state
                .store
                .browser()
                .list_jobs(10, state.now())
                .unwrap()
                .is_empty(),
            "nothing was queued for the browser"
        );

        state.confirm_draft(LEGACY, &draft_id, false).unwrap();
        assert!(!waiting().contains(&draft_id));
        let command = next_command(&mut socket).await;
        assert_eq!(command["action"], "submit_post");
        assert_eq!(command["payload"]["draftId"], draft_id);
        assert_eq!(command["payload"]["text"], "Only if you say so");
        assert!(command["payload"].get("visibleAccountIdentity").is_none());
    }

    /// The same words asked for twice are one waiting post, and while one
    /// post is going out another can only take a queue slot.
    #[tokio::test]
    async fn a_repeated_request_reuses_the_waiting_post_and_only_one_goes_out_at_a_time() {
        let fixture = Fixture::start().await;
        let state = &fixture.state;
        let mut socket = pair(&fixture, &["submit_post"]).await;
        let request = || {
            job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": "Once" }
            }))
        };

        let first = state.start(LEGACY, &request()).unwrap();
        let again = state.start(LEGACY, &request()).unwrap();
        assert_eq!(first["draft"]["id"], again["draft"]["id"]);
        assert_eq!(state.pending_drafts(LEGACY).unwrap().len(), 1);
        let first_id = first["draft"]["id"].as_str().unwrap().to_owned();

        let second = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": "Twice" }
            })))
            .unwrap();
        let second_id = second["draft"]["id"].as_str().unwrap().to_owned();
        assert_eq!(state.pending_drafts(LEGACY).unwrap().len(), 2);

        state.confirm_draft(LEGACY, &first_id, false).unwrap();
        assert!(state.sending(LEGACY).unwrap());
        let refused = state.confirm_draft(LEGACY, &second_id, false).unwrap_err();
        assert_eq!(refused.code, "busy");
        assert_eq!(
            state
                .store
                .browser()
                .get_draft(&second_id)
                .unwrap()
                .unwrap()
                .status,
            "pending",
            "a refused send leaves the post waiting"
        );
        state.confirm_draft(LEGACY, &second_id, true).unwrap();

        let command = next_command(&mut socket).await;
        assert_eq!(command["payload"]["draftId"], first_id);
    }

    /// Text over X's limit is asked for as one post and goes out as a thread:
    /// one command, every part in it.
    #[tokio::test]
    async fn a_long_post_becomes_one_thread_command() {
        let fixture = Fixture::start().await;
        let state = &fixture.state;
        let mut socket = pair(&fixture, &["submit_post"]).await;
        let sentence = "This sentence is exactly long enough to matter here.";
        let text = std::iter::repeat_n(sentence, 8)
            .collect::<Vec<_>>()
            .join(" ");

        let started = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": text }
            })))
            .unwrap();
        let parts = started["draft"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        let draft_id = started["draft"]["id"].as_str().unwrap().to_owned();

        state.confirm_draft(LEGACY, &draft_id, false).unwrap();
        let command = next_command(&mut socket).await;
        assert_eq!(command["action"], "submit_post");
        assert_eq!(command["payload"]["parts"], json!(parts));

        let single = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": "Short." }
            })))
            .unwrap();
        assert!(single["draft"]["parts"].as_array().unwrap().is_empty());
    }

    /// A reply carries the post it answers, and nothing read from the page.
    #[tokio::test]
    async fn a_requested_reply_becomes_one_submit_command_when_confirmed() {
        let fixture = Fixture::start().await;
        let state = &fixture.state;
        let mut socket = pair(&fixture, &["submit_reply"]).await;

        let started = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "reply", "targetUrl": "https://x.com/owner/status/42",
                "payload": { "postId": "42", "text": "Thanks for this." }
            })))
            .unwrap();
        let draft_id = started["draft"]["id"].as_str().unwrap().to_owned();
        assert_eq!(started["draft"]["kind"], "reply");
        assert_eq!(started["draft"]["postId"], "42");

        state.confirm_draft(LEGACY, &draft_id, false).unwrap();
        let command = next_command(&mut socket).await;
        assert_eq!(command["action"], "submit_reply");
        assert_eq!(command["targetUrl"], "https://x.com/owner/status/42");
        assert_eq!(command["payload"]["postId"], "42");
        assert_eq!(command["payload"]["text"], "Thanks for this.");
        assert!(command["payload"].get("targetExcerpt").is_none());
    }

    // Full request -> confirm -> submit lifecycle for a new X post, through
    // the real dispatch loop and draft-consumption path. Also proves that
    // confirming the same draft twice is refused, so a retried confirm can
    // never publish the same post again.
    #[tokio::test]
    async fn server_fixture_composes_and_submits_a_post_without_duplication() {
        let fixture = Fixture::start().await;
        let state = &fixture.state;
        let mut socket = pair(&fixture, &["submit_post"]).await;

        let started = state
            .start(LEGACY, &job_request(json!({
                "platform": "x", "action": "post", "payload": { "text": "Exact post text" }
            })))
            .unwrap();
        let draft_id = started["draft"]["id"].as_str().unwrap().to_owned();
        assert_eq!(started["draft"]["targetUrl"], "https://x.com/compose/post");

        let creation = confirm_draft_value(state, LEGACY, &draft_id, true).unwrap();
        assert_eq!(creation["job"]["action"], "submit_post");
        let submit_job_id = creation["job"]["id"].as_str().unwrap().to_owned();
        let scheduled_at = creation["draft"]["scheduledAt"].as_i64().unwrap();
        assert_eq!(
            state.get_job(LEGACY, &submit_job_id).unwrap().unwrap().status,
            "queued"
        );

        state.set_test_now(scheduled_at);
        state.pump(LEGACY);

        let submit_command = next_command(&mut socket).await;
        assert_eq!(submit_command["action"], "submit_post");
        assert_eq!(submit_command["issuedAt"], scheduled_at);
        assert_eq!(submit_command["payload"]["draftId"], draft_id);
        assert_eq!(submit_command["payload"]["text"], "Exact post text");
        assert!(submit_command["payload"].get("scheduledAt").is_none());
        assert!(submit_command["payload"].get("postId").is_none());
        send(&mut socket, json!({
            "version":1,"type":"result","jobId":submit_job_id,
            "commandId":submit_command["commandId"],
            "issuedAt":state.now(),"expiresAt":submit_command["expiresAt"],"outcome":"succeeded",
            "data":{
                "kind":"submission",
                "platform":"x",
                "postedId":"999",
                "postedUrl":"https://x.com/owner/status/999"
            }
        })).await;
        let submitted = wait_for_status(state, &submit_job_id, "succeeded").await;
        assert_eq!(
            submitted.result.unwrap()["postedUrl"],
            "https://x.com/owner/status/999"
        );

        assert!(
            confirm_draft_value(state, LEGACY, &draft_id, true).is_err(),
            "confirming an already-submitted post draft must be refused"
        );
    }

    // Slow, real-transport reproduction: drives the production TypeScript
    // BrowserBridge (extension/transport-repro.ts) against this real Axum
    // WebSocket server over a real loopback socket, so any drift between the
    // hand-maintained Rust and TypeScript protocol implementations shows up
    // here instead of only in production. Run explicitly with
    // `cargo test -p pluk-browser real_extension_transport -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn real_extension_transport_survives_slow_command_and_reconnect() {
        let fixture = Fixture::start().await;
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let script = repo_root
            .join("extension")
            .join("src")
            .join("transport-repro.ts");

        let mut child = Command::new("bun")
            .arg("run")
            .arg(&script)
            .arg(&fixture.base)
            .arg(TOKEN)
            .arg("45000")
            .current_dir(&repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to start bun transport-repro.ts");

        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("transport-repro.ts did not exit within 120s");
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "transport-repro.ts exited with {:?}\n{}",
            output.status,
            stdout
        );

        let events: Vec<Value> = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("REPRO_EVENT "))
            .map(|json_text| serde_json::from_str(json_text).unwrap())
            .collect();
        let by_phase = |phase: &str| -> &Value {
            events
                .iter()
                .find(|event| event["phase"] == phase)
                .unwrap_or_else(|| panic!("missing REPRO_EVENT for phase {phase}\n{stdout}"))
        };

        assert_eq!(
            by_phase("job1-finished")["status"],
            "succeeded",
            "job1 should complete over the connection that stayed open through the slow command and its heartbeats: {stdout}"
        );
        assert_eq!(
            by_phase("reconnected")["status"]["state"],
            "connected",
            "the bridge should reconnect after a settings cycle: {stdout}"
        );
        assert_eq!(
            by_phase("job2-finished")["status"],
            "succeeded",
            "a command dispatched after reconnect should also complete: {stdout}"
        );
    }

    #[tokio::test]
    async fn loopback_http_requires_auth_and_allowed_origin() {
        let fixture = Fixture::start().await;
        let client = reqwest::Client::new();
        let status_url = format!("{}/wande/status", fixture.base);
        let missing = client.get(&status_url).send().await.unwrap();
        assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
        let hostile = client
            .get(&status_url)
            .bearer_auth(TOKEN)
            .header(ORIGIN, "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(hostile.status(), reqwest::StatusCode::FORBIDDEN);
        let valid = client
            .get(&status_url)
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(valid.status(), reqwest::StatusCode::OK);
        let body: Value = valid.json().await.unwrap();
        assert_eq!(body["version"], 1);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn tool_catalog_and_route_dispatch_current_action() {
        let fixture = Fixture::start().await;
        let client = reqwest::Client::new();
        let tools_url = format!("{}/wande/tools", fixture.base);

        let listed = client
            .get(&tools_url)
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(listed.status(), reqwest::StatusCode::OK);
        let body: Value = listed.json().await.unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["id"] == "x.inspect"));
        assert!(tools.iter().any(|tool| tool["id"] == "x.read_profile"));
        assert!(tools.iter().any(|tool| tool["id"] == "x.read_post"));
        assert!(tools.iter().all(|tool| tool["platform"] == "x"));

        // An unauthenticated call is rejected before the catalog is even read.
        let missing_auth = client.get(&tools_url).send().await.unwrap();
        assert_eq!(missing_auth.status(), reqwest::StatusCode::UNAUTHORIZED);

        let invoke = |tool: &str, body: Value| {
            let url = format!("{}/wande/tools/{tool}", fixture.base);
            let client = client.clone();
            async move {
                client
                    .post(url)
                    .bearer_auth(TOKEN)
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
            }
        };

        // An unknown tool id fails before any job is created.
        assert_eq!(
            invoke(
                "x.not_a_tool",
                json!({ "targetUrl": "https://x.com/status/42" })
            )
            .await
            .status(),
            reqwest::StatusCode::NOT_FOUND
        );

        // A tool on a platform Pluk no longer drives is rejected the same way.
        assert_eq!(
            invoke("gmail.read_feed", json!({ "payload": {} }))
                .await
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );

        // Invalid arguments fail before dispatch: no job is queued.
        assert_eq!(
            invoke(
                "x.inspect",
                json!({ "targetUrl": "https://not-x.example/status/42" })
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert!(
            fixture
                .state
                .store
                .browser()
                .list_jobs(10, fixture.state.now())
                .unwrap()
                .is_empty()
        );

        // A valid route dispatches a current action over the existing job store.
        let valid = invoke(
            "x.inspect",
            json!({ "targetUrl": "https://x.com/status/42" }),
        )
        .await;
        assert_eq!(valid.status(), reqwest::StatusCode::ACCEPTED);
        let created: Value = valid.json().await.unwrap();
        assert_eq!(created["job"]["platform"], "x");
        assert_eq!(created["job"]["action"], "inspect");
        assert_eq!(created["job"]["status"], "queued");

        // The typed x.read_post route accepts a post URL alone and derives the post ID.
        let url_only = invoke(
            "x.read_post",
            json!({ "targetUrl": "https://x.com/status/42" }),
        )
        .await;
        assert_eq!(url_only.status(), reqwest::StatusCode::ACCEPTED);
        let url_only_job: Value = url_only.json().await.unwrap();
        assert_eq!(url_only_job["job"]["payload"]["postId"], "42");

        // ...or a bare post ID alone, deriving the canonical target URL.
        let post_id_only = invoke("x.read_post", json!({ "payload": { "postId": "42" } })).await;
        assert_eq!(post_id_only.status(), reqwest::StatusCode::ACCEPTED);
        let post_id_only_job: Value = post_id_only.json().await.unwrap();
        assert_eq!(
            post_id_only_job["job"]["targetUrl"],
            "https://x.com/i/status/42"
        );

        // Neither a URL nor a post ID fails before any job is created.
        assert_eq!(
            invoke("x.read_post", json!({})).await.status(),
            reqwest::StatusCode::BAD_REQUEST
        );

        // A mismatched pair is rejected rather than silently preferring one.
        assert_eq!(
            invoke(
                "x.read_post",
                json!({ "targetUrl": "https://x.com/status/42", "payload": { "postId": "999" } })
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );

        // x.read_feed needs no body at all: it defaults to the fixed feed target.
        let read_feed = invoke("x.read_feed", json!({ "payload": {} })).await;
        assert_eq!(read_feed.status(), reqwest::StatusCode::ACCEPTED);
        let read_feed_job: Value = read_feed.json().await.unwrap();
        assert_eq!(read_feed_job["job"]["targetUrl"], "https://x.com/home");

        // x.read_profile resolves a username to the canonical profile URL.
        let read_profile = invoke(
            "x.read_profile",
            json!({ "payload": { "username": "jack" } }),
        )
        .await;
        assert_eq!(read_profile.status(), reqwest::StatusCode::ACCEPTED);
        let read_profile_job: Value = read_profile.json().await.unwrap();
        assert_eq!(read_profile_job["job"]["targetUrl"], "https://x.com/jack");

        // An invalid username is rejected before any job is created.
        assert_eq!(
            invoke(
                "x.read_profile",
                json!({ "payload": { "username": "not a handle" } })
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );

        // reply needs both a post ID and exact text; a missing one
        // fails before dispatch.
        assert_eq!(
            invoke(
                "x.reply",
                json!({
                    "targetUrl": "https://x.com/status/42",
                    "payload": { "text": "exact reply text" }
                })
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        let prepared = invoke(
            "x.reply",
            json!({
                "targetUrl": "https://x.com/status/42",
                "payload": { "postId": "42", "text": "exact reply text" }
            }),
        )
        .await;
        assert_eq!(prepared.status(), reqwest::StatusCode::ACCEPTED);
        let prepared: Value = prepared.json().await.unwrap();
        assert_eq!(prepared["draft"]["kind"], "reply");
        assert_eq!(prepared["draft"]["status"], "pending");
        assert_eq!(prepared["draft"]["targetUrl"], "https://x.com/status/42");
        assert!(
            prepared.get("job").is_none(),
            "a reply request is not a browser job"
        );

        // Submissions are reached only by confirming a draft.
        assert_eq!(
            invoke("x.submit_post", json!({ "payload": {} }))
                .await
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn extension_query_token_is_percent_decoded() {
        let fixture = Fixture::with_token("token/with?value-123456").await;
        let uri: Uri = "/wande/extension/ws?token=token%2Fwith%3Fvalue-123456"
            .parse()
            .unwrap();

        assert_eq!(
            fixture
                .state
                .extension_token(&HeaderMap::new(), &uri)
                .as_deref(),
            Some("token/with?value-123456")
        );
    }

    #[tokio::test]
    async fn pairing_with_the_wrong_key_is_refused() {
        let fixture = Fixture::start().await;
        assert!(
            connect_async(fixture.socket_url("not-the-pairing-key"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pairing_rejection_is_visible_in_status() {
        let fixture = Fixture::start().await;
        assert!(
            connect_async(fixture.socket_url("not-the-pairing-key"))
                .await
                .is_err()
        );

        let status = reqwest::Client::new()
            .get(format!("{}/wande/status", fixture.base))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        let body: Value = status.json().await.unwrap();
        assert!(body["extension"]["pairingRejectedAt"].is_i64());
    }
}
