use serde_json::{Map, Value, json};
use url::Url;

use crate::thread::{MAX_THREAD_PARTS, X_POST_LIMIT, split_into_parts, weighted_length};

pub const PROTOCOL_VERSION: u64 = 1;
pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_SCREENSHOT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXTRACT_BYTES: usize = 256 * 1024;
/// A page's HTML attached to a failed job when the caller asked for debug output.
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_JOB_TTL_MS: i64 = 5 * 60 * 1000;
pub const MIN_JOB_TTL_MS: i64 = 1_000;
pub const DEFAULT_JOB_TTL_MS: i64 = 2 * 60 * 1000;
pub const MAX_TEXT_LENGTH: usize = 4_000;
pub const MAX_URL_LENGTH: usize = 2_048;
pub const MAX_ID_LENGTH: usize = 256;
pub const HEARTBEAT_INTERVAL_MS: i64 = 20_000;
pub const MAX_CLOCK_SKEW_MS: i64 = 30_000;

/// The one site Pluk drives. Kept as a type so the wire envelope still
/// carries a platform and a second site stays a variant away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    X,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "x" => Some(Self::X),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Inspect,
    ReadProfile,
    ReadPost,
    ReadFeed,
    ReadTrends,
    Refresh,
    Capture,
    Reply,
    SubmitReply,
    Post,
    SubmitPost,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::ReadProfile => "read_profile",
            Self::ReadPost => "read_post",
            Self::ReadFeed => "read_feed",
            Self::ReadTrends => "read_trends",
            Self::Refresh => "refresh",
            Self::Capture => "capture",
            Self::Reply => "reply",
            Self::SubmitReply => "submit_reply",
            Self::Post => "post",
            Self::SubmitPost => "submit_post",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "inspect" => Some(Self::Inspect),
            "read_profile" => Some(Self::ReadProfile),
            "read_post" => Some(Self::ReadPost),
            "read_feed" => Some(Self::ReadFeed),
            "read_trends" => Some(Self::ReadTrends),
            "refresh" => Some(Self::Refresh),
            "capture" => Some(Self::Capture),
            "reply" => Some(Self::Reply),
            "submit_reply" => Some(Self::SubmitReply),
            "post" => Some(Self::Post),
            "submit_post" => Some(Self::SubmitPost),
            _ => None,
        }
    }

    pub fn is_supported_on(self, platform: Platform) -> bool {
        match platform {
            Platform::X => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateJobRequest {
    pub platform: Platform,
    pub action: Action,
    pub target_url: String,
    pub payload: Value,
    pub ttl_ms: i64,
}

pub(crate) struct CommandInput<'a> {
    pub job_id: &'a str,
    pub command_id: &'a str,
    pub platform: Platform,
    pub action: Action,
    pub target_url: &'a str,
    pub issued_at: i64,
    pub expires_at: i64,
    pub payload: &'a Value,
}

#[derive(Clone, Debug)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ExtensionCapability {
    pub platform: Platform,
    pub capabilities: Vec<Action>,
}

#[derive(Clone, Debug)]
pub struct HelloMessage {
    pub capabilities: Vec<ExtensionCapability>,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct HeartbeatMessage {
    pub nonce: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct ResultMessage {
    pub job_id: String,
    pub command_id: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub succeeded: bool,
    pub data: Option<Value>,
    pub error: Option<ProtocolError>,
}

#[derive(Clone, Debug)]
pub enum ExtensionMessage {
    Hello(HelloMessage),
    Heartbeat(HeartbeatMessage),
    Result(ResultMessage),
}

#[derive(Clone, Debug)]
pub struct ValidationError {
    pub code: &'static str,
    pub message: String,
}

pub type ValidationResult<T> = Result<T, ValidationError>;

// Actions with one fixed destination: the caller supplies no target URL and
// the site's default is used. An explicit targetUrl is still accepted and
// validated normally.
pub(crate) fn fixed_feed_target(platform: Platform) -> &'static str {
    match platform {
        Platform::X => "https://x.com/home",
    }
}

pub(crate) fn fixed_trends_target(platform: Platform) -> &'static str {
    match platform {
        Platform::X => "https://x.com/explore",
    }
}

// x.post has one fixed destination and, unlike read_feed/read_trends,
// never accepts a caller-supplied override: there is no other page a new
// post could be composed on.
pub(crate) fn fixed_compose_target(platform: Platform) -> &'static str {
    match platform {
        Platform::X => "https://x.com/compose/post",
    }
}

// Path segments that collide with X's own feature pages, so they can never be
// a real handle even though they match the username pattern. Shared in spirit
// with the site driver, which applies the same check against the live page's
// URL (duplicated there because injected page scripts cannot reference
// outside module state).
fn is_reserved_profile_handle(value: &str) -> bool {
    const RESERVED: [&str; 9] = [
        "home",
        "explore",
        "notifications",
        "messages",
        "settings",
        "search",
        "compose",
        "login",
        "i",
    ];
    RESERVED.contains(&value.to_ascii_lowercase().as_str())
}

fn is_valid_profile_username(value: &str) -> bool {
    !value.is_empty()
        && !is_reserved_profile_handle(value)
        && value.len() <= 50
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn canonical_profile_url(username: &str) -> String {
    format!("https://x.com/{username}")
}

fn single_path_segment(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    let trimmed = rest.strip_suffix('/').unwrap_or(rest);
    (!trimmed.is_empty() && !trimmed.contains('/')).then_some(trimmed)
}

fn extract_profile_username(url: &Url) -> Option<String> {
    let segment = single_path_segment(url.path())?;
    is_valid_profile_username(segment).then(|| segment.to_owned())
}

fn is_valid_post_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn canonical_post_url(post_id: &str) -> String {
    format!("https://x.com/status/{post_id}")
}

// Digits embedded in a post page's own URL. Mirrors extract_profile_username:
// lets a caller pass either a full post URL or a bare post ID, with the other
// one derived.
fn extract_post_id(url: &Url) -> Option<String> {
    let rest = url.path().split("/status/").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}

fn resolve_post_target(
    platform: Platform,
    target_url: Option<&str>,
    payload: Option<&Value>,
) -> ValidationResult<(String, Value)> {
    let object = payload
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Payload must be an object."))?;
    if !has_only_keys(object, &["postId"])
        || object.get("postId").is_some_and(|value| !value.is_string())
    {
        return Err(invalid(
            "Read-post payload accepts only an optional postId field.",
        ));
    }
    let payload_post_id = object.get("postId").and_then(Value::as_str);
    match (target_url, payload_post_id) {
        (None, None) => Err(invalid("Provide a post URL or a post ID.")),
        (None, Some(post_id)) => {
            if !is_valid_post_id(post_id) {
                return Err(invalid("Enter a valid post ID for this site."));
            }
            let canonical = canonicalize_target_url(&canonical_post_url(post_id), platform)?;
            Ok((canonical, json!({ "kind": "read_post", "postId": post_id })))
        }
        (Some(url), post_id_field) => {
            let canonical = canonicalize_target_url(url, platform)?;
            let parsed = Url::parse(&canonical).map_err(|_| {
                invalid("Target URL is not a recognized post page for this site. Pass a post ID instead.")
            })?;
            let url_post_id = extract_post_id(&parsed).ok_or_else(|| {
                invalid("Target URL is not a recognized post page for this site. Pass a post ID instead.")
            })?;
            if let Some(post_id) = post_id_field
                && post_id != url_post_id
            {
                return Err(invalid(
                    "The post ID does not match the post visible at targetUrl.",
                ));
            }
            Ok((
                canonical,
                json!({ "kind": "read_post", "postId": url_post_id }),
            ))
        }
    }
}

fn resolve_fixed_destination_target(
    platform: Platform,
    action: Action,
    target_url: Option<&str>,
) -> ValidationResult<String> {
    let fallback = match target_url {
        Some(value) => return canonicalize_target_url(value, platform),
        None if action == Action::ReadTrends => fixed_trends_target(platform),
        None => fixed_feed_target(platform),
    };
    canonicalize_target_url(fallback, platform)
}

fn resolve_compose_target(
    platform: Platform,
    target_url: Option<&str>,
) -> ValidationResult<String> {
    if target_url.is_some() {
        return Err(invalid("This action does not accept a target URL."));
    }
    canonicalize_target_url(fixed_compose_target(platform), platform)
}

fn resolve_profile_target(
    platform: Platform,
    target_url: Option<&str>,
    payload: Option<&Value>,
) -> ValidationResult<(String, Value)> {
    let object = payload.and_then(Value::as_object);
    if let Some(object) = object {
        if let Some(username_value) = object.get("username") {
            if !has_only_keys(object, &["username"]) {
                return Err(invalid("Profile payload needs a single username field."));
            }
            let Some(username) = username_value.as_str() else {
                return Err(invalid("Profile payload needs a single username field."));
            };
            let username = username.strip_prefix('@').unwrap_or(username);
            if !is_valid_profile_username(username) {
                return Err(invalid("Enter a valid username for this site."));
            }
            return Ok((canonical_profile_url(username), json!({ "kind": "empty" })));
        }
        if !object.is_empty() {
            return Err(invalid("Profile payload needs a single username field."));
        }
    }
    // Legacy compatibility: a full profile URL in place of a username.
    let target_url = target_url.ok_or_else(|| invalid("Target URL is missing or too long."))?;
    let canonical = canonicalize_target_url(target_url, platform)?;
    let parsed = Url::parse(&canonical).map_err(|_| {
        invalid(
            "Target URL is not a recognized profile page for this site. Pass a username instead.",
        )
    })?;
    if extract_profile_username(&parsed).is_none() {
        return Err(invalid(
            "Target URL is not a recognized profile page for this site. Pass a username instead.",
        ));
    }
    Ok((canonical, json!({ "kind": "empty" })))
}

pub fn parse_create_job_request(value: &Value) -> ValidationResult<CreateJobRequest> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Job request has an unsupported shape."))?;
    if !has_only_keys(
        object,
        &["platform", "action", "targetUrl", "payload", "ttlMs"],
    ) {
        return Err(invalid("Job request has an unsupported shape."));
    }
    let platform = string_field(object, "platform")
        .and_then(Platform::from_str)
        .ok_or_else(|| invalid("Job request has an unsupported platform or action."))?;
    let action = string_field(object, "action")
        .and_then(Action::from_str)
        .filter(|action| *action != Action::SubmitReply && *action != Action::SubmitPost)
        .ok_or_else(|| invalid("Job request has an unsupported platform or action."))?;
    if !action.is_supported_on(platform) {
        return Err(ValidationError {
            code: "unsupported_action",
            message: "This driver does not support that action.".to_owned(),
        });
    }
    let target_url_field = object.get("targetUrl").and_then(Value::as_str);
    let (target_url, payload) = if action == Action::ReadProfile {
        resolve_profile_target(platform, target_url_field, object.get("payload"))?
    } else if action == Action::Post {
        let target_url = resolve_compose_target(platform, target_url_field)?;
        let payload = parse_public_payload(object.get("payload"), action)?;
        (target_url, payload)
    } else if action == Action::ReadPost {
        resolve_post_target(platform, target_url_field, object.get("payload"))?
    } else if action == Action::ReadFeed || action == Action::ReadTrends {
        let target_url = resolve_fixed_destination_target(platform, action, target_url_field)?;
        let payload = parse_public_payload(object.get("payload"), action)?;
        (target_url, payload)
    } else {
        let target_url = canonicalize_target_url(
            target_url_field.ok_or_else(|| invalid("Target URL is missing or too long."))?,
            platform,
        )?;
        let payload = parse_public_payload(object.get("payload"), action)?;
        (target_url, payload)
    };
    let ttl_ms = object
        .get("ttlMs")
        .map(number_as_i64)
        .transpose()
        .map_err(|_| invalid("Job expiry must be between one second and five minutes."))?
        .unwrap_or(DEFAULT_JOB_TTL_MS);
    if !(MIN_JOB_TTL_MS..=MAX_JOB_TTL_MS).contains(&ttl_ms) {
        return Err(invalid(
            "Job expiry must be between one second and five minutes.",
        ));
    }
    Ok(CreateJobRequest {
        platform,
        action,
        target_url,
        payload,
        ttl_ms,
    })
}

fn parse_public_payload(value: Option<&Value>, action: Action) -> ValidationResult<Value> {
    let mut object = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Payload must be an object."))?
        .clone();
    let debug = match object.remove("debug") {
        None => false,
        Some(Value::Bool(value)) => value,
        Some(_) => return Err(invalid("debug must be true or false.")),
    };
    if debug && !matches!(action, Action::Post | Action::Reply) {
        return Err(invalid("Only posts and replies take debug."));
    }
    let object = &object;
    if action == Action::Post {
        let parts = parse_post_parts(object)?;
        let text = if parts.len() == 1 {
            parts[0].clone()
        } else {
            parts.join("\n\n")
        };
        return Ok(with_debug(
            json!({ "kind": "compose", "text": text, "parts": parts }),
            debug,
        ));
    }
    if action == Action::Reply {
        if !has_only_keys(object, &["postId", "text"]) {
            return Err(invalid("Reply payload needs a post ID and exact text."));
        }
        let post_id = object
            .get("postId")
            .and_then(Value::as_str)
            .filter(|value| is_identifier(value))
            .ok_or_else(|| invalid("Reply payload needs a post ID and exact text."))?;
        let text = object
            .get("text")
            .and_then(Value::as_str)
            .filter(|value| is_string(value, MAX_TEXT_LENGTH))
            .ok_or_else(|| invalid("Reply payload needs a post ID and exact text."))?;
        if weighted_length(text) > X_POST_LIMIT {
            return Err(ValidationError {
                code: "too_long",
                message: format!(
                    "This reply weighs {} on X, over the {X_POST_LIMIT} limit. Links count 23. Shorten it.",
                    weighted_length(text)
                ),
            });
        }
        return Ok(with_debug(
            json!({ "kind": "reply", "postId": post_id, "text": text }),
            debug,
        ));
    }
    if !object.is_empty() {
        return Err(invalid("This action does not accept a payload."));
    }
    Ok(json!({ "kind": "empty" }))
}

/// Mark a payload whose failure should come back with a screenshot and the
/// page's HTML. Absent when not asked for, so the wire shape stays as before.
fn with_debug(mut payload: Value, debug: bool) -> Value {
    if debug {
        payload["debug"] = Value::Bool(true);
    }
    payload
}

fn debug_is_flag(object: &Map<String, Value>) -> bool {
    object.get("debug").is_none_or(Value::is_boolean)
}

/// The posts a compose request becomes: `thread` as given, or `text` cut
/// into posts that each fit X's limit.
fn parse_post_parts(object: &Map<String, Value>) -> ValidationResult<Vec<String>> {
    if has_only_keys(object, &["thread"]) {
        let parts: Vec<String> = object
            .get("thread")
            .and_then(Value::as_array)
            .filter(|parts| !parts.is_empty() && parts.len() <= MAX_THREAD_PARTS)
            .ok_or_else(|| invalid("A thread needs 1 to 25 posts, each a string."))?
            .iter()
            .map(|part| {
                part.as_str()
                    .filter(|value| is_string(value, MAX_TEXT_LENGTH))
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("A thread needs 1 to 25 posts, each a string."))
            })
            .collect::<Result<_, _>>()?;
        if let Some((index, part)) = parts
            .iter()
            .enumerate()
            .find(|(_, part)| weighted_length(part) > X_POST_LIMIT)
        {
            return Err(ValidationError {
                code: "too_long",
                message: format!(
                    "Post {} of the thread weighs {} on X, over the {X_POST_LIMIT} limit. Links count 23.",
                    index + 1,
                    weighted_length(part)
                ),
            });
        }
        return Ok(parts);
    }
    if !has_only_keys(object, &["text"]) {
        return Err(invalid("Post payload needs exact text, or a thread of posts."));
    }
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| is_string(value, MAX_TEXT_LENGTH))
        .ok_or_else(|| invalid("Post payload needs exact text, or a thread of posts."))?;
    split_into_parts(text).ok_or_else(|| ValidationError {
        code: "too_long",
        message: format!(
            "This text cannot be cut into posts under X's {X_POST_LIMIT} limit. Pass a thread of shorter posts."
        ),
    })
}

pub fn parse_command_envelope(value: &Value) -> ValidationResult<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Command envelope has an unsupported shape."))?;
    if !has_only_keys(
        object,
        &[
            "version",
            "type",
            "jobId",
            "commandId",
            "platform",
            "action",
            "targetUrl",
            "issuedAt",
            "expiresAt",
            "payload",
        ],
    ) || object.get("version") != Some(&json!(PROTOCOL_VERSION))
        || object.get("type").and_then(Value::as_str) != Some("command")
    {
        return Err(invalid("Command envelope has an unsupported shape."));
    }
    let job_id = object
        .get("jobId")
        .and_then(Value::as_str)
        .filter(|value| is_identifier(value))
        .ok_or_else(|| invalid("Command envelope has an unsupported shape."))?;
    let command_id = object
        .get("commandId")
        .and_then(Value::as_str)
        .filter(|value| is_identifier(value))
        .ok_or_else(|| invalid("Command envelope has an unsupported shape."))?;
    let platform = string_field(object, "platform")
        .and_then(Platform::from_str)
        .ok_or_else(|| invalid("Command envelope has an unsupported shape."))?;
    let action = string_field(object, "action")
        .and_then(Action::from_str)
        .ok_or_else(|| invalid("Command envelope has an unsupported shape."))?;
    if !action.is_supported_on(platform) {
        return Err(ValidationError {
            code: "unsupported_action",
            message: "Command action is not supported by this driver.".to_owned(),
        });
    }
    let target_url = canonicalize_target_url(
        object
            .get("targetUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Target URL is missing or too long."))?,
        platform,
    )?;
    let (issued_at, expires_at) = parse_envelope_times(object)?;
    let payload = parse_command_payload(object.get("payload"), action)?;
    let mut parsed = Map::new();
    parsed.insert("version".to_owned(), json!(PROTOCOL_VERSION));
    parsed.insert("type".to_owned(), json!("command"));
    parsed.insert("jobId".to_owned(), json!(job_id));
    parsed.insert("commandId".to_owned(), json!(command_id));
    parsed.insert("platform".to_owned(), json!(platform.as_str()));
    parsed.insert("action".to_owned(), json!(action.as_str()));
    parsed.insert("targetUrl".to_owned(), json!(target_url));
    parsed.insert("issuedAt".to_owned(), json!(issued_at));
    parsed.insert("expiresAt".to_owned(), json!(expires_at));
    parsed.insert("payload".to_owned(), payload);
    Ok(Value::Object(parsed))
}

fn parse_command_payload(value: Option<&Value>, action: Action) -> ValidationResult<Value> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Command payload must be an object."))?;
    if action == Action::Reply {
        if !has_only_keys(object, &["kind", "postId", "text"])
            || object.get("kind").and_then(Value::as_str) != Some("reply")
            || !object
                .get("postId")
                .and_then(Value::as_str)
                .is_some_and(is_identifier)
            || !object
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| is_string(value, MAX_TEXT_LENGTH))
        {
            return Err(invalid("Reply command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if action == Action::ReadPost {
        if !has_only_keys(object, &["kind", "postId"])
            || object.get("kind").and_then(Value::as_str) != Some("read_post")
            || !object
                .get("postId")
                .and_then(Value::as_str)
                .is_some_and(is_identifier)
        {
            return Err(invalid("Read-post command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if action == Action::Post {
        if !has_only_keys(object, &["kind", "text", "parts"])
            || object.get("kind").and_then(Value::as_str) != Some("compose")
            || !object
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| is_string(value, MAX_TEXT_LENGTH))
            || !is_thread(object.get("parts"))
        {
            return Err(invalid("Post command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if action == Action::SubmitPost {
        if !has_only_keys(object, &["kind", "draftId", "text", "parts", "debug"])
            || !debug_is_flag(object)
            || object.get("kind").and_then(Value::as_str) != Some("post_submission")
            || !object
                .get("draftId")
                .and_then(Value::as_str)
                .is_some_and(is_identifier)
            || !object
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| is_string(value, MAX_TEXT_LENGTH))
            || !is_thread(object.get("parts"))
        {
            return Err(invalid("Post submission command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if action == Action::SubmitReply {
        if !has_only_keys(object, &["kind", "draftId", "postId", "text", "debug"])
            || !debug_is_flag(object)
            || object.get("kind").and_then(Value::as_str) != Some("submission")
            || !object
                .get("draftId")
                .and_then(Value::as_str)
                .is_some_and(is_identifier)
            || !object
                .get("postId")
                .and_then(Value::as_str)
                .is_some_and(is_identifier)
            || !object
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| is_string(value, MAX_TEXT_LENGTH))
        {
            return Err(invalid("Submission command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if !has_only_keys(object, &["kind"])
        || object.get("kind").and_then(Value::as_str) != Some("empty")
    {
        return Err(invalid("This command does not accept a payload."));
    }
    Ok(value.cloned().unwrap_or(Value::Null))
}

pub fn parse_extension_message(value: &Value) -> ValidationResult<ExtensionMessage> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Message version or type is invalid."))?;
    if object.get("version") != Some(&json!(PROTOCOL_VERSION))
        || !object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| is_string(value, 32))
    {
        return Err(invalid("Message version or type is invalid."));
    }
    match object.get("type").and_then(Value::as_str) {
        Some("hello") => parse_hello(object).map(ExtensionMessage::Hello),
        Some("heartbeat") => parse_heartbeat(object).map(ExtensionMessage::Heartbeat),
        Some("result") => parse_result(object).map(ExtensionMessage::Result),
        _ => Err(invalid("Message type is not supported.")),
    }
}

fn parse_hello(object: &Map<String, Value>) -> ValidationResult<HelloMessage> {
    if !has_only_keys(
        object,
        &[
            "version",
            "type",
            "extensionVersion",
            "capabilities",
            "issuedAt",
            "expiresAt",
        ],
    ) || !object
        .get("extensionVersion")
        .and_then(Value::as_str)
        .is_some_and(|value| is_string(value, 64))
    {
        return Err(invalid("Hello message has an unsupported shape."));
    }
    let capabilities = parse_capabilities(object.get("capabilities"))?;
    let (issued_at, expires_at) = parse_envelope_times(object)?;
    Ok(HelloMessage {
        capabilities,
        issued_at,
        expires_at,
    })
}

fn parse_heartbeat(object: &Map<String, Value>) -> ValidationResult<HeartbeatMessage> {
    if !has_only_keys(
        object,
        &["version", "type", "nonce", "issuedAt", "expiresAt"],
    ) || !object
        .get("nonce")
        .and_then(Value::as_str)
        .is_some_and(is_identifier)
    {
        return Err(invalid("Heartbeat message has an unsupported shape."));
    }
    let (issued_at, expires_at) = parse_envelope_times(object)?;
    Ok(HeartbeatMessage {
        nonce: object
            .get("nonce")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        issued_at,
        expires_at,
    })
}

fn parse_result(object: &Map<String, Value>) -> ValidationResult<ResultMessage> {
    let job_id = object
        .get("jobId")
        .and_then(Value::as_str)
        .filter(|value| is_identifier(value))
        .ok_or_else(|| invalid("Result message has an unsupported shape."))?;
    let command_id = object
        .get("commandId")
        .and_then(Value::as_str)
        .filter(|value| is_identifier(value))
        .ok_or_else(|| invalid("Result message has an unsupported shape."))?;
    let (issued_at, expires_at) = parse_envelope_times(object)?;
    if object.get("outcome").and_then(Value::as_str) == Some("succeeded") {
        if !has_only_keys(
            object,
            &[
                "version",
                "type",
                "jobId",
                "commandId",
                "issuedAt",
                "expiresAt",
                "outcome",
                "data",
            ],
        ) {
            return Err(invalid("Successful result needs bounded data."));
        }
        let data = parse_result_data(object.get("data"))?;
        return Ok(ResultMessage {
            job_id: job_id.to_owned(),
            command_id: command_id.to_owned(),
            issued_at,
            expires_at,
            succeeded: true,
            data: Some(data),
            error: None,
        });
    }
    if object.get("outcome").and_then(Value::as_str) == Some("failed") {
        if !has_only_keys(
            object,
            &[
                "version",
                "type",
                "jobId",
                "commandId",
                "issuedAt",
                "expiresAt",
                "outcome",
                "error",
            ],
        ) {
            return Err(invalid("Failed result has an unsupported shape."));
        }
        return Ok(ResultMessage {
            job_id: job_id.to_owned(),
            command_id: command_id.to_owned(),
            issued_at,
            expires_at,
            succeeded: false,
            data: None,
            error: Some(parse_protocol_error(object.get("error"))?),
        });
    }
    Err(invalid("Result message has an unsupported shape."))
}

fn parse_capabilities(value: Option<&Value>) -> ValidationResult<Vec<ExtensionCapability>> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Extension capabilities must be a bounded list."))?;
    if list.len() > 5 {
        return Err(invalid("Extension capabilities must be a bounded list."));
    }
    let mut result = Vec::new();
    let mut seen = Vec::new();
    for item in list {
        let object = item
            .as_object()
            .ok_or_else(|| invalid("Extension capabilities contain an unsupported platform."))?;
        if !has_only_keys(object, &["platform", "capabilities"]) {
            return Err(invalid(
                "Extension capabilities contain an unsupported platform.",
            ));
        }
        let platform = object
            .get("platform")
            .and_then(Value::as_str)
            .and_then(Platform::from_str)
            .ok_or_else(|| invalid("Extension capabilities contain an unsupported platform."))?;
        if seen.contains(&platform) {
            return Err(invalid(
                "Extension capabilities contain an unsupported platform.",
            ));
        }
        seen.push(platform);
        let actions = object
            .get("capabilities")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("Extension capabilities contain an unsupported action."))?;
        if actions.len() > 11 {
            return Err(invalid(
                "Extension capabilities contain an unsupported action.",
            ));
        }
        let mut parsed_actions = Vec::new();
        for item in actions {
            let action = item
                .as_str()
                .and_then(Action::from_str)
                .ok_or_else(|| invalid("Extension capabilities contain an unsupported action."))?;
            if !action.is_supported_on(platform) || parsed_actions.contains(&action) {
                return Err(invalid(
                    "Extension capabilities exceed the driver contract.",
                ));
            }
            parsed_actions.push(action);
        }
        result.push(ExtensionCapability {
            platform,
            capabilities: parsed_actions,
        });
    }
    Ok(result)
}

fn parse_result_data(value: Option<&Value>) -> ValidationResult<Value> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Result data is missing or exceeds its bounds."))?;
    if !object
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|value| is_string(value, 64))
        || !is_bounded_json(value.unwrap_or(&Value::Null), 0)
    {
        return Err(invalid("Result data is missing or exceeds its bounds."));
    }
    if object.get("kind").and_then(Value::as_str) == Some("scheduled_submission")
        || object.contains_key("scheduledAt")
    {
        return Err(invalid(
            "Result data contains an unsupported scheduling field.",
        ));
    }
    Ok(Value::Object(object.clone()))
}

fn parse_protocol_error(value: Option<&Value>) -> ValidationResult<ProtocolError> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Result error has an unsupported shape."))?;
    if !has_only_keys(object, &["code", "message"])
        || !object
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|value| is_string(value, 64))
        || !object
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|value| is_string(value, 512))
    {
        return Err(invalid("Result error has an unsupported shape."));
    }
    Ok(ProtocolError {
        code: object
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        message: object
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

fn parse_envelope_times(object: &Map<String, Value>) -> ValidationResult<(i64, i64)> {
    let issued_at = object
        .get("issuedAt")
        .and_then(number_as_i64_opt)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("Envelope has an unsupported expiry."))?;
    let expires_at = object
        .get("expiresAt")
        .and_then(number_as_i64_opt)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("Envelope has an unsupported expiry."))?;
    if expires_at <= issued_at || expires_at - issued_at > MAX_JOB_TTL_MS {
        return Err(invalid("Envelope has an unsupported expiry."));
    }
    Ok((issued_at, expires_at))
}

pub fn canonicalize_target_url(value: &str, platform: Platform) -> ValidationResult<String> {
    if !is_string(value, MAX_URL_LENGTH) {
        return Err(ValidationError {
            code: "invalid_target",
            message: "Target URL is missing or too long.".to_owned(),
        });
    }
    let mut url = Url::parse(value).map_err(|_| ValidationError {
        code: "invalid_target",
        message: "Target URL must be valid HTTPS.".to_owned(),
    })?;
    let hostname = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || (url.port().is_some() && url.port() != Some(443))
        || !hostnames(platform).contains(&hostname.as_str())
        || url.fragment().is_some()
    {
        return Err(ValidationError {
            code: "invalid_target",
            message: "Target URL must use an exact supported HTTPS host.".to_owned(),
        });
    }
    let _ = url.set_host(Some(&hostname));
    if url.port() == Some(443) {
        let _ = url.set_port(None);
    }
    Ok(url.to_string())
}

pub fn make_ready_envelope(connection_id: &str, now: i64) -> Value {
    let capabilities = [
        Action::Inspect,
        Action::ReadProfile,
        Action::ReadPost,
        Action::ReadFeed,
        Action::ReadTrends,
        Action::Refresh,
        Action::Capture,
        Action::SubmitReply,
        Action::SubmitPost,
    ]
    .map(Action::as_str);
    json!({
        "version": PROTOCOL_VERSION,
        "type": "ready",
        "connectionId": connection_id,
        "heartbeatIntervalMs": HEARTBEAT_INTERVAL_MS,
        "contracts": [{
            "platform": Platform::X.as_str(),
            "hostnames": hostnames(Platform::X),
            "capabilities": capabilities,
        }],
        "issuedAt": now,
        "expiresAt": now + HEARTBEAT_INTERVAL_MS,
    })
}

pub fn make_heartbeat(now: i64) -> Value {
    let nonce = uuid::Uuid::new_v4().to_string();
    json!({
        "version": PROTOCOL_VERSION,
        "type": "heartbeat",
        "nonce": nonce,
        "issuedAt": now,
        "expiresAt": now + HEARTBEAT_INTERVAL_MS,
    })
}

pub fn make_heartbeat_ack(nonce: &str, now: i64) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "type": "heartbeat_ack",
        "nonce": nonce,
        "issuedAt": now,
        "expiresAt": now + HEARTBEAT_INTERVAL_MS,
    })
}

pub(crate) fn make_command(input: CommandInput<'_>) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "type": "command",
        "jobId": input.job_id,
        "commandId": input.command_id,
        "platform": input.platform.as_str(),
        "action": input.action.as_str(),
        "targetUrl": input.target_url,
        "issuedAt": input.issued_at,
        "expiresAt": input.expires_at,
        "payload": input.payload,
    })
}

pub fn hostnames(platform: Platform) -> Vec<&'static str> {
    match platform {
        Platform::X => vec!["x.com", "www.x.com", "twitter.com", "www.twitter.com"],
    }
}

pub fn is_allowed_extension_origin(origin: &str) -> bool {
    let prefix = "chrome-extension://";
    origin
        .strip_prefix(prefix)
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
}

/// The posts of a thread on the wire: one to 25 bounded strings.
fn is_thread(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|parts| {
        !parts.is_empty()
            && parts.len() <= MAX_THREAD_PARTS
            && parts
                .iter()
                .all(|part| part.as_str().is_some_and(|value| is_string(value, MAX_TEXT_LENGTH)))
    })
}

pub fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn has_only_keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn is_string(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_length
        && !value.chars().any(|character| {
            let code = character as u32;
            (code < 0x20 && code != 0x09 && code != 0x0a && code != 0x0d) || code == 0x7f
        })
}

fn number_as_i64(value: &Value) -> Result<i64, ()> {
    value.as_i64().ok_or(())
}

fn number_as_i64_opt(value: &Value) -> Option<i64> {
    value.as_i64()
}

fn invalid(message: &str) -> ValidationError {
    ValidationError {
        code: "invalid_schema",
        message: message.to_owned(),
    }
}

fn is_bounded_json(value: &Value, depth: usize) -> bool {
    if depth > 6 {
        return false;
    }
    match value {
        Value::Null | Value::Bool(_) => true,
        Value::Number(number) => number.is_f64() || number.is_i64() || number.is_u64(),
        Value::String(value) => value.chars().count() <= MAX_EXTRACT_BYTES,
        Value::Array(values) => {
            values.len() <= 100 && values.iter().all(|item| is_bounded_json(item, depth + 1))
        }
        Value::Object(object) => {
            object.len() <= 64
                && object.iter().all(|(key, item)| {
                    key.chars().count() <= 128 && is_bounded_json(item, depth + 1)
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_supported_target_rules() {
        assert_eq!(
            canonicalize_target_url("https://x.com:443/status/42", Platform::X).unwrap(),
            "https://x.com/status/42"
        );
        assert!(canonicalize_target_url("https://x.com.evil/status/42", Platform::X).is_err());
        assert!(canonicalize_target_url("http://x.com/status/42", Platform::X).is_err());
        assert!(canonicalize_target_url("https://x.com/home#top", Platform::X).is_err());
    }

    #[test]
    fn a_zero_input_feed_request_resolves_to_the_fixed_feed_target() {
        let feed = parse_create_job_request(&json!({
            "platform": "x", "action": "read_feed", "payload": {}
        }))
        .unwrap();
        assert_eq!(feed.target_url, "https://x.com/home");

        let trends = parse_create_job_request(&json!({
            "platform": "x", "action": "read_trends", "payload": {}
        }))
        .unwrap();
        assert_eq!(trends.target_url, "https://x.com/explore");
    }

    #[test]
    fn a_profile_read_resolves_a_username_to_the_canonical_profile_url() {
        let by_handle = parse_create_job_request(&json!({
            "platform": "x", "action": "read_profile", "payload": { "username": "@jack" }
        }))
        .unwrap();
        assert_eq!(by_handle.target_url, "https://x.com/jack");

        let legacy_url = parse_create_job_request(&json!({
            "platform": "x", "action": "read_profile",
            "targetUrl": "https://x.com/jack", "payload": {}
        }))
        .unwrap();
        assert_eq!(legacy_url.target_url, "https://x.com/jack");
    }

    #[test]
    fn rejects_invalid_profile_usernames_and_reserved_handles() {
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_profile",
                "payload": { "username": "not a handle" }
            }))
            .is_err()
        );
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_profile", "payload": {}
            }))
            .is_err()
        );
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_profile",
                "targetUrl": "https://x.com/explore", "payload": {}
            }))
            .is_err()
        );
    }

    #[test]
    fn a_post_read_accepts_either_a_url_or_a_post_id_and_derives_the_other() {
        let url_only = parse_create_job_request(&json!({
            "platform": "x", "action": "read_post",
            "targetUrl": "https://x.com/status/42", "payload": {}
        }))
        .unwrap();
        assert_eq!(url_only.target_url, "https://x.com/status/42");
        assert_eq!(url_only.payload["postId"], "42");

        let post_id_only = parse_create_job_request(&json!({
            "platform": "x", "action": "read_post", "payload": { "postId": "42" }
        }))
        .unwrap();
        assert_eq!(post_id_only.target_url, "https://x.com/status/42");
    }

    #[test]
    fn rejects_a_post_read_with_neither_input_or_a_mismatched_pair() {
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_post", "payload": {}
            }))
            .is_err()
        );
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_post",
                "targetUrl": "https://x.com/status/42", "payload": { "postId": "999" }
            }))
            .is_err()
        );
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "read_post",
                "targetUrl": "https://x.com/explore", "payload": {}
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_every_platform_but_x() {
        for platform in ["linkedin", "instagram", "gmail", "tiktok"] {
            assert!(
                parse_create_job_request(&json!({
                    "platform": platform, "action": "read_feed", "payload": {}
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn composing_refuses_a_caller_supplied_target() {
        let composed = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "text": "Exact text" }
        }))
        .unwrap();
        assert_eq!(composed.target_url, "https://x.com/compose/post");
        assert!(
            parse_create_job_request(&json!({
                "platform": "x", "action": "post",
                "targetUrl": "https://x.com/compose/post", "payload": { "text": "Exact text" }
            }))
            .is_err()
        );
    }

    #[test]
    fn a_post_request_becomes_parts_and_a_thread_is_taken_as_given() {
        let single = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "text": "Short." }
        }))
        .unwrap();
        assert_eq!(single.payload["parts"], json!(["Short."]));

        let thread = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "thread": ["One.", "Two."] }
        }))
        .unwrap();
        assert_eq!(thread.payload["parts"], json!(["One.", "Two."]));
        assert_eq!(thread.payload["text"], "One.\n\nTwo.");

        let over = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "thread": ["x".repeat(281)] }
        }))
        .unwrap_err();
        assert_eq!(over.code, "too_long");

        let long_reply = parse_create_job_request(&json!({
            "platform": "x", "action": "reply", "targetUrl": "https://x.com/status/42",
            "payload": { "postId": "42", "text": "word ".repeat(70) }
        }))
        .unwrap_err();
        assert_eq!(long_reply.code, "too_long");
    }

    #[test]
    fn a_job_request_cannot_ask_for_a_submission_directly() {
        for action in ["submit_post", "submit_reply"] {
            assert!(
                parse_create_job_request(&json!({
                    "platform": "x", "action": action,
                    "targetUrl": "https://x.com/compose/post", "payload": {}
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn accepts_immediate_post_submission_payload_without_schedule() {
        let now = 1_000;
        let envelope = json!({
            "version": 1,
            "type": "command",
            "jobId": "job-1",
            "commandId": "command-1",
            "platform": "x",
            "action": "submit_post",
            "targetUrl": "https://x.com/compose/post",
            "issuedAt": now,
            "expiresAt": now + 60_000,
            "payload": {
                "kind": "post_submission",
                "draftId": "draft-1",
                "text": "Hello",
                "parts": ["Hello"]
            }
        });
        assert!(parse_command_envelope(&envelope).is_ok());
        let mut invalid = envelope;
        invalid["payload"]["scheduledAt"] = Value::Null;
        assert!(parse_command_envelope(&invalid).is_err());
    }

    #[test]
    fn rejects_the_removed_native_scheduling_result() {
        let message = json!({
            "version": 1,
            "type": "result",
            "jobId": "job-1",
            "commandId": "command-1",
            "issuedAt": 100,
            "expiresAt": 200,
            "outcome": "succeeded",
            "data": {
                "kind": "scheduled_submission",
                "platform": "x",
                "accountIdentity": "@owner",
                "scheduledAt": 1_000,
                "scheduledNotification": "Your post was scheduled."
            }
        });
        assert!(parse_extension_message(&message).is_err());
    }

    #[test]
    fn parses_correlated_result() {
        let result = parse_extension_message(&json!({
            "version": 1,
            "type": "result",
            "jobId": "job-1",
            "commandId": "command-1",
            "issuedAt": 100,
            "expiresAt": 200,
            "outcome": "succeeded",
            "data": { "kind": "page", "title": "Example" }
        }))
        .unwrap();
        assert!(matches!(
            result,
            ExtensionMessage::Result(ResultMessage {
                succeeded: true,
                ..
            })
        ));
    }
}
