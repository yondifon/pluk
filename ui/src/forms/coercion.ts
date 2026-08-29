import type { ConfigFieldDef } from "./catalog.ts";

export function coerceToStored(field: ConfigFieldDef, uiValue: string): unknown {
  if (uiValue === "") return undefined;
  switch (field.type) {
    case "number": {
      const n = Number(uiValue);
      if (uiValue.trim() === "" || Number.isNaN(n)) return uiValue;
      // Store as integer when integral, mirroring Swift's Int(value) branch
      return Number.isInteger(n) ? Math.trunc(n) : n;
    }
    case "toggle":
      return uiValue === "true";
    default:
      return uiValue;
  }
}

export function coerceFromStored(field: ConfigFieldDef, stored: unknown): string {
  if (stored == null) return "";
  switch (field.type) {
    case "toggle":
      if (typeof stored === "boolean") return stored ? "true" : "false";
      if (typeof stored === "string") return stored;
      return stored ? "true" : "false";
    case "number":
      return String(stored);
    default:
      return String(stored);
  }
}

export function serializeConfig(fields: ConfigFieldDef[], config: Record<string, string>): Record<string, unknown> {
  const typeByKey = new Map(fields.map((f) => [f.key, f.type] as const));
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(config)) {
    if (value === "") continue;
    const t = typeByKey.get(key);
    const field: ConfigFieldDef = {
      key,
      label: key,
      type: (t ?? "text") as ConfigFieldDef["type"],
    };
    const coerced = coerceToStored(field, value);
    if (coerced !== undefined) out[key] = coerced;
  }
  return out;
}

export function parseConfig(fields: ConfigFieldDef[], stored: Record<string, unknown>): Record<string, string> {
  const typeByKey = new Map(fields.map((f) => [f.key, f.type] as const));
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(stored)) {
    const t = typeByKey.get(key);
    const field: ConfigFieldDef = {
      key,
      label: key,
      type: (t ?? "text") as ConfigFieldDef["type"],
    };
    out[key] = coerceFromStored(field, value);
  }
  return out;
}

export function serializeToolSettings(
  tools: Array<{ name: string; settings?: ConfigFieldDef[] }>,
  toolConfig: Record<string, { enabled: boolean; settings: Record<string, string> }>,
): Record<string, { enabled: boolean; settings: Record<string, unknown> }> {
  const typeByToolAndKey = new Map<string, Map<string, string>>();
  for (const t of tools) {
    const m = new Map<string, string>();
    for (const s of t.settings ?? []) m.set(s.key, s.type);
    typeByToolAndKey.set(t.name, m);
  }
  const out: Record<string, { enabled: boolean; settings: Record<string, unknown> }> = {};
  for (const [name, state] of Object.entries(toolConfig)) {
    const types = typeByToolAndKey.get(name);
    const settings: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(state.settings)) {
      if (value === "") continue;
      const t = types?.get(key);
      const f: ConfigFieldDef = { key, label: key, type: (t as ConfigFieldDef["type"]) ?? "text" };
      const c = coerceToStored(f, value);
      if (c !== undefined) settings[key] = c;
    }
    out[name] = { enabled: state.enabled, settings };
  }
  return out;
}
