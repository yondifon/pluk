use std::sync::Arc;
use pluk_store::Store;
use crate::adapter::{ApiRequest, ApiResponse};

fn json_body(body: Option<&str>) -> Option<serde_json::Value> {
    let s = body?;
    serde_json::from_str(s).ok()
}

pub async fn handle_ssh_api(store: Arc<Store>, conn: &pluk_store::Integration, request: ApiRequest, subpath: &str) -> Option<ApiResponse> {
    let re = regex::Regex::new(r"^/saved_commands(?:/([^/]+))?$").unwrap();
    let caps = re.captures(subpath)?;
    let saved_name = caps.get(1).map(|m| urlencoding::decode(m.as_str()).unwrap_or_else(|_| m.as_str().into()).into_owned());

    match request.method.as_str() {
        "GET" => {
            let commands = store.list_saved_commands(&conn.id).unwrap_or_default();
            Some(ApiResponse::json(200, &serde_json::json!({ "ok": true, "commands": commands })))
        },
        "POST" => {
            let body = json_body(request.body.as_deref());
            if body.is_none() {
                return Some(ApiResponse::json(400, &serde_json::json!({ "ok": false, "error": "Invalid JSON body" })));
            }
            let body = body.unwrap();
            let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let command = body.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let working_dir = body.get("working_dir").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            if name.is_empty() || command.is_empty() {
                return Some(ApiResponse::json(400, &serde_json::json!({ "ok": false, "error": "name and command required" })));
            }
            let input = pluk_store::SavedCommandInput { connection_id: conn.id.clone(), name: name.clone(), command, working_dir };
            match store.create_saved_command(&input) {
                Ok(cmd) => Some(ApiResponse::json(200, &serde_json::json!({ "ok": true, "command": cmd }))),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("UNIQUE") || msg.contains("unique") {
                        return Some(ApiResponse::json(409, &serde_json::json!({ "ok": false, "error": "A saved command with that name already exists." })));
                    }
                    Some(ApiResponse::json(500, &serde_json::json!({ "ok": false, "error": msg })))
                }
            }
        },
        "DELETE" => {
            if let Some(name) = saved_name {
                let ok = store.delete_saved_command(&conn.id, &name).unwrap_or(false);
                Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })))
            } else {
                Some(ApiResponse::text(405, "Method not allowed"))
            }
        },
        _ => Some(ApiResponse::text(405, "Method not allowed")),
    }
}
