use serde::Serialize;
use serde_json::{Value, json};

use crate::protocol::{
    Action, DEFAULT_JOB_TTL_MS, MAX_JOB_TTL_MS, MAX_TEXT_LENGTH, MIN_JOB_TTL_MS, Platform,
    fixed_compose_target, fixed_feed_target, fixed_trends_target, hostnames,
};

const PLATFORMS: [Platform; 1] = [Platform::X];

/// Tool classes, matching the categories the adapter layer groups by.
const READ: &str = "read";
const WRITE: &str = "write";

/// Every action the catalog publishes, with the summary an agent reads and
/// the class its enable-by-default state derives from. A new action declares
/// its own class here; nothing downstream names actions.
const ACTIONS: [(Action, &str, &str); 9] = [
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
                    "debug": {
                        "type": "boolean",
                        "required": false,
                        "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
                    },
                },
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
                        "items": { "type": "string", "maxLength": MAX_TEXT_LENGTH },
                        "maxItems": 25,
                        "description": "The posts of a thread, in order, each under 280 weighted characters. Pass this or text, not both.",
                    },
                    "debug": {
                        "type": "boolean",
                        "required": false,
                        "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
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
                        "type": "boolean",
                        "required": false,
                        "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
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
                        "type": "boolean",
                        "required": false,
                        "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
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
                        "type": "boolean",
                        "required": false,
                        "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
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
                    "type": "boolean",
                    "required": false,
                    "description": "When the page refuses it, attach a screenshot and the page's HTML to the failed job so the refusal can be read.",
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
                    "description": "Set for reply and post. The post is put to the user, and their answer is what sends it; read this draft to see what they decided. Nothing here publishes it.",
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
                "x.post",
            ]
        );
        // Submissions are reached by confirming a draft, never by invoking a tool.
        assert!(!ids.iter().any(|id| id.ends_with(".submit_reply")));
        assert!(!ids.iter().any(|id| id.ends_with(".submit_post")));
    }

    #[test]
    fn find_tool_rejects_unknown_and_unsupported_pairs() {
        assert!(find_tool("x.inspect").is_some());
        assert!(find_tool("x.read_profile").is_some());
        assert!(find_tool("x.read_post").is_some());
        assert!(find_tool("x.post").is_some());
        assert!(find_tool("linkedin.read_post").is_none());
        assert!(find_tool("gmail.read_feed").is_none());
        assert!(find_tool("tiktok.read_profile").is_none());
        assert!(find_tool("not-a-tool").is_none());
        assert!(find_tool("x.not_an_action").is_none());
    }
}
