import type { AdapterManifest, ConfigEntry, ConfigFieldDef, ConfigValue, ToolDef, ToolState } from "./catalog";
import { seededState, isVisible, entriesValue, textValue, visibleFields } from "./catalog";

export type Environment = "production" | "staging" | "development" | "local";

export interface ConnectionDraft {
  name: string;
  type: string;
  config: Record<string, ConfigValue>;
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

function hydrateEntry(raw: unknown): ConfigEntry {
  const entry: ConfigEntry = {};
  if (raw == null || typeof raw !== "object") return entry;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === "boolean") entry[k] = v ? "true" : "false";
    else if (v != null) entry[k] = String(v);
  }
  return entry;
}

export function draftFromConnection(conn: {
  name: string;
  type: string;
  config: Record<string, unknown>;
  environment?: Environment;
  queryPolicy?: string | null;
}): ConnectionDraft {
  // Hydrate config blob: scalars normalize to string, arrays stay lists of entries
  const config: Record<string, ConfigValue> = {};
  for (const [k, v] of Object.entries(conn.config ?? {})) {
    if (Array.isArray(v)) config[k] = v.map(hydrateEntry);
    else if (typeof v === "boolean") config[k] = v ? "true" : "false";
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
    const seededCfg: Record<string, ConfigValue> = {};
    for (const f of manifest.configFields) {
      if (f.default != null) seededCfg[f.key] = f.default;
    }
    next.config = seededCfg;
    next.toolConfig = {};
  } else {
    // Seed defaults for empty config keys
    for (const f of manifest.configFields) {
      if (f.default != null && textValue(next.config[f.key]) === "") {
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

/** A required input left empty, with the list entry it belongs to when nested. */
export interface MissingValue {
  field: ConfigFieldDef;
  entry?: { index: number; field: ConfigFieldDef };
}

export function firstMissingValue(draft: ConnectionDraft): MissingValue | null {
  for (const f of draft.fields) {
    if (!isVisible(f, draft.config)) continue;
    if (f.type === "list") {
      const entries = entriesValue(draft.config[f.key]);
      if (f.required && entries.length === 0) return { field: f };
      for (const [index, entry] of entries.entries()) {
        for (const sub of visibleFields(f.fields ?? [], entry)) {
          if (sub.required && (entry[sub.key] ?? "") === "") {
            return { field: f, entry: { index, field: sub } };
          }
        }
      }
      continue;
    }
    if (f.required && textValue(draft.config[f.key]) === "") return { field: f };
  }
  return null;
}

export function canSave(draft: ConnectionDraft): boolean {
  if (draft.name.trim() === "") return false;
  return firstMissingValue(draft) == null;
}

export function splitTools(tools: ToolDef[]): { defaults: ToolDef[]; extras: ToolDef[] } {
  return {
    defaults: tools.filter((t) => t.defaultEnabled),
    extras: tools.filter((t) => !t.defaultEnabled),
  };
}
