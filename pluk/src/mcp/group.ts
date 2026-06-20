import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { getAdapter, type ConfigField } from "../adapters/index.js";
import { resolveMembers, type Group } from "../store/groups.js";
import type { Integration } from "../store/integrations.js";
import { namespacedHost, slug } from "./namespace.js";
import { logError } from "../log.js";

// Merge a member's per-group overrides into its config, coercing each value to the
// type the adapter declared for that field (number/toggle) so e.g. a Linear
// `team_key` override or a per-group database name lands with the right type.
export function applyOverrides(
  integration: Integration,
  overrides: Record<string, unknown> | undefined,
  fields: ConfigField[]
): Integration {
  if (!overrides || Object.keys(overrides).length === 0) return integration;
  const typeByKey = new Map(fields.map((f) => [f.key, f.type]));
  const coerced: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(overrides)) {
    if (value === "" || value === null || value === undefined) continue; // blank → inherit
    const type = typeByKey.get(key);
    if (type === "number") coerced[key] = typeof value === "string" ? Number(value) : value;
    else if (type === "toggle") coerced[key] = value === true || value === "true";
    else coerced[key] = value;
  }
  return { ...integration, config: { ...integration.config, ...coerced } };
}

// Build one MCP server that aggregates every member integration of a group. Each
// member's tools/resources/prompts are registered through a namespaced host
// (prefix = slug of the member name) so identically-named tools across members
// (e.g. two SQL DBs each exposing "query") don't collide. Per-member overrides
// are merged into the integration's config before registration.
export function buildGroupServer(group: Group, sessionIdRef: { value: string }): McpServer {
  const server = new McpServer({ name: group.name, version: "1.0.0" });

  const members = resolveMembers(group);
  const used = new Map<string, number>();

  for (const { integration, overrides } of members) {
    const adapter = getAdapter(integration.type);
    if (!adapter) {
      logError("group member has no adapter", new Error(integration.type), { group: group.name, member: integration.name });
      continue;
    }
    // Disambiguate members that slugify to the same prefix (e.g. two "Prod DB").
    let ns = slug(integration.name);
    const seen = used.get(ns) ?? 0;
    used.set(ns, seen + 1);
    if (seen > 0) ns = `${ns}_${seen + 1}`;

    const scoped = applyOverrides(integration, overrides, adapter.configFields);
    adapter.register(namespacedHost(server, ns), scoped, sessionIdRef);
  }

  return server;
}
