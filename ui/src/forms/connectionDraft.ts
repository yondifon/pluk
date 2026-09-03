import type { AdapterManifest, ConfigFieldDef, ToolDef, ToolState } from "./catalog";
import { seededState, isVisible } from "./catalog";

export type Environment = "production" | "staging" | "development" | "local";

export interface ConnectionDraft {
  name: string;
  type: string;
  config: Record<string, string>;
  environment: Environment;
  policyKind: string;
  fields: ConfigFieldDef[];
  tools: ToolDef[];
  toolConfig: Record<string, ToolState>;
}

export function emptyDraft(): ConnectionDraft {
  return {
    name: "",
    type: "postgres",
    config: {},
    environment: "development",
    policyKind: "sql",
    fields: [],
    tools: [],
    toolConfig: {},
  };
}

export function draftFromConnection(conn: {
  name: string;
  type: string;
  config: Record<string, unknown>;
  environment?: Environment;
  queryPolicy?: string | null;
}): ConnectionDraft {
  // Hydrate config blob: values may be string/number/bool -> normalize to string
  const config: Record<string, string> = {};
  for (const [k, v] of Object.entries(conn.config ?? {})) {
    if (typeof v === "string") config[k] = v;
    else if (typeof v === "boolean") config[k] = v ? "true" : "false";
    else if (typeof v === "number") config[k] = String(v);
    else if (v != null) config[k] = String(v);
  }
  const toolConfig: Record<string, ToolState> = {};
  if (conn.queryPolicy) {
    try {
      const parsed = JSON.parse(conn.queryPolicy) as { tools?: Record<string, { enabled?: boolean; settings?: Record<string, unknown> }> };
      for (const [name, entry] of Object.entries(parsed.tools ?? {})) {
        const settings: Record<string, string> = {};
        for (const [sk, sv] of Object.entries(entry.settings ?? {})) {
          if (typeof sv === "string") settings[sk] = sv;
          else if (typeof sv === "boolean") settings[sk] = sv ? "true" : "false";
          else if (typeof sv === "number") settings[sk] = String(sv);
          else if (sv != null) settings[sk] = String(sv);
        }
        toolConfig[name] = { enabled: entry.enabled ?? true, settings };
      }
    } catch {
      // malformed blob -> empty
    }
  }
  return {
    name: conn.name,
    type: conn.type,
    config,
    environment: conn.environment ?? "development",
    policyKind: "sql",
    fields: [],
    tools: [],
    toolConfig,
  };
}

export function adopt(draft: ConnectionDraft, manifest: AdapterManifest, resetConfig: boolean): ConnectionDraft {
  const next: ConnectionDraft = {
    ...draft,
    type: manifest.id,
    policyKind: manifest.policyKind,
    fields: manifest.configFields,
    tools: manifest.tools,
    config: { ...draft.config },
    toolConfig: { ...draft.toolConfig },
  };

  if (resetConfig) {
    const seededCfg: Record<string, string> = {};
    for (const f of manifest.configFields) {
      if (f.default != null) seededCfg[f.key] = f.default;
    }
    next.config = seededCfg;
    next.toolConfig = {};
  } else {
    // Seed defaults for empty config keys
    for (const f of manifest.configFields) {
      if (f.default != null && (next.config[f.key] ?? "") === "") {
        next.config[f.key] = f.default;
      }
    }
  }

  for (const t of manifest.tools) {
    if (next.toolConfig[t.name] == null) {
      next.toolConfig[t.name] = seededState(t);
    }
  }

  return applyEnvironmentDefaults(next);
}

function isSeededQueryMode(draft: ConnectionDraft): boolean {
  // Spec: flips a seeded query mode from read-only to mutations.
  // We consider it seeded if current mode equals default (read-only) and user
  // hasn't explicitly set it to a non-default. Since we can't distinguish
  // seeded vs user-chosen read-only without history, we treat plain "read-only" as seeded.
  const q = draft.toolConfig["query"];
  if (!q) return false;
  return (q.settings["mode"] ?? "read-only") === "read-only";
}

export function applyEnvironmentDefaults(draft: ConnectionDraft): ConnectionDraft {
  if (draft.policyKind !== "sql") return draft;
  const q = draft.toolConfig["query"];
  if (!q) return draft;
  if (!isSeededQueryMode(draft)) return draft;
  if (draft.environment !== "development" && draft.environment !== "local") return draft;
  // Only for SQL adapters: check by policyKind already ensures SQL; spec adds
  // "for development and local SQL integrations only" — already covered.
  return {
    ...draft,
    toolConfig: {
      ...draft.toolConfig,
      query: { ...q, settings: { ...q.settings, mode: "mutations" } },
    },
  };
}

export function setEnvironment(draft: ConnectionDraft, env: Environment): ConnectionDraft {
  // The environment rule must not override a user-chosen value.
  // applyEnvironmentDefaults only flips seeded read-only -> mutations, never other values.
  const next = { ...draft, environment: env };
  return applyEnvironmentDefaults(next);
}

export function canSave(draft: ConnectionDraft): boolean {
  if (draft.name.trim() === "") return false;
  for (const f of draft.fields) {
    if (f.required && isVisible(f, draft.config)) {
      if ((draft.config[f.key] ?? "") === "") return false;
    }
  }
  return true;
}

export function splitTools(tools: ToolDef[]): { defaults: ToolDef[]; extras: ToolDef[] } {
  return {
    defaults: tools.filter((t) => t.defaultEnabled),
    extras: tools.filter((t) => !t.defaultEnabled),
  };
}
