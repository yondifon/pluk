use std::sync::Arc;

use pluk_store::Store;

use crate::adapter::{ApiRequest, ApiResponse};

fn json_body(body: Option<&str>) -> Option<serde_json::Value> {
    let s = body?;
    serde_json::from_str(s).ok()
}

pub async fn handle_sql_api(
    store: Arc<Store>,
    conn: &pluk_store::Integration,
    request: ApiRequest,
    subpath: &str,
) -> Option<ApiResponse> {
    // saved_queries
    if let Some(caps) = regex::Regex::new(r"^/saved_queries(?:/([^/]+))?$")
        .unwrap()
        .captures(subpath)
    {
        let saved_name = caps.get(1).map(|m| {
            urlencoding::decode(m.as_str())
                .unwrap_or_else(|_| m.as_str().into())
                .into_owned()
        });
        match request.method.as_str() {
            "GET" => {
                let queries = store.list_saved_queries(&conn.id).unwrap_or_default();
                return Some(ApiResponse::json(
                    200,
                    &serde_json::json!({ "ok": true, "queries": queries }),
                ));
            }
            "POST" => {
                let body = json_body(request.body.as_deref())?;
                let name = body
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let sql = body
                    .get("sql")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() || sql.is_empty() {
                    return Some(ApiResponse::json(
                        400,
                        &serde_json::json!({ "ok": false, "error": "name and sql required" }),
                    ));
                }
                let input = pluk_store::SavedQueryInput {
                    connection_id: conn.id.clone(),
                    name: name.clone(),
                    sql,
                };
                match store.create_saved_query(&input) {
                    Ok(q) => {
                        return Some(ApiResponse::json(
                            200,
                            &serde_json::json!({ "ok": true, "query": q }),
                        ));
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("UNIQUE") || msg.contains("unique") {
                            return Some(ApiResponse::json(
                                409,
                                &serde_json::json!({ "ok": false, "error": "A saved query with that name already exists." }),
                            ));
                        }
                        return Some(ApiResponse::json(
                            500,
                            &serde_json::json!({ "ok": false, "error": msg }),
                        ));
                    }
                }
            }
            "DELETE" => {
                if let Some(name) = saved_name {
                    let ok = store.delete_saved_query(&conn.id, &name).unwrap_or(false);
                    return Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })));
                } else {
                    return Some(ApiResponse::text(405, "Method not allowed"));
                }
            }
            _ => return Some(ApiResponse::text(405, "Method not allowed")),
        }
    }

    if let Some(caps) = regex::Regex::new(r"^/masked_columns(?:/([^/]+))?$")
        .unwrap()
        .captures(subpath)
    {
        let col_name = caps.get(1).map(|m| {
            urlencoding::decode(m.as_str())
                .unwrap_or_else(|_| m.as_str().into())
                .into_owned()
        });
        match request.method.as_str() {
            "GET" => {
                let cols = store.list_masked_columns(&conn.id).unwrap_or_default();
                // need to return MaskedColumn objects, not just names; fetch full objects via list?
                // For simplicity return names as objects
                let columns: Vec<serde_json::Value> = cols
                    .iter()
                    .map(|c| serde_json::json!({ "column_name": c }))
                    .collect();
                return Some(ApiResponse::json(
                    200,
                    &serde_json::json!({ "ok": true, "columns": columns }),
                ));
            }
            "POST" => {
                let body = json_body(request.body.as_deref());
                if body.is_none() {
                    return Some(ApiResponse::json(
                        400,
                        &serde_json::json!({ "ok": false, "error": "Invalid JSON body" }),
                    ));
                }
                let body = body.unwrap();
                let col = body
                    .get("column_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if col.is_empty() {
                    return Some(ApiResponse::json(
                        400,
                        &serde_json::json!({ "ok": false, "error": "column_name required" }),
                    ));
                }
                match store.add_masked_column(&conn.id, &col) {
                    Ok(c) => {
                        return Some(ApiResponse::json(
                            200,
                            &serde_json::json!({ "ok": true, "column": c }),
                        ));
                    }
                    Err(e) => {
                        return Some(ApiResponse::json(
                            500,
                            &serde_json::json!({ "ok": false, "error": e.to_string() }),
                        ));
                    }
                }
            }
            "DELETE" => {
                if let Some(name) = col_name {
                    let ok = store.remove_masked_column(&conn.id, &name).unwrap_or(false);
                    return Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })));
                } else {
                    return Some(ApiResponse::text(405, "Method not allowed"));
                }
            }
            _ => return Some(ApiResponse::text(405, "Method not allowed")),
        }
    }

    None
}

pub fn handle_sql_log_api(
    request: &ApiRequest,
    path: &str,
    cancels: Option<Arc<crate::sql::server::SqlCancelRegistry>>,
) -> Option<ApiResponse> {
    let re = regex::Regex::new(r"^/api/log/(\d+)/cancel$").unwrap();
    if let Some(caps) = re.captures(path) {
        if request.method != "POST" {
            return None;
        }
        let id: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
        let ok = if let Some(reg) = cancels {
            reg.cancel(id)
        } else {
            false
        };
        return Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })));
    }
    None
}
