//! The browser control queue: jobs, drafts, artifacts and schedule
//! reservations.
//!
//! The tables live in `pluk.db` alongside everything else Pluk records, and
//! evolve through the same `user_version` ladder. A `post` or `reply` request
//! writes a draft and touches nothing else; only `consume_draft` turns one
//! into a submission job — that is where the publish boundary is enforced.

pub mod images;
pub mod schedule;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::Store;
use images::StagedImage;
use schedule::{ScheduleSettings, next_slot};

/// Whether a confirmed draft carrying images can actually be turned into a
/// submission job. The extension attaches them to the composer through its
/// own debugger session before it types or submits anything; while this is
/// `false`, [`BrowserStore::consume_draft`] refuses rather than risk
/// publishing the text alone.
pub const IMAGE_UPLOAD_IMPLEMENTED: bool = true;

/// Default job expiry, and how long a confirmed submission job stays
/// dispatchable. A page either answers inside this or it has stalled.
pub const DEFAULT_JOB_TTL_MS: i64 = 3 * 60 * 1000;

pub const MAX_JOBS: i64 = 1_000;
pub const MAX_ARTIFACTS: i64 = 2_000;
/// Owner used by browser rows created before integrations had their own queue.
pub const LEGACY_INTEGRATION_ID: &str = "browser";
/// How long a requested post waits on a person before it expires unposted.
/// Longer than a job's own expiry on purpose: a page that stalls for two
/// minutes is broken, but a person who takes two minutes to answer is not.
pub const DRAFT_TTL_MS: i64 = 10 * 60 * 1000;

// A reservation's post text lives on its draft, and the draft is pruned once
// it has settled, so the join stays outer and the text can come back empty.
const SELECT_RESERVATION: &str = "SELECT browser_schedule_reservations.integration_id, browser_schedule_reservations.id, browser_schedule_reservations.draft_id, browser_schedule_reservations.scheduled_at, browser_schedule_reservations.status, browser_schedule_reservations.created_at, browser_schedule_reservations.committed_at, browser_schedule_reservations.released_at, browser_drafts.text FROM browser_schedule_reservations LEFT JOIN browser_drafts ON browser_drafts.id = browser_schedule_reservations.draft_id";

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
    pub integration_id: String,
    pub id: String,
    pub platform: String,
    pub kind: String,
    pub target_url: String,
    pub post_id: Option<String>,
    /// The whole post as one text; the parts joined when it is a thread.
    pub text: String,
    /// The posts that go out, in order. One entry for a plain post.
    pub parts: Vec<String>,
    /// The images this post was asked for. Each carries the part it attaches
    /// to; a plain post's own images carry part 0.
    pub images: Vec<DraftImage>,
    /// The post a quote quotes. Nothing is read off the page to ask about
    /// it, so its author and words are there only when an earlier read in
    /// this integration already saw that post.
    pub quoted: Option<QuotedPost>,
    /// Whether a failure should carry a screenshot and the page's HTML.
    pub debug: bool,
    pub status: String,
    pub created_at: i64,
    pub confirmed_at: Option<i64>,
    pub submitted_at: Option<i64>,
    pub scheduled_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotedPost {
    pub url: String,
    pub author: Option<String>,
    pub text: Option<String>,
}

/// One image a draft carries, as far as anything outside this store ever
/// needs to know: never the staged path underneath it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftImage {
    pub id: String,
    /// The post this image attaches to: an index into `parts` (or 0 for a
    /// plain post's single, implicit part).
    pub part_index: i64,
    /// This image's order among the others on the same part.
    pub ordinal: i64,
    pub content_type: String,
    pub bytes: i64,
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
    pub integration_id: String,
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
    pub integration_id: String,
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
    /// What the owner is being asked for: `post`, `reply`, `repost` or
    /// `quote`. Each but a post carries a post ID, so nothing else here
    /// tells them apart.
    pub kind: &'a str,
    pub target_url: &'a str,
    pub post_id: Option<&'a str>,
    pub text: &'a str,
    /// The posts of a thread, in order. Empty for a plain post.
    pub parts: &'a [String],
    /// Each part's own images: 0 to [`images::MAX_IMAGES`] absolute local
    /// PNG or JPEG paths per entry, staged as part of writing the draft.
    /// Exactly one entry per part, or exactly one entry (part 0) when
    /// `parts` is empty — a plain post's own images.
    pub parts_images: &'a [Vec<String>],
    /// Whether a failure should carry a screenshot and the page's HTML.
    pub debug: bool,
    /// How long the job its confirmation starts may run.
    pub ttl_ms: i64,
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
    integration_id: String,
    max_jobs: i64,
    max_artifacts: i64,
    images_dir: &'a Path,
}

impl Store {
    /// Borrow the browser tables. Blocks until the store lock is free.
    pub fn browser(&self) -> BrowserStore<'_> {
        self.browser_for(LEGACY_INTEGRATION_ID)
    }

    /// Borrow the browser tables for one integration.
    pub fn browser_for(&self, integration_id: &str) -> BrowserStore<'_> {
        BrowserStore {
            conn: self.conn.lock().expect("store lock"),
            integration_id: integration_id.to_owned(),
            max_jobs: MAX_JOBS,
            max_artifacts: MAX_ARTIFACTS,
            images_dir: &self.images_dir,
        }
    }
}

impl BrowserStore<'_> {
    pub fn recover_in_flight(&mut self, now: i64) -> Result<(), BrowserError> {
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'unknown', finished_at = ?, error_code = 'server_restarted', error_message = 'The server restarted before this job completed.' WHERE integration_id = ? AND status = 'running'",
            params![now, self.integration_id.as_str()],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'unknown' WHERE integration_id = ? AND status = 'confirmed' AND id IN (SELECT draft_id FROM browser_jobs WHERE integration_id = ? AND action IN ('submit_reply', 'submit_repost', 'submit_quote', 'submit_post') AND status = 'unknown' AND draft_id IS NOT NULL)",
            params![self.integration_id.as_str(), self.integration_id.as_str()],
        )?;
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = 'unknown' WHERE integration_id = ? AND status = 'reserved' AND draft_id IN (SELECT draft_id FROM browser_jobs WHERE integration_id = ? AND action IN ('submit_reply', 'submit_repost', 'submit_quote', 'submit_post') AND status = 'unknown' AND draft_id IS NOT NULL)",
            params![self.integration_id.as_str(), self.integration_id.as_str()],
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
        // A request carrying images is never folded into an older pending
        // draft: the two are not the same ask, even when the text matches.
        if input.parts_images.iter().any(|part| !part.is_empty()) {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT id, platform, kind, target_url, post_id, text, parts_json, status, created_at, confirmed_at, submitted_at, scheduled_at, debug, integration_id FROM browser_drafts WHERE integration_id = ? AND status = 'pending' AND platform = ? AND kind = ? AND target_url = ? AND post_id IS ? AND text = ? AND NOT EXISTS (SELECT 1 FROM browser_draft_images WHERE browser_draft_images.integration_id = browser_drafts.integration_id AND browser_draft_images.draft_id = browser_drafts.id) ORDER BY created_at DESC LIMIT 1",
                params![
                    self.integration_id.as_str(),
                    input.platform,
                    input.kind,
                    input.target_url,
                    input.post_id,
                    input.text
                ],
                read_draft_row,
            )
            .optional()?
            .map(|row| self.load_draft(row))
            .transpose()
    }

    /// Whether a post is on its way into the page right now: confirmed to go
    /// out immediately and not yet landed. One goes at a time.
    pub fn submission_in_flight(&mut self, now: i64) -> Result<bool, BrowserError> {
        self.expire_queued(now)?;
        self.conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM browser_jobs WHERE integration_id = ? AND action IN ('submit_post', 'submit_reply', 'submit_repost', 'submit_quote') AND status IN ('queued', 'running') AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = browser_jobs.integration_id AND browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.status = 'reserved' AND browser_schedule_reservations.scheduled_at > ?))",
                params![self.integration_id.as_str(), now],
                |row| row.get(0),
            )
            .map_err(BrowserError::from)
    }

    /// Write a post that is waiting on a person. Nothing is sent to the
    /// browser until someone confirms it.
    ///
    /// Any images are staged and validated first, outside the database
    /// transaction; if the draft insert that follows fails for any reason,
    /// bytes this call itself copied (and nothing another draft still
    /// references) are cleaned up before the error is returned.
    pub fn create_draft(
        &mut self,
        input: &DraftInput<'_>,
        now: i64,
    ) -> Result<Draft, BrowserError> {
        self.expire_drafts(now)?;
        self.prune_drafts()?;
        let expected_parts = input.parts.len().max(1);
        if input.parts_images.len() != expected_parts {
            return Err(BrowserError::InvalidData(
                "Each part needs exactly one images entry, even when it carries none.".to_owned(),
            ));
        }
        let mut staged: Vec<(usize, StagedImage)> = Vec::new();
        for (part_index, part_images) in input.parts_images.iter().enumerate() {
            match images::stage_images(self.images_dir, &self.integration_id, part_images) {
                Ok(part_staged) => {
                    staged.extend(part_staged.into_iter().map(|image| (part_index, image)));
                }
                Err(error) => {
                    for (_, image) in &staged {
                        self.drop_staged_if_unreferenced(&image.staged_path);
                    }
                    return Err(error);
                }
            }
        }
        let id = Uuid::new_v4().to_string();
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.create_draft_transaction(&id, input, &staged, now);
        match result {
            Ok(draft) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(draft)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                for (_, image) in &staged {
                    self.drop_staged_if_unreferenced(&image.staged_path);
                }
                Err(error)
            }
        }
    }

    fn create_draft_transaction(
        &mut self,
        id: &str,
        input: &DraftInput<'_>,
        staged: &[(usize, StagedImage)],
        now: i64,
    ) -> Result<Draft, BrowserError> {
        let parts_json = serde_json::to_string(input.parts)
            .map_err(|_| BrowserError::InvalidData("Thread parts could not be stored.".to_owned()))?;
        self.conn.execute(
            "INSERT INTO browser_drafts (id, platform, kind, target_url, post_id, text, parts_json, debug, status, created_at, integration_id, ttl_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)",
            params![
                id,
                input.platform,
                input.kind,
                input.target_url,
                input.post_id,
                input.text,
                parts_json,
                input.debug,
                now,
                self.integration_id.as_str(),
                input.ttl_ms
            ],
        )?;
        let mut next_ordinal: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
        for (part_index, image) in staged {
            let ordinal = next_ordinal.entry(*part_index).or_insert(0);
            self.conn.execute(
                "INSERT INTO browser_draft_images (id, integration_id, draft_id, part_index, ordinal, content_type, bytes, sha256, staged_path, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    Uuid::new_v4().to_string(),
                    self.integration_id.as_str(),
                    id,
                    *part_index as i64,
                    *ordinal,
                    image.content_type,
                    image.bytes,
                    image.sha256,
                    image.staged_path.to_string_lossy(),
                    now,
                ],
            )?;
            *ordinal += 1;
        }
        self.get_draft_row(id)?
            .map(|row| self.load_draft(row))
            .transpose()?
            .ok_or_else(|| {
                BrowserError::InvalidData("Created draft could not be read back.".to_owned())
            })
    }

    /// Delete a staged file only once nothing under this integration still
    /// references its path — another draft may have deduplicated onto the
    /// same content hash.
    fn drop_staged_if_unreferenced(&self, staged_path: &std::path::Path) {
        let path_text = staged_path.to_string_lossy().into_owned();
        let referenced: bool = self
            .conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM browser_draft_images WHERE integration_id = ? AND staged_path = ?)",
                params![self.integration_id.as_str(), path_text],
                |row| row.get(0),
            )
            .unwrap_or(true);
        if !referenced {
            images::remove_staged_file(self.images_dir, &self.integration_id, staged_path);
        }
    }

    pub fn create_job(&mut self, request: &JobInput<'_>, now: i64) -> Result<Job, BrowserError> {
        self.expire_drafts(now)?;
        self.expire_queued(now)?;
        self.prune_jobs()?;
        let id = Uuid::new_v4().to_string();
        let command_id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, integration_id) VALUES (?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
            params![
                id,
                command_id,
                request.platform,
                request.action,
                request.target_url,
                request.payload.to_string(),
                now,
                now + request.ttl_ms,
                self.integration_id.as_str()
            ],
        )?;
        self.get_job(&id, now)?.ok_or_else(|| {
            BrowserError::InvalidData("Created job could not be read back.".to_owned())
        })
    }

    pub fn claim_next(&mut self, now: i64) -> Result<Option<Job>, BrowserError> {
        self.expire_queued(now)?;
        let next: Option<String> = self.conn
            .query_row(
                "SELECT browser_jobs.id FROM browser_jobs WHERE browser_jobs.integration_id = ? AND browser_jobs.status = 'queued' AND browser_jobs.expires_at > ? AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = browser_jobs.integration_id AND browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.status IN ('reserved', 'unknown') AND (browser_schedule_reservations.status = 'unknown' OR browser_schedule_reservations.scheduled_at > ?)) ORDER BY browser_jobs.created_at ASC LIMIT 1",
                params![self.integration_id.as_str(), now, now],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = next else {
            return Ok(None);
        };
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'running', started_at = ?, dispatch_count = dispatch_count + 1 WHERE id = ? AND integration_id = ? AND status = 'queued'",
            params![now, id, self.integration_id.as_str()],
        )?;
        self.get_job(&id, now)
    }

    pub fn next_scheduled_job_at(&self, now: i64) -> Result<Option<i64>, BrowserError> {
        self.conn
            .query_row(
                "SELECT MIN(browser_schedule_reservations.scheduled_at) FROM browser_jobs JOIN browser_schedule_reservations ON browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.integration_id = browser_jobs.integration_id WHERE browser_jobs.integration_id = ? AND browser_jobs.action = 'submit_post' AND browser_jobs.status = 'queued' AND browser_jobs.expires_at > ? AND browser_schedule_reservations.status = 'reserved' AND browser_schedule_reservations.scheduled_at > ?",
                params![self.integration_id.as_str(), now, now],
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
            "UPDATE browser_jobs SET status = ?, finished_at = ?, error_code = ?, error_message = ?, result_json = ? WHERE id = ? AND command_id = ? AND integration_id = ? AND status = 'running'",
            params![
                completion.outcome,
                completion.now,
                completion.error.map(|value| value.code.as_str()),
                completion.error.map(|value| value.message.as_str()),
                completion.result.map(Value::to_string),
                completion.id,
                completion.command_id,
                self.integration_id.as_str(),
            ],
        )?;
        if is_submission(&action) {
            self.update_submission_status(&payload_json, completion.outcome, completion.now)?;
        }
        Ok(Completion { accepted: true })
    }

    /// Settle a job that ran past its deadline with the success the browser
    /// reported afterwards.
    ///
    /// Only a job still `unknown` because its deadline passed, under the same
    /// command, is touched: anything that has moved on since — a person
    /// resolving the post by hand, a later command — stays as it is. A late
    /// failure never lands here, since it cannot prove a write did not happen.
    pub fn reconcile_late_success(
        &mut self,
        id: &str,
        command_id: &str,
        result: &Value,
        now: i64,
    ) -> Result<bool, BrowserError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let outcome = (|| {
            let changed = self.conn.execute(
                "UPDATE browser_jobs SET status = 'succeeded', finished_at = ?, error_code = NULL, error_message = NULL, result_json = ? WHERE id = ? AND command_id = ? AND integration_id = ? AND status = 'unknown' AND error_code = 'deadline_exceeded'",
                params![now, result.to_string(), id, command_id, self.integration_id.as_str()],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            let Some((_, _, action, payload_json)) = self.job_identity(id)? else {
                return Ok(false);
            };
            if is_submission(&action) {
                let payload: Value = serde_json::from_str(&payload_json).map_err(|_| {
                    BrowserError::InvalidData("Stored submission payload is invalid.".to_owned())
                })?;
                if let Some(draft_id) = payload.get("draftId").and_then(Value::as_str) {
                    self.conn.execute(
                        "UPDATE browser_drafts SET status = 'submitted', submitted_at = ? WHERE integration_id = ? AND id = ? AND status = 'unknown'",
                        params![now, self.integration_id.as_str(), draft_id],
                    )?;
                    self.conn.execute(
                        "UPDATE browser_schedule_reservations SET status = 'committed', committed_at = ? WHERE integration_id = ? AND draft_id = ? AND status = 'unknown'",
                        params![now, self.integration_id.as_str(), draft_id],
                    )?;
                }
            }
            Ok(true)
        })();
        match outcome {
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
                    "UPDATE browser_jobs SET status = ?, finished_at = ?, error_code = ?, error_message = ? WHERE id = ? AND command_id = ? AND integration_id = ? AND status = 'running'",
                    params![
                        status,
                        now,
                        error.code,
                        error.message,
                        id,
                        command_id,
                        self.integration_id.as_str()
                    ],
                )?;
                if let Some((_, _, action, payload)) = row
                    && is_submission(&action)
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
        let has_images: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM browser_draft_images WHERE integration_id = ? AND draft_id = ?)",
            params![self.integration_id.as_str(), draft.id],
            |row| row.get(0),
        )?;
        if has_images && !IMAGE_UPLOAD_IMPLEMENTED {
            return Err(BrowserError::InvalidData(
                "This post's images can't be sent yet: image upload into the page isn't wired up. Discard it, or wait for that to land.".to_owned(),
            ));
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
                "INSERT INTO browser_schedule_reservations (id, integration_id, draft_id, platform, scheduled_at, status, created_at) VALUES (?, ?, ?, ?, ?, 'reserved', ?)",
                params![
                    Uuid::new_v4().to_string(),
                    self.integration_id.as_str(),
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
        let staged = if has_images {
            self.read_draft_image_refs(&draft.id)?
        } else {
            Vec::new()
        };
        let image_json = |part_index: usize| -> Value {
            serde_json::json!(
                staged
                    .iter()
                    .filter(|(part, _, _)| *part == part_index)
                    .map(|(_, image_id, content_type)| serde_json::json!({
                        "imageId": image_id,
                        "contentType": content_type,
                    }))
                    .collect::<Vec<_>>()
            )
        };
        let (action, mut payload) = if draft.kind == "post" || draft.kind == "quote" {
            let stored: Vec<String> = serde_json::from_str(&draft.parts_json).unwrap_or_default();
            let parts = if stored.is_empty() {
                vec![draft.text.clone()]
            } else {
                stored
            };
            // A quote goes out through the quoted post's own composer, so it
            // is a post that also names that post.
            let quote = draft.kind == "quote";
            let mut payload = serde_json::json!({
                "kind": if quote { "quote_submission" } else { "post_submission" },
                "draftId": draft.id,
                "text": draft.text,
                "parts": parts.clone(),
            });
            if quote {
                payload["postId"] = serde_json::json!(draft.post_id);
            }
            if has_images {
                // Every part carries its own list, empty ones included, so
                // the extension knows exactly which of its composers get
                // nothing rather than guessing from a shorter array.
                payload["partImages"] = serde_json::json!(
                    (0..parts.len()).map(image_json).collect::<Vec<_>>()
                );
            }
            (if quote { "submit_quote" } else { "submit_post" }, payload)
        } else if draft.kind == "repost" {
            (
                "submit_repost",
                serde_json::json!({
                    "kind": "repost_submission",
                    "draftId": draft.id,
                    "postId": draft.post_id,
                }),
            )
        } else {
            let mut payload = serde_json::json!({
                "kind": "submission",
                "draftId": draft.id,
                "postId": draft.post_id,
                "text": draft.text,
            });
            if has_images {
                payload["images"] = image_json(0);
            }
            ("submit_reply", payload)
        };
        if draft.debug {
            payload["debug"] = serde_json::Value::Bool(true);
        }
        let ttl_ms: i64 = self.conn.query_row(
            "SELECT COALESCE(ttl_ms, ?) FROM browser_drafts WHERE id = ? AND integration_id = ?",
            params![DEFAULT_JOB_TTL_MS, draft.id, self.integration_id.as_str()],
            |row| row.get(0),
        )?;
        let expires_at = scheduled_at
            .and_then(|value| value.checked_add(ttl_ms))
            .unwrap_or(now + ttl_ms);
        self.conn.execute(
            "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, draft_id, integration_id) VALUES (?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?)",
            params![
                job_id,
                command_id,
                draft.platform,
                action,
                draft.target_url,
                payload.to_string(),
                now,
                expires_at,
                draft.id,
                self.integration_id.as_str()
            ],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'confirmed', confirmed_at = ?, scheduled_at = ? WHERE id = ? AND integration_id = ? AND status = 'pending'",
            params![now, scheduled_at, draft.id, self.integration_id.as_str()],
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
            "UPDATE browser_drafts SET status = 'cancelled' WHERE integration_id = ? AND id = ? AND status = 'pending'",
            params![self.integration_id.as_str(), draft_id],
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
            "SELECT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.draft_id = ? AND browser_schedule_reservations.status = 'reserved')",
            params![self.integration_id.as_str(), draft_id],
            |row| row.get(0),
        )?;
        if !reserved {
            return Ok(None);
        }
        let dropped = self.conn.execute(
            "DELETE FROM browser_jobs WHERE integration_id = ? AND draft_id = ? AND status = 'queued'",
            params![self.integration_id.as_str(), draft_id],
        )?;
        if dropped == 0 {
            return Ok(None);
        }
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = 'released', released_at = ? WHERE integration_id = ? AND draft_id = ? AND status = 'reserved'",
            params![now, self.integration_id.as_str(), draft_id],
        )?;
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'cancelled' WHERE integration_id = ? AND id = ? AND status = 'confirmed'",
            params![self.integration_id.as_str(), draft_id],
        )?;
        self.conn
            .query_row(
                &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.draft_id = ?"),
                params![self.integration_id.as_str(), draft_id],
                read_schedule_reservation,
            )
            .optional()
            .map_err(BrowserError::from)
    }

    pub fn get_job(&mut self, id: &str, now: i64) -> Result<Option<Job>, BrowserError> {
        self.expire_queued(now)?;
        let row = self.conn.query_row(
            "SELECT id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, started_at, finished_at, error_code, error_message, result_json, dispatch_count, draft_id, integration_id FROM browser_jobs WHERE id = ? AND integration_id = ?",
            params![id, self.integration_id.as_str()],
            read_job_row,
        ).optional()?;
        row.map(|row| self.load_job(row)).transpose()
    }

    pub fn list_jobs(&mut self, limit: i64, now: i64) -> Result<Vec<Job>, BrowserError> {
        self.expire_queued(now)?;
        let bounded_limit = limit.clamp(1, 100);
        let mut statement = self.conn.prepare(
            "SELECT id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, started_at, finished_at, error_code, error_message, result_json, dispatch_count, draft_id, integration_id FROM browser_jobs WHERE integration_id = ? ORDER BY created_at DESC LIMIT ?",
        )?;
        let rows = statement.query_map(
            params![self.integration_id.as_str(), bounded_limit],
            read_job_row,
        )?;
        let rows: Result<Vec<JobRow>, rusqlite::Error> = rows.collect();
        drop(statement);
        rows.map_err(BrowserError::from)?
            .into_iter()
            .map(|value| self.load_job(value))
            .collect()
    }

    pub fn get_draft(&mut self, id: &str) -> Result<Option<Draft>, BrowserError> {
        self.expire_drafts(now_millis())?;
        self.get_draft_row(id)?.map(|row| self.load_draft(row)).transpose()
    }

    /// Every draft still waiting on a confirmation, newest first.
    pub fn list_pending_drafts(&mut self, now: i64) -> Result<Vec<Draft>, BrowserError> {
        self.expire_drafts(now)?;
        let mut statement = self.conn.prepare(
            "SELECT id, platform, kind, target_url, post_id, text, parts_json, status, created_at, confirmed_at, submitted_at, scheduled_at, debug, integration_id FROM browser_drafts WHERE integration_id = ? AND status = 'pending' ORDER BY created_at DESC",
        )?;
        let rows: Vec<DraftRow> = statement
            .query_map([self.integration_id.as_str()], read_draft_row)?
            .collect::<Result<_, _>>()?;
        drop(statement);
        rows.into_iter().map(|row| self.load_draft(row)).collect()
    }

    /// A draft's own row plus the images it was asked with, in order.
    fn load_draft(&self, row: DraftRow) -> Result<Draft, BrowserError> {
        let images = self.read_draft_images(&row.id)?;
        let quoted = match (row.kind.as_str(), &row.post_id) {
            ("quote", Some(post_id)) => Some(self.quoted_post(&row.platform, post_id, &row.target_url)?),
            _ => None,
        };
        Ok(to_draft(row, images, quoted)?)
    }

    /// The quoted post as the newest successful read that saw it shows it,
    /// or only its address when no read did.
    fn quoted_post(&self, platform: &str, post_id: &str, url: &str) -> Result<QuotedPost, BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT result_json FROM browser_jobs WHERE integration_id = ? AND platform = ? AND status = 'succeeded' AND result_json LIKE ? ORDER BY finished_at DESC LIMIT 10",
        )?;
        let results = statement
            .query_map(
                params![
                    self.integration_id.as_str(),
                    platform,
                    format!("%\"postId\":\"{post_id}\"%"),
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let seen = results
            .iter()
            .filter_map(|json| serde_json::from_str::<Value>(json).ok())
            .find_map(|result| {
                std::iter::once(&result["post"])
                    .chain(result["posts"].as_array().into_iter().flatten())
                    .find(|post| post["postId"] == post_id)
                    .cloned()
            });
        let field = |name: &str| {
            seen.as_ref()
                .and_then(|post| post[name].as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Ok(QuotedPost {
            url: url.to_owned(),
            author: field("author"),
            text: field("text"),
        })
    }

    fn read_draft_images(&self, draft_id: &str) -> Result<Vec<DraftImage>, BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT id, part_index, ordinal, content_type, bytes FROM browser_draft_images WHERE integration_id = ? AND draft_id = ? ORDER BY part_index ASC, ordinal ASC",
        )?;
        let rows = statement.query_map(params![self.integration_id.as_str(), draft_id], |row| {
            Ok(DraftImage {
                id: row.get(0)?,
                part_index: row.get(1)?,
                ordinal: row.get(2)?,
                content_type: row.get(3)?,
                bytes: row.get(4)?,
            })
        })?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    /// The staged file each of a draft's images actually lives at, grouped by
    /// part and in order within it, for building the submission job's
    /// payload. Never handed back outside this store.
    /// Each of a draft's images by part, in order: its id (for the extension
    /// to fetch the bytes over its own authenticated connection — never a
    /// local path, which a browser tab cannot read anyway) and content type.
    fn read_draft_image_refs(
        &self,
        draft_id: &str,
    ) -> Result<Vec<(usize, String, String)>, BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT part_index, id, content_type FROM browser_draft_images WHERE integration_id = ? AND draft_id = ? ORDER BY part_index ASC, ordinal ASC",
        )?;
        let rows = statement.query_map(params![self.integration_id.as_str(), draft_id], |row| {
            Ok((row.get::<_, i64>(0)? as usize, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    /// The exact bytes and content type of one image a draft was approved
    /// with — the same snapshot staging captured, never the live source
    /// file. Scoped to this integration and that one draft; an id that does
    /// not name an image on it, however it spells a path, reads nothing.
    pub fn read_draft_image(
        &self,
        draft_id: &str,
        image_id: &str,
    ) -> Result<Option<(String, Vec<u8>)>, BrowserError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT content_type, staged_path FROM browser_draft_images WHERE integration_id = ? AND draft_id = ? AND id = ?",
                params![self.integration_id.as_str(), draft_id, image_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((content_type, staged_path)) = row else {
            return Ok(None);
        };
        let bytes = std::fs::read(&staged_path).map_err(|error| {
            BrowserError::InvalidData(format!("Could not read staged image: {error}"))
        })?;
        Ok(Some((content_type, bytes)))
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
            "{SELECT_RESERVATION} WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.status IN ('reserved', 'committed', 'unknown', 'released') AND (browser_schedule_reservations.status IN ('reserved', 'unknown') OR browser_schedule_reservations.scheduled_at >= ?) ORDER BY browser_schedule_reservations.scheduled_at ASC LIMIT ?",
        ))?;
        let rows = statement.query_map(
            params![self.integration_id.as_str(), now, bounded_limit],
            read_schedule_reservation,
        )?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    pub fn schedule_summary(&self) -> Result<ScheduleSummary, BrowserError> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN browser_schedule_reservations.status = 'reserved' THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN browser_schedule_reservations.status = 'unknown' THEN 1 ELSE 0 END), 0) FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = ?",
                [self.integration_id.as_str()],
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
                    &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.draft_id = ? AND browser_schedule_reservations.status = 'unknown'"),
                    params![self.integration_id.as_str(), draft_id],
                    read_schedule_reservation,
                )
                .optional()?;
            let Some(reservation) = reservation else {
                return Ok(None);
            };
            let status = if published { "committed" } else { "released" };
            self.conn.execute(
                "UPDATE browser_schedule_reservations SET status = ?, committed_at = ?, released_at = ? WHERE integration_id = ? AND draft_id = ? AND status = 'unknown'",
                params![
                    status,
                    published.then_some(now),
                    (!published).then_some(now),
                    self.integration_id.as_str(),
                    draft_id,
                ],
            )?;
            self.conn.execute(
                "UPDATE browser_drafts SET status = ?, submitted_at = ? WHERE integration_id = ? AND id = ? AND status = 'unknown'",
                params![
                    if published { "submitted" } else { "failed" },
                    published.then_some(now),
                    self.integration_id.as_str(),
                    draft_id,
                ],
            )?;
            self.conn
                .query_row(
                    &format!("{SELECT_RESERVATION} WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.id = ?"),
                    params![self.integration_id.as_str(), reservation.id],
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
            "SELECT id, job_id, kind, content_type, bytes, created_at, expires_at FROM browser_artifacts WHERE id = ? AND expires_at > ? AND EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?)",
            params![id, now_millis(), self.integration_id.as_str()],
            read_artifact,
        ).optional().map_err(BrowserError::from)
    }

    pub fn get_artifact_body(&mut self, id: &str) -> Result<Option<ArtifactBody>, BrowserError> {
        self.conn.query_row(
            "SELECT id, job_id, kind, content_type, bytes, data, created_at, expires_at FROM browser_artifacts WHERE id = ? AND expires_at > ? AND EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?)",
            params![id, now_millis(), self.integration_id.as_str()],
            read_artifact_body,
        ).optional().map_err(BrowserError::from)
    }

    /// The extract a job's page handed back, if any. A job that uploads a
    /// debug capture writes a second `extract` artifact after this one, so
    /// this reads the earliest to reach for the one the driver actually
    /// produced as its result.
    pub fn extract_artifact(&mut self, job_id: &str) -> Result<Option<ArtifactBody>, BrowserError> {
        self.conn.query_row(
            "SELECT id, job_id, kind, content_type, bytes, data, created_at, expires_at FROM browser_artifacts WHERE job_id = ? AND kind = 'extract' AND expires_at > ? AND EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?) ORDER BY created_at ASC LIMIT 1",
            params![job_id, now_millis(), self.integration_id.as_str()],
            read_artifact_body,
        ).optional().map_err(BrowserError::from)
    }

    pub fn count_queued(&mut self, now: i64) -> Result<i64, BrowserError> {
        self.expire_queued(now)?;
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM browser_jobs WHERE integration_id = ? AND status = 'queued'",
                [self.integration_id.as_str()],
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
            integration_id: row.integration_id,
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
            "SELECT id, job_id, kind, content_type, bytes, created_at, expires_at FROM browser_artifacts WHERE job_id = ? AND expires_at > ? AND EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?) ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(
            params![job_id, now_millis(), self.integration_id.as_str()],
            read_artifact,
        )?;
        rows.map(|row| row.map_err(BrowserError::from)).collect()
    }

    fn get_draft_row(&self, id: &str) -> Result<Option<DraftRow>, BrowserError> {
        self.conn.query_row(
            "SELECT id, platform, kind, target_url, post_id, text, parts_json, status, created_at, confirmed_at, submitted_at, scheduled_at, debug, integration_id FROM browser_drafts WHERE id = ? AND integration_id = ?",
            params![id, self.integration_id.as_str()],
            read_draft_row,
        ).optional().map_err(BrowserError::from)
    }

    fn latest_schedule_for_platform(
        &self,
        platform: &str,
        now: i64,
    ) -> Result<Option<i64>, BrowserError> {
        let has_uncertain: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.platform = ? AND browser_schedule_reservations.status = 'unknown')",
            params![self.integration_id.as_str(), platform],
            |row| row.get(0),
        )?;
        if has_uncertain {
            return Err(BrowserError::ScheduleBlocked);
        }
        self.conn
            .query_row(
                "SELECT MAX(browser_schedule_reservations.scheduled_at) FROM browser_schedule_reservations WHERE browser_schedule_reservations.integration_id = ? AND browser_schedule_reservations.platform = ? AND browser_schedule_reservations.status IN ('reserved', 'committed') AND browser_schedule_reservations.scheduled_at > ?",
                params![self.integration_id.as_str(), platform, now],
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
                "SELECT command_id, status, action, payload_json FROM browser_jobs WHERE id = ? AND integration_id = ?",
                params![id, self.integration_id.as_str()],
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
            "UPDATE browser_drafts SET status = ?, submitted_at = ? WHERE integration_id = ? AND id = ? AND status = 'confirmed'",
            params![
                status,
                (outcome == "succeeded").then_some(now),
                self.integration_id.as_str(),
                draft_id
            ],
        )?;
        let reservation_status = match outcome {
            "succeeded" => "committed",
            "unknown" => "unknown",
            _ => "released",
        };
        self.conn.execute(
            "UPDATE browser_schedule_reservations SET status = ?, committed_at = ?, released_at = ? WHERE integration_id = ? AND draft_id = ? AND status = 'reserved'",
            params![
                reservation_status,
                (outcome == "succeeded").then_some(now),
                (outcome != "succeeded" && outcome != "unknown").then_some(now),
                self.integration_id.as_str(),
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
                "UPDATE browser_drafts SET status = 'unknown' WHERE integration_id = ? AND id = ? AND status = 'confirmed'",
                params![self.integration_id.as_str(), draft_id],
            )?;
            self.conn.execute(
                "UPDATE browser_schedule_reservations SET status = 'unknown' WHERE integration_id = ? AND draft_id = ? AND status = 'reserved'",
                params![self.integration_id.as_str(), draft_id],
            )?;
        }
        Ok(())
    }

    fn expire_queued(&self, now: i64) -> Result<(), BrowserError> {
        let mut statement = self.conn.prepare(
            "SELECT action, payload_json FROM browser_jobs WHERE integration_id = ? AND status = 'queued' AND expires_at <= ?",
        )?;
        let expired: Vec<(String, String)> = statement
            .query_map(params![self.integration_id.as_str(), now], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        self.conn.execute(
            "UPDATE browser_jobs SET status = 'expired', finished_at = ?, error_code = 'expired', error_message = 'The job expired before it started.' WHERE integration_id = ? AND status = 'queued' AND expires_at <= ?",
            params![now, self.integration_id.as_str(), now],
        )?;
        for (action, payload) in expired {
            if is_submission(&action) {
                self.update_submission_status(&payload, "expired", now)?;
            }
        }
        Ok(())
    }

    fn expire_drafts(&self, now: i64) -> Result<(), BrowserError> {
        self.conn.execute(
            "UPDATE browser_drafts SET status = 'expired' WHERE integration_id = ? AND status = 'pending' AND created_at <= ?",
            params![self.integration_id.as_str(), now.saturating_sub(DRAFT_TTL_MS)],
        )?;
        Ok(())
    }

    fn prune_drafts(&self) -> Result<(), BrowserError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM browser_drafts WHERE integration_id = ?",
                [self.integration_id.as_str()],
                |row| row.get(0),
            )?;
        if count < self.max_jobs {
            return Ok(());
        }
        let remove = count - self.max_jobs + 1;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let candidate_ids: Vec<String> = {
            let mut statement = tx.prepare(
                "SELECT id FROM browser_drafts WHERE integration_id = ? AND status IN ('submitted', 'failed', 'cancelled', 'expired') AND NOT EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.draft_id = browser_drafts.id AND (browser_jobs.integration_id != browser_drafts.integration_id OR browser_jobs.status IN ('queued', 'running', 'unknown'))) AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.draft_id = browser_drafts.id AND browser_schedule_reservations.status IN ('reserved', 'unknown')) ORDER BY created_at ASC LIMIT ?",
            )?;
            let rows = statement.query_map(params![self.integration_id.as_str(), remove], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        if (candidate_ids.len() as i64) < remove {
            return Err(BrowserError::Full);
        }
        let mut staged_paths = Vec::new();
        for id in &candidate_ids {
            let mut statement = tx.prepare(
                "SELECT staged_path FROM browser_draft_images WHERE integration_id = ? AND draft_id = ?",
            )?;
            staged_paths.extend(
                statement
                    .query_map(params![self.integration_id.as_str(), id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?,
            );
            tx.execute(
                "DELETE FROM browser_jobs WHERE integration_id = ? AND draft_id = ?",
                params![self.integration_id.as_str(), id],
            )?;
            tx.execute(
                "DELETE FROM browser_drafts WHERE integration_id = ? AND id = ?",
                params![self.integration_id.as_str(), id],
            )?;
        }
        tx.commit()?;
        for path in staged_paths {
            self.drop_staged_if_unreferenced(Path::new(&path));
        }
        Ok(())
    }

    fn prune_jobs(&self) -> Result<(), BrowserError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM browser_jobs WHERE integration_id = ?",
                [self.integration_id.as_str()],
                |row| row.get(0),
            )?;
        if count < self.max_jobs {
            return Ok(());
        }
        let remove = count - self.max_jobs + 1;
        self.conn.execute(
            "DELETE FROM browser_jobs WHERE integration_id = ? AND id IN (SELECT browser_jobs.id FROM browser_jobs WHERE browser_jobs.integration_id = ? AND (browser_jobs.status IN ('succeeded', 'failed', 'expired') OR (browser_jobs.status = 'unknown' AND browser_jobs.action NOT IN ('submit_post', 'submit_reply', 'submit_repost', 'submit_quote'))) AND NOT EXISTS (SELECT 1 FROM browser_drafts WHERE browser_drafts.id = browser_jobs.draft_id AND browser_drafts.status IN ('pending', 'confirmed', 'unknown')) AND NOT EXISTS (SELECT 1 FROM browser_schedule_reservations WHERE browser_schedule_reservations.draft_id = browser_jobs.draft_id AND browser_schedule_reservations.status IN ('reserved', 'unknown')) ORDER BY browser_jobs.created_at ASC LIMIT ?)",
            params![self.integration_id.as_str(), self.integration_id.as_str(), remove],
        )?;
        let after: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM browser_jobs WHERE integration_id = ?",
                [self.integration_id.as_str()],
                |row| row.get(0),
            )?;
        if after >= self.max_jobs {
            return Err(BrowserError::Full);
        }
        Ok(())
    }

    fn prune_artifacts(&self, now: i64) -> Result<(), BrowserError> {
        crate::purge_expired_artifacts(&self.conn, now)?;
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM browser_artifacts WHERE EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?)",
            [self.integration_id.as_str()],
            |row| row.get(0),
        )?;
        if count < self.max_artifacts {
            return Ok(());
        }
        let remove = count - self.max_artifacts + 1;
        self.conn.execute(
            "DELETE FROM browser_artifacts WHERE id IN (SELECT browser_artifacts.id FROM browser_artifacts WHERE EXISTS (SELECT 1 FROM browser_jobs WHERE browser_jobs.id = browser_artifacts.job_id AND browser_jobs.integration_id = ?) ORDER BY browser_artifacts.created_at ASC LIMIT ?)",
            params![self.integration_id.as_str(), remove],
        )?;
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
    integration_id: String,
}

struct DraftRow {
    id: String,
    platform: String,
    kind: String,
    target_url: String,
    post_id: Option<String>,
    text: String,
    parts_json: String,
    status: String,
    created_at: i64,
    confirmed_at: Option<i64>,
    submitted_at: Option<i64>,
    scheduled_at: Option<i64>,
    debug: bool,
    integration_id: String,
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
        integration_id: row.get(16)?,
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
        parts_json: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        confirmed_at: row.get(9)?,
        submitted_at: row.get(10)?,
        scheduled_at: row.get(11)?,
        debug: row.get(12)?,
        integration_id: row.get(13)?,
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

/// Matches the column order every `ArtifactBody` query selects in:
/// metadata columns, then `data`, then the remaining metadata columns.
fn read_artifact_body(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactBody> {
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
}

fn to_draft(
    row: DraftRow,
    images: Vec<DraftImage>,
    quoted: Option<QuotedPost>,
) -> rusqlite::Result<Draft> {
    Ok(Draft {
        integration_id: row.integration_id,
        id: row.id,
        platform: row.platform,
        kind: row.kind,
        target_url: row.target_url,
        post_id: row.post_id,
        text: row.text,
        parts: serde_json::from_str(&row.parts_json).unwrap_or_default(),
        images,
        quoted,
        debug: row.debug,
        status: row.status,
        created_at: row.created_at,
        confirmed_at: row.confirmed_at,
        submitted_at: row.submitted_at,
        scheduled_at: row.scheduled_at,
    })
}

fn read_schedule_reservation(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleReservation> {
    Ok(ScheduleReservation {
        integration_id: row.get(0)?,
        id: row.get(1)?,
        draft_id: row.get(2)?,
        scheduled_at: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        committed_at: row.get(6)?,
        released_at: row.get(7)?,
        text: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
    })
}

/// Whether a job action is one that sends an approved draft into the page,
/// and so settles that draft when it finishes.
fn is_submission(action: &str) -> bool {
    matches!(
        action,
        "submit_reply" | "submit_repost" | "submit_quote" | "submit_post"
    )
}

pub(crate) fn now_millis() -> i64 {
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
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text,
                    parts: &[],
                    parts_images: &[vec![]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
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
                    kind: "reply",
                    target_url: "https://x.com/status/42",
                    post_id: Some("42"),
                    text,
                    parts: &[],
                    parts_images: &[vec![]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                now,
            )
            .unwrap()
    }

    fn seed_history(store: &BrowserStore<'_>, count: i64) {
        store.conn.execute_batch("BEGIN").unwrap();
        for i in 0..count {
            let id = format!("{}-{i}", store.integration_id);
            store.conn.execute(
                "INSERT INTO browser_drafts (id, platform, kind, target_url, text, status, created_at, integration_id) VALUES (?, 'x', 'post', '', '', 'submitted', ?, ?)",
                params![id, i, store.integration_id],
            ).unwrap();
            store.conn.execute(
                "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, draft_id, integration_id) VALUES (?, ?, 'x', 'submit_post', '', '{}', 'succeeded', ?, ?, ?, ?)",
                params![id, id, i, i, id, store.integration_id],
            ).unwrap();
        }
        store.conn.execute_batch("COMMIT").unwrap();
    }

    #[test]
    fn full_settled_history_admits_another_draft_and_keeps_other_owners() {
        let (_dir, database) = crate::testing::temp_store();
        seed_history(&database.browser_for("other"), 1);
        let mut store = database.browser_for("owner");
        seed_history(&store, MAX_JOBS);
        store.conn.execute(
            "INSERT INTO browser_artifacts (id, job_id, kind, content_type, bytes, data, created_at, expires_at) VALUES ('artifact', 'owner-0', 'screenshot', 'image/png', 1, X'00', 0, 1)",
            [],
        ).unwrap();
        let draft = prepare_post_draft(&mut store, "Next post", now_millis());
        assert_eq!(draft.status, "pending");
        assert!(store.job_identity("owner-0").unwrap().is_none());
        assert!(store.get_draft_row("owner-0").unwrap().is_none());
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM browser_artifacts", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM browser_drafts WHERE integration_id = 'owner'", [], |row| row.get::<_, i64>(0)).unwrap(), MAX_JOBS);
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM browser_jobs WHERE integration_id = 'other'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        assert!(store.consume_draft(&draft.id, now_millis(), false).unwrap().is_some());
    }

    #[test]
    fn history_eviction_preserves_unresolved_jobs_drafts_and_reservations() {
        let (_dir, database) = crate::testing::temp_store();
        let mut store = database.browser_for("owner");
        store.max_jobs = 3;
        seed_history(&store, 3);
        store.conn.execute("UPDATE browser_jobs SET status = 'unknown' WHERE id = 'owner-0'", []).unwrap();
        store.conn.execute("UPDATE browser_drafts SET status = 'unknown' WHERE id = 'owner-1'", []).unwrap();
        store.conn.execute(
            "INSERT INTO browser_schedule_reservations (id, draft_id, platform, scheduled_at, status, created_at, integration_id) VALUES ('reservation', 'owner-2', 'x', 1, 'unknown', 0, 'owner')",
            [],
        ).unwrap();
        assert!(matches!(store.prune_drafts(), Err(BrowserError::Full)));
        assert!(matches!(store.prune_jobs(), Err(BrowserError::Full)));
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM browser_jobs", [], |row| row.get::<_, i64>(0)).unwrap(), 3);
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM browser_drafts", [], |row| row.get::<_, i64>(0)).unwrap(), 3);
        store.conn.execute("UPDATE browser_schedule_reservations SET status = 'committed' WHERE id = 'reservation'", []).unwrap();
        store.prune_drafts().unwrap();
        assert!(store.job_identity("owner-2").unwrap().is_none());
        assert!(store.job_identity("owner-0").unwrap().is_some());
        assert!(store.get_draft_row("owner-1").unwrap().is_some());
    }

    #[test]
    fn history_eviction_rolls_back_jobs_if_draft_deletion_fails() {
        let (_dir, database) = crate::testing::temp_store();
        let mut store = database.browser_for("owner");
        store.max_jobs = 1;
        seed_history(&store, 1);
        store.conn.execute_batch("CREATE TRIGGER keep_draft BEFORE DELETE ON browser_drafts BEGIN SELECT RAISE(ABORT, 'keep draft'); END;").unwrap();
        assert!(store.prune_drafts().is_err());
        assert!(store.job_identity("owner-0").unwrap().is_some());
        assert!(store.get_draft_row("owner-0").unwrap().is_some());
        assert!(store.conn.is_autocommit());
    }

    #[test]
    fn expiry_respects_boundary_status_and_integration() {
        let (_dir, database) = crate::testing::temp_store();
        let store = database.browser_for("owner");
        seed_history(&store, 3);
        store.conn.execute_batch(
            "UPDATE browser_jobs SET status = 'queued', expires_at = 100;
             UPDATE browser_jobs SET expires_at = 101 WHERE id = 'owner-1';
             UPDATE browser_jobs SET status = 'running' WHERE id = 'owner-2';
             UPDATE browser_drafts SET status = 'pending', created_at = 0;
             UPDATE browser_drafts SET created_at = 1 WHERE id = 'owner-1';
             UPDATE browser_drafts SET integration_id = 'other' WHERE id = 'owner-2';",
        ).unwrap();
        store.expire_queued(100).unwrap();
        assert_eq!(store.job_identity("owner-0").unwrap().unwrap().1, "expired");
        assert_eq!(store.job_identity("owner-1").unwrap().unwrap().1, "queued");
        assert_eq!(store.job_identity("owner-2").unwrap().unwrap().1, "running");
        store.expire_drafts(DRAFT_TTL_MS).unwrap();
        assert_eq!(store.get_draft_row("owner-0").unwrap().unwrap().status, "expired");
        assert_eq!(store.get_draft_row("owner-1").unwrap().unwrap().status, "pending");
        let other: String = store.conn.query_row("SELECT status FROM browser_drafts WHERE id = 'owner-2'", [], |row| row.get(0)).unwrap();
        assert_eq!(other, "pending");
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
            kind: "post",
            target_url: "https://x.com/compose/post",
            post_id: None,
            text: "Same words",
            parts: &[],
            parts_images: &[vec![]],
            debug: false,
            ttl_ms: DEFAULT_JOB_TTL_MS,
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
    fn a_thread_keeps_its_parts_and_hands_them_to_the_submission() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let parts = vec!["First post.".to_owned(), "Second post.".to_owned()];
        let draft = store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text: "First post.\n\nSecond post.",
                    parts: &parts,
                    parts_images: &[vec![], vec![]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        assert_eq!(draft.parts, parts);
        let submission = store.consume_draft(&draft.id, 100, false).unwrap().unwrap();
        assert_eq!(submission.job.payload["parts"], serde_json::json!(parts));
        let plain = prepare_post_draft(&mut store, "Just one", 100);
        assert!(plain.parts.is_empty());
    }

    /// A quote shows the post it quotes as an earlier read saw it, and goes
    /// out as a post that also names that post.
    #[test]
    fn a_quote_draft_shows_the_quoted_post_and_submits_through_it() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let input = DraftInput {
            platform: "x",
            kind: "quote",
            target_url: "https://x.com/i/status/42",
            post_id: Some("42"),
            text: "Worth reading.",
            parts: &[],
            parts_images: &[vec![]],
            debug: false,
            ttl_ms: DEFAULT_JOB_TTL_MS,
        };
        let now = now_millis();
        let unseen = store.create_draft(&input, now).unwrap();
        assert_eq!(
            unseen.quoted,
            Some(QuotedPost {
                url: "https://x.com/i/status/42".to_owned(),
                author: None,
                text: None,
            })
        );

        store
            .conn
            .execute(
                "INSERT INTO browser_jobs (id, command_id, platform, action, target_url, payload_json, status, created_at, expires_at, finished_at, result_json, integration_id) VALUES ('read-1', 'command-1', 'x', 'read_post', 'https://x.com/i/status/42', '{}', 'succeeded', 50, 60000, 60, ?, ?)",
                params![
                    serde_json::json!({
                        "kind": "x_post",
                        "post": { "postId": "42", "author": "Owner @owner", "text": "The original." },
                    })
                    .to_string(),
                    store.integration_id.as_str(),
                ],
            )
            .unwrap();
        let seen = store.get_draft(&unseen.id).unwrap().unwrap();
        let quoted = seen.quoted.clone().unwrap();
        assert_eq!(quoted.author.as_deref(), Some("Owner @owner"));
        assert_eq!(quoted.text.as_deref(), Some("The original."));
        assert!(!seen.can_queue());

        let submission = store.consume_draft(&unseen.id, now, false).unwrap().unwrap();
        assert_eq!(submission.job.action, "submit_quote");
        assert_eq!(
            submission.job.payload,
            serde_json::json!({
                "kind": "quote_submission",
                "draftId": unseen.id,
                "postId": "42",
                "text": "Worth reading.",
                "parts": ["Worth reading."],
            })
        );
    }

    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    fn write_png(dir: &std::path::Path, name: &str, pixels: &[u8]) -> String {
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(pixels);
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn approved_images_are_staged_once_and_survive_a_source_edit() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let sources = tempdir().unwrap();
        let path_a = write_png(sources.path(), "a.png", b"pixels-a");
        let path_b = write_png(sources.path(), "b.png", b"pixels-a");

        // Two drafts asking for the same bytes, from different files, land on
        // one staged file.
        let first = store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text: "First",
                    parts: &[],
                    parts_images: &[vec![path_a.clone()]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        assert_eq!(first.images.len(), 1);
        assert_eq!(first.images[0].content_type, "image/png");
        let second = store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text: "Second",
                    parts: &[],
                    parts_images: &[vec![path_b]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        fn staged_files(store: &BrowserStore<'_>) -> usize {
            std::fs::read_dir(store.images_dir.join(&store.integration_id))
                .map(|entries| entries.count())
                .unwrap_or(0)
        }
        assert_eq!(staged_files(&store), 1);

        // Editing the original file after staging must never reach the bytes
        // either draft was approved with, even once confirmed.
        std::fs::write(&path_a, b"tampered").unwrap();
        store.consume_draft(&second.id, 100, false).unwrap();
        assert_eq!(staged_files(&store), 1);
        let (_, bytes) = store.read_draft_image(&second.id, &second.images[0].id).unwrap().unwrap();
        let mut expected = PNG_MAGIC.to_vec();
        expected.extend_from_slice(b"pixels-a");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn confirming_a_draft_carries_each_parts_own_images_on_the_submission_job() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let sources = tempdir().unwrap();
        let path_first = write_png(sources.path(), "a.png", b"pixels-first");
        let path_third_a = write_png(sources.path(), "c1.png", b"pixels-third-a");
        let path_third_b = write_png(sources.path(), "c2.png", b"pixels-third-b");
        let parts = vec!["First.".to_owned(), "Second.".to_owned(), "Third.".to_owned()];
        let draft = store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text: "First.\n\nSecond.\n\nThird.",
                    parts: &parts,
                    parts_images: &[
                        vec![path_first],
                        vec![],
                        vec![path_third_a, path_third_b],
                    ],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        assert_eq!(draft.images.iter().filter(|image| image.part_index == 0).count(), 1);
        assert_eq!(draft.images.iter().filter(|image| image.part_index == 1).count(), 0);
        assert_eq!(draft.images.iter().filter(|image| image.part_index == 2).count(), 2);

        let submission = store.consume_draft(&draft.id, 100, false).unwrap().unwrap();
        let part_images = submission.job.payload["partImages"].as_array().unwrap();
        // One entry per part, every part's own images and nothing more —
        // an empty middle part stays a real, present empty array.
        assert_eq!(part_images.len(), 3);
        assert_eq!(part_images[0].as_array().unwrap().len(), 1);
        assert_eq!(part_images[1].as_array().unwrap().len(), 0);
        assert_eq!(part_images[2].as_array().unwrap().len(), 2);
        assert_eq!(part_images[0][0]["contentType"], "image/png");
        // Never a local path a browser tab could not read anyway — an id
        // the extension fetches the bytes for over its own connection.
        assert_eq!(
            part_images[0][0]["imageId"],
            serde_json::json!(draft.images.iter().find(|image| image.part_index == 0).unwrap().id),
        );
        assert!(part_images[0][0].get("path").is_none());
    }

    #[test]
    fn an_invalid_image_is_rejected_and_leaves_no_draft_behind() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let sources = tempdir().unwrap();
        let path = sources.path().join("not-an-image.png");
        std::fs::write(&path, b"just text").unwrap();
        let result = store.create_draft(
            &DraftInput {
                platform: "x",
                kind: "post",
                target_url: "https://x.com/compose/post",
                post_id: None,
                text: "Bad image",
                parts: &[],
                parts_images: &[vec![path.to_string_lossy().into_owned()]],
                debug: false,
                ttl_ms: DEFAULT_JOB_TTL_MS,
            },
            100,
        );
        assert!(result.is_err());
        assert!(store.list_pending_drafts(100).unwrap().is_empty());
    }

    #[test]
    fn a_draft_image_reads_back_only_for_its_own_draft_and_id() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let mut store = database.browser();
        let sources = tempdir().unwrap();
        let path = write_png(sources.path(), "a.png", b"pixels");
        let draft = store
            .create_draft(
                &DraftInput {
                    platform: "x",
                    kind: "post",
                    target_url: "https://x.com/compose/post",
                    post_id: None,
                    text: "With an image",
                    parts: &[],
                    parts_images: &[vec![path]],
                    debug: false,
                    ttl_ms: DEFAULT_JOB_TTL_MS,
                },
                100,
            )
            .unwrap();
        let image_id = &draft.images[0].id;
        let (content_type, bytes) = store.read_draft_image(&draft.id, image_id).unwrap().unwrap();
        assert_eq!(content_type, "image/png");
        assert!(bytes.starts_with(&PNG_MAGIC));

        // Neither a wrong image id nor a wrong draft id reads anything back.
        assert!(store.read_draft_image(&draft.id, "not-a-real-id").unwrap().is_none());
        assert!(store.read_draft_image("not-a-real-draft", image_id).unwrap().is_none());
        let draft_id = draft.id.clone();
        let image_id = image_id.clone();
        drop(store);

        // Nor does another integration's own scope, even with the right ids.
        let other = database.browser_for("other-integration");
        assert!(other.read_draft_image(&draft_id, &image_id).unwrap().is_none());
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

    #[test]
    fn scheduled_reservations_are_isolated_between_integrations() {
        let directory = tempdir().unwrap();
        let database = open(&directory);
        let (first_job_id, first_draft_id) = {
            let mut first = database.browser_for("wande-1");
            let draft = prepare_post_draft(&mut first, "First post", 100);
            let submission = first.consume_draft(&draft.id, 100, true).unwrap().unwrap();
            let reservation = first
                .list_schedule_reservations(10, 100)
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(reservation.integration_id, "wande-1");
            (submission.job.id, draft.id)
        };

        let mut second = database.browser_for("wande-2");
        assert!(second.get_job(&first_job_id, 100).unwrap().is_none());
        assert!(second.get_draft(&first_draft_id).unwrap().is_none());
        assert!(
            second
                .list_schedule_reservations(10, 100)
                .unwrap()
                .is_empty()
        );
        assert_eq!(second.schedule_summary().unwrap().pending_reservations, 0);
        let own_draft = prepare_post_draft(&mut second, "Second post", 100);
        assert!(
            second
                .consume_draft(&own_draft.id, 100, true)
                .unwrap()
                .is_some()
        );
    }
}
