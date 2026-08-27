export type Environment = "production" | "staging" | "development" | "local";

export interface Integration {
  id: string;
  name: string;
  type: string;
  environment?: Environment;
  config: Record<string, string>;
  toolConfig: Record<string, { enabled: boolean; settings: Record<string, string> }>;
  token: string;
  createdAt: string;
  readOnly?: boolean;
}

export interface ConfigField {
  key: string;
  label: string;
  type: string;
  secret?: boolean;
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
  added: string[];
  skipped: string[];
  failed: Array<{ client: string; reason: string }>;
}
