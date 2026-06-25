/**
 * Per-tool configuration — the unified policy model for every adapter.
 *
 * Each adapter exposes a fixed set of tools. An integration turns individual
 * tools on or off (to shrink the surface the agent sees) and gives each enabled
 * tool its own settings (e.g. the SQL `query` tool's mode: read-only / mutations
 * / destructive). This replaces the old split policy model (SQL statement
 * categories vs. action read/write) with one shape stored in the existing
 * `query_policy` column — no schema change, and Swift already round-trips it.
 *
 * Stored shape:
 *   { "tools": { "<name>": { "enabled": bool, "settings": { ... } } } }
 *
 * A tool with no stored entry falls back to its declared default (read tools on,
 * write/delete tools off), so an unconfigured integration fails safe.
 */

export interface StoredToolState {
  enabled?: boolean;
  settings?: Record<string, unknown>;
}

export type ToolConfig = Record<string, StoredToolState>;

/** Resolved view of an integration's tool config, used by adapters at register time. */
export interface ToolGate {
  /** Whether `name` is enabled; `fallback` is the tool's declared default. */
  enabled(name: string, fallback: boolean): boolean;
  /** The stored settings for `name` (empty object if none). */
  settings(name: string): Record<string, unknown>;
}

/** Default-on state for a tool of the given coarse category: read tools are on
 *  by default; anything that can modify state (write/delete/admin) is off until
 *  the integration opts in. */
export function defaultEnabledForCategory(category: string): boolean {
  return category === "read" || category === "inspect";
}

/** Parse the `query_policy` blob into a tool config. Tolerates legacy/garbage. */
export function parseToolConfig(raw: string | null | undefined): ToolConfig {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as { tools?: unknown };
    if (parsed && typeof parsed === "object" && parsed.tools && typeof parsed.tools === "object") {
      return parsed.tools as ToolConfig;
    }
  } catch {
    // legacy/non-JSON policy → no per-tool config; everything falls to defaults
  }
  return {};
}

export function toolGate(raw: string | null | undefined): ToolGate {
  const tools = parseToolConfig(raw);
  return {
    enabled(name, fallback) {
      const state = tools[name];
      return state && state.enabled !== undefined ? !!state.enabled : fallback;
    },
    settings(name) {
      const state = tools[name];
      return state?.settings && typeof state.settings === "object" ? state.settings : {};
    },
  };
}

// ── Settings readers (typed accessors over the loose settings blob) ──────────

export function settingString(settings: Record<string, unknown>, key: string, fallback: string): string {
  const v = settings[key];
  return typeof v === "string" && v.length > 0 ? v : fallback;
}

export function settingBool(settings: Record<string, unknown>, key: string, fallback: boolean): boolean {
  const v = settings[key];
  if (typeof v === "boolean") return v;
  if (v === "true") return true;
  if (v === "false") return false;
  return fallback;
}

/** A numeric setting where a missing value (or a non-positive one) means "no limit" → null. */
export function settingNumberOrNull(settings: Record<string, unknown>, key: string, fallback: number | null): number | null {
  const v = settings[key];
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v) : NaN;
  if (Number.isFinite(n)) return n > 0 ? n : null;
  return fallback;
}
