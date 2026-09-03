//! What a MongoDB query may not contain.
//!
//! The tool set never exposes a raw `runCommand`, so the surface an agent can
//! reach is filters, projections, sorts, update documents and aggregation
//! pipelines. This module rejects the parts of that surface that run
//! server-side code, write to another collection, or read the deployment's
//! own state — before the call reaches the server.

use serde_json::Value;

/// Operators that evaluate server-side code, write somewhere the tool's name
/// does not say, or report on the deployment rather than the data. Rejected
/// anywhere they appear, at any depth.
const BLOCKED_OPERATORS: &[&str] = &[
    "$where",
    "$function",
    "$accumulator",
    "$out",
    "$merge",
    "$eval",
    "$currentop",
    "$listsessions",
    "$listlocalsessions",
    "$plancachestats",
    "$collstats",
    "$indexstats",
    "$shardeddatadistribution",
];

/// Database commands Pluk never runs. Unlike the operators above these carry
/// no `$`, so they are only rejected in query shapes — an inserted document
/// may still hold a field called `eval`.
const BLOCKED_COMMANDS: &[&str] = &["eval", "mapreduce"];

fn blocked(key: &str, commands: bool) -> bool {
    let lower = key.to_ascii_lowercase();
    BLOCKED_OPERATORS.contains(&lower.as_str())
        || (commands && BLOCKED_COMMANDS.contains(&lower.as_str()))
}

fn scan(value: &Value, commands: bool) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if blocked(key, commands) {
                    return Err(format!("`{key}` is not allowed on this connection."));
                }
                scan(child, commands)?;
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(|item| scan(item, commands)),
        _ => Ok(()),
    }
}

/// Check a filter, projection, sort, update or pipeline.
pub fn check_query(value: &Value) -> Option<String> {
    scan(value, true).err()
}

/// Check a document being inserted. Field names are the user's data, so only
/// the `$`-prefixed operators are rejected here.
pub fn check_document(value: &Value) -> Option<String> {
    scan(value, false).err()
}

/// An update or delete without a filter would touch the whole collection.
pub fn require_filter(filter: &Value) -> Option<String> {
    match filter {
        Value::Object(map) if !map.is_empty() => None,
        _ => Some(
            "`filter` must name at least one condition — an empty filter would match every document."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_side_code_is_rejected_wherever_it_hides() {
        assert!(check_query(&json!({"$where": "this.a == 1"})).is_some());
        assert!(check_query(&json!({"a": {"b": {"$function": {"body": "…"}}}})).is_some());
        assert!(
            check_query(&json!([{"$group": {"total": {"$accumulator": {}}}}])).is_some(),
            "a pipeline is scanned to its leaves"
        );
    }

    #[test]
    fn stages_that_write_or_read_the_deployment_are_rejected() {
        for stage in ["$out", "$merge", "$currentOp", "$collStats", "$indexStats"] {
            assert!(
                check_query(&json!([{"$match": {}}, {stage: "x"}])).is_some(),
                "{stage} must be blocked"
            );
        }
    }

    #[test]
    fn blocked_names_match_regardless_of_case() {
        assert!(check_query(&json!({"$Where": "…"})).is_some());
        assert!(check_query(&json!([{"mapReduce": "docs"}])).is_some());
        assert!(check_query(&json!({"eval": 1})).is_some());
    }

    #[test]
    fn ordinary_queries_pass() {
        assert_eq!(
            check_query(&json!({"status": "open", "age": {"$gt": 30}})),
            None
        );
        assert_eq!(
            check_query(&json!([{"$match": {"a": 1}}, {"$group": {"_id": "$a", "n": {"$sum": 1}}}])),
            None
        );
    }

    #[test]
    fn an_inserted_document_may_hold_a_field_named_eval() {
        assert_eq!(check_document(&json!({"eval": "grade B", "mapReduce": 2})), None);
        assert!(check_document(&json!({"$where": "…"})).is_some());
    }

    #[test]
    fn updates_and_deletes_need_a_condition() {
        assert!(require_filter(&json!({})).is_some());
        assert!(require_filter(&json!(null)).is_some());
        assert_eq!(require_filter(&json!({"_id": 1})), None);
    }
}
