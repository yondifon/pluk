export type FieldType = "text" | "password" | "number" | "file" | "select" | "toggle";

export interface FieldOption {
  value: string;
  label: string;
}

export interface ShowIf {
  key: string;
  equals: string;
}

export interface ConfigFieldDef {
  key: string;
  label: string;
  type: FieldType;
  group?: string;
  required?: boolean;
  secret?: boolean;
  placeholder?: string;
  fileTypes?: string[];
  options?: FieldOption[];
  showIf?: ShowIf;
  default?: string;
  help?: string;
  danger?: boolean;
}

export interface ToolDef {
  name: string;
  description: string;
  category: string;
  defaultEnabled: boolean;
  settings?: ConfigFieldDef[];
}

export interface AdapterManifest {
  id: string;
  label: string;
  category: string;
  policyKind: string;
  agentHint: string;
  tools: ToolDef[];
  configFields: ConfigFieldDef[];
}

export interface ToolState {
  enabled: boolean;
  settings: Record<string, string>;
}

export function seededState(tool: ToolDef): ToolState {
  const s: Record<string, string> = {};
  for (const f of tool.settings ?? []) {
    if (f.default != null) s[f.key] = f.default;
  }
  return { enabled: tool.defaultEnabled, settings: s };
}

export function groupedFields(manifest: AdapterManifest): Array<{ group: string; fields: ConfigFieldDef[] }> {
  const order: string[] = [];
  const byGroup = new Map<string, ConfigFieldDef[]>();
  for (const f of manifest.configFields) {
    const g = f.group ?? "General";
    if (!byGroup.has(g)) {
      order.push(g);
      byGroup.set(g, []);
    }
    byGroup.get(g)!.push(f);
  }
  return order.map((g) => ({ group: g, fields: byGroup.get(g)! }));
}

export function groupedByCategory(adapters: AdapterManifest[]): Array<{ category: string; items: AdapterManifest[] }> {
  const order: string[] = [];
  const byCat = new Map<string, AdapterManifest[]>();
  for (const a of adapters) {
    if (!byCat.has(a.category)) {
      order.push(a.category);
      byCat.set(a.category, []);
    }
    byCat.get(a.category)!.push(a);
  }
  return order.map((c) => ({ category: c, items: byCat.get(c)! }));
}

export function prettyCategory(c: string): string {
  return c.replace(/-/g, " ").replace(/\b\w/g, (ch) => ch.toUpperCase());
}

export function isVisible(field: ConfigFieldDef, config: Record<string, string>): boolean {
  if (!field.showIf) return true;
  return (config[field.showIf.key] ?? "") === field.showIf.equals;
}

export function visibleFields(fields: ConfigFieldDef[], config: Record<string, string>): ConfigFieldDef[] {
  // Resolve visibility transitively: if a driver field is hidden, its dependents hide too.
  // Simple iterative filter until stable (handles chained conditions).
  let visible = new Set(fields.filter((f) => isVisible(f, config)).map((f) => f.key));
  // For chained deps, re-evaluate dependents whose showIf key is itself hidden.
  // If the driver key isn't in any field, it's a raw config value (always considered visible if present).
  let changed = true;
  const fieldByKey = new Map(fields.map((f) => [f.key, f] as const));
  while (changed) {
    changed = false;
    for (const f of fields) {
      if (!f.showIf) continue;
      const driverKey = f.showIf.key;
      const driverField = fieldByKey.get(driverKey);
      if (driverField && !visible.has(driverKey) && visible.has(f.key)) {
        visible.delete(f.key);
        changed = true;
      }
    }
  }
  return fields.filter((f) => visible.has(f.key));
}
