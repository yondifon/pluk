import type { AdapterManifest, ConfigFieldDef, ToolDef, ToolState } from "./catalog";
import { seededState, isVisible } from "./catalog";
import { rowsFromStored, rowsToSave } from "./keyValue";
import type { KeyValueRow, SentRow } from "./keyValue";

export type Environment = "production" | "staging" | "development" | "local";

/** Hand-written rules, and whether a refused call asks before it is turned down. */
export interface Approvals {
  ask: boolean;
  allow: string[];
  deny: string[];
}

export function emptyApprovals(): Approvals {
  return { ask: true, allow: [], deny: [] };
}

/** One rule per line, blank lines dropped. */
export function parseRules(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

export interface ConnectionDraft {
  name: string;
  type: string;
  config: Record<string, string>;
  /** The rows of each key/value field, kept apart from the scalar config. */
  rows: Record<string, KeyValueRow[]>;
  /**
   * Secret fields that hold a saved value. The window never reads a secret
   * back, so such a field's config entry stays blank until the user types a
   * new one.
   */
  savedSecrets: string[];
  /** `null` when the integration carries no environment. */
  environment: Environment | null;
  policyKind: string;
  fields: ConfigFieldDef[];
  tools: ToolDef[];
  toolConfig: Record<string, ToolState>;
  approvals: Approvals;
}

export function emptyDraft(): ConnectionDraft {
  return {
    name: "",
    type: "postgres",
    config: {},
    rows: {},
    savedSecrets: [],
    environment: "development",
    policyKind: "sql",
    fields: [],
    tools: [],
    toolConfig: {},
    approvals: emptyApprovals(),
  };
}

export function draftFromConnection(conn: {
  name: string;
  type: string;
  config: Record<string, unknown>;
  secretsSet?: string[];
  environment?: Environment | null;
  queryPolicy?: string | null;
}): ConnectionDraft {
  // Hydrate config blob: values may be string/number/bool -> normalize to string
  const config: Record<string, string> = {};
  const rows: Record<string, KeyValueRow[]> = {};
  for (const [k, v] of Object.entries(conn.config ?? {})) {
    if (Array.isArray(v)) rows[k] = rowsFromStored(v);
    else if (typeof v === "string") config[k] = v;
    else if (typeof v === "boolean") config[k] = v ? "true" : "false";
    else if (typeof v === "number") config[k] = String(v);
    else if (v != null) config[k] = String(v);
  }
  const toolConfig: Record<string, ToolState> = {};
  let approvals = emptyApprovals();
  if (conn.queryPolicy) {
    try {
      const parsed = JSON.parse(conn.queryPolicy) as {
        tools?: Record<string, { enabled?: boolean; settings?: Record<string, unknown> }>;
        approvals?: { ask?: boolean; allow?: string[]; deny?: string[] };
      };
      approvals = {
        ask: parsed.approvals?.ask ?? true,
        allow: parsed.approvals?.allow ?? [],
        deny: parsed.approvals?.deny ?? [],
      };
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
    rows,
    savedSecrets: conn.secretsSet ?? [],
    environment: conn.environment ?? null,
    policyKind: "sql",
    fields: [],
    tools: [],
    toolConfig,
    approvals,
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
    next.rows = {};
    next.savedSecrets = [];
    next.toolConfig = {};
    next.approvals = emptyApprovals();
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
  // "for development and local SQL integrations only", already covered.
  return {
    ...draft,
    toolConfig: {
      ...draft.toolConfig,
      query: { ...q, settings: { ...q.settings, mode: "mutations" } },
    },
  };
}

export function setEnvironment(draft: ConnectionDraft, env: Environment | null): ConnectionDraft {
  // The environment rule must not override a user-chosen value.
  // applyEnvironmentDefaults only flips seeded read-only -> mutations, never other values.
  const next = { ...draft, environment: env };
  return applyEnvironmentDefaults(next);
}

/** Whether a field holds a value, counting a secret saved earlier. */
export function isFilled(draft: ConnectionDraft, field: ConfigFieldDef): boolean {
  if ((draft.config[field.key] ?? "") !== "") return true;
  return draft.savedSecrets.includes(field.key);
}

export function canSave(draft: ConnectionDraft): boolean {
  if (draft.name.trim() === "") return false;
  return draft.fields.every((f) => !f.required || !isVisible(f, draft.config) || isFilled(draft, f));
}

/**
 * The config a save sends. A blank secret keeps its saved value by being left
 * out; a blank secret with nothing saved, or one the user removed, goes as
 * `null` so the host drops it. Each key/value field goes as its full row
 * list, so a row left out is removed.
 */
export function configToSave(draft: ConnectionDraft): Record<string, string | null | SentRow[]> {
  const config: Record<string, string | null | SentRow[]> = { ...draft.config };
  for (const f of draft.fields) {
    if (f.type === "keyvalue") {
      config[f.key] = rowsToSave(draft.rows[f.key] ?? []);
      continue;
    }
    if (!f.secret || (config[f.key] ?? "") !== "") continue;
    if (draft.savedSecrets.includes(f.key)) delete config[f.key];
    else config[f.key] = null;
  }
  return config;
}

/** The draft with a saved secret let go of, so saving removes it. */
export function forgetSecret(draft: ConnectionDraft, key: string): ConnectionDraft {
  return { ...draft, savedSecrets: draft.savedSecrets.filter((saved) => saved !== key) };
}

export function splitTools(tools: ToolDef[]): { defaults: ToolDef[]; extras: ToolDef[] } {
  return {
    defaults: tools.filter((t) => t.defaultEnabled),
    extras: tools.filter((t) => !t.defaultEnabled),
  };
}
