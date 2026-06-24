import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { Integration } from "../store/integrations.js";
import type { Adapter } from "./types.js";
import { sqlAdapters } from "./sql/index.js";
import { linearAdapter } from "./linear/index.js";
import { sentryAdapter } from "./sentry/index.js";
import { sshAdapter } from "./ssh/index.js";
import { githubAdapter } from "./github/index.js";
import { redisAdapter } from "./redis/index.js";
import { slackAdapter } from "./slack/index.js";

// The adapter registry. To add a service: build an Adapter module and register
// it here. Nothing else (store, MCP transport, REST layer, UI) needs editing.
const registry = new Map<string, Adapter>();

function register(adapters: Adapter[]): void {
  for (const adapter of adapters) {
    if (registry.has(adapter.id)) throw new Error(`Duplicate adapter id: ${adapter.id}`);
    registry.set(adapter.id, adapter);
  }
}

register(sqlAdapters);
register([linearAdapter, sentryAdapter, sshAdapter, githubAdapter, redisAdapter, slackAdapter]);

export function getAdapter(type: string): Adapter | undefined {
  return registry.get(type);
}

export function listAdapters(): Adapter[] {
  return [...registry.values()];
}

/**
 * Build a standalone MCP server for a single integration: a bare McpServer
 * carrying the adapter's session instructions, with its surface registered onto
 * it. Group endpoints register members onto a shared (namespaced) host instead.
 */
export function buildAdapterServer(
  adapter: Adapter,
  integration: Integration,
  sessionIdRef: { value: string },
): McpServer {
  const server = new McpServer(
    { name: integration.name, version: "1.0.0" },
    { instructions: adapter.instructions(integration) },
  );
  adapter.register(server, integration, sessionIdRef);
  return server;
}

export type { Adapter, ConfigField, FieldType, PolicyKind } from "./types.js";
