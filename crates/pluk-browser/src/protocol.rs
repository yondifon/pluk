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
pub const DEFAULT_JOB_TTL_MS: i64 = pluk_store::browser::DEFAULT_JOB_TTL_MS;
pub const MAX_TEXT_LENGTH: usize = 4_000;
pub const MAX_URL_LENGTH: usize = 2_048;
pub const MAX_ID_LENGTH: usize = 256;
/// How many local images a post or reply can carry, and how long a staged
/// path is ever allowed to be. Mirrors [`pluk_store::browser::images`], the
/// layer that actually stages and validates them.
pub const MAX_IMAGES: usize = pluk_store::browser::images::MAX_IMAGES;
pub const MAX_IMAGE_PATH_LENGTH: usize = pluk_store::browser::images::MAX_SOURCE_PATH_LENGTH;
/// The longest array a result's JSON can carry anywhere in its tree. Sized
/// to the Instagram grid's own 600-entry cap, the largest of the driver's
/// result lists.
pub const MAX_RESULT_ARRAY_LEN: usize = 600;
pub const MAX_DEBUG_GLOB_LEN: usize = 200;
pub const HEARTBEAT_INTERVAL_MS: i64 = 20_000;
pub const MAX_CLOCK_SKEW_MS: i64 = 30_000;

/// The sites Pluk drives. Kept as a type so the wire envelope always
/// carries a platform and adding a site stays a variant away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    X,
    Instagram,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Instagram => "instagram",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "x" => Some(Self::X),
            "instagram" => Some(Self::Instagram),
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
            // Instagram has no feed, trends, or compose surface: it reads
            // profiles and posts and takes screenshots, nothing else.
            Platform::Instagram => matches!(
                self,
                Self::Inspect | Self::ReadProfile | Self::ReadPost | Self::Refresh | Self::Capture
            ),
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
        // is_supported_on rejects read_feed for Instagram before a caller
        // ever reaches this, so there is no real destination to fake here.
        Platform::Instagram => unreachable!("Instagram does not support read_feed"),
    }
}

pub(crate) fn fixed_trends_target(platform: Platform) -> &'static str {
    match platform {
        Platform::X => "https://x.com/explore",
        Platform::Instagram => unreachable!("Instagram does not support read_trends"),
    }
}

// x.post has one fixed destination and, unlike read_feed/read_trends,
// never accepts a caller-supplied override: there is no other page a new
// post could be composed on.
pub(crate) fn fixed_compose_target(platform: Platform) -> &'static str {
    match platform {
        Platform::X => "https://x.com/compose/post",
        Platform::Instagram => unreachable!("Instagram does not support post"),
    }
}

// Path segments that collide with a platform's own feature pages, so they
// can never be a real handle even though they match the username pattern.
// Shared in spirit with the site driver, which applies the same check
// against the live page's URL (duplicated there because injected page
// scripts cannot reference outside module state).
fn is_reserved_profile_handle(platform: Platform, value: &str) -> bool {
    let reserved: &[&str] = match platform {
        Platform::X => &[
            "home",
            "explore",
            "notifications",
            "messages",
            "settings",
            "search",
            "compose",
            "login",
            "i",
        ],
        // "p" and "reel" are Instagram's own post routes; a profile handle
        // there would collide with a post target's first path segment.
        Platform::Instagram => &["p", "reel"],
    };
    reserved.contains(&value.to_ascii_lowercase().as_str())
}

fn is_valid_profile_username(platform: Platform, value: &str) -> bool {
    if value.is_empty() || is_reserved_profile_handle(platform, value) {
        return false;
    }
    match platform {
        Platform::X => {
            value.len() <= 50
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }
        Platform::Instagram => {
            value.len() <= 30
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.')
        }
    }
}

fn canonical_profile_url(platform: Platform, username: &str) -> String {
    match platform {
        Platform::X => format!("https://x.com/{username}"),
        Platform::Instagram => format!("https://www.instagram.com/{username}/"),
    }
}

fn single_path_segment(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    let trimmed = rest.strip_suffix('/').unwrap_or(rest);
    (!trimmed.is_empty() && !trimmed.contains('/')).then_some(trimmed)
}

fn extract_profile_username(platform: Platform, url: &Url) -> Option<String> {
    let segment = single_path_segment(url.path())?;
    is_valid_profile_username(platform, segment).then(|| segment.to_owned())
}

fn is_valid_post_id(platform: Platform, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    match platform {
        Platform::X => value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit()),
        // Instagram shortcode: 1 to 30 characters of A-Za-z0-9_-.
        Platform::Instagram => {
            value.len() <= 30
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        }
    }
}

fn canonical_post_url(platform: Platform, post_id: &str) -> String {
    match platform {
        Platform::X => format!("https://x.com/i/status/{post_id}"),
        Platform::Instagram => format!("https://www.instagram.com/p/{post_id}/"),
    }
}

// The post's own identifier embedded in a post page's URL. Mirrors
// extract_profile_username: lets a caller pass either a full post URL or a
// bare post ID, with the other one derived.
fn extract_post_id(platform: Platform, url: &Url) -> Option<String> {
    match platform {
        Platform::X => {
            let rest = url.path().split("/status/").nth(1)?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty()).then_some(digits)
        }
        // A post's shortcode sits right after a `p` or `reel` segment, either
        // bare (/p/<shortcode>/) or with the author's handle in front
        // (/<username>/p/<shortcode>/), which is the shape Instagram's own
        // post grid links use.
        Platform::Instagram => {
            let segments: Vec<&str> = url.path_segments()?.collect();
            let shortcode = match segments.as_slice() {
                [kind, shortcode, ..] if *kind == "p" || *kind == "reel" => *shortcode,
                [_username, kind, shortcode, ..] if *kind == "p" || *kind == "reel" => *shortcode,
                _ => return None,
            };
            is_valid_post_id(platform, shortcode).then(|| shortcode.to_owned())
        }
    }
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
            if !is_valid_post_id(platform, post_id) {
                return Err(invalid("Enter a valid post ID for this site."));
            }
            let canonical =
                canonicalize_target_url(&canonical_post_url(platform, post_id), platform)?;
            Ok((canonical, json!({ "kind": "read_post", "postId": post_id })))
        }
        (Some(url), post_id_field) => {
            let canonical = canonicalize_target_url(url, platform)?;
            let parsed = Url::parse(&canonical).map_err(|_| {
                invalid("Target URL is not a recognized post page for this site. Pass a post ID instead.")
            })?;
            let url_post_id = extract_post_id(platform, &parsed).ok_or_else(|| {
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
            if !is_valid_profile_username(platform, username) {
                return Err(invalid("Enter a valid username for this site."));
            }
            return Ok((
                canonical_profile_url(platform, username),
                json!({ "kind": "empty" }),
            ));
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
    if extract_profile_username(platform, &parsed).is_none() {
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
    let (payload_field, debug) = split_debug(object.get("payload"))?;
    let uses_thread = payload_field
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|object| object.contains_key("thread"));
    let (payload_field, images) = if action == Action::Post || action == Action::Reply {
        split_images(payload_field.as_ref())?
    } else {
        (payload_field, None)
    };
    if action == Action::Post && uses_thread && images.is_some() {
        return Err(invalid(
            "images cannot be given alongside thread; put each part's own images inside that part as { text, images }.",
        ));
    }
    let payload_field = payload_field.as_ref();
    let (target_url, payload) = if action == Action::ReadProfile {
        resolve_profile_target(platform, target_url_field, payload_field)?
    } else if action == Action::Post {
        let target_url = resolve_compose_target(platform, target_url_field)?;
        let payload = parse_public_payload(payload_field, action)?;
        (target_url, payload)
    } else if action == Action::ReadPost {
        resolve_post_target(platform, target_url_field, payload_field)?
    } else if action == Action::ReadFeed || action == Action::ReadTrends {
        let target_url = resolve_fixed_destination_target(platform, action, target_url_field)?;
        let payload = parse_public_payload(payload_field, action)?;
        (target_url, payload)
    } else {
        let target_url = canonicalize_target_url(
            target_url_field.ok_or_else(|| invalid("Target URL is missing or too long."))?,
            platform,
        )?;
        let payload = parse_public_payload(payload_field, action)?;
        (target_url, payload)
    };
    let payload = with_images(payload, images, action);
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
        payload: with_debug(payload, debug),
        ttl_ms,
    })
}

fn parse_public_payload(value: Option<&Value>, action: Action) -> ValidationResult<Value> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Payload must be an object."))?;
    if action == Action::Post {
        let (parts, parts_images) = parse_post_parts(object)?;
        let text = if parts.len() == 1 {
            parts[0].clone()
        } else {
            parts.join("\n\n")
        };
        let mut payload = json!({ "kind": "compose", "text": text, "parts": parts });
        if parts_images.iter().any(|images| !images.is_empty()) {
            payload["partImages"] = json!(parts_images);
        }
        return Ok(payload);
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
        return Ok(json!({ "kind": "reply", "postId": post_id, "text": text }));
    }
    if !object.is_empty() {
        return Err(invalid("This action does not accept a payload."));
    }
    Ok(json!({ "kind": "empty" }))
}

/// Take the `debug` request off a payload so the action's own parser sees the
/// shape it expects. `true` asks for everything; a glob asks for the captured
/// responses whose URL matches it.
fn split_debug(value: Option<&Value>) -> ValidationResult<(Option<Value>, Option<Value>)> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Ok((value.cloned(), None));
    };
    let mut object = object.clone();
    let debug = match object.remove("debug") {
        None | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(Value::Bool(true)),
        Some(Value::String(glob)) if is_debug_glob(&glob) => Some(Value::String(glob)),
        Some(_) => {
            return Err(invalid(
                "debug must be true, false, or a URL glob of at most 200 characters.",
            ));
        }
    };
    Ok((Some(Value::Object(object)), debug))
}

/// Mark a payload whose job should carry the extra diagnostics back. Absent
/// when not asked for, so the wire shape stays as before.
fn with_debug(mut payload: Value, debug: Option<Value>) -> Value {
    if let Some(debug) = debug {
        payload["debug"] = debug;
    }
    payload
}

/// Take `images` off a payload before the action's own parser sees it, the
/// way [`split_debug`] takes `debug` off. Only meaningful for the plain,
/// non-thread shape — a thread gives each of its own parts images instead,
/// via `{ text, images }`.
fn split_images(value: Option<&Value>) -> ValidationResult<(Option<Value>, Option<Vec<String>>)> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Ok((value.cloned(), None));
    };
    let mut object = object.clone();
    let Some(raw) = object.remove("images") else {
        return Ok((Some(Value::Object(object)), None));
    };
    let images = parse_image_paths(&raw)?;
    Ok((Some(Value::Object(object)), Some(images)))
}

/// Put top-level `images` back once the action's own parser has produced its
/// payload. A reply is always one part, so the images are simply its own; a
/// post's images become the first part's, matching what this field always
/// meant before a thread could give its own parts images individually.
fn with_images(mut payload: Value, images: Option<Vec<String>>, action: Action) -> Value {
    let Some(images) = images else {
        return payload;
    };
    if action == Action::Reply {
        payload["images"] = json!(images);
        return payload;
    }
    let part_count = payload["parts"].as_array().map_or(1, Vec::len).max(1);
    let mut parts_images = vec![Value::Array(Vec::new()); part_count];
    parts_images[0] = json!(images);
    payload["partImages"] = json!(parts_images);
    payload
}

fn is_debug_glob(glob: &str) -> bool {
    !glob.is_empty() && glob.len() <= MAX_DEBUG_GLOB_LEN
}

fn debug_is_flag(object: &Map<String, Value>) -> bool {
    match object.get("debug") {
        None => true,
        Some(Value::Bool(_)) => true,
        Some(Value::String(glob)) => is_debug_glob(glob),
        Some(_) => false,
    }
}

/// The posts a compose request becomes: `thread` as given, or `text` cut
/// into posts that each fit X's limit.
/// The images a single part asked for: 1 to [`MAX_IMAGES`] absolute local
/// file paths. What they point at is not checked here — staging is where the
/// bytes actually get read, hashed and validated as a real PNG or JPEG.
fn parse_image_paths(value: &Value) -> ValidationResult<Vec<String>> {
    let invalid_images = || invalid("images needs 1 to 4 absolute local file paths.");
    let items = value
        .as_array()
        .filter(|items| !items.is_empty() && items.len() <= MAX_IMAGES)
        .ok_or_else(invalid_images)?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| is_string(value, MAX_IMAGE_PATH_LENGTH) && value.starts_with('/'))
                .map(str::to_owned)
                .ok_or_else(invalid_images)
        })
        .collect()
}

/// One entry of a `thread` array: exact text, or exact text paired with that
/// part's own images. Kept as a union so a thread with no images anywhere in
/// it reads exactly as it always has.
fn parse_thread_entry(value: &Value) -> ValidationResult<(String, Vec<String>)> {
    let thread_error =
        || invalid("A thread needs 1 to 25 posts, each exact text or { text, images }.");
    if let Some(text) = value.as_str() {
        return if is_string(text, MAX_TEXT_LENGTH) {
            Ok((text.to_owned(), Vec::new()))
        } else {
            Err(thread_error())
        };
    }
    let object = value.as_object().ok_or_else(thread_error)?;
    if !has_only_keys(object, &["text", "images"]) {
        return Err(thread_error());
    }
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| is_string(value, MAX_TEXT_LENGTH))
        .ok_or_else(thread_error)?
        .to_owned();
    let images = object.get("images").map(parse_image_paths).transpose()?.unwrap_or_default();
    Ok((text, images))
}

/// The posts a compose request becomes, and each one's own images: `thread`
/// as given, or `text` cut into posts that each fit X's limit (never carries
/// images of its own — use `thread` for that).
fn parse_post_parts(object: &Map<String, Value>) -> ValidationResult<(Vec<String>, Vec<Vec<String>>)> {
    if has_only_keys(object, &["thread"]) {
        let entries = object
            .get("thread")
            .and_then(Value::as_array)
            .filter(|parts| !parts.is_empty() && parts.len() <= MAX_THREAD_PARTS)
            .ok_or_else(|| invalid("A thread needs 1 to 25 posts, each exact text or { text, images }."))?;
        let parsed: Vec<(String, Vec<String>)> =
            entries.iter().map(parse_thread_entry).collect::<Result<_, _>>()?;
        if let Some((index, (part, _))) = parsed
            .iter()
            .enumerate()
            .find(|(_, (part, _))| weighted_length(part) > X_POST_LIMIT)
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
        return Ok(parsed.into_iter().unzip());
    }
    if !has_only_keys(object, &["text"]) {
        return Err(invalid("Post payload needs exact text, or a thread of posts."));
    }
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| is_string(value, MAX_TEXT_LENGTH))
        .ok_or_else(|| invalid("Post payload needs exact text, or a thread of posts."))?;
    let parts = split_into_parts(text).ok_or_else(|| ValidationError {
        code: "too_long",
        message: format!(
            "This text cannot be cut into posts under X's {X_POST_LIMIT} limit. Pass a thread of shorter posts."
        ),
    })?;
    let parts_images = vec![Vec::new(); parts.len()];
    Ok((parts, parts_images))
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
        if !has_only_keys(object, &["kind", "postId", "debug"])
            || !debug_is_flag(object)
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
        let part_count = object.get("parts").and_then(Value::as_array).map_or(0, Vec::len);
        if !has_only_keys(object, &["kind", "draftId", "text", "parts", "debug", "partImages"])
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
            || !is_part_images(object.get("partImages"), part_count)
        {
            return Err(invalid("Post submission command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if action == Action::SubmitReply {
        if !has_only_keys(object, &["kind", "draftId", "postId", "text", "debug", "images"])
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
            || !is_image_attachments(object.get("images"))
        {
            return Err(invalid("Submission command payload is invalid."));
        }
        return Ok(value.cloned().unwrap_or(Value::Null));
    }
    if !has_only_keys(object, &["kind", "debug"]) || !debug_is_flag(object)
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
    let (issued_at, expires_at) = parse_result_times(object)?;
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

pub(crate) fn parse_result_data(value: Option<&Value>) -> ValidationResult<Value> {
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

/// A result may be issued after its command expired: the page can finish
/// past the deadline, and the server decides whether that late answer can
/// still settle the job against the expiry its command carried.
fn parse_result_times(object: &Map<String, Value>) -> ValidationResult<(i64, i64)> {
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
    Ok((issued_at, expires_at))
}

fn parse_envelope_times(object: &Map<String, Value>) -> ValidationResult<(i64, i64)> {
    let (issued_at, expires_at) = parse_result_times(object)?;
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

// The commands dispatched to the extension for a platform: every read
// action it is supported on, plus the submit actions that replace the
// held-back post/reply once the user confirms.
fn ready_capabilities(platform: Platform) -> Vec<Action> {
    [
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
    .into_iter()
    .filter(|action| action.is_supported_on(platform))
    .collect()
}

pub fn make_ready_envelope(connection_id: &str, now: i64) -> Value {
    let contracts: Vec<Value> = [Platform::X, Platform::Instagram]
        .into_iter()
        .map(|platform| {
            json!({
                "platform": platform.as_str(),
                "hostnames": hostnames(platform),
                "capabilities": ready_capabilities(platform).into_iter().map(Action::as_str).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "version": PROTOCOL_VERSION,
        "type": "ready",
        "connectionId": connection_id,
        "heartbeatIntervalMs": HEARTBEAT_INTERVAL_MS,
        "contracts": contracts,
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
        Platform::Instagram => vec!["instagram.com", "www.instagram.com"],
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

/// One `{ imageId, contentType }` pair naming an image already staged
/// locally. No image bytes and no local path ever travel this way — a
/// browser tab could not read a host path anyway. The extension fetches the
/// actual bytes itself, over its own authenticated connection, by this id.
fn is_image_attachment_object(item: &Value) -> bool {
    item.as_object().is_some_and(|object| {
        has_only_keys(object, &["imageId", "contentType"])
            && object.get("imageId").and_then(Value::as_str).is_some_and(is_identifier)
            && matches!(
                object.get("contentType").and_then(Value::as_str),
                Some("image/png" | "image/jpeg")
            )
    })
}

/// A list of image attachments: up to [`MAX_IMAGES`], empty allowed only
/// when a part can legitimately carry none of its own.
fn is_image_attachment_list(value: &Value, allow_empty: bool) -> bool {
    value.as_array().is_some_and(|items| {
        (allow_empty || !items.is_empty())
            && items.len() <= MAX_IMAGES
            && items.iter().all(is_image_attachment_object)
    })
}

/// The images a reply carries, on the wire to the extension: absent, or 1 to
/// [`MAX_IMAGES`] attachments. A reply is always one part, so there is
/// nothing to associate this list with beyond the reply itself.
fn is_image_attachments(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(value) => is_image_attachment_list(value, false),
    }
}

/// The images a post carries, on the wire to the extension: absent, or
/// exactly one list per part — each 0 to [`MAX_IMAGES`] attachments — so an
/// image-free part is a present, empty entry rather than a gap the extension
/// has to guess the meaning of.
fn is_part_images(value: Option<&Value>, expected_parts: usize) -> bool {
    match value {
        None => true,
        Some(value) => value.as_array().is_some_and(|parts| {
            parts.len() == expected_parts
                && parts.iter().all(|part| is_image_attachment_list(part, true))
        }),
    }
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
            values.len() <= MAX_RESULT_ARRAY_LEN
                && values.iter().all(|item| is_bounded_json(item, depth + 1))
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
        assert_eq!(post_id_only.target_url, "https://x.com/i/status/42");
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
    fn rejects_every_platform_but_x_and_instagram() {
        for platform in ["linkedin", "gmail", "tiktok"] {
            assert!(
                parse_create_job_request(&json!({
                    "platform": platform, "action": "read_feed", "payload": {}
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn instagram_post_urls_canonicalise() {
        assert_eq!(
            canonicalize_target_url(
                "https://www.instagram.com/p/CxYz_1-2Ab/",
                Platform::Instagram
            )
            .unwrap(),
            "https://www.instagram.com/p/CxYz_1-2Ab/"
        );
        assert_eq!(
            canonicalize_target_url(
                "https://instagram.com:443/reel/CxYz_1-2Ab/",
                Platform::Instagram
            )
            .unwrap(),
            "https://instagram.com/reel/CxYz_1-2Ab/"
        );
    }

    #[test]
    fn rejects_a_non_instagram_hostname() {
        assert!(
            canonicalize_target_url("https://instagram.com.evil/p/abc/", Platform::Instagram)
                .is_err()
        );
        assert!(canonicalize_target_url("https://x.com/p/abc/", Platform::Instagram).is_err());
    }

    #[test]
    fn instagram_rejects_read_trends_as_unsupported() {
        let error = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_trends", "payload": {}
        }))
        .unwrap_err();
        assert_eq!(error.code, "unsupported_action");
    }

    #[test]
    fn instagram_read_post_accepts_a_p_or_reel_url_and_derives_the_shortcode() {
        let by_post = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_post",
            "targetUrl": "https://www.instagram.com/p/CxYz_1-2Ab/", "payload": {}
        }))
        .unwrap();
        assert_eq!(
            by_post.target_url,
            "https://www.instagram.com/p/CxYz_1-2Ab/"
        );
        assert_eq!(by_post.payload["postId"], "CxYz_1-2Ab");

        let by_reel = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_post",
            "targetUrl": "https://www.instagram.com/reel/CxYz_1-2Ab/", "payload": {}
        }))
        .unwrap();
        assert_eq!(by_reel.payload["postId"], "CxYz_1-2Ab");

        let by_shortcode = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_post",
            "payload": { "postId": "CxYz_1-2Ab" }
        }))
        .unwrap();
        assert_eq!(
            by_shortcode.target_url,
            "https://www.instagram.com/p/CxYz_1-2Ab/"
        );
    }

    #[test]
    fn instagram_read_post_accepts_a_username_prefixed_p_or_reel_url() {
        let by_post = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_post",
            "targetUrl": "https://www.instagram.com/onenigaofficial1/p/CxYz_1-2Ab/",
            "payload": {}
        }))
        .unwrap();
        assert_eq!(
            by_post.target_url,
            "https://www.instagram.com/onenigaofficial1/p/CxYz_1-2Ab/"
        );
        assert_eq!(by_post.payload["postId"], "CxYz_1-2Ab");

        let by_reel = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_post",
            "targetUrl": "https://www.instagram.com/onenigaofficial1/reel/CxYz_1-2Ab/",
            "payload": {}
        }))
        .unwrap();
        assert_eq!(by_reel.payload["postId"], "CxYz_1-2Ab");
    }

    #[test]
    fn instagram_read_profile_resolves_a_username_to_the_canonical_profile_url() {
        let by_handle = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_profile", "payload": { "username": "@jack" }
        }))
        .unwrap();
        assert_eq!(by_handle.target_url, "https://www.instagram.com/jack/");
    }

    #[test]
    fn debug_takes_a_flag_or_a_url_glob() {
        let all = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_profile",
            "payload": { "username": "jack", "debug": true }
        }))
        .unwrap();
        assert_eq!(all.payload["debug"], json!(true));

        let matching = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_profile",
            "payload": { "username": "jack", "debug": "*/graphql*" }
        }))
        .unwrap();
        assert_eq!(matching.payload["debug"], json!("*/graphql*"));

        let off = parse_create_job_request(&json!({
            "platform": "instagram", "action": "read_profile",
            "payload": { "username": "jack", "debug": false }
        }))
        .unwrap();
        assert!(off.payload.get("debug").is_none());

        for refused in [json!(1), json!(""), json!("x".repeat(MAX_DEBUG_GLOB_LEN + 1))] {
            assert!(
                parse_create_job_request(&json!({
                    "platform": "instagram", "action": "read_profile",
                    "payload": { "username": "jack", "debug": refused }
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
    fn a_post_or_reply_request_accepts_bounded_images_and_rejects_the_rest() {
        // A plain, non-thread post's top-level images become the one
        // implicit part's own images.
        let with_images = parse_create_job_request(&json!({
            "platform": "x", "action": "post",
            "payload": { "text": "Short.", "images": ["/tmp/a.png", "/tmp/b.jpg"] }
        }))
        .unwrap();
        assert_eq!(
            with_images.payload["partImages"],
            json!([["/tmp/a.png", "/tmp/b.jpg"]])
        );

        let reply_with_images = parse_create_job_request(&json!({
            "platform": "x", "action": "reply", "targetUrl": "https://x.com/status/42",
            "payload": { "postId": "42", "text": "Reply", "images": ["/tmp/a.png"] }
        }))
        .unwrap();
        assert_eq!(reply_with_images.payload["images"], json!(["/tmp/a.png"]));

        // Each thread part can carry its own images, an image-free part
        // included, and a plain string thread entry keeps working exactly
        // as before.
        let thread_with_images = parse_create_job_request(&json!({
            "platform": "x", "action": "post",
            "payload": { "thread": [
                { "text": "First.", "images": ["/tmp/a.png"] },
                "Second.",
                { "text": "Third.", "images": ["/tmp/c1.png", "/tmp/c2.png"] },
            ] }
        }))
        .unwrap();
        assert_eq!(
            thread_with_images.payload["partImages"],
            json!([["/tmp/a.png"], [], ["/tmp/c1.png", "/tmp/c2.png"]])
        );

        let none = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "text": "Short." }
        }))
        .unwrap();
        assert!(none.payload.get("partImages").is_none());

        // Top-level images alongside a thread is ambiguous, not guessed at.
        let ambiguous = parse_create_job_request(&json!({
            "platform": "x", "action": "post",
            "payload": { "thread": ["First.", "Second."], "images": ["/tmp/a.png"] }
        }))
        .unwrap_err();
        assert_eq!(ambiguous.code, "invalid_schema");

        let empty = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "text": "Short.", "images": [] }
        }))
        .unwrap_err();
        assert_eq!(empty.code, "invalid_schema");

        let too_many = parse_create_job_request(&json!({
            "platform": "x", "action": "post",
            "payload": { "text": "Short.", "images": ["/a.png", "/b.png", "/c.png", "/d.png", "/e.png"] }
        }))
        .unwrap_err();
        assert_eq!(too_many.code, "invalid_schema");

        let relative = parse_create_job_request(&json!({
            "platform": "x", "action": "post", "payload": { "text": "Short.", "images": ["a.png"] }
        }))
        .unwrap_err();
        assert_eq!(relative.code, "invalid_schema");

        let wrong_type = parse_create_job_request(&json!({
            "platform": "x", "action": "read_feed", "payload": { "images": ["/a.png"] }
        }))
        .unwrap_err();
        assert_eq!(wrong_type.code, "invalid_schema");
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
    fn a_submit_post_command_accepts_bounded_per_part_image_attachments_and_rejects_the_rest() {
        let base = |part_images: Value| {
            json!({
                "version": 1,
                "type": "command",
                "jobId": "job-1",
                "commandId": "command-1",
                "platform": "x",
                "action": "submit_post",
                "targetUrl": "https://x.com/compose/post",
                "issuedAt": 1_000,
                "expiresAt": 61_000,
                "payload": {
                    "kind": "post_submission",
                    "draftId": "draft-1",
                    "text": "Hello\n\nWorld",
                    "parts": ["Hello", "World"],
                    "partImages": part_images,
                }
            })
        };
        // One entry per part; an image-free part is a present, empty list.
        // Never a path here either — an id the extension fetches the bytes
        // for over its own authenticated connection.
        assert!(
            parse_command_envelope(&base(json!([
                [{ "imageId": "img-1", "contentType": "image/png" }],
                [],
            ])))
            .is_ok()
        );
        // Both parts empty is fine too.
        assert!(parse_command_envelope(&base(json!([[], []]))).is_ok());
        // Wrong length against `parts` is refused outright.
        assert!(
            parse_command_envelope(&base(json!([
                [{ "imageId": "img-1", "contentType": "image/png" }],
            ])))
            .is_err()
        );
        assert!(
            parse_command_envelope(&base(json!([
                [{ "imageId": "img-1", "contentType": "image/gif" }],
                [],
            ])))
            .is_err()
        );
        assert!(
            parse_command_envelope(&base(json!([
                [{ "path": "/tmp/a.png", "contentType": "image/png" }],
                [],
            ])))
            .is_err()
        );
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

    #[test]
    fn a_result_array_up_to_600_entries_passes_and_one_more_fails() {
        let within_bound = json!({
            "version": 1,
            "type": "result",
            "jobId": "job-1",
            "commandId": "command-1",
            "issuedAt": 100,
            "expiresAt": 200,
            "outcome": "succeeded",
            "data": { "kind": "instagram_profile", "posts": vec![json!({}); MAX_RESULT_ARRAY_LEN] }
        });
        assert!(parse_extension_message(&within_bound).is_ok());

        let over_bound = json!({
            "version": 1,
            "type": "result",
            "jobId": "job-1",
            "commandId": "command-1",
            "issuedAt": 100,
            "expiresAt": 200,
            "outcome": "succeeded",
            "data": { "kind": "instagram_profile", "posts": vec![json!({}); MAX_RESULT_ARRAY_LEN + 1] }
        });
        assert!(parse_extension_message(&over_bound).is_err());
    }
}
