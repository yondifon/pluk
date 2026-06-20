import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

// A group exposes several integrations through one MCP server. Their tool/prompt/
// resource names collide (two SQL DBs both register "query"), so in group mode we
// register each member through a namespaced host that prefixes every name with a
// per-member slug. Single-integration endpoints register on the bare McpServer
// and are unaffected.

/** The subset of McpServer an adapter uses to register its surface. */
export type ToolHost = Pick<McpServer, "tool" | "prompt" | "resource">;

/** Slugify a member name into a tool-name-safe prefix segment. */
export function slug(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "member";
}

/** Prefix a resource URI so two members' URIs (e.g. `schema://full`) stay unique. */
function namespaceUri(ns: string, uri: string): string {
  const sep = uri.indexOf("://");
  if (sep === -1) return `${ns}+${uri}`;
  return `${uri.slice(0, sep)}://${ns}/${uri.slice(sep + 3)}`;
}

/**
 * Wrap a real McpServer so tool/prompt/resource registrations are prefixed with
 * `ns`. Names become `${ns}__${name}`; resource URIs are namespaced too.
 */
export function namespacedHost(server: McpServer, ns: string): ToolHost {
  const prefix = (name: string) => `${ns}__${name}`;
  return {
    tool: ((name: string, ...rest: unknown[]) =>
      (server.tool as (...a: unknown[]) => unknown)(prefix(name), ...rest)) as McpServer["tool"],
    prompt: ((name: string, ...rest: unknown[]) =>
      (server.prompt as (...a: unknown[]) => unknown)(prefix(name), ...rest)) as McpServer["prompt"],
    resource: ((name: string, uri: string, ...rest: unknown[]) =>
      (server.resource as (...a: unknown[]) => unknown)(prefix(name), namespaceUri(ns, uri), ...rest)) as McpServer["resource"],
  };
}
