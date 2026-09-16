use serde::Serialize;
use serde_json::{Value, json};

use crate::protocol::{
    Action, MAX_DEBUG_GLOB_LEN, DEFAULT_JOB_TTL_MS, MAX_IMAGES, MAX_JOB_TTL_MS, MAX_TEXT_LENGTH, MIN_JOB_TTL_MS,
    Platform, fixed_compose_target, fixed_feed_target, fixed_trends_target, hostnames,
};

const PLATFORMS: [Platform; 2] = [Platform::X, Platform::Instagram];

/// Tool classes, matching the categories the adapter layer groups by.
const READ: &str = "read";
const WRITE: &str = "write";

/// Every action the catalog publishes, with the summary an agent reads and
/// the class its enable-by-default state derives from. A new action declares
/// its own class here; nothing downstream names actions.
const ACTIONS: [(Action, &str, &str); 11] = [
    (
        Action::Inspect,
        "Read the page context at an exact target URL.",
        READ,
    ),
    (
        Action::ReadProfile,
        "Read a profile's bounded bio context by username. Returns a canonical profile target.",
        READ,
    ),
    (
        Action::ReadPost,
        "Read one exact post by its URL or its typed post ID. Returns a canonical post target.",
        READ,
    ),
    (
        Action::ReadFeed,
        "Read the visible feed. No input needed; defaults to this platform's feed.",
        READ,
    ),
    (
        Action::ReadTrends,
        "Read the visible trending/explore page. No input needed.",
        READ,
    ),
    (
        Action::Refresh,
        "Reload an exact target URL and read its context again.",
        READ,
    ),
    (
        Action::Capture,
        "Capture a screenshot of an exact target URL.",
        READ,
    ),
    (
        Action::Reply,
        "Reply to an exact post ID with exact text. It waits in Pluk until the user sends it; no page is touched before then.",
        WRITE,
    ),
    (
        Action::Repost,
        "Repost an exact post ID as it stands, adding nothing. It waits in Pluk until the user sends it; no page is touched before then.",
        WRITE,
    ),
    (
        Action::Quote,
        "Quote an exact post ID with exact text of your own, optionally with images or as a thread, under the same rules as posting. It waits in Pluk until the user sends it; no page is touched before then.",
        WRITE,
    ),
    (
        Action::Post,
        "Post exact text, or a thread. X allows 280 weighted characters per post (a link counts 23); longer text is cut into a thread at sentence ends, or pass `thread` for exact parts. It waits in Pluk until the user sends it now or queues it; no page is touched before then.",
        WRITE,
    ),
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub id: String,
    pub platform: &'static str,
    pub action: &'static str,
    pub summary: &'static str,
    /// `read` or `write`: what the action does, and whether it ships enabled.
    pub category: &'static str,
    pub hostnames: Vec<&'static str>,
    pub args_schema: Value,
    pub result_schema: Value,
}

pub fn tools() -> Vec<ToolSpec> {
    let mut list = Vec::new();
    for platform in PLATFORMS {
        for (action, summary, category) in ACTIONS {
            if !action.is_supported_on(platform) {
                continue;
            }
            list.push(ToolSpec {
                id: tool_id(platform, action),
                platform: platform.as_str(),
                action: action.as_str(),
                summary,
                category,
                hostnames: hostnames(platform),
                args_schema: args_schema(platform, action),
                result_schema: result_schema(),
            });
        }
    }
    list
}

pub fn find_tool(id: &str) -> Option<(Platform, Action)> {
    let (platform_id, action_id) = id.split_once('.')?;
    let platform = PLATFORMS
        .into_iter()
        .find(|platform| platform.as_str() == platform_id)?;
    let (action, _, _) = ACTIONS
        .into_iter()
        .find(|(action, _, _)| action.as_str() == action_id)?;
    action
        .is_supported_on(platform)
        .then_some((platform, action))
}

pub fn catalog_value() -> Value {
    json!(tools())
}

fn tool_id(platform: Platform, action: Action) -> String {
    format!("{}.{}", platform.as_str(), action.as_str())
}

fn args_schema(platform: Platform, action: Action) -> Value {
    let target_url = json!({
        "type": "string",
        "required": true,
        "description": "Exact HTTPS target URL on this tool's allowed hostnames.",
    });
    let ttl_ms = json!({
        "type": "integer",
        "required": false,
        "minimum": MIN_JOB_TTL_MS,
        "maximum": MAX_JOB_TTL_MS,
        "default": DEFAULT_JOB_TTL_MS,
        "description": "Job expiry window in milliseconds.",
    });
    let images = json!({
        "type": "array",
        "required": false,
        "items": { "type": "string" },
        "maxItems": MAX_IMAGES,
        "description": format!(
            "1 to {MAX_IMAGES} absolute local PNG or JPEG file paths, each under 5 MiB. Shown to the user for approval alongside the text.",
        ),
    });
    if action == Action::Reply {
        return json!({
            "targetUrl": target_url,
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "postId": {
                        "type": "string",
                        "required": true,
                        "description": "Exact post identifier to reply to.",
                    },
                    "text": {
                        "type": "string",
                        "required": true,
                        "maxLength": MAX_TEXT_LENGTH,
                        "description": "Exact reply text, submitted verbatim and never generated here.",
                    },
                    "images": images,
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::Repost {
        return json!({
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "postId": {
                        "type": "string",
                        "required": true,
                        "description": "Exact post identifier to repost.",
                    },
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
                "description": "No target URL accepted: the post's own page is derived from postId.",
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::Quote {
        return json!({
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "postId": {
                        "type": "string",
                        "required": true,
                        "description": "Exact post identifier to quote.",
                    },
                    "text": {
                        "type": "string",
                        "required": false,
                        "maxLength": MAX_TEXT_LENGTH,
                        "description": "Exact quote text, submitted verbatim. Over 280 weighted characters it is cut into a thread at sentence ends. A quote needs text: pass this or thread, not both.",
                    },
                    "thread": {
                        "type": "array",
                        "required": false,
                        "items": {
                            "type": ["string", "object"],
                            "maxLength": MAX_TEXT_LENGTH,
                            "properties": {
                                "text": { "type": "string", "required": true, "maxLength": MAX_TEXT_LENGTH },
                                "images": images,
                            },
                        },
                        "maxItems": 25,
                        "description": format!(
                            "The posts of a quote thread, in order, each under 280 weighted characters; the first is the quote itself. Pass this or text, not both. Each entry is exact text, or {{ text, images }} to give that one post its own 1 to {MAX_IMAGES} images. Do not also pass the top-level images field alongside thread; it is refused as ambiguous.",
                        ),
                    },
                    "images": {
                        "type": "array",
                        "required": false,
                        "items": { "type": "string" },
                        "maxItems": MAX_IMAGES,
                        "description": format!(
                            "1 to {MAX_IMAGES} absolute local PNG or JPEG file paths for a quote that is not a thread. Not accepted together with thread — give each part its own images there instead.",
                        ),
                    },
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
                "description": "No target URL accepted: the quoted post's own page is derived from postId.",
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::Post {
        let default_target = fixed_compose_target(platform);
        return json!({
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "text": {
                        "type": "string",
                        "required": false,
                        "maxLength": MAX_TEXT_LENGTH,
                        "description": "Exact post text, submitted verbatim. Over 280 weighted characters it is cut into a thread at sentence ends. Pass this or thread, not both.",
                    },
                    "thread": {
                        "type": "array",
                        "required": false,
                        "items": {
                            "type": ["string", "object"],
                            "maxLength": MAX_TEXT_LENGTH,
                            "properties": {
                                "text": { "type": "string", "required": true, "maxLength": MAX_TEXT_LENGTH },
                                "images": images,
                            },
                        },
                        "maxItems": 25,
                        "description": format!(
                            "The posts of a thread, in order, each under 280 weighted characters. Pass this or text, not both. Each entry is exact text, or {{ text, images }} to give that one post its own 1 to {MAX_IMAGES} images — every part can carry its own, none included. Do not also pass the top-level images field alongside thread; it is refused as ambiguous.",
                        ),
                    },
                    "images": {
                        "type": "array",
                        "required": false,
                        "items": { "type": "string" },
                        "maxItems": MAX_IMAGES,
                        "description": format!(
                            "1 to {MAX_IMAGES} absolute local PNG or JPEG file paths for a plain, non-thread post. Not accepted together with thread — give each part its own images there instead.",
                        ),
                    },
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
                "description": format!(
                    "No target URL accepted: always posts at {default_target}.",
                ),
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::ReadPost {
        return json!({
            "targetUrl": {
                "type": "string",
                "required": false,
                "description": "A full post URL. Provide this or payload.postId, not neither; a pair that does not match each other is refused.",
            },
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "postId": {
                        "type": "string",
                        "required": false,
                        "description": "Exact post identifier to read. Provide this or targetUrl, not neither; the missing one is derived.",
                    },
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::ReadProfile {
        return json!({
            "targetUrl": {
                "type": "string",
                "required": false,
                "description": "A full profile URL, accepted for backward compatibility. Prefer payload.username.",
            },
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "username": {
                        "type": "string",
                        "required": true,
                        "description": "Handle to read, with or without a leading @. Resolved to this site's canonical profile URL.",
                    },
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
            },
            "ttlMs": ttl_ms,
        });
    }
    if action == Action::ReadFeed || action == Action::ReadTrends {
        let default_target = if action == Action::ReadTrends {
            fixed_trends_target(platform)
        } else {
            fixed_feed_target(platform)
        };
        return json!({
            "targetUrl": {
                "type": "string",
                "required": false,
                "description": format!("No input needed: always reads {default_target}."),
            },
            "payload": {
                "type": "object",
                "required": true,
                "properties": {
                    "debug": {
                        "type": ["boolean", "string"],
                        "required": false,
                        "maxLength": MAX_DEBUG_GLOB_LEN,
                        "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                    },
                },
                "description": "Empty unless debug is wanted.",
            },
            "ttlMs": ttl_ms,
        });
    }
    json!({
        "targetUrl": target_url,
        "payload": {
            "type": "object",
            "required": true,
            "properties": {
                "debug": {
                    "type": ["boolean", "string"],
                    "required": false,
                    "maxLength": MAX_DEBUG_GLOB_LEN,
                    "description": "Attach what the page was doing to the job. `true` attaches everything read off the page; a URL glob such as `*/graphql*` attaches only the responses the page fetched whose address matches it.",
                },
            },
            "description": "Empty unless debug is wanted.",
        },
        "ttlMs": ttl_ms,
    })
}

fn result_schema() -> Value {
    json!({
        "job": {
            "type": "object",
            "description": "Queued job envelope. Poll GET /wande/jobs/{id} until status leaves queued/running.",
            "properties": {
                "id": { "type": "string" },
                "platform": { "type": "string" },
                "action": { "type": "string" },
                "targetUrl": { "type": "string" },
                "status": {
                    "type": "string",
                    "enum": ["queued", "running", "succeeded", "failed", "expired", "unknown"],
                },
                "createdAt": { "type": "integer" },
                "expiresAt": { "type": "integer" },
                "result": {
                    "type": ["object", "null"],
                    "description": "Bounded driver result once status is succeeded.",
                },
                "error": {
                    "type": ["object", "null"],
                    "description": "Stable { code, message } once status is failed, expired, or unknown.",
                },
                "draftId": {
                    "type": ["string", "null"],
                    "description": "Set for reply, repost, quote and post. The post is put to the user, and their answer is what sends it; read this draft to see what they decided. Nothing here publishes it.",
                },
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_only_lists_implemented_capabilities() {
        let ids: Vec<String> = tools().into_iter().map(|tool| tool.id).collect();
        assert_eq!(
            ids,
            vec![
                "x.inspect",
                "x.read_profile",
                "x.read_post",
                "x.read_feed",
                "x.read_trends",
                "x.refresh",
                "x.capture",
                "x.reply",
                "x.repost",
                "x.quote",
                "x.post",
                "instagram.inspect",
                "instagram.read_profile",
                "instagram.read_post",
                "instagram.refresh",
                "instagram.capture",
            ]
        );
        // Submissions are reached by confirming a draft, never by invoking a tool.
        assert!(!ids.iter().any(|id| id.ends_with(".submit_reply")));
        assert!(!ids.iter().any(|id| id.ends_with(".submit_repost")));
        assert!(!ids.iter().any(|id| id.ends_with(".submit_quote")));
        assert!(!ids.iter().any(|id| id.ends_with(".submit_post")));
    }

    #[test]
    fn catalog_publishes_exactly_the_instagram_tools_instagram_supports() {
        let ids: Vec<String> = tools()
            .into_iter()
            .map(|tool| tool.id)
            .filter(|id| id.starts_with("instagram."))
            .collect();
        assert_eq!(
            ids,
            vec![
                "instagram.inspect",
                "instagram.read_profile",
                "instagram.read_post",
                "instagram.refresh",
                "instagram.capture",
            ]
        );
    }

    #[test]
    fn find_tool_rejects_unknown_and_unsupported_pairs() {
        assert!(find_tool("x.inspect").is_some());
        assert!(find_tool("x.read_profile").is_some());
        assert!(find_tool("x.read_post").is_some());
        assert!(find_tool("x.post").is_some());
        assert!(find_tool("instagram.inspect").is_some());
        assert!(find_tool("instagram.read_post").is_some());
        assert!(find_tool("instagram.read_trends").is_none());
        assert!(find_tool("x.repost").is_some());
        assert!(find_tool("instagram.post").is_none());
        assert!(find_tool("instagram.repost").is_none());
        assert!(find_tool("x.quote").is_some());
        assert!(find_tool("instagram.quote").is_none());
        assert!(find_tool("linkedin.read_post").is_none());
        assert!(find_tool("gmail.read_feed").is_none());
        assert!(find_tool("tiktok.read_profile").is_none());
        assert!(find_tool("not-a-tool").is_none());
        assert!(find_tool("x.not_an_action").is_none());
    }
}
