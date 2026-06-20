import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { Integration } from "../store/integrations.js";
import type { ToolHost } from "../mcp/namespace.js";

/**
 * The adapter contract. One module per service (a DB family, Linear, Sentry, …)
 * exports an Adapter and registers it in `adapters/index.ts`. Adding a service
 * means adding one module — no edits to the store, MCP transport, or REST layer.
 */

export type FieldType = "text" | "password" | "number" | "file" | "select" | "toggle";

/** A single config input, rendered dynamically by the UI form. */
export interface ConfigField {
  key: string;                          // -> integration.config[key]
  label: string;
  type: FieldType;
  group?: string;                       // UI section, e.g. "Connection", "Auth"
  required?: boolean;
  secret?: boolean;                     // never echoed back to the UI
  placeholder?: string;
  default?: unknown;
  options?: { value: string; label: string }[];  // for `select`
  showIf?: { key: string; equals: unknown };      // conditional visibility
  fileTypes?: string[];                  // for `file` picker (pem/key/sqlite…)
}

/** How the policy/audit layer interprets this adapter. */
export type PolicyKind = "sql" | "action";

export interface Adapter {
  id: string;                            // matches Integration.type
  label: string;
  category: string;                      // "database" | "issue-tracker" | …
  policyKind: PolicyKind;
  configFields: ConfigField[];
  /** Verify the config can reach the service. Throws on failure. */
  testConnection(integration: Integration): Promise<void>;
  /** Build a standalone MCP server (tools/resources/prompts) for one session. */
  buildServer(integration: Integration, sessionIdRef: { value: string }): McpServer;
  /**
   * Register this integration's tools/resources/prompts onto a shared host. Used
   * by group endpoints to aggregate several integrations under one server; pass a
   * namespaced host to avoid name collisions across members.
   */
  register(host: ToolHost, integration: Integration, sessionIdRef: { value: string }): void;
}
