//! The browser control queue: jobs, drafts, artifacts and schedule
//! reservations.
//!
//! The tables live in `pluk.db` alongside everything else Pluk records, and
//! evolve through the same `user_version` ladder. A `post` or `reply` request
//! writes a draft and touches nothing else; only `consume_draft` turns one
//! into a submission job — that is where the publish boundary is enforced.

pub mod schedule;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::Store;
use schedule::{ScheduleSettings, next_slot};

/// Default job expiry, and how long a confirmed submission job stays
/// dispatchable. A page either answers inside this or it has stalled.
pub const DEFAULT_JOB_TTL_MS: i64 = 2 * 60 * 1000;

pub const MAX_JOBS: i64 = 1_000;
pub const MAX_ARTIFACTS: i64 = 2_000;
/// How long a requested post waits on a person before it expires unposted.
/// Longer than a job's own expiry on purpose: a page that stalls for two
/// minutes is broken, but a person who takes two minutes to answer is not.
pub const DRAFT_TTL_MS: i64 = 10 * 60 * 1000;

// A reservation's post text lives on its draft, and the draft is pruned once
// it has settled, so the join stays outer and the text can come back empty.
const SELECT_RESERVATION: &str = "SELECT browser_schedule_reservations.id, browser_schedule_reservations.draft_id, browser_schedule_reservations.scheduled_at, browser_schedule_reservations.status, browser_schedule_reservations.created_at, browser_schedule_reservations.committed_at, browser_schedule_reservations.released_at, browser_drafts.text FROM browser_schedule_reservations LEFT JOIN browser_drafts ON browser_drafts.id = browser_schedule_reservations.draft_id";

#[derive(Debug)]
pub enum BrowserError {
    Sqlite(rusqlite::Error),
    Full,
    ScheduleBlocked,
    InvalidData(String),
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::Full => write!(
                formatter,
                "Job history is full; no completed job is available for automatic cleanup."
            ),
            Self::ScheduleBlocked => formatter.write_str(
                "An earlier queued post needs checking before another post can be queued.",
            ),
            Self::InvalidData(message) => formatter.write_str(message),
        }
    }
}

impl From<rusqlite::Error> for BrowserError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub job_id: String,
    pub kind: String,
    pub content_type: String,
    pub bytes: i64,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct ArtifactBody {
    pub metadata: Artifact,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub platform: String,
    pub kind: String,
    pub target_url: String,
    pub post_id: Option<String>,
    pub text: String,
    pub status: String,
    pub created_at: i64,
    pub confirmed_at: Option<i64>,
    pub submitted_at: Option<i64>,
    pub scheduled_at: Option<i64>,
}

impl Draft {
    /// Whether this draft can take a queue slot instead of going out as soon
    /// as it is confirmed. The queue holds whole posts on X; replies and
    /// every other platform publish on confirmation.
    ///
    /// Mirrors what [`BrowserStore::consume_draft`] accepts.
    pub fn can_queue(&self) -> bool {
        self.kind == "post" && self.platform == "x"
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleReservation {
    pub id: String,
    pub draft_id: String,
    pub scheduled_at: i64,
    pub status: String,
    pub created_at: i64,
    pub committed_at: Option<i64>,
    pub released_at: Option<i64>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSummary {
    pub pending_reservations: i64,
    pub uncertain_reservations: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub command_id: String,
    pub platform: String,
    pub action: String,
    pub target_url: String,
    pub payload: Value,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<ProtocolErrorView>,
    pub result: Option<Value>,
    pub dispatch_count: i64,
    pub draft_id: Option<String>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProtocolErrorView {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct SubmissionCreation {
    pub draft: Draft,
    pub job: Job,
}

/// A validated job ready to enqueue. The protocol layer owns validation; the
/// store writes the resolved platform, action, target and payload as given.
#[derive(Clone, Debug)]
pub struct JobInput<'a> {
    pub platform: &'a str,
    pub action: &'a str,
    pub target_url: &'a str,
    pub payload: &'a Value,
    pub ttl_ms: i64,
}

/// A post or reply an agent asked for, validated by the protocol layer.
/// Nothing reaches the page until the draft is confirmed; `post_id` is what
/// makes it a reply.
#[derive(Clone, Debug)]
pub struct DraftInput<'a> {
    pub platform: &'a str,
    pub target_url: &'a str,
    pub post_id: Option<&'a str>,
    pub text: &'a str,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub accepted: bool,
}

pub struct JobCompletion<'a> {
    pub id: &'a str,
    pub command_id: &'a str,
    pub outcome: &'a str,
    pub result: Option<&'a Value>,
    pub error: Option<&'a ProtocolErrorView>,
    pub now: i64,
}

/// The browser job queue, drafts, artifacts and schedule reservations, held
/// open for the duration of one call.
///
/// Every method runs against the same pooled connection the rest of the store
/// uses, so a browser write and an activity-log write can never interleave
/// mid-transaction. Acquire one per call and drop it — never hold one across
/// an await point or a second [`Store`] method.
pub struct BrowserStore<'a> {
    conn: std::sync::MutexGuard<'a, Connection>,
    max_jobs: i64,
    max_artifacts: i64,
}

impl Store {
    /// Borrow the browser tables. Blocks until the store lock is free.
    pub fn browser(&self) -> BrowserStore<'_> {
        BrowserStore {
            conn: self.conn.lock().expect("store lock"),
            max_jobs: MAX_JOBS,
            max_artifacts: MAX_ARTIFACTS,
        }
    }
}

impl BrowserStore<'_> {
    pub fn recover_in_flight(&mut self, now: i64) -> Result<(), BrowserError> {
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'unknown', finished_at = ?, error_code = 'server_restarted', error_message = 'The server restarted before this job completed.' WHERE status = 'running'",
            [now],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'unknown' WHERE status = 'confirmed' AND id IN (SELECT draft_id FROM browser_jobs WHERE action IN ('submit_reply', 'submit_post') AND status = 'unknown' AND draft_id IS NOT NULL)",
            [],
        )?;
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = 'unknown' WHERE status = 'reserved' AND draft_id IN (SELECT draft_id FROM browser_jobs WHERE action IN ('submit_reply', 'submit_post') AND status = 'unknown' AND draft_id IS NOT NULL)",
            [],
        )?;
        self.expire_drafts(now)?;
        self.expire_queued(now)?;
        Ok(())
    }

    /// The waiting draft that already says exactly this, if there is one, so
    /// asking twice never produces two posts.
    pub fn find_pending_draft(
        &mut self,
        input: &DraftInput<'_>,
        now: i64,
    ) -> Result<Option<Draft>, BrowserError> {
        self.expire_drafts(now)?;
        self.conn
            .query_row(
                "SELECT id, platform, kind, target_url, post_id, text, status, created_at, confirmed_at, submitted_at, scheduled_at FROM browser_drafts WHERE status = 'pending' AND platform = ? AND target_url = ? AND post_id IS ? AND text = ? ORDER BY created_at DESC LIMIT 1",
                params![input.platform, input.target_url, input.post_id, input.text],
                read_draft_row,
            )
            .optional()?
            .map(to_draft)
            .transpose()
            .map_err(BrowserError::from)
    }

    /// Whether a post is on its way into the page right now: confirmed to go
    /// out immediately and not yet landed. One goes at a time.
    pub fn submission_in_flight(&mut self, now: i64) -> Result<bool, BrowserError> {
        self.expire_queued(now)?;
        self.conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM browser_jobs WHERE action IN ('submit_post', 'submit_reply') AND status IN ('queued', 'running') AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.status = 'reserved' AND browser_schedule_reservations.scheduled_at > ?))",
                [now],
                |row| row.get(0),
            )
            .map_err(BrowserError::from)
    }

    /// Write a post that is waiting on a person. Nothing is sent to the
    /// browser until someone confirms it.
    pub fn create_draft(
        &mut self,
        input: &DraftInput<'_>,
        now: i64,
    ) -> Result<Draft, BrowserError> {
        self.expire_drafts(now)?;
        self.prune_drafts()?;
        let id = Uuid::new_v4().to_string();
        let kind = if input.post_id.is_some() {
            "reply"
        } else {
            "post"
        };
        self.conn.execute(
            "INSERT INTO browser_drafts (id, platform, kind, target_url, post_id, text, status, created_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
            params![id, input.platform, kind, input.target_url, input.post_id, input.text, now],
        )?;
        self.get_draft_row(&id)?
            .map(to_draft)
            .transpose()?
            .ok_or_else(|| {
                BrowserError::InvalidData("Created draft could not be read back.".to_owned())
            })
    }

    pub fn create_job(&mut self, request: &JobInput<'_>, now: i64) -> Result<Job, BrowserError> {
        self.expire_drafts(now)?;
        self.expire_queued(now)?;
        self.prune_jobs()?;
        let id = Uuid::new_v4().to_string();
        let command_id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, 'queued', ?, ?)",
            params![id, command_id, request.platform, request.action, request.target_url, request.payload.to_string(), now, now + request.ttl_ms],
        )?;
        self.get_job(&id, now)?.ok_or_else(|| {
            BrowserError::InvalidData("Created job could not be read back.".to_owned())
        })
    }

    pub fn claim_next(&mut self, now: i64) -> Result<Option<Job>, BrowserError> {
        self.expire_queued(now)?;
        let next: Option<String> = self.conn
            .query_row(
                "SELECT browser_jobs.id FROM browser_jobs WHERE browser_jobs.status = 'queued' AND browser_jobs.expires_at > ? AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.status IN ('reserved', 'unknown') AND (browser_schedule_reservations.status = 'unknown' OR browser_schedule_reservations.scheduled_at > ?)) ORDER BY browser_jobs.created_at ASC LIMIT 1",
                params![now, now],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = next else {
            return Ok(None);
        };
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'running', started_at = ?, dispatch_count = dispatch_count + 1 WHERE id = ? AND status = 'queued'",
            params![now, id],
        )?;
        self.get_job(&id, now)
    }

    pub fn next_scheduled_job_at(&self, now: i64) -> Result<Option<i64>, BrowserError> {
        self.conn
            .query_row(
                "SELECT MIN(browser_schedule_reservations.scheduled_at) FROM browser_jobs JOIN browser_schedule_reservations ON browser_schedule_reservations.draft_id = browser_jobs.draft_id WHERE browser_jobs.action = 'submit_post' AND browser_jobs.status = 'queued' AND browser_jobs.expires_at > ? AND browser_schedule_reservations.status = 'reserved' AND browser_schedule_reservations.scheduled_at > ?",
                params![now, now],
                |row| row.get(0),
            )
            .map_err(BrowserError::from)
    }

    pub fn complete_job(
        &mut self,
        completion: JobCompletion<'_>,
    ) -> Result<Completion, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.complete_job_transaction(completion);
        match result {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn complete_job_transaction(
        &mut self,
        completion: JobCompletion<'_>,
    ) -> Result<Completion, BrowserError> {
        let row = self.job_identity(completion.id)?;
        let Some((stored_command_id, status, action, payload_json)) = row else {
            return Ok(Completion { accepted: false });
        };
        if status != "running" || stored_command_id != completion.command_id {
            return Ok(Completion { accepted: false });
        }
        self.conn.execute(
            "UPDATE browser_jobs SET status = ?, finished_at = ?, error_code = ?, error_message = ?, result_json = ? WHERE id = ? AND command_id = ? AND status = 'running'",
            params![
                completion.outcome,
                completion.now,
                completion.error.map(|value| value.code.as_str()),
                completion.error.map(|value| value.message.as_str()),
                completion.result.map(Value::to_string),
                completion.id,
                completion.command_id,
            ],
        )?;
        if action == "submit_reply" || action == "submit_post" {
            self.update_submission_status(&payload_json, completion.outcome, completion.now)?;
        }
        Ok(Completion { accepted: true })
    }

    pub fn mark_in_flight(
        &mut self,
        id: &str,
        command_id: &str,
        status: &str,
        error: &ProtocolErrorView,
        now: i64,
    ) -> Result<bool, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let row = self.job_identity(id)?;
            let accepted = row
                .as_ref()
                .is_some_and(|(stored_command_id, current_status, _, _)| {
                    stored_command_id == command_id && current_status == "running"
                });
            if accepted {
                self.conn.execute(
                    "UPDATE browser_jobs SET status = ?, finished_at = ?, error_code = ?, error_message = ? WHERE id = ? AND command_id = ? AND status = 'running'",
                    params![status, now, error.code, error.message, id, command_id],
                )?;
                if let Some((_, _, action, payload)) = row
                    && (action == "submit_reply" || action == "submit_post")
                {
                    if status == "unknown" {
                        self.mark_submission_unknown(&payload)?;
                    } else {
                        self.update_submission_status(&payload, status, now)?;
                    }
                }
            }
            Ok(accepted)
        })();
        match result {
            Ok(accepted) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(accepted)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn consume_draft(
        &mut self,
        draft_id: &str,
        now: i64,
        queue: bool,
    ) -> Result<Option<SubmissionCreation>, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.consume_draft_transaction(draft_id, now, queue);
        match result {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn consume_draft_transaction(
        &mut self,
        draft_id: &str,
        now: i64,
        queue: bool,
    ) -> Result<Option<SubmissionCreation>, BrowserError> {
        self.expire_drafts(now)?;
        self.expire_queued(now)?;
        let draft = self.get_draft_row(draft_id)?;
        let Some(draft) = draft else {
            return Ok(None);
        };
        if draft.status != "pending" {
            return Ok(None);
        }
        self.prune_jobs()?;
        let job_id = Uuid::new_v4().to_string();
        let command_id = Uuid::new_v4().to_string();
        let scheduled_at = if queue && draft.kind == "post" {
            if draft.platform != "x" {
                return Err(BrowserError::InvalidData(
                    "Only X posts can be queued.".to_owned(),
                ));
            }
            let settings = self.get_schedule_settings()?;
            let latest_scheduled_at = self.latest_schedule_for_platform(&draft.platform, now)?;
            let scheduled_at = next_slot(now, latest_scheduled_at, &settings)
                .map_err(|error| BrowserError::InvalidData(error.to_string()))?;
            self.conn.execute(
                "INSERT INTO browser_schedule_reservations (id, draft_id, platform, scheduled_at, status, created_at) VALUES (?, ?, ?, ?, 'reserved', ?)",
                params![
                    Uuid::new_v4().to_string(),
                    draft.id,
                    draft.platform,
                    scheduled_at,
                    now,
                ],
            )?;
            Some(scheduled_at)
        } else {
            None
        };
        let (action, payload) = if draft.kind == "post" {
            let payload = serde_json::json!({
                "kind": "post_submission",
                "draftId": draft.id,
                "text": draft.text,
            });
            ("submit_post", payload)
        } else {
            (
                "submit_reply",
                serde_json::json!({
                    "kind": "submission",
                    "draftId": draft.id,
                    "postId": draft.post_id,
                    "text": draft.text,
                }),
            )
        };
        let expires_at = scheduled_at
            .and_then(|value| value.checked_add(DEFAULT_JOB_TTL_MS))
            .unwrap_or(now + DEFAULT_JOB_TTL_MS);
        self.conn.execute(
            "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, draft_id) VALUES (?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
            params![job_id, command_id, draft.platform, action, draft.target_url, payload.to_string(), now, expires_at, draft.id],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'confirmed', confirmed_at = ?, scheduled_at = ? WHERE id = ? AND status = 'pending'",
            params![now, scheduled_at, draft.id],
        )?;
        let stored_draft = self.get_draft(draft_id)?.ok_or_else(|| {
            BrowserError::InvalidData("Confirmed draft could not be read back.".to_owned())
        })?;
        let job = self.get_job(&job_id, now)?.ok_or_else(|| {
            BrowserError::InvalidData("Submission job could not be read back.".to_owned())
        })?;
        Ok(Some(SubmissionCreation {
            draft: stored_draft,
            job,
        }))
    }

    pub fn cancel_draft(
        &mut self,
        draft_id: &str,
        now: i64,
    ) -> Result<Option<Draft>, BrowserError> {
        self.expire_drafts(now)?;
        let draft = self.get_draft_row(draft_id)?;
        if draft.as_ref().is_none_or(|value| value.status != "pending") {
            return Ok(None);
        }
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'cancelled' WHERE id = ? AND status = 'pending'",
            [draft_id],
        )?;
        self.get_draft(draft_id)
    }

    /// Drop a post that is holding a queue slot: the slot is released and the
    /// submission never runs.
    ///
    /// `None` once the slot has come due and the submission has been picked
    /// up — at that point there is nothing left to call back.
    pub fn cancel_scheduled(
        &mut self,
        draft_id: &str,
        now: i64,
    ) -> Result<Option<ScheduleReservation>, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.cancel_scheduled_transaction(draft_id, now);
        match result {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn cancel_scheduled_transaction(
        &mut self,
        draft_id: &str,
        now: i64,
    ) -> Result<Option<ScheduleReservation>, BrowserError> {
        let reserved: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE draft_id = ? AND status = 'reserved')",
            [draft_id],
            |row| row.get(0),
        )?;
        if !reserved {
            return Ok(None);
        }
        let dropped = self.conn.execute(
            "DELETE FROM browser_jobs WHERE draft_id = ? AND status = 'queued'",
            [draft_id],
        )?;
        if dropped == 0 {
            return Ok(None);
        }
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = 'released', released_at = ? WHERE draft_id = ? AND status = 'reserved'",
            params![now, draft_id],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'cancelled' WHERE id = ? AND status = 'confirmed'",
            [draft_id],
        )?;
        self.conn
            .query_row(
                &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.draft_id = ?"),
                [draft_id],
                read_schedule_reservation,
            )
            .optional()
            .map_err(BrowserError::from)
    }

    pub fn get_job(&mut self, id: &str, now: i64) -> Result<Option<Job>, BrowserError> {
        self.expire_queued(now)?;
        let row = self.conn.query_row(
            "SELECT id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, started_at, finished_at, error_code, error_message, result_json, dispatch_count, draft_id FROM browser_jobs WHERE id = ?",
            [id],
            read_job_row,
        ).optional()?;
        row.map(|row| self.load_job(row)).transpose()
    }

    pub fn list_jobs(&mut self, limit: i64, now: i64) -> Result<Vec<Job>, BrowserError> {
        self.expire_queued(now)?;
        let bounded_limit = limit.clamp(1, 100);
        let mut statement = self.conn.prepare(
            "SELECT id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, started_at, finished_at, error_code, error_message, result_json, dispatch_count, draft_id FROM browser_jobs ORDER BY created_at DESC LIMIT ?",
        )?;
        let rows = statement.query_map([bounded_limit], read_job_row)?;
        let rows: Result<Vec<JobRow>, rusqlite::Error> = rows.collect();
        drop(statement);
        rows.map_err(BrowserError::from)?
            .into_iter()
            .map(|value| self.load_job(value))
            .collect()
    }

    pub fn get_draft(&mut self, id: &str) -> Result<Option<Draft>, BrowserError> {
        self.expire_drafts(now_millis())?;
        Ok(self.get_draft_row(id)?.map(to_draft).transpose()?)
    }

    /// Every draft still waiting on a confirmation, newest first.
    pub fn list_pending_drafts(&mut self, now: i64) -> Result<Vec<Draft>, BrowserError> {
        self.expire_drafts(now)?;
        let mut statement = self.conn.prepare(
            "SELECT id, platform, kind, target_url, post_id, text, status, created_at, confirmed_at, submitted_at, scheduled_at FROM browser_drafts WHERE status = 'pending' ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], read_draft_row)?;
        rows.map(|row| Ok(to_draft(row?)?)).collect()
    }

    pub fn get_schedule_settings(&self) -> Result<ScheduleSettings, BrowserError> {
        let (window_start, window_end, min_gap_minutes, max_gap_minutes): (String, String, i64, i64) = self.conn
            .query_row(
                "SELECT window_start, window_end, min_gap_minutes, max_gap_minutes FROM browser_schedule_settings WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(BrowserError::from)
            ?;
        ScheduleSettings::new(&window_start, &window_end, min_gap_minutes, max_gap_minutes)
            .map_err(|error| BrowserError::InvalidData(error.to_string()))
    }

    pub fn update_schedule_settings(
        &mut self,
        settings: &ScheduleSettings,
    ) -> Result<ScheduleSettings, BrowserError> {
        settings
            .to_config()
            .map_err(|error| BrowserError::InvalidData(error.to_string()))?;
        self.conn.execute(
            "UPDATE browser_schedule_settings SET window_start = ?, window_end = ?, min_gap_minutes = ?, max_gap_minutes = ? WHERE id = 1",
            params![
                settings.window_start,
                settings.window_end,
                settings.min_gap_minutes,
                settings.max_gap_minutes,
            ],
        )?;
        self.get_schedule_settings()
    }

    pub fn list_schedule_reservations(
        &self,
        limit: i64,
        now: i64,
    ) -> Result<Vec<ScheduleReservation>, BrowserError> {
        self.expire_queued(now)?;
        let bounded_limit = limit.clamp(1, 100);
        let mut statement = self.conn.prepare(&format!(
            "{SELECT_RESERVATION} WHERE browser_schedule_reservations.status IN ('reserved', 'committed', 'unknown', 'released') AND (browser_schedule_reservations.status IN ('reserved', 'unknown') OR browser_schedule_reservations.scheduled_at >= ?) ORDER BY browser_schedule_reservations.scheduled_at ASC LIMIT ?",
        ))?;
        let rows = statement.query_map(params![now, bounded_limit], read_schedule_reservation)?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    pub fn schedule_summary(&self) -> Result<ScheduleSummary, BrowserError> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN status = 'reserved' THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN status = 'unknown' THEN 1 ELSE 0 END), 0) FROM browser_schedule_reservations",
                [],
                |row| {
                    Ok(ScheduleSummary {
                        pending_reservations: row.get(0)?,
                        uncertain_reservations: row.get(1)?,
                    })
                },
            )
            .map_err(BrowserError::from)
    }

    pub fn resolve_schedule(
        &mut self,
        draft_id: &str,
        published: bool,
        now: i64,
    ) -> Result<Option<ScheduleReservation>, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let reservation = self.conn
                .query_row(
                    &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.draft_id = ? AND browser_schedule_reservations.status = 'unknown'"),
                    [draft_id],
                    read_schedule_reservation,
                )
                .optional()?;
            let Some(reservation) = reservation else {
                return Ok(None);
            };
            let status = if published { "committed" } else { "released" };
            self.conn.execute(
                "UPDATE browser_schedule_reservations SET status = ?, committed_at = ?, released_at = ? WHERE draft_id = ? AND status = 'unknown'",
                params![
                    status,
                    published.then_some(now),
                    (!published).then_some(now),
                    draft_id,
                ],
            )?;
            self.conn.execute(
                "UPDATE browser_drafts SET status = ?, submitted_at = ? WHERE id = ? AND status = 'unknown'",
                params![
                    if published { "submitted" } else { "failed" },
                    published.then_some(now),
                    draft_id,
                ],
            )?;
            self.conn
                .query_row(
                    &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.id = ?"),
                    [reservation.id],
                    read_schedule_reservation,
                )
                .map(Some)
        })();
        match result {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(BrowserError::from(error))
            }
        }
    }

    pub fn create_artifact(
        &mut self,
        job_id: &str,
        kind: &str,
        content_type: &str,
        data: &[u8],
        now: i64,
    ) -> Result<Artifact, BrowserError> {
        if self.job_identity(job_id)?.is_none() {
            return Err(BrowserError::InvalidData("Job not found".to_owned()));
        }
        self.prune_artifacts(now)?;
        let id = Uuid::new_v4().to_string();
        let expires_at = now + 24 * 60 * 60 * 1000;
        self.conn.execute(
            "INSERT INTO browser_artifacts (id, job_id, kind, content_type, bytes, data, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![id, job_id, kind, content_type, data.len() as i64, data, now, expires_at],
        )?;
        self.get_artifact(&id)?.ok_or_else(|| {
            BrowserError::InvalidData("Created artifact could not be read back.".to_owned())
        })
    }

    pub fn get_artifact(&mut self, id: &str) -> Result<Option<Artifact>, BrowserError> {
        self.conn.query_row(
            "SELECT id, job_id, kind, content_type, bytes, created_at, expires_at FROM browser_artifacts WHERE id = ? AND expires_at > ?",
            params![id, now_millis()],
            read_artifact,
        ).optional().map_err(BrowserError::from)
    }

    pub fn get_artifact_body(&mut self, id: &str) -> Result<Option<ArtifactBody>, BrowserError> {
        self.conn.query_row(
            "SELECT id, job_id, kind, content_type, bytes, data, created_at, expires_at FROM browser_artifacts WHERE id = ? AND expires_at > ?",
            params![id, now_millis()],
            |row| {
                let metadata = Artifact {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    kind: row.get(2)?,
                    content_type: row.get(3)?,
                    bytes: row.get(4)?,
                    created_at: row.get(6)?,
                    expires_at: row.get(7)?,
                };
                let data: Vec<u8> = row.get(5)?;
                Ok(ArtifactBody { metadata, data })
            },
        ).optional().map_err(BrowserError::from)
    }

    pub fn count_queued(&mut self, now: i64) -> Result<i64, BrowserError> {
        self.expire_queued(now)?;
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM browser_jobs WHERE status = 'queued'",
                [],
                |row| row.get(0),
            )
            .map_err(BrowserError::from)
    }

    fn load_job(&mut self, row: JobRow) -> Result<Job, BrowserError> {
        let mut payload: Value = serde_json::from_str(&row.payload_json)
            .map_err(|_| BrowserError::InvalidData("Stored job payload is invalid.".to_owned()))?;
        if row.action == "submit_post"
            && let Some(object) = payload.as_object_mut()
        {
            object.remove("scheduledAt");
        }
        let result = row
            .result_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| BrowserError::InvalidData("Stored job result is invalid.".to_owned()))?;
        let error = match (row.error_code, row.error_message) {
            (Some(code), Some(message)) => Some(ProtocolErrorView { code, message }),
            _ => None,
        };
        Ok(Job {
            id: row.id.clone(),
            command_id: row.command_id,
            platform: row.platform,
            action: row.action,
            target_url: row.target_url,
            payload,
            status: row.status,
            created_at: row.created_at,
            expires_at: row.expires_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            error,
            result,
            dispatch_count: row.dispatch_count,
            draft_id: row.draft_id,
            artifacts: self.list_artifacts(&row.id)?,
        })
    }

    fn list_artifacts(&self, job_id: &str) -> Result<Vec<Artifact>, BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT id, job_id, kind, content_type, bytes, created_at, expires_at FROM browser_artifacts WHERE job_id = ? AND expires_at > ? ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(params![job_id, now_millis()], read_artifact)?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    fn get_draft_row(&self, id: &str) -> Result<Option<DraftRow>, BrowserError> {
        self.conn.query_row(
            "SELECT id, platform, kind, target_url, post_id, text, status, created_at, confirmed_at, submitted_at, scheduled_at FROM browser_drafts WHERE id = ?",
            [id],
            read_draft_row,
        ).optional().map_err(BrowserError::from)
    }

    fn latest_schedule_for_platform(
        &self,
        platform: &str,
        now: i64,
    ) -> Result<Option<i64>, BrowserError> {
        let has_uncertain: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE platform = ? AND status = 'unknown')",
            [platform],
            |row| row.get(0),
        )?;
        if has_uncertain {
            return Err(BrowserError::ScheduleBlocked);
        }
        self.conn
            .query_row(
                "SELECT MAX(scheduled_at) FROM browser_schedule_reservations WHERE platform = ? AND status IN ('reserved', 'committed') AND scheduled_at > ?",
                params![platform, now],
                |row| row.get(0),
            )
            .map_err(BrowserError::from)
    }

    fn job_identity(
        &self,
        id: &str,
    ) -> Result<Option<(String, String, String, String)>, BrowserError> {
        self.conn
            .query_row(
                "SELECT command_id, status, action, payload_json FROM browser_jobs WHERE id = ?",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(BrowserError::from)
    }

    fn update_submission_status(
        &self,
        payload_json: &str,
        outcome: &str,
        now: i64,
    ) -> Result<(), BrowserError> {
        let payload: Value = serde_json::from_str(payload_json).map_err(|_| {
            BrowserError::InvalidData("Stored submission payload is invalid.".to_owned())
        })?;
        let Some(draft_id) = payload.get("draftId").and_then(Value::as_str) else {
            return Ok(());
        };
        let status = match outcome {
            "succeeded" => "submitted",
            "unknown" => "unknown",
            _ => "failed",
        };
        self.conn.execute(
            "UPDATE browser_drafts SET status = ?, submitted_at = ? WHERE id = ? AND status = 'confirmed'",
            params![status, (outcome == "succeeded").then_some(now), draft_id],
        )?;
        let reservation_status = match outcome {
            "succeeded" => "committed",
            "unknown" => "unknown",
            _ => "released",
        };
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = ?, committed_at = ?, released_at = ? WHERE draft_id = ? AND status = 'reserved'",
            params![
                reservation_status,
                (outcome == "succeeded").then_some(now),
                (outcome != "succeeded" && outcome != "unknown").then_some(now),
                draft_id,
            ],
        )?;
        Ok(())
    }

    fn mark_submission_unknown(&self, payload_json: &str) -> Result<(), BrowserError> {
        let payload: Value = serde_json::from_str(payload_json).map_err(|_| {
            BrowserError::InvalidData("Stored submission payload is invalid.".to_owned())
        })?;
        if let Some(draft_id) = payload.get("draftId").and_then(Value::as_str) {
            self.conn.execute(
                "UPDATE browser_drafts SET status = 'unknown' WHERE id = ? AND status = 'confirmed'",
                [draft_id],
            )?;
            self.conn.execute(
                "UPDATE browser_schedule_reservations SET status = 'unknown' WHERE draft_id = ? AND status = 'reserved'",
                [draft_id],
            )?;
        }
        Ok(())
    }

    fn expire_queued(&self, now: i64) -> Result<(), BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT action, payload_json FROM browser_jobs WHERE status = 'queued' AND expires_at <= ?",
        )?;
        let expired: Vec<(String, String)> = statement
            .query_map([now], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'expired', finished_at = ?, error_code = 'expired', error_message = 'The job expired before it started.' WHERE status = 'queued' AND expires_at <= ?",
            [now, now],
        )?;
        for (action, payload) in expired {
            if action == "submit_reply" || action == "submit_post" {
                self.update_submission_status(&payload, "expired", now)?;
            }
        }
        Ok(())
    }

    fn expire_drafts(&self, now: i64) -> Result<(), BrowserError> {
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'expired' WHERE status = 'pending' AND created_at + ? <= ?",
            [DRAFT_TTL_MS, now],
        )?;
        Ok(())
    }

    /// Drop settled drafts whose submission job is gone too, so the table does
    /// not grow with every post ever asked for.
    fn prune_drafts(&self) -> Result<(), BrowserError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM browser_drafts", [], |row| row.get(0))?;
        if count < self.max_jobs {
            return Ok(());
        }
        let remove = count - self.max_jobs + 1;
        self.conn.execute(
            "DELETE FROM browser_drafts WHERE id IN (SELECT id FROM browser_drafts WHERE status NOT IN ('pending', 'confirmed') AND NOT EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.draft_id = browser_drafts.id) ORDER BY created_at ASC LIMIT ?)",
            [remove],
        )?;
        let after: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM browser_drafts", [], |row| row.get(0))?;
        if after >= self.max_jobs {
            return Err(BrowserError::Full);
        }
        Ok(())
    }

    fn prune_jobs(&self) -> Result<(), BrowserError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM browser_jobs", [], |row| row.get(0))?;
        if count < self.max_jobs {
            return Ok(());
        }
        let remove = count - self.max_jobs + 1;
        self.conn.execute(
            "DELETE FROM browser_jobs WHERE id IN (SELECT browser_jobs.id FROM browser_jobs WHERE status IN ('succeeded', 'failed', 'expired', 'unknown') AND NOT EXISTS (SELECT 1 FROM browser_drafts WHERE browser_drafts.id = browser_jobs.draft_id AND browser_drafts.status = 'confirmed') ORDER BY created_at ASC LIMIT ?)",
            [remove],
        )?;
        let after: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM browser_jobs", [], |row| row.get(0))?;
        if after >= self.max_jobs {
            return Err(BrowserError::Full);
        }
        Ok(())
    }

    fn prune_artifacts(&self, now: i64) -> Result<(), BrowserError> {
        self.conn
            .execute("DELETE FROM browser_artifacts WHERE expires_at <= ?", [now])?;
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM browser_artifacts", [], |row| {
                    row.get(0)
                })?;
        if count < self.max_artifacts {
            return Ok(());
        }
        let remove = count - self.max_artifacts + 1;
        self.conn.execute("DELETE FROM browser_artifacts WHERE id IN (SELECT id FROM browser_artifacts ORDER BY created_at ASC LIMIT ?)", [remove])?;
        Ok(())
    }
}

#[derive(Clone)]
struct JobRow {
    id: String,
    command_id: String,
    platform: String,
    action: String,
    target_url: String,
    payload_json: String,
    status: String,
    created_at: i64,
    expires_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    error_code: Option<String>,
    error_message: Option<String>,
    result_json: Option<String>,
    dispatch_count: i64,
    draft_id: Option<String>,
}

struct DraftRow {
    id: String,
    platform: String,
    kind: String,
    target_url: String,
    post_id: Option<String>,
    text: String,
    status: String,
    created_at: i64,
    confirmed_at: Option<i64>,
    submitted_at: Option<i64>,
    scheduled_at: Option<i64>,
}

fn read_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: row.get(0)?,
        command_id: row.get(1)?,
        platform: row.get(2)?,
        action: row.get(3)?,
        target_url: row.get(4)?,
        payload_json: row.get(5)?,
        status: row.get(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
        started_at: row.get(9)?,
        finished_at: row.get(10)?,
        error_code: row.get(11)?,
        error_message: row.get(12)?,
        result_json: row.get(13)?,
        dispatch_count: row.get(14)?,
        draft_id: row.get(15)?,
    })
}

fn read_draft_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DraftRow> {
    Ok(DraftRow {
        id: row.get(0)?,
        platform: row.get(1)?,
        kind: row.get(2)?,
        target_url: row.get(3)?,
        post_id: row.get(4)?,
        text: row.get(5)?,
        status: row.get(6)?,
        created_at: row.get(7)?,
        confirmed_at: row.get(8)?,
        submitted_at: row.get(9)?,
        scheduled_at: row.get(10)?,
    })
}

fn read_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: row.get(0)?,
        job_id: row.get(1)?,
        kind: row.get(2)?,
        content_type: row.get(3)?,
        bytes: row.get(4)?,
        created_at: row.get(5)?,
        expires_at: row.get(6)?,
    })
}

fn to_draft(row: DraftRow) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: row.id,
        platform: row.platform,
        kind: row.kind,
        target_url: row.target_url,
        post_id: row.post_id,
        text: row.text,
        status: row.status,
        created_at: row.created_at,
        confirmed_at: row.confirmed_at,
        submitted_at: row.submitted_at,
        scheduled_at: row.scheduled_at,
    })
}

fn read_schedule_reservation(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleReservation> {
    Ok(ScheduleReservation {
        id: row.get(0)?,
        draft_id: row.get(1)?,
        scheduled_at: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        committed_at: row.get(5)?,
        released_at: row.get(6)?,
        text: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    fn open(directory: &TempDir) -> Store {
        Store::open(&directory.path().join("pluk.db")).unwrap()
    }

    fn prepare_post_draft(store: &mut BrowserStore<'_>, text: &str, now: i64) -> Draft {
        store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text,
                },
                now,
            )
            .unwrap()
    }

    fn prepare_reply_draft(store: &mut BrowserStore<'_>, text: &str, now: i64) -> Draft {
        store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    target_url: "https://x.com/status/42",
                    post_id: Some("42"),
                    text,
                },
                now,
            )
            .unwrap()
    }

    #[test]
    fn restart_recovery_marks_running_jobs_unknown() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let payload = serde_json::json!({"kind": "empty"});
        let job = store
            .create_job(
                &JobInput {
                    platform: "x",
                    action: "inspect",
                    target_url: "https://x.com/status/42",
                    payload: &payload,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        store.claim_next(100).unwrap();
        store.recover_in_flight(200).unwrap();
        assert_eq!(
            store.get_job(&job.id, 200).unwrap().unwrap().status,
            "unknown"
        );
    }

    #[test]
    fn a_draft_waits_in_pluk_and_no_job_is_queued_for_it() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_reply_draft(&mut store, "Exact reply", 100);
        assert_eq!(draft.kind, "reply");
        assert_eq!(draft.post_id.as_deref(), Some("42"));
        assert_eq!(draft.status, "pending");
        assert!(store.claim_next(100).unwrap().is_none());
        assert!(!draft.can_queue());
    }

    #[test]
    fn asking_for_the_same_post_twice_finds_the_one_already_waiting() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let input = DraftInput {
            platform: "x",
            target_url: "https://x.com/compose/post",
            post_id: None,
            text: "Same words",
        };
        assert!(store.find_pending_draft(&input, 100).unwrap().is_none());
        let first = store.create_draft(&input, 100).unwrap();
        assert_eq!(
            store.find_pending_draft(&input, 101).unwrap().unwrap().id,
            first.id
        );
        let other = DraftInput { text: "Other words", ..input };
        assert!(store.find_pending_draft(&other, 101).unwrap().is_none());
        store.cancel_draft(&first.id, 102).unwrap();
        assert!(store.find_pending_draft(&input, 103).unwrap().is_none());
    }

    #[test]
    fn one_post_goes_out_at_a_time() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        assert!(!store.submission_in_flight(100).unwrap());
        let now_post = prepare_post_draft(&mut store, "Now", 100);
        store.consume_draft(&now_post.id, 100, false).unwrap().unwrap();
        assert!(store.submission_in_flight(100).unwrap());
        let dispatched = store.claim_next(100).unwrap().unwrap();
        assert!(store.submission_in_flight(100).unwrap());
        store
            .complete_job(JobCompletion {
                id: &dispatched.id,
                command_id: &dispatched.command_id,
                outcome: "succeeded",
                result: Some(&serde_json::json!({
                    "kind": "submission",
                    "platform": "x",
                    "postedId": "1",
                    "postedUrl": "https://x.com/owner/status/1"
                })),
                error: None,
                now: 200,
            })
            .unwrap();
        assert!(!store.submission_in_flight(200).unwrap());

        let later = prepare_post_draft(&mut store, "Later", 200);
        store.consume_draft(&later.id, 200, true).unwrap().unwrap();
        assert!(
            !store.submission_in_flight(200).unwrap(),
            "a post holding a queue slot is not going out yet"
        );
    }

    #[test]
    fn confirmation_is_consumed_once() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_reply_draft(&mut store, "Exact reply", 100);
        let submission = store.consume_draft(&draft.id, 100, true).unwrap().unwrap();
        assert_eq!(submission.job.action, "submit_reply");
        assert_eq!(submission.job.payload["postId"], "42");
        assert_eq!(submission.job.payload["text"], "Exact reply");
        assert!(submission.draft.scheduled_at.is_none());
        assert!(store.consume_draft(&draft.id, 100, true).unwrap().is_none());
    }

    #[test]
    fn dispatched_submission_is_not_retried_after_restart() {
        let directory = tempdir().unwrap();
        let (draft_id, dispatched_id) = {
            let database = open(&directory);
            let mut store = database.browser();
            let draft = prepare_reply_draft(&mut store, "Exact reply", 100);
            let submission = store.consume_draft(&draft.id, 100, true).unwrap().unwrap();
            let dispatched = store.claim_next(100).unwrap().unwrap();
            assert_eq!(dispatched.id, submission.job.id);
            (draft.id, dispatched.id)
        };

        let database = open(&directory);
        let mut restarted = database.browser();
        restarted.recover_in_flight(200).unwrap();

        let job = restarted.get_job(&dispatched_id, 200).unwrap().unwrap();
        assert_eq!(job.status, "unknown");
        assert_eq!(job.dispatch_count, 1);
        assert_eq!(
            restarted.get_draft(&draft_id).unwrap().unwrap().status,
            "unknown"
        );
        assert!(restarted.claim_next(200).unwrap().is_none());
    }

    #[test]
    fn post_draft_confirmation_produces_a_submit_post_job_and_is_consumed_once() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_post_draft(&mut store, "Exact post text", 100);
        assert_eq!(draft.target_url, "https://x.com/compose/post");
        let draft_id = draft.id;
        let submission = store.consume_draft(&draft_id, 100, true).unwrap().unwrap();
        assert_eq!(submission.job.action, "submit_post");
        assert_eq!(submission.job.target_url, "https://x.com/compose/post");
        assert_eq!(submission.job.draft_id.as_deref(), Some(draft_id.as_str()));
        assert!(submission.job.payload.get("postId").is_none());
        assert!(submission.job.payload.get("scheduledAt").is_none());
        assert_eq!(
            submission.job.expires_at,
            submission.draft.scheduled_at.unwrap() + DEFAULT_JOB_TTL_MS
        );
        assert_eq!(store.schedule_summary().unwrap().pending_reservations, 1);
        assert!(store.consume_draft(&draft_id, 100, true).unwrap().is_none());
    }

    #[test]
    fn successful_post_submission_commits_its_reservation() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_post_draft(&mut store, "Scheduled post", 100);
        let submission = store.consume_draft(&draft.id, 100, true).unwrap().unwrap();
        let scheduled_at = submission.draft.scheduled_at.unwrap();
        assert!(store.claim_next(scheduled_at - 1).unwrap().is_none());
        let dispatched = store.claim_next(scheduled_at).unwrap().unwrap();
        store
            .complete_job(JobCompletion {
                id: &dispatched.id,
                command_id: &dispatched.command_id,
                outcome: "succeeded",
                result: Some(&serde_json::json!({
                    "kind": "submission",
                    "platform": "x",
                    "postedId": "999",
                    "postedUrl": "https://x.com/owner/status/999"
                })),
                error: None,
                now: 200,
            })
            .unwrap();
        assert_eq!(
            store.get_draft(&draft.id).unwrap().unwrap().status,
            "submitted"
        );
        assert_eq!(store.schedule_summary().unwrap().pending_reservations, 0);
        assert_eq!(
            store
                .list_schedule_reservations(10, 100)
                .unwrap()
                .first()
                .unwrap()
                .status,
            "committed"
        );
    }

    #[test]
    fn an_immediate_confirmation_reserves_no_slot() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_post_draft(&mut store, "Immediate post", 100);

        let submission = store.consume_draft(&draft.id, 100, false).unwrap().unwrap();

        assert!(submission.draft.scheduled_at.is_none());
        assert!(
            store
                .list_schedule_reservations(10, 100)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.claim_next(100).unwrap().unwrap().action,
            "submit_post"
        );
    }

    #[test]
    fn waiting_drafts_are_listed_newest_first_until_they_are_answered() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        // Wall-clock timestamps: reading a draft back expires the ones that
        // have aged out, against the real clock rather than the one passed in.
        let now = now_millis();
        let first = prepare_post_draft(&mut store, "First post", now);
        let second = prepare_post_draft(&mut store, "Second post", now + 1);

        let waiting = store.list_pending_drafts(now + 2).unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|draft| draft.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Second post", "First post"]
        );
        assert!(waiting.iter().all(Draft::can_queue));

        store.cancel_draft(&first.id, now + 2).unwrap().unwrap();
        store
            .consume_draft(&second.id, now + 2, false)
            .unwrap()
            .unwrap();
        assert!(store.list_pending_drafts(now + 2).unwrap().is_empty());
    }

    #[test]
    fn a_queued_post_can_be_taken_back_until_its_slot_comes_due() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_post_draft(&mut store, "Queued post", 100);
        let slot = store
            .consume_draft(&draft.id, 100, true)
            .unwrap()
            .unwrap()
            .draft
            .scheduled_at
            .unwrap();

        let released = store.cancel_scheduled(&draft.id, 200).unwrap().unwrap();

        assert_eq!(released.status, "released");
        assert_eq!(
            store.get_draft(&draft.id).unwrap().unwrap().status,
            "cancelled"
        );
        assert!(store.claim_next(slot).unwrap().is_none());
        assert!(store.cancel_scheduled(&draft.id, 300).unwrap().is_none());
    }

    #[test]
    fn queued_posts_stack_behind_each_other() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let first = prepare_post_draft(&mut store, "First post", 100);
        let first_slot = store
            .consume_draft(&first.id, 100, true)
            .unwrap()
            .unwrap()
            .draft
            .scheduled_at
            .unwrap();
        let second = prepare_post_draft(&mut store, "Second post", 100);
        let second_slot = store
            .consume_draft(&second.id, 100, true)
            .unwrap()
            .unwrap()
            .draft
            .scheduled_at
            .unwrap();

        assert!(second_slot > first_slot);
        let reservations = store.list_schedule_reservations(10, 100).unwrap();
        assert_eq!(
            reservations
                .iter()
                .map(|value| value.text.as_str())
                .collect::<Vec<_>>(),
            vec!["First post", "Second post"]
        );
    }

    #[test]
    fn uncertain_post_submission_blocks_the_queue() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let first = prepare_post_draft(&mut store, "First post", 100);
        let submission = store.consume_draft(&first.id, 100, true).unwrap().unwrap();
        let scheduled_at = submission.draft.scheduled_at.unwrap();
        let dispatched = store.claim_next(scheduled_at).unwrap().unwrap();
        store
            .mark_in_flight(
                &dispatched.id,
                &dispatched.command_id,
                "unknown",
                &ProtocolErrorView {
                    code: "submission_unknown".to_owned(),
                    message: "Check X.".to_owned(),
                },
                200,
            )
            .unwrap();
        assert_eq!(store.schedule_summary().unwrap().uncertain_reservations, 1);
        let second = prepare_post_draft(&mut store, "Second post", 300);
        assert!(matches!(
            store.consume_draft(&second.id, 300, true),
            Err(BrowserError::ScheduleBlocked)
        ));
        assert_eq!(
            store.get_draft(&first.id).unwrap().unwrap().scheduled_at,
            submission.draft.scheduled_at
        );
    }

    #[test]
    fn pre_click_post_failure_releases_its_reservation() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let first = prepare_post_draft(&mut store, "First post", 100);
        store.consume_draft(&first.id, 100, true).unwrap().unwrap();
        let scheduled_at = store
            .get_draft(&first.id)
            .unwrap()
            .unwrap()
            .scheduled_at
            .unwrap();
        let dispatched = store.claim_next(scheduled_at).unwrap().unwrap();
        store
            .mark_in_flight(
                &dispatched.id,
                &dispatched.command_id,
                "failed",
                &ProtocolErrorView {
                    code: "site_markup_changed".to_owned(),
                    message: "No post control.".to_owned(),
                },
                200,
            )
            .unwrap();
        assert_eq!(store.schedule_summary().unwrap().pending_reservations, 0);
        assert_eq!(
            store.get_draft(&first.id).unwrap().unwrap().status,
            "failed"
        );
        let second = prepare_post_draft(&mut store, "Second post", 300);
        assert!(
            store
                .consume_draft(&second.id, 300, true)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn expired_post_submission_releases_its_reservation_when_schedule_is_read() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let draft = prepare_post_draft(&mut store, "Expired post", 100);
        let submission = store.consume_draft(&draft.id, 100, true).unwrap().unwrap();

        let reservations = store
            .list_schedule_reservations(10, submission.job.expires_at)
            .unwrap();

        assert!(reservations.is_empty());
        assert_eq!(store.schedule_summary().unwrap().pending_reservations, 0);
        assert_eq!(
            store.get_draft(&draft.id).unwrap().unwrap().status,
            "failed"
        );
    }
}
