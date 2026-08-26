//! Group namespacing: slug prefixes for tool/prompt/resource names.
//!
//! A group exposes several integrations through one MCP server. Their
//! tool/prompt/resource names collide (two SQL DBs both register `query`), so
//! in group mode each member registers through a [`namespaced`] host that
//! prefixes every name with a per-member slug. Single-integration endpoints
//! register on the bare builder and are unaffected.
//!
//! Ported from `pluk/src/mcp/namespace.ts`.

use pluk_adapters::ToolHost;

/// Slugify a member name into a tool-name-safe prefix segment.
pub fn slug(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_sep = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            last_was_sep = false;
        } else if !last_was_sep && !out.is_empty() {
            out.push('_');
            last_was_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() { "member".into() } else { out }
}

/// Prefix a resource URI so two members' URIs (e.g. `schema://full`) stay unique.
pub fn namespace_uri(ns: &str, uri: &str) -> String {
    match uri.find("://") {
        Some(sep) => format!("{}://{}/{}", &uri[..sep], ns, &uri[sep + 3..]),
        None => format!("{ns}+{uri}"),
    }
}

/// Wrap a host so tool/prompt/resource registrations are prefixed with `ns`.
/// Names become `<ns>__<name>`; resource URIs are namespaced too.
pub struct NamespacedHost<'a> {
    inner: &'a mut dyn ToolHost,
    ns: String,
}

impl<'a> NamespacedHost<'a> {
    pub fn new(inner: &'a mut dyn ToolHost, ns: impl Into<String>) -> Self {
        NamespacedHost { inner, ns: ns.into() }
    }

    fn prefix(&self, name: &str) -> String {
        format!("{}__{}", self.ns, name)
    }
}

impl ToolHost for NamespacedHost<'_> {
    fn register_tool(
        &mut self,
        registration: pluk_adapters::ToolRegistration,
        handler: pluk_adapters::ToolHandler,
    ) {
        let registration = pluk_adapters::ToolRegistration { name: self.prefix(&registration.name), ..registration };
        self.inner.register_tool(registration, handler);
    }

    fn register_prompt(
        &mut self,
        name: &str,
        description: &str,
        args_schema: Option<serde_json::Map<String, serde_json::Value>>,
        handler: pluk_adapters::PromptHandler,
    ) {
        self.inner.register_prompt(&self.prefix(name), description, args_schema, handler);
    }

    fn register_resource(
        &mut self,
        name: &str,
        uri: &str,
        mime_type: &str,
        description: Option<&str>,
        handler: pluk_adapters::ResourceHandler,
    ) {
        let uri = namespace_uri(&self.ns, uri);
        self.inner.register_resource(&self.prefix(name), &uri, mime_type, description, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use pluk_adapters::{PromptHandler, ResourceHandler, ToolHandler};

    #[derive(Default)]
    struct RecordingHost {
        calls: Vec<String>,
    }

    impl ToolHost for RecordingHost {
        fn register_tool(&mut self, registration: pluk_adapters::ToolRegistration, _handler: ToolHandler) {
            self.calls.push(format!("tool:{}", registration.name));
        }

        fn register_prompt(&mut self, name: &str, _description: &str, _args_schema: Option<serde_json::Map<String, serde_json::Value>>, _handler: PromptHandler) {
            self.calls.push(format!("prompt:{name}"));
        }

        fn register_resource(&mut self, name: &str, uri: &str, _mime_type: &str, _description: Option<&str>, _handler: ResourceHandler) {
            self.calls.push(format!("res:{name}:{uri}"));
        }
    }

    fn noop_tool() -> ToolHandler {
        Arc::new(|_| Box::pin(async { unreachable!() }))
    }

    #[test]
    fn slug_makes_a_tool_safe_prefix() {
        assert_eq!(slug("Metrics DB"), "metrics_db");
        assert_eq!(slug("DB — Production!"), "db_production");
        assert_eq!(slug(""), "member");
        assert_eq!(slug("__--__"), "member");
    }

    #[test]
    fn namespaced_host_prefixes_names_and_uris() {
        let mut fake = RecordingHost::default();
        {
            let mut host = NamespacedHost::new(&mut fake, "metrics_db");
            host.register_tool(pluk_adapters::ToolRegistration::no_args("query", "Q"), noop_tool());
            host.register_prompt("summarize_schema", "S", None, Arc::new(|_| Box::pin(async { unreachable!() })));
            host.register_resource("schema", "schema://full", "text/plain", None, Arc::new(|| Box::pin(async { unreachable!() })));
        }
        assert_eq!(
            fake.calls,
            [
                "tool:metrics_db__query".to_string(),
                "prompt:metrics_db__summarize_schema".to_string(),
                "res:metrics_db__schema:schema://metrics_db/full".to_string(),
            ]
        );
    }

    #[test]
    fn uris_without_a_scheme_get_a_plus_namespace() {
        assert_eq!(namespace_uri("a", "table/users"), "a+table/users");
    }
}
