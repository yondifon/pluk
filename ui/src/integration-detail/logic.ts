import type { AdapterManifest, ConfigField, ConnHealth, ConnStatus, FanOutResult, Integration, ToolSpec } from "./types";

export function deriveStatus(health: ConnHealth | null | undefined): ConnStatus {
  if (!health) return "unknown";
  return health.status === "error" ? "failing" : "ok";
}

export function statusLabel(status: ConnStatus): string {
  switch (status) {
    case "ok":
      return "Healthy";
    case "failing":
      return "Failing";
    case "unknown":
      return "Not checked";
  }
}

export function statusColor(status: ConnStatus): string {
  switch (status) {
    case "ok":
      return "var(--status-success)";
    case "failing":
      return "var(--status-danger)";
    case "unknown":
      return "var(--surface-tertiary-label)";
  }
}

export function formatRelativeTime(checkedAt: number | undefined | null): string | null {
  if (checkedAt == null) return null;
  // checkedAt is epoch ms like Swift's `at`
  const s = Math.floor((Date.now() - checkedAt) / 1000);
  if (s < 0) return null;
  if (s < 60) return `${Math.max(s, 1)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function isToolEnabled(tool: ToolSpec, toolConfig: Integration["toolConfig"]): boolean {
  const state = toolConfig[tool.name];
  if (state != null) return state.enabled;
  return tool.defaultEnabled;
}

export function enabledCount(tools: ToolSpec[], toolConfig: Integration["toolConfig"]): number {
  return tools.filter((t) => isToolEnabled(t, toolConfig)).length;
}

export function orderedTools(tools: ToolSpec[], toolConfig: Integration["toolConfig"]): ToolSpec[] {
  const enabled = tools.filter((t) => isToolEnabled(t, toolConfig));
  const disabled = tools.filter((t) => !isToolEnabled(t, toolConfig));
  return [...enabled, ...disabled];
}

export function settingsSummary(tool: ToolSpec, toolConfig: Integration["toolConfig"]): string | null {
  if (!tool.settings || tool.settings.length === 0) return null;
  const state = toolConfig[tool.name];
  const parts: string[] = [];
  for (const f of tool.settings) {
    const raw = state?.settings[f.key] ?? f.default ?? "";
    if (!raw) continue;
    const display = f.options?.find((o) => o.value === raw)?.label ?? raw;
    parts.push(`${f.label}: ${display}`);
  }
  return parts.length ? parts.join(" · ") : null;
}

const MASK = "••••••";

export function genericConfigRows(
  config: Record<string, string>,
  fields: ConfigField[],
): Array<[string, string]> {
  const secretKeys = new Set(fields.filter((f) => f.secret).map((f) => f.key));
  return Object.keys(config)
    .sort()
    .map((key) => {
      const pretty = key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
      const value = secretKeys.has(key) ? MASK : config[key];
      return [pretty, value] as [string, string];
    });
}

export function overviewRows(
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
): Array<[string, string]> {
  const fields = manifest?.configFields ?? [];
  const secretKeys = new Set(fields.filter((f) => f.secret).map((f) => f.key));
  const masked = (key: string, fallback = "-"): string => {
    if (secretKeys.has(key)) return MASK;
    return integration.config[key] ?? fallback;
  };

  if (integration.type === "sqlite") {
    const sshOn = integration.config["use_ssh"] === "true";
    return [
      ["File", masked("filename")],
      ["SSH", sshOn ? masked("ssh_host") : "Off"],
    ];
  }

  // networked database: host/port/user/database present
  const hasConnectionType = integration.config["host"] != null || integration.config["port"] != null;
  // Also treat postgres/mysql/sqlite generic fallback: if adapter category is database and not sqlite
  const isNetworked = hasConnectionType || ["postgres", "mysql", "postgresql"].includes(integration.type);
  if (isNetworked) {
    const sshOn = integration.config["use_ssh"] === "true";
    const sslOn = integration.config["use_ssl"] === "true";
    return [
      ["Host", masked("host")],
      ["Port", masked("port")],
      ["User", masked("user")],
      ["Database", masked("database")],
      ["SSH", sshOn ? masked("ssh_host") : "Off"],
      ["SSL", sslOn ? masked("ssl_mode", "On") : "Off"],
    ];
  }

  return genericConfigRows(integration.config, fields);
}

export function formatMetaLine(integration: Integration, manifest: AdapterManifest | null | undefined): string {
  const env = integration.environment ?? "development";
  const envLabel = env.charAt(0).toUpperCase() + env.slice(1);
  const typeLabel = manifest?.label ?? integration.type;
  const parts = [`${typeLabel} · ${envLabel}`];
  const tools = manifest?.tools ?? [];
  if (tools.length) {
    parts.push(`${enabledCount(tools, integration.toolConfig)}/${tools.length} tools`);
  }
  return parts.join("  ·  ");
}

export function mcpUrl(token: string): string {
  return `http://localhost:4242/mcp/${token}`;
}

export function endpointCopyConfirmState(): { copied: boolean; trigger(): void; reset(): void } {
  let copied = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  return {
    get copied() {
      return copied;
    },
    trigger() {
      copied = true;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        copied = false;
      }, 1500);
    },
    reset() {
      copied = false;
      if (timer) clearTimeout(timer);
    },
  };
}

// Fan-out result message: actionable, no internal vocab.
export function formatFanOutMessage(key: string, result: FanOutResult): { kind: "success" | "error"; message: string } {
  const { added, skipped, failed } = result;

  if (added.length === 0 && skipped.length === 0 && failed.length === 0) {
    return { kind: "success", message: "Nothing to update." };
  }

  // Single target: keep path-like wording short but product-facing.
  const totalTargets = added.length + skipped.length + failed.length;
  if (totalTargets === 1) {
    if (failed.length === 1) {
      return { kind: "error", message: `Couldn’t update ${failed[0].client}: ${failed[0].reason}` };
    }
    if (added.length === 1) {
      return { kind: "success", message: `Added “${key}” to ${added[0]}` };
    }
    return { kind: "success", message: `“${key}” already in ${skipped[0]} — left unchanged` };
  }

  const parts: string[] = [];
  if (added.length) parts.push(`Added to ${added.join(", ")}`);
  if (skipped.length) parts.push(`Already set up in ${skipped.join(", ")}`);
  if (failed.length) {
    const fails = failed.map((f) => `${f.client}: ${f.reason}`).join("; ");
    parts.push(`Couldn’t update ${fails}`);
  }
  const message = parts.join(" · ");
  const kind = failed.length ? "error" : "success";
  return { kind, message };
}

export function prettyPath(path: string): string {
  // Browser has no HOME; keep as-is. Tauri side expands ~ already.
  return path;
}
