export type Environment = "production" | "staging" | "development" | "local";

/** The Wande integration's type (`pluk_browser::INTEGRATION_TYPE`). */
export const WANDE_TYPE = "wande";

/** The type of an integration that re-exposes another MCP server's tools. */
export const MCP_TYPE = "mcp";

export interface Integration {
  id: string;
  name: string;
  type: string;
  environment?: Environment | null;
  /**
   * Holds no secret values; `secretsSet` names the secret fields that are
   * saved. A key/value field reads as the names of its rows.
   */
  config: Record<string, string>;
  secretsSet?: string[];
  toolConfig: Record<string, { enabled: boolean; settings: Record<string, string> }>;
  /** This integration's own tools, when its adapter publishes a list per integration. */
  tools?: ToolSpec[];
  approvals?: { ask: boolean; allow: string[]; deny: string[] };
  /** Tools an imported config turned off that the server has not listed yet. */
  pendingToolsOff?: string[];
  token: string;
  createdAt: string;
  readOnly?: boolean;
}

export interface ConfigField {
  key: string;
  label: string;
  type: string;
  secret?: boolean;
  required?: boolean;
  default?: string;
  options?: Array<{ value: string; label: string }>;
}

export interface ToolSpec {
  name: string;
  description: string;
  category: string;
  defaultEnabled: boolean;
  settings?: ConfigField[];
}

export interface AdapterManifest {
  id: string;
  label: string;
  category: string;
  agentHint: string;
  tools: ToolSpec[];
  configFields: ConfigField[];
}

export interface ConnHealth {
  status: "ok" | "error";
  error?: string | null;
  at: number;
}

export type ConnStatus = "ok" | "failing" | "unknown";

export type McpClientId = "opencode" | "codex" | "claudeCode" | "cursor" | "windsurf" | "antigravity";

export interface McpClientMeta {
  id: McpClientId;
  label: string;
  supportsProject: boolean;
}

export type ConfigScope = "global" | "project";

export interface FanOutResult {
  added: Array<{ client: string; path: string }>;
  skipped: Array<{ client: string; path: string }>;
  failed: Array<{ client: string; reason: string }>;
}
