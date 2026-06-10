/**
 * Query policy engine for pluk.
 *
 * Classifies SQL using a real AST (node-sql-parser) with a conservative
 * keyword fallback. Evaluates against a per-connection policy that controls
 * which statement categories are allowed and which structural guards apply.
 */
import { Parser } from "node-sql-parser";
import type { Connection } from "../store/connections.js";

const parser = new Parser();

// ── Category definitions ────────────────────────────────────────────────────

export type StatementCategory =
  | "select"      // SELECT / WITH-select / VALUES
  | "inspect"     // SHOW / EXPLAIN / DESCRIBE / PRAGMA (read)
  | "insert"
  | "update"
  | "delete"
  | "merge"       // REPLACE / MERGE / UPSERT
  | "create"
  | "alter"
  | "drop"
  | "truncate"
  | "rename"
  | "transaction" // BEGIN / COMMIT / ROLLBACK / SAVEPOINT
  | "session"     // SET / RESET / USE
  | "procedure"   // CALL / EXEC / DO
  | "maintenance" // VACUUM / ANALYZE / REINDEX / OPTIMIZE / CHECKPOINT
  | "grant";      // GRANT / REVOKE

export interface CategoryGroup {
  label: string;
  categories: StatementCategory[];
}

export const CATEGORY_GROUPS: CategoryGroup[] = [
  { label: "Read",        categories: ["select", "inspect"] },
  { label: "Write",       categories: ["insert", "update", "delete", "merge"] },
  { label: "Schema",      categories: ["create", "alter", "drop", "truncate", "rename"] },
  { label: "Admin",       categories: ["transaction", "session", "procedure", "maintenance", "grant"] },
];

export const ALL_CATEGORIES: StatementCategory[] = CATEGORY_GROUPS.flatMap(g => g.categories);

// ── Preset definitions ───────────────────────────────────────────────────────

export type PresetName = "read-only" | "read-write" | "migrations" | "unrestricted" | "custom";

export interface QueryPolicy {
  preset: PresetName;
  allowed: StatementCategory[];
  blockStacked: boolean;
  requireWhere: boolean;
  allowFilesystem: boolean;
  maxRows: number | null;
}

export const PRESETS: Record<Exclude<PresetName, "custom">, QueryPolicy> = {
  "read-only": {
    preset: "read-only",
    allowed: ["select", "inspect"],
    blockStacked: true,
    requireWhere: false,
    allowFilesystem: false,
    maxRows: 1000,
  },
  "read-write": {
    preset: "read-write",
    allowed: ["select", "inspect", "insert", "update", "delete", "merge", "transaction", "session"],
    blockStacked: true,
    requireWhere: true,
    allowFilesystem: false,
    maxRows: 1000,
  },
  "migrations": {
    preset: "migrations",
    allowed: [
      "select", "inspect",
      "insert", "update", "delete", "merge",
      "create", "alter", "drop", "truncate", "rename",
      "transaction", "session", "procedure", "maintenance",
    ],
    blockStacked: false,
    requireWhere: true,
    allowFilesystem: false,
    maxRows: null,
  },
  "unrestricted": {
    preset: "unrestricted",
    allowed: ALL_CATEGORIES,
    blockStacked: false,
    requireWhere: false,
    allowFilesystem: true,
    maxRows: null,
  },
};

export function defaultPolicyFor(environment: string): QueryPolicy {
  if (environment === "production" || environment === "staging") {
    return { ...PRESETS["read-only"] };
  }
  return { ...PRESETS["read-write"] };
}

/**
 * Parse a stored policy JSON string, falling back to the legacy read_only flag.
 */
export function parsePolicy(raw: string | null | undefined, legacyReadOnly: number): QueryPolicy {
  if (raw) {
    try {
      const p = JSON.parse(raw) as Partial<QueryPolicy>;
      // Validate and fill defaults
      const preset = (PRESETS[p.preset as Exclude<PresetName, "custom">] || null) ? p.preset! : "custom";
      return {
        preset,
        allowed: Array.isArray(p.allowed) ? p.allowed.filter(c => ALL_CATEGORIES.includes(c)) : [],
        blockStacked: p.blockStacked ?? true,
        requireWhere: p.requireWhere ?? false,
        allowFilesystem: p.allowFilesystem ?? false,
        maxRows: typeof p.maxRows === "number" ? p.maxRows : p.maxRows === null ? null : null,
      };
    } catch {
      // fall through to legacy
    }
  }

  // Legacy: read_only=1 → read-only, 0 → unrestricted
  if (legacyReadOnly) {
    return { ...PRESETS["read-only"] };
  }
  return { ...PRESETS["unrestricted"] };
}

// ── Dialect mapping ──────────────────────────────────────────────────────────

type Dialect = "PostgreSQL" | "MySQL" | "SQLite";

export function dialectFor(type: Connection["type"]): Dialect {
  switch (type) {
    case "postgres": return "PostgreSQL";
    case "mysql":    return "MySQL";
    case "sqlite":   return "SQLite";
    default:         return "PostgreSQL";
  }
}

// ── AST → Category mapping ───────────────────────────────────────────────────

const AST_TYPE_MAP: Record<string, StatementCategory> = {
  select:      "select",
  insert:      "insert",
  update:      "update",
  delete:      "delete",
  replace:     "merge",   // MySQL REPLACE
  merge:       "merge",
  create:      "create",
  alter:       "alter",
  drop:        "drop",
  truncate:    "truncate",
  rename:      "rename",
  transaction: "transaction",
  show:        "inspect",
  desc:        "inspect",
  describe:    "inspect",
  explain:     "inspect",
  set:         "session",
  use:         "session",
  grant:       "grant",
  revoke:      "grant",
  call:        "procedure",
  exec:        "procedure",
};

// AST type prefixes that also map (for compound node types returned by some dialects)
const AST_PREFIX_MAP: [string, StatementCategory][] = [
  ["create",      "create"],
  ["alter",       "alter"],
  ["drop",        "drop"],
  ["rename",      "rename"],
  ["transaction", "transaction"],
];

function astTypeToCategory(type: string): StatementCategory | null {
  const lower = type.toLowerCase();
  if (AST_TYPE_MAP[lower]) return AST_TYPE_MAP[lower];
  for (const [prefix, cat] of AST_PREFIX_MAP) {
    if (lower.startsWith(prefix)) return cat;
  }
  return null;
}

// ── Keyword fallback classifier ──────────────────────────────────────────────

const KEYWORD_MAP: [RegExp, StatementCategory][] = [
  [/^\s*select\b/i,           "select"],
  [/^\s*with\b/i,             "select"],  // CTE
  [/^\s*values\b/i,           "select"],
  [/^\s*table\b/i,            "select"],  // TABLE t (pg)
  [/^\s*insert\b/i,           "insert"],
  [/^\s*update\b/i,           "update"],
  [/^\s*delete\b/i,           "delete"],
  [/^\s*replace\b/i,          "merge"],
  [/^\s*merge\b/i,            "merge"],
  [/^\s*upsert\b/i,           "merge"],
  [/^\s*create\b/i,           "create"],
  [/^\s*alter\b/i,            "alter"],
  [/^\s*drop\b/i,             "drop"],
  [/^\s*truncate\b/i,         "truncate"],
  [/^\s*rename\b/i,           "rename"],
  [/^\s*copy\b/i,             "insert"],  // COPY ... FROM = import; COPY ... TO = export; dangerous variant caught by scanDangerous
  [/^\s*begin\b/i,            "transaction"],
  [/^\s*commit\b/i,           "transaction"],
  [/^\s*rollback\b/i,         "transaction"],
  [/^\s*savepoint\b/i,        "transaction"],
  [/^\s*release\s+savepoint/i,"transaction"],
  [/^\s*set\b/i,              "session"],
  [/^\s*reset\b/i,            "session"],
  [/^\s*use\b/i,              "session"],
  [/^\s*show\b/i,             "inspect"],
  [/^\s*explain\b/i,          "inspect"],
  [/^\s*describe\b/i,         "inspect"],
  [/^\s*desc\b/i,             "inspect"],
  [/^\s*pragma\b/i,           "inspect"],
  [/^\s*call\b/i,             "procedure"],
  [/^\s*exec(?:ute)?\b/i,     "procedure"],
  [/^\s*do\b/i,               "procedure"],
  [/^\s*vacuum\b/i,           "maintenance"],
  [/^\s*analyze\b/i,          "maintenance"],
  [/^\s*reindex\b/i,          "maintenance"],
  [/^\s*optimize\b/i,         "maintenance"],
  [/^\s*checkpoint\b/i,       "maintenance"],
  [/^\s*cluster\b/i,          "maintenance"],
  [/^\s*grant\b/i,            "grant"],
  [/^\s*revoke\b/i,           "grant"],
];

function keywordClassify(sql: string): StatementCategory | null {
  // Strip line and block comments before matching to defeat comment-prefix bypass
  const stripped = sql
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/--[^\n]*/g, " ")
    .replace(/#[^\n]*/g, " ")    // MySQL # comment
    .trim();

  for (const [re, cat] of KEYWORD_MAP) {
    if (re.test(stripped)) return cat;
  }
  return null;
}

// ── Classification result ────────────────────────────────────────────────────

export interface ClassifyResult {
  categories: StatementCategory[];          // one per statement
  statementCount: number;
  hasUpdateOrDeleteWithoutWhere: boolean;
  dangerous: DangerousConstruct | null;
}

export type DangerousConstruct =
  | "copy-program"
  | "into-outfile"
  | "load-data"
  | "attach-database"
  | "pg-read-file"
  | "lo-import";

/**
 * Scan raw SQL for filesystem/RCE constructs that the parser may not handle.
 * These are detected by token pattern regardless of parse result.
 */
function scanDangerous(sql: string, _dialect: Dialect): DangerousConstruct | null {
  const s = sql.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/--[^\n]*/g, " ");

  if (/\bCOPY\b[\s\S]*?\bFROM\s+PROGRAM\b/i.test(s)) return "copy-program";
  if (/\bCOPY\b[\s\S]*?\bTO\s+PROGRAM\b/i.test(s))   return "copy-program";
  if (/\bINTO\s+OUTFILE\b/i.test(s))                   return "into-outfile";
  if (/\bLOAD\s+DATA\b/i.test(s))                      return "load-data";
  if (/\bATTACH\s+(DATABASE\s+)?['"\w]/i.test(s))      return "attach-database";
  if (/\bpg_read_file\b/i.test(s))                     return "pg-read-file";
  if (/\blo_import\b/i.test(s))                        return "lo-import";

  return null;
}

/**
 * Classify a SQL string: attempt full AST parse per dialect, fall back
 * to keyword classifier on failure. Fail-closed: unknown → null category.
 */
export function classify(sql: string, dialect: Dialect): ClassifyResult {
  const dangerous = scanDangerous(sql, dialect);
  let categories: (StatementCategory | null)[] = [];
  let hasUpdateOrDeleteWithoutWhere = false;

  try {
    const ast = parser.astify(sql, { database: dialect });
    const stmts = Array.isArray(ast) ? ast : [ast];

    categories = stmts.map((s: any) => {
      const cat = astTypeToCategory(s.type ?? "");

      if (s.type === "update" || s.type === "delete") {
        if (!s.where) hasUpdateOrDeleteWithoutWhere = true;
      }

      return cat;
    });
  } catch {
    // AST failed — use keyword fallback on the whole sql, split on semicolons
    // Split on semicolons to approximate statement count for stacked check
    const parts = sql
      .split(/;/)
      .map(p => p.trim())
      .filter(p => p.length > 0);

    if (parts.length === 0) {
      categories = [null];
    } else {
      categories = parts.map(part => keywordClassify(part));

      // For update/delete without WHERE fallback: can't reliably detect without AST,
      // treat as no-where if the keyword matches and there's no WHERE keyword present
      for (const part of parts) {
        if (/^\s*(update|delete)\b/i.test(part) && !/\bwhere\b/i.test(part)) {
          hasUpdateOrDeleteWithoutWhere = true;
        }
      }
    }
  }

  return {
    categories: categories as StatementCategory[],
    statementCount: categories.length,
    hasUpdateOrDeleteWithoutWhere,
    dangerous,
  };
}

// ── Policy evaluation ────────────────────────────────────────────────────────

export interface EvalResult {
  ok: boolean;
  reason: string | null;
  categories: string;   // csv — for the audit log
}

export function evaluate(sql: string, policy: QueryPolicy, dialect: Dialect): EvalResult {
  const result = classify(sql, dialect);
  const cats = result.categories.join(",");

  // 1. Block stacked statements
  if (policy.blockStacked && result.statementCount > 1) {
    return {
      ok: false,
      reason: `Stacked statements blocked (${result.statementCount} statements). Split into separate queries.`,
      categories: cats,
    };
  }

  // 2. Dangerous filesystem/RCE constructs (checked before fail-closed so the reason is specific)
  if (result.dangerous && !policy.allowFilesystem) {
    return {
      ok: false,
      reason: `Filesystem/RCE construct '${result.dangerous}' is blocked on this connection.`,
      categories: cats,
    };
  }

  // 3. Block unknown / disallowed categories (fail-closed)
  for (const cat of result.categories) {
    if (!cat) {
      return {
        ok: false,
        reason: "Statement type could not be identified (fail-closed). If this is a valid query, contact the pluk admin.",
        categories: cats,
      };
    }
    if (!policy.allowed.includes(cat)) {
      return {
        ok: false,
        reason: `Statement type '${cat}' is not allowed on this connection. Allowed: ${policy.allowed.join(", ")}.`,
        categories: cats,
      };
    }
  }

  // 4. Update/Delete without WHERE
  if (policy.requireWhere && result.hasUpdateOrDeleteWithoutWhere) {
    return {
      ok: false,
      reason: "UPDATE or DELETE without a WHERE clause is blocked on this connection (requireWhere).",
      categories: cats,
    };
  }

  return { ok: true, reason: null, categories: cats };
}

// ── Row cap (applied post-query) ─────────────────────────────────────────────

export interface CapResult {
  rows: unknown[];
  truncated: boolean;
  limit: number | null;
}

export function capRows(rows: unknown[], maxRows: number | null): CapResult {
  if (maxRows === null || rows.length <= maxRows) {
    return { rows, truncated: false, limit: maxRows };
  }
  return { rows: rows.slice(0, maxRows), truncated: true, limit: maxRows };
}

// ── Policy description (for MCP tool doc) ───────────────────────────────────

export function policyDescription(policy: QueryPolicy): string {
  const caps = policy.allowed.join(", ");
  const guards: string[] = [];
  if (policy.blockStacked) guards.push("no stacked statements");
  if (policy.requireWhere) guards.push("WHERE required on UPDATE/DELETE");
  if (!policy.allowFilesystem) guards.push("no filesystem/COPY ops");
  if (policy.maxRows !== null) guards.push(`max ${policy.maxRows} rows returned`);
  return `Allowed: ${caps}.${guards.length ? " Guards: " + guards.join("; ") + "." : ""}`;
}
