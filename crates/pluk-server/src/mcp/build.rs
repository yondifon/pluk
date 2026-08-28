//! Endpoint surfaces: one integration's tools on their own server, or many
//! integrations aggregated behind a group's server.
//!
//! Ported from `pluk/src/adapters/index.ts` (`buildAdapterServer`) and
//! `pluk/src/mcp/group.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use super::namespace::slug;
use pluk_adapters::{Adapter, AdapterRegistry, ConfigField, FieldType};
use pluk_store::{Group, Integration, LogGroup, Store};

use super::namespace::NamespacedHost;
use super::surface::{Surface, SurfaceBuilder};
use crate::logging;

/// What an `/mcp/<token>` path resolves to.
pub enum Owner {
    /// A single integration served by its own adapter.
    Integration {
        integration: Box<Integration>,
        adapter: Arc<dyn Adapter>,
    },
    /// A group fronting several member integrations.
    Group { group: Box<Group> },
}

impl Owner {
    pub fn owner_id(&self) -> &str {
        match self {
            Owner::Integration { integration, .. } => &integration.id,
            Owner::Group { group } => &group.id,
        }
    }
}

/// Resolve an endpoint token against live store state: an integration's token
/// wins, then a group's. Returns `None` when the token matches neither.
pub fn resolve_owner(
    store: &Store,
    registry: &AdapterRegistry,
    token: &str,
) -> Result<Option<Owner>, String> {
    if let Some(integration) = store
        .integration_by_token(token)
        .map_err(|e| e.to_string())?
    {
        return match registry.get(&integration.r#type) {
            Some(adapter) => Ok(Some(Owner::Integration {
                integration: Box::new(integration),
                adapter,
            })),
            None => Err(format!("No adapter for type: {}", integration.r#type)),
        };
    }
    if let Some(group) = store.group_by_token(token).map_err(|e| e.to_string())? {
        return Ok(Some(Owner::Group {
            group: Box::new(group),
        }));
    }
    Ok(None)
}

/// Build the MCP surface for one owner, from current store state. Called on
/// every protocol request — never cached — so configuration edits and tool
/// enable/disable take effect immediately.
pub fn build_owner_surface(
    owner: &Owner,
    store: &Store,
    registry: &AdapterRegistry,
) -> Result<Surface, String> {
    let owner_id = owner.owner_id();
    match owner {
        Owner::Integration {
            integration,
            adapter,
        } => build_integration_surface(adapter.as_ref(), integration.as_ref(), owner_id),
        Owner::Group { group } => build_group_surface(group, store, registry, owner_id),
    }
}

/// A standalone MCP surface for a single integration: its adapter's
/// instructions, with its full surface registered unnamespaced.
pub fn build_integration_surface(
    adapter: &dyn Adapter,
    conn: &Integration,
    owner_id: &str,
) -> Result<Surface, String> {
    let mut builder = SurfaceBuilder::default();
    builder.set_server_name(conn.name.clone());
    builder.set_instructions(Some(adapter.instructions(conn)));
    adapter
        .register(&mut builder, conn, owner_id)
        .map_err(|e| e.to_string())?;
    builder.build()
}

/// One member resolved for registration inside a group.
struct GroupedMember {
    ns: String,
    adapter: Arc<dyn Adapter>,
    scoped: Integration,
}

/// Merge a member's per-group overrides into its config, coercing each value
/// to the type the adapter declared for that field (`number`/`toggle`) so e.g.
/// a per-group database name or port lands with the right type.
///
/// Ported from `applyOverrides` in `pluk/src/mcp/group.ts`.
pub fn apply_overrides(
    integration: &Integration,
    overrides: Option<&serde_json::Map<String, serde_json::Value>>,
    fields: &[ConfigField],
) -> Integration {
    let Some(overrides) = overrides.filter(|o| !o.is_empty()) else {
        return integration.clone();
    };
    let type_by_key: HashMap<&str, FieldType> = fields
        .iter()
        .map(|f| (f.key.as_str(), f.field_type))
        .collect();

    let mut coerced = serde_json::Map::new();
    for (key, value) in overrides {
        // Blank means inherit: the member's own config value stands.
        if value.is_null() || value.as_str() == Some("") {
            continue;
        }
        let coerced_value = match type_by_key.get(key.as_str()) {
            // Integers stay integers; only true fractions go through f64.
            // Unparsable strings keep the raw value rather than NaN/null.
            Some(FieldType::Number) => match value {
                serde_json::Value::String(text) => text
                    .parse::<i64>()
                    .map(|n| serde_json::Value::Number(n.into()))
                    .unwrap_or_else(|_| {
                        text.parse::<f64>()
                            .ok()
                            .and_then(serde_json::Number::from_f64)
                            .map(serde_json::Value::Number)
                            .unwrap_or_else(|| value.clone())
                    }),
                other => other.clone(),
            },
            Some(FieldType::Toggle) => serde_json::Value::Bool(
                *value == serde_json::Value::Bool(true) || value.as_str() == Some("true"),
            ),
            _ => value.clone(),
        };
        coerced.insert(key.clone(), coerced_value);
    }

    let mut scoped = integration.clone();
    scoped.config.extend(coerced);
    scoped
}

/// Build one MCP surface aggregating every usable member of a group. Each
/// member registers through a namespaced host (prefix = slug of its name) so
/// identically-named tools across members don't collide; per-member overrides
/// are merged before registration, and the member is tagged so its log rows
/// record the group that fronted the call.
pub fn build_group_surface(
    group: &Group,
    store: &Store,
    registry: &AdapterRegistry,
    owner_id: &str,
) -> Result<Surface, String> {
    let members = store.resolve_members(group).map_err(|e| e.to_string())?;
    let mut used_ns: HashMap<String, usize> = HashMap::new();
    let mut resolved: Vec<GroupedMember> = Vec::new();

    for member in members {
        let Some(adapter) = registry.get(&member.integration.r#type) else {
            logging::log_error(
                "group member has no adapter",
                &member.integration.r#type.clone(),
                Some(serde_json::json!({ "group": group.name, "member": member.integration.name })),
            );
            continue;
        };

        let base = slug(&member.integration.name);
        let seen = used_ns.entry(base.clone()).or_insert(0);
        *seen += 1;
        let ns = if *seen > 1 {
            format!("{base}_{seen}")
        } else {
            base
        };

        let mut scoped = apply_overrides(
            &member.integration,
            Some(&member.overrides),
            adapter.config_fields(),
        );
        // Tag the member so its log rows attribute to this group.
        scoped.via_group = Some(LogGroup {
            id: group.id.clone(),
            name: group.name.clone(),
        });
        resolved.push(GroupedMember {
            ns,
            adapter,
            scoped,
        });
    }

    let instructions = group_instructions(&group.name, &resolved);
    let mut builder = SurfaceBuilder::default();
    builder.set_server_name(group.name.clone());
    builder.set_instructions(Some(instructions));
    for member in &resolved {
        member
            .adapter
            .register(
                &mut NamespacedHost::new(&mut builder, member.ns.clone()),
                &member.scoped,
                owner_id,
            )
            .map_err(|e| e.to_string())?;
    }
    builder.build()
}

/// Agent-facing guidance for a group endpoint: one server fronts several
/// integrations, each under a `<namespace>__` tool prefix. Each member's own
/// instructions block (built from its scoped config) is embedded under its
/// prefix, so a connecting agent learns not just which member to pick but how
/// each one is constrained.
fn group_instructions(group_name: &str, resolved: &[GroupedMember]) -> String {
    if resolved.is_empty() {
        return format!("Group \"{group_name}\" has no usable integrations.");
    }
    let blocks = resolved
        .iter()
        .map(|member| {
            let body = member
                .adapter
                .instructions(&member.scoped)
                .split('\n')
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Member tools prefixed \"{}__\":\n{}", member.ns, body)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let plural = if resolved.len() == 1 { "" } else { "s" };
    format!(
        "Group \"{group_name}\" fronts {count} integration{plural}. Each member's tools are prefixed \
         with its namespace (e.g. {first}__<tool>). Pick the member that matches your task; each \
         enforces its own policy, described below.\n\n{blocks}",
        count = resolved.len(),
        first = resolved[0].ns,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pluk_adapters::ConfigField;

    fn integration(config: serde_json::Value) -> Integration {
        Integration {
            id: "lin".into(),
            name: "Linear".into(),
            r#type: "linear".into(),
            config: config.as_object().cloned().unwrap_or_default(),
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    fn fields() -> Vec<ConfigField> {
        vec![
            ConfigField::new("api_key", "API key", FieldType::Password),
            ConfigField::new("team_key", "Team", FieldType::Text),
            ConfigField::new("limit", "Limit", FieldType::Number),
            ConfigField::new("active", "Active", FieldType::Toggle),
        ]
    }

    #[test]
    fn overrides_merge_coerced_to_declared_types() {
        let base = integration(serde_json::json!({ "api_key": "secret", "team_key": "ENG" }));
        let overrides = serde_json::from_str::<serde_json::Map<_, _>>(
            r#"{"team_key":"PROJ1","limit":"50","active":"true"}"#,
        )
        .unwrap();

        let scoped = apply_overrides(&base, Some(&overrides), &fields());

        assert_eq!(scoped.config["team_key"], serde_json::json!("PROJ1"));
        assert_eq!(scoped.config["limit"], serde_json::json!(50));
        assert_eq!(scoped.config["active"], serde_json::json!(true));
        assert_eq!(scoped.config["api_key"], serde_json::json!("secret"));
        // The base integration is untouched.
        assert_eq!(base.config["team_key"], serde_json::json!("ENG"));
    }

    #[test]
    fn blank_overrides_inherit_and_empties_are_noops() {
        let base = integration(serde_json::json!({ "team_key": "ENG" }));

        let blank = serde_json::json!({ "team_key": "" })
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(
            apply_overrides(&base, Some(&blank), &fields()).config["team_key"],
            serde_json::json!("ENG")
        );

        assert_eq!(apply_overrides(&base, None, &fields()).config, base.config);
        assert_eq!(
            apply_overrides(&base, Some(&Default::default()), &fields()).config,
            base.config
        );
    }

    #[test]
    fn numeric_override_that_parses_becomes_a_number() {
        let base = integration(serde_json::json!({}));
        let overrides = serde_json::json!({ "limit": "abc" })
            .as_object()
            .cloned()
            .unwrap();
        // Unparsable strings stay verbatim instead of collapsing to null.
        assert_eq!(
            apply_overrides(&base, Some(&overrides), &fields()).config["limit"],
            serde_json::json!("abc")
        );
    }
}
