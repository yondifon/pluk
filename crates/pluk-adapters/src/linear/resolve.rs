use serde_json::{json, Value};

use crate::error::AdapterError;
use super::client::linear_graphql;

fn cap(items: &[String]) -> String {
    let shown = items.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 10 { format!("{shown}, …") } else { shown }
}

pub async fn resolve_team(api_key: &str, key_or_name: &str) -> Result<Value, AdapterError> {
    let data = linear_graphql(api_key, "{ teams { nodes { id key name } } }", json!({})).await?;
    let nodes = data.get("teams").and_then(|t| t.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let want = key_or_name.trim().to_lowercase();
    let exact: Vec<&Value> = nodes.iter().filter(|t| {
        t.get("key").and_then(|k| k.as_str()).map(|k| k.to_lowercase() == want).unwrap_or(false)
            || t.get("name").and_then(|n| n.as_str()).map(|n| n.to_lowercase() == want).unwrap_or(false)
    }).collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        let list = exact.iter().map(|t| format!("{} ({})", t.get("key").and_then(|k| k.as_str()).unwrap_or("?"), t.get("name").and_then(|n| n.as_str()).unwrap_or("?"))).collect::<Vec<_>>().join(", ");
        return Err(AdapterError::new(format!("Team \"{key_or_name}\" matches more than one team: {list}. Pass the exact team key.")));
    }
    let known: Vec<String> = nodes.iter().map(|t| format!("{} ({})", t.get("key").and_then(|k| k.as_str()).unwrap_or("?"), t.get("name").and_then(|n| n.as_str()).unwrap_or("?"))).collect();
    Err(AdapterError::new(format!("No team named \"{key_or_name}\". Known teams: {}.", cap(&known))))
}

pub async fn resolve_user(api_key: &str, email_or_name: &str) -> Result<Value, AdapterError> {
    let term = email_or_name.trim().to_string();
    let by_email = term.contains('@');
    let filter = if by_email { json!({ "email": { "containsIgnoreCase": term } }) } else { json!({ "name": { "containsIgnoreCase": term } }) };
    let data = linear_graphql(api_key, "query($filter:UserFilter){ users(first: 50, filter: $filter){ nodes { id name email } } }", json!({ "filter": filter })).await?;
    let nodes = data.get("users").and_then(|u| u.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let want = term.to_lowercase();
    let exact: Vec<&Value> = nodes.iter().filter(|u| {
        if by_email { u.get("email").and_then(|e| e.as_str()).map(|e| e.to_lowercase() == want).unwrap_or(false) } else { u.get("name").and_then(|n| n.as_str()).map(|n| n.to_lowercase() == want).unwrap_or(false) }
    }).collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        let list = exact.iter().map(|u| { let name = u.get("name").and_then(|n| n.as_str()).unwrap_or("?"); match u.get("email").and_then(|e| e.as_str()) { Some(e) => format!("{name} <{e}>"), None => name.to_string() } }).collect::<Vec<_>>().join(", ");
        return Err(AdapterError::new(format!("Assignee \"{email_or_name}\" matches more than one user: {list}. Pass a unique email or name.")));
    }
    let near: Vec<String> = nodes.iter().map(|u| { let name = u.get("name").and_then(|n| n.as_str()).unwrap_or("?"); match u.get("email").and_then(|e| e.as_str()) { Some(e) => format!("{name} <{e}>"), None => name.to_string() } }).collect();
    if !near.is_empty() {
        return Err(AdapterError::new(format!("No user matches \"{email_or_name}\". Near matches: {}.", cap(&near))));
    }
    Err(AdapterError::new(format!("No user matches \"{email_or_name}\".")))
}

pub async fn resolve_state(api_key: &str, team_key: &str, name: &str) -> Result<Value, AdapterError> {
    let data = linear_graphql(api_key, "query($filter:WorkflowStateFilter){ workflowStates(filter: $filter){ nodes { id name } } }", json!({ "filter": { "team": { "key": { "eq": team_key } } } })).await?;
    let nodes = data.get("workflowStates").and_then(|w| w.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let want = name.trim().to_lowercase();
    let exact: Vec<&Value> = nodes.iter().filter(|s| s.get("name").and_then(|n| n.as_str()).map(|n| n.to_lowercase() == want).unwrap_or(false)).collect();
    if exact.len() == 1 { return Ok(exact[0].clone()); }
    if exact.len() > 1 {
        let list = exact.iter().filter_map(|s| s.get("name").and_then(|n| n.as_str())).collect::<Vec<_>>().join(", ");
        return Err(AdapterError::new(format!("State \"{name}\" matches more than one workflow state: {list}. Pass the exact state name.")));
    }
    let names: Vec<String> = nodes.iter().filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())).collect();
    Err(AdapterError::new(format!("No workflow state named \"{name}\" in team {team_key}. States: {}.", cap(&names))))
}

pub async fn resolve_labels(api_key: &str, names: &[String]) -> Result<Vec<String>, AdapterError> {
    let data = linear_graphql(api_key, "{ issueLabels(first: 250){ nodes { id name } } }", json!({})).await?;
    let nodes = data.get("issueLabels").and_then(|l| l.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let mut ids = Vec::new();
    for raw in names {
        let want = raw.trim().to_lowercase();
        let exact: Vec<&Value> = nodes.iter().filter(|l| l.get("name").and_then(|n| n.as_str()).map(|n| n.to_lowercase() == want).unwrap_or(false)).collect();
        if exact.len() == 1 {
            ids.push(exact[0].get("id").and_then(|id| id.as_str()).unwrap_or("").to_string());
        } else if exact.len() > 1 {
            let list = exact.iter().filter_map(|l| l.get("name").and_then(|n| n.as_str())).collect::<Vec<_>>().join(", ");
            return Err(AdapterError::new(format!("Label \"{raw}\" matches more than one label: {list}. Pass the exact label name.")));
        } else {
            let known: Vec<String> = nodes.iter().filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())).collect();
            return Err(AdapterError::new(format!("No label named \"{raw}\". Existing labels: {}.", cap(&known))));
        }
    }
    Ok(ids)
}
