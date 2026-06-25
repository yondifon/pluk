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
  danger?: boolean;                      // flag a risky setting (UI styles it red)
  help?: string;                         // one-line explanation shown under the control
}

/** How the policy/audit layer interprets this adapter.
 *  - "sql":    statement-category policy (SELECT/INSERT/…) + SQL guards.
 *  - "action": read/write action policy.
 *  - "none":   no policy gate; every call is confirmed by the client instead. */
export type PolicyKind = "sql" | "action" | "none";

/** Static description of one tool an adapter exposes, for the catalog/UI. Drives
 *  the per-tool enable toggle and the (optional) expandable settings form. Tool
 *  definitions don't depend on a live connection, so this is built once. */
export interface ToolSpec {
  name: string;
  description: string;
  /** Coarse class for grouping + default-on: "read" tools default enabled,
   *  "write"/"delete"/"admin" default disabled. */
  category: string;
  /** Whether this tool is on by default for a fresh integration. Derived from
   *  `category` when omitted. */
  defaultEnabled: boolean;
  /** This tool's own settings, rendered when the tool is expanded. Reuses the
   *  ConfigField shape; keys are scoped to the tool's settings object. */
  settings?: ConfigField[];
}

export interface Adapter {
  id: string;                            // matches Integration.type
  label: string;
  category: string;                      // "database" | "issue-tracker" | …
  policyKind: PolicyKind;
  agentHint: string;                      // shown in the UI beside the MCP URL
  /** The fixed set of tools this adapter exposes. Each is individually toggled
   *  on/off and may carry its own settings. Drives the catalog/UI tool list and
   *  the per-tool defaults used when an integration hasn't configured a tool. */
  toolSpecs: ToolSpec[];
  configFields: ConfigField[];
  /** Verify the config can reach the service. Throws on failure. */
  testConnection(integration: Integration): Promise<void>;
  /**
   * Agent-facing guidance for this integration, built per session from live
   * config + policy (see mcp/instructions.ts). Returned in the MCP `initialize`
   * handshake by the standalone server, and embedded per member by group
   * endpoints so the group's instructions reflect each member's real policy.
   */
  instructions(integration: Integration): string;
  /**
   * Register this integration's tools/resources/prompts onto a shared host. Used
   * by group endpoints to aggregate several integrations under one server; pass a
   * namespaced host to avoid name collisions across members.
   */
  register(host: ToolHost, integration: Integration, sessionIdRef: { value: string }): void;
}
