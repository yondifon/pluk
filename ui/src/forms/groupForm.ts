import type { AdapterManifest, ConfigFieldDef, ToolDef, ToolState } from "./catalog.ts";

export interface GroupMember {
  id: string;
  overrides: Record<string, string>;
  /** Absent means the group exposes every tool the integration has on. */
  tools?: string[];
}

export interface GroupDraft {
  name: string;
  environment: string | null; // null = any/mixed
  included: Set<string>;
  overrides: Record<string, Record<string, string>>;
  /** Per member, the tools it exposes here. Missing = every tool it has on. */
  tools: Record<string, string[]>;
}

/** One integration as the group form sees it. */
export interface GroupFormConnection {
  id: string;
  name: string;
  type: string;
  environment?: string | null;
  config: Record<string, string>;
  /** The integration's own tools, already resolved against its adapter. */
  tools: ToolDef[];
  toolConfig: Record<string, ToolState>;
}

export function groupDraftFrom(group: { name: string; environment?: string | null; members: GroupMember[] }): GroupDraft {
  const tools: Record<string, string[]> = {};
  for (const member of group.members) {
    if (member.tools != null) tools[member.id] = [...member.tools];
  }
  return {
    name: group.name,
    environment: group.environment ?? null,
    included: new Set(group.members.map((m) => m.id)),
    overrides: Object.fromEntries(group.members.map((m) => [m.id, { ...m.overrides }])),
    tools,
  };
}

/** The tools an integration has on — all a group can ever reach. */
export function availableTools(conn: GroupFormConnection): ToolDef[] {
  return conn.tools.filter((t) => conn.toolConfig[t.name]?.enabled ?? t.defaultEnabled);
}

/**
 * The tools this member exposes in the group: its own pick, or everything the
 * integration has on. A picked tool the integration has since turned off, or
 * dropped, is left out.
 */
export function memberTools(draft: GroupDraft, conn: GroupFormConnection): string[] {
  const available = availableTools(conn).map((t) => t.name);
  const picked = draft.tools[conn.id];
  return picked == null ? available : available.filter((name) => picked.includes(name));
}

/** Whether this member exposes a hand-picked subset rather than all it has on. */
export function hasPickedTools(draft: GroupDraft, connId: string): boolean {
  return draft.tools[connId] != null;
}

/** Add or remove one tool for a member, starting from what it exposes today. */
export function setMemberTool(
  draft: GroupDraft,
  conn: GroupFormConnection,
  tool: string,
  on: boolean,
): GroupDraft {
  const exposed = new Set(memberTools(draft, conn));
  if (on) exposed.add(tool);
  else exposed.delete(tool);
  const picked = availableTools(conn)
    .map((t) => t.name)
    .filter((name) => exposed.has(name));
  return { ...draft, tools: { ...draft.tools, [conn.id]: picked } };
}

/** Go back to exposing every tool the integration has on, now and later. */
export function clearMemberTools(draft: GroupDraft, connId: string): GroupDraft {
  const tools = { ...draft.tools };
  delete tools[connId];
  return { ...draft, tools };
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
      const member: GroupMember = { id: c.id, overrides: overridesOrEmpty(draft.overrides[c.id] ?? {}) };
      const picked = draft.tools[c.id];
      if (picked != null) member.tools = [...picked];
      return member;
    });
}

function overridesOrEmpty(m: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(m)) {
    if (v !== "" && v != null) out[k] = v;
  }
  return out;
}
