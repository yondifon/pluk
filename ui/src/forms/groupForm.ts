import type { AdapterManifest, ConfigFieldDef } from "./catalog.ts";

export interface GroupMember {
  id: string;
  overrides: Record<string, string>;
}

export interface GroupDraft {
  name: string;
  environment: string | null; // null = any/mixed
  included: Set<string>;
  overrides: Record<string, Record<string, string>>;
}

export function groupDraftFrom(group: { name: string; environment?: string | null; members: GroupMember[] }): GroupDraft {
  return {
    name: group.name,
    environment: group.environment ?? null,
    included: new Set(group.members.map((m) => m.id)),
    overrides: Object.fromEntries(group.members.map((m) => [m.id, { ...m.overrides }])),
  };
}

export function canSaveGroup(draft: GroupDraft): boolean {
  return draft.name.trim() !== "";
}

export function overridableFields(manifest: AdapterManifest | undefined): ConfigFieldDef[] {
  if (!manifest) return [];
  return manifest.configFields.filter((f) => !(f.secret ?? false));
}

export function inheritPlaceholder(
  connConfig: Record<string, string>,
  field: ConfigFieldDef,
): string {
  const current = connConfig[field.key];
  if (current != null && current !== "") return `inherit (${current})`;
  return field.placeholder ?? "inherit";
}

export function updateOverride(
  overrides: Record<string, Record<string, string>>,
  connId: string,
  key: string,
  rawValue: string,
): Record<string, Record<string, string>> {
  const next = { ...overrides };
  const m = { ...(next[connId] ?? {}) };
  const trimmed = rawValue.trim();
  if (trimmed === "") {
    delete m[key];
  } else {
    m[key] = rawValue;
  }
  next[connId] = m;
  return next;
}

export function serializeGroup(draft: GroupDraft, orderedConnections: Array<{ id: string }>): GroupMember[] {
  return orderedConnections
    .filter((c) => draft.included.has(c.id))
    .map((c) => {
      const ov = overridesOrEmpty(draft.overrides[c.id] ?? {});
      return { id: c.id, overrides: ov };
    });
}

function overridesOrEmpty(m: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(m)) {
    if (v !== "" && v != null) out[k] = v;
  }
  return out;
}
