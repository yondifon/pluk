import { z } from "zod";

/**
 * Shared `only` field-selection for read tools. A tool declares a FieldMap
 * (its default dot paths, its named presets, and the full set of top-level
 * field names it will accept); `applyOnly` projects a fetched payload down to
 * whatever the caller asked for, or the default set when `only` is omitted.
 */

/** A preset either expands to more dot paths, or — when the fields it covers
 *  can't be named in advance (e.g. Sentry's `has*` capability flags) —
 *  computes its own slice of one item directly. */
export type Preset = string[] | ((item: Record<string, unknown>) => Record<string, unknown>);

export interface FieldMap {
  /** Every top-level field name this tool's payload may carry. Used to
   *  validate `only` entries and to list "valid fields" in the error. */
  fields: string[];
  /** Dot paths returned when `only` is omitted. */
  default: string[];
  /** Named shortcuts. A preset name must not be mistaken for a literal path,
   *  so pick names that don't collide with entries in `fields`. */
  presets?: Record<string, Preset>;
}

export function onlySchema(presetNames: string[]) {
  const presetLine = presetNames.length ? ` Presets: ${presetNames.join(", ")}.` : "";
  return z
    .array(z.string())
    .optional()
    .describe(`Trim the response to just these fields — omit for a lighter default, pass ["*"] for the full payload. Entries are dot paths (e.g. "project.slug") or presets.${presetLine}`);
}

export function onlyValue(args: { only?: unknown }): string[] | undefined {
  return args.only as string[] | undefined;
}

type PathTree = Map<string, PathTree>;

function buildTree(paths: string[]): PathTree {
  const root: PathTree = new Map();
  for (const path of paths) {
    let node = root;
    for (const segment of path.split(".")) {
      let next = node.get(segment);
      if (!next) {
        next = new Map();
        node.set(segment, next);
      }
      node = next;
    }
  }
  return root;
}

function projectTree(value: unknown, tree: PathTree): unknown {
  if (Array.isArray(value)) return value.map((item) => projectTree(item, tree));
  if (value === null || value === undefined || typeof value !== "object") return value;
  const out: Record<string, unknown> = {};
  for (const [key, subtree] of tree) {
    const raw = (value as Record<string, unknown>)[key];
    out[key] = subtree.size === 0 ? raw : projectTree(raw, subtree);
  }
  return out;
}

/** Project `value` onto the given dot paths, mapping over arrays wherever
 *  they occur and preserving the original nesting. */
export function pickPaths(value: unknown, paths: string[]): unknown {
  const tree = buildTree(paths);
  return projectTree(value, tree);
}

function unknownFieldError(entry: string, map: FieldMap): Error {
  const presetNames = map.presets ? Object.keys(map.presets) : [];
  const presetLine = presetNames.length ? ` Presets: ${presetNames.join(", ")}.` : "";
  return new Error(`Unknown "only" field "${entry}". Valid fields: ${map.fields.join(", ")}.${presetLine}`);
}

function projectOne(item: unknown, entries: string[], map: FieldMap): unknown {
  const paths: string[] = [];
  const reducers: ((v: Record<string, unknown>) => Record<string, unknown>)[] = [];
  for (const entry of entries) {
    const preset = map.presets?.[entry];
    if (preset) {
      if (typeof preset === "function") reducers.push(preset);
      else paths.push(...preset);
      continue;
    }
    const top = entry.split(".")[0]!;
    if (!map.fields.includes(top)) throw unknownFieldError(entry, map);
    paths.push(entry);
  }
  const base = (paths.length ? pickPaths(item, paths) : {}) as Record<string, unknown>;
  if (!reducers.length) return base;
  const record = item as Record<string, unknown>;
  return reducers.reduce((acc, fn) => ({ ...acc, ...fn(record) }), base);
}

/** Project a fetched payload (single object or array of objects) according to
 *  `only`: `["*"]` bypasses filtering entirely; an omitted or empty `only`
 *  falls back to `map.default`; otherwise each entry is a preset name or a
 *  dot path, validated against `map.fields`. */
export function applyOnly(data: unknown, only: string[] | undefined, map: FieldMap): unknown {
  if (only?.includes("*")) return data;
  const entries = only?.length ? only : map.default;
  return Array.isArray(data) ? data.map((item) => projectOne(item, entries, map)) : projectOne(data, entries, map);
}
