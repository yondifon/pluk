import { z } from "zod";
import { mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { Integration } from "../../store/integrations.js";
import type { Driver } from "../../db/index.js";
import { isValidDatabaseName } from "../../db/dbName.js";
import { sqlPolicyFromSettings, evaluate, capRows, dialectFor, policyDescription, parsePostgresCost } from "../../mcp/policy.js";
import { toolGate } from "../../mcp/toolConfig.js";
import type { ConfigField, ToolSpec } from "../types.js";
import { logQuery } from "../../store/queryLog.js";
import { listSavedQueries, getSavedQuery } from "../../store/savedQueries.js";
import { listMaskedColumns, maskRow } from "../../store/maskedColumns.js";
import {
  getDriver,
  evictDriver,
  withToolTimeout,
  withCancellable,
  registerQueryAbort,
  clearQueryAbort,
} from "./pool.js";
import { logError } from "../../log.js";
import { buildInstructions } from "../../mcp/instructions.js";
import { ok, err, runGated, type ToolResult, type LogSnapshot } from "../kit.js";
import type { ToolHost } from "../../mcp/namespace.js";
import { formatSqlError } from "./errors.js";
import { isSshPending } from "../../ssh/pending.js";

// Human label for a SQL adapter id — single source for the manifest and the
// agent-facing instructions so they never drift.
export function sqlLabel(type: string): string {
  switch (type) {
    case "postgres": return "PostgreSQL";
    case "mysql": return "MySQL";
    case "sqlite": return "SQLite";
    default: return type;
  }
}

export function sqlAgentHint(type: string): string {
  const db = type === "sqlite" ? "SQLite" : type === "mysql" ? "MySQL" : "PostgreSQL";
  return type === "sqlite"
    ? `Use this to query and inspect a ${db} database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT before wider queries.`
    : `Use this to query and inspect a ${db} database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.`;
}

// Live, per-session guidance handed to connecting agents (see instructions.ts).
// Reflects the current query policy, so a read-only DB and an unrestricted one
// announce different constraints.
export function sqlInstructions(conn: Integration): string {
  const gate = toolGate(conn.query_policy);
  const policy = sqlPolicyFromSettings(gate.settings("query"));
  return buildInstructions(conn, {
    kind: sqlLabel(conn.type),
    access: "Query and inspect this database. Every statement is checked against the policy below and recorded in the activity log.",
    policy: policyDescription(policy),
    start: "Start with list_tables and describe_table to learn the schema, then read with SELECT … LIMIT.",
    hint: sqlAgentHint(conn.type),
  });
}

// The `query` tool's settings: the SQL policy, expressed as a single mode plus
// structural guards. Shared by every statement-running tool on the adapter.
const QUERY_SETTINGS: ConfigField[] = [
  {
    key: "mode", label: "Statements", type: "select", default: "read-only",
    help: "Which kinds of SQL this connection may run.",
    options: [
      { value: "read-only", label: "Read-only (SELECT)" },
      { value: "mutations", label: "Mutations (INSERT/UPDATE/DELETE)" },
      { value: "destructive", label: "Destructive (DROP/TRUNCATE, DDL)" },
    ],
  },
  { key: "require_where", label: "Require WHERE on UPDATE/DELETE", type: "toggle", default: true,
    help: "Block UPDATE or DELETE without a WHERE clause." },
  { key: "block_stacked", label: "Block stacked statements", type: "toggle", default: true,
    help: "Reject queries containing more than one statement (SELECT 1; DROP …)." },
  { key: "allow_filesystem", label: "Allow filesystem / COPY ops", type: "toggle", default: false, danger: true,
    help: "Allow COPY … PROGRAM, INTO OUTFILE, LOAD DATA, ATTACH DATABASE, pg_read_file." },
  { key: "max_rows", label: "Max rows returned", type: "number", default: 1000,
    help: "Cap rows returned to the agent. 0 = no cap." },
];

// Static tool catalog for the SQL family. Every tool is individually toggleable
// (all default on); only `query` carries settings (they form the SQL policy that
// also governs export_query / run_saved_query).
export function sqlToolSpecs(): ToolSpec[] {
  // Opt-in tools ship default-off so a fresh connection exposes only the lean set
  // most developers use out of the box (query + core inspection). The rest —
  // perf/discovery/niche/setup-dependent/side-effecting — are one click to enable.
  const optIn = new Set([
    "explain_query", "list_relationships", "table_stats",
    "list_schemas", "list_databases", "export_query",
    "run_saved_query", "list_saved_queries",
  ]);
  const read = (name: string, description: string, settings?: ConfigField[]): ToolSpec =>
    ({ name, description, category: "read", defaultEnabled: !optIn.has(name), settings });
  return [
    read("query", "Run a SQL query against the database.", QUERY_SETTINGS),
    read("list_tables", "List all tables in the database."),
    read("sample_table", "Preview rows from a table without writing SQL."),
    read("explain_query", "Show a query's execution plan without running it."),
    read("describe_table", "Get column definitions for a table."),
    read("list_relationships", "List foreign key relationships between tables."),
    read("search_schema", "Find tables or columns matching a term."),
    read("table_stats", "Get cheap table statistics (estimated rows, size, indexes)."),
    read("list_schemas", "List all schemas or databases."),
    read("list_databases", "List databases on the server (targets for the `database` argument)."),
    read("export_query", "Run a SQL query and save results to a local CSV or JSON file."),
    read("run_saved_query", "Run a saved query by name."),
    read("list_saved_queries", "List saved queries for this connection."),
  ];
}

// Register the SQL surface onto a host (a bare McpServer for a single endpoint,
// or a namespaced host when aggregated into a group).
export function registerSqlServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  const gate = toolGate(conn.query_policy);
  // The SQL policy (mode + guards) lives as the `query` tool's settings and
  // governs every statement-running tool (query / export_query / run_saved_query).
  const policy = sqlPolicyFromSettings(gate.settings("query"));
  const dialect = dialectFor(conn.type);
  const policyDesc = policyDescription(policy);
  // Every SQL tool is individually toggleable. The default-on state is the single
  // source of truth in sqlToolSpecs(); a disabled tool is simply not registered,
  // so the agent never sees it.
  const toolDefaults = new Map(sqlToolSpecs().map((t) => [t.name, t.defaultEnabled]));
  const on = (name: string): boolean => gate.enabled(name, toolDefaults.get(name) ?? true);

  const readOnlyMode = policy.allowed.length === 2 && policy.allowed.includes("select") && policy.allowed.includes("inspect");
  const maskedColumns = listMaskedColumns(conn.id);
  const usesSsh = conn.config.use_ssh === true || conn.config.use_ssh === "true";
  const timeoutOption: Record<string, z.ZodTypeAny> = conn.type === "sqlite" && !usesSsh ? {} : {
    timeout: z.number().int().positive().max(600).optional().describe("Max seconds to wait before aborting the query (default 30)."),
  };

  // Multi-database targeting. A connection configured WITH a database at setup is
  // *pinned*: it can only ever reach that database, and the `database` argument is
  // not even exposed (so the agent cannot ask for another). A connection with no
  // database is multi-db: each call may name a target, served by its own isolated
  // pool. Cross-database data isolation is ultimately enforced by the privileges
  // GRANTed to the connection's DB user — the pin is the app-level scope on top.
  const pinnedDb = typeof conn.config.database === "string" && conn.config.database.trim() !== ""
    ? conn.config.database.trim()
    : undefined;
  const supportsDbArg = !pinnedDb && conn.type !== "sqlite";
  const dbOption: Record<string, z.ZodTypeAny> = supportsDbArg ? {
    database: z.string().max(128).optional().describe(
      "Database to run against on this server. This connection has no fixed database, so name the one to use (see list_schemas). Access is limited to databases the connection's user was granted."
    ),
  } : {};

  // Bind parameters for placeholders in the SQL. Prefer these over inlining
  // values: the driver escapes them, values can never change the statement's
  // category, and quoting is handled for you. Placeholder syntax is dialect
  // specific ($1/$2 on Postgres, ? on MySQL/SQLite).
  const paramsOption: Record<string, z.ZodTypeAny> = {
    params: z.array(z.union([z.string(), z.number(), z.boolean(), z.null()])).optional().describe(
      conn.type === "postgres"
        ? "Values to bind to $1, $2, … placeholders in the SQL. Prefer this over inlining values."
        : "Values to bind to ? placeholders in the SQL. Prefer this over inlining values."
    ),
  };

  // Postgres scopes tables by schema (default "public"); MySQL/SQLite don't have
  // the concept (MySQL uses the `database` arg instead), so the arg is Postgres
  // only. Discover schemas with list_schemas, then pass one here.
  const supportsSchemaArg = conn.type === "postgres";
  const schemaOption: Record<string, z.ZodTypeAny> = supportsSchemaArg ? {
    schema: z.string().max(128).optional().describe("Postgres schema to inspect (default: public). See list_schemas for options."),
  } : {};

  // Validate a requested schema (identifier charset, same as a database name) so
  // a hostile value can't reach a quoted identifier in the driver.
  function resolveSchema(requested?: string): { ok: true; schema?: string } | { ok: false; error: string } {
    if (!requested) return { ok: true, schema: undefined };
    if (!isValidDatabaseName(requested)) {
      return { ok: false, error: `Invalid schema name "${requested}". Allowed: letters, digits, _, $, -.` };
    }
    return { ok: true, schema: requested };
  }

  // Resolve a requested target database against the pin rule. Pinned connections
  // ignore any stray value (the arg isn't exposed); multi-db connections validate
  // the name and fall back to the server default when none is given.
  function resolveDatabase(requested?: string): { ok: true; db?: string } | { ok: false; error: string } {
    if (pinnedDb) return { ok: true, db: undefined };
    if (requested === undefined || requested === "") return { ok: true, db: undefined };
    if (!isValidDatabaseName(requested)) {
      return { ok: false, error: `Invalid database name "${requested}". Allowed: letters, digits, _, $, -.` };
    }
    return { ok: true, db: requested };
  }

  // Block statements that switch the connection's active database out from under
  // the pool. `USE db` (MySQL) both defeats the database pin and poisons a pooled
  // connection for later queries; the correct way to reach another database is the
  // `database` argument, which routes to a separate, isolated pool.
  function switchBlock(sql: string): string | undefined {
    const s = sql.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/--[^\n]*/g, " ").replace(/#[^\n]*/g, " ");
    // Match a USE at the start or after a `;` — the latter matters only when
    // stacked statements are allowed (migrations/destructive mode).
    if (/(^|;)\s*use\s+\S/i.test(s)) {
      return pinnedDb
        ? `This connection is locked to database "${pinnedDb}". USE is blocked.`
        : "USE is blocked. Pass the `database` argument to choose a database instead.";
    }
    return undefined;
  }

  // Read-only introspection tools share one shape: acquire the pooled driver,
  // run under the tool timeout, evict on failure. `fn` produces the response text.
  // Introspection statements are recorded by the driver layer, so there is no
  // tool-level log entry here (only the gated query tools below create one).
  async function introspect(label: string, fn: (driver: Driver) => Promise<string>, database?: string): Promise<ToolResult> {
    const sid = sessionIdRef.value;
    try {
      const driver = await getDriver(sid, conn, database);
      return await withToolTimeout((async (): Promise<ToolResult> => ok(await fn(driver)))(), label);
    } catch (e) {
      // A connect awaiting an interactive approval isn't broken — evicting it
      // would close the tunnel the moment the user approves. Leave it in the
      // pool so the approval lands and the next retry succeeds.
      if (!isSshPending(e)) {
        evictDriver(sid, conn.id);
        logError(`tool ${label} failed`, e, { integration: conn.name, type: conn.type });
      }
      return err(formatSqlError(e));
    }
  }

  // Resolve target database + schema from tool args, then run an introspection
  // under them. Surfaces a pin/validation error before touching the pool. The
  // resolved schema (undefined outside Postgres) is handed to the fn so it can
  // scope the driver call.
  function introspectScoped(
    label: string,
    args: { database?: string; schema?: string },
    fn: (driver: Driver, schema?: string) => Promise<string>
  ): Promise<ToolResult> {
    const d = resolveDatabase(args.database);
    if (!d.ok) return Promise.resolve(err(d.error));
    const s = resolveSchema(args.schema);
    if (!s.ok) return Promise.resolve(err(s.error));
    return introspect(label, (driver) => fn(driver, s.schema), d.db);
  }

  type QueryRows = { rows: unknown[]; fields?: string[] };
  type SqlResultMeta = {
    env: string;
    host: string;
    connection: string;
    type: string;
    database?: string;
    fields?: string[];
    rows: unknown[];
    truncated: boolean;
    row_cap: number | null;
    row_count: number;
    returned_rows: number;
  };

  // The database a call actually ran against: the per-call target, else the
  // connection's pinned/configured database (undefined = unpinned server default).
  function effectiveDb(database?: string): string | undefined {
    return database ?? pinnedDb;
  }

  function resultWithMeta(result: QueryRows, rows: unknown[], truncated: boolean, rowCap: number | null, database?: string): SqlResultMeta {
    return {
      env: conn.environment ?? "development",
      host: String(conn.config.host ?? conn.config.filename ?? "localhost"),
      connection: conn.name,
      type: conn.type,
      database: effectiveDb(database),
      fields: result.fields ?? [],
      rows,
      truncated,
      row_cap: rowCap,
      row_count: result.rows.length,
      returned_rows: rows.length,
    };
  }

  function missingSqlArg(): ToolResult {
    return err('Missing SQL. Pass either "sql" or "query".');
  }

  function toTimeoutMs(timeout?: number): number | undefined {
    return (conn.type === "sqlite" && !usesSsh) || !timeout ? undefined : timeout * 1000;
  }

  // The postgres cost gate: returns a block reason if the planner's estimate
  // exceeds the policy's row/cost ceiling, else undefined. No-op off postgres or
  // when no ceiling is set.
  async function costBlock(driver: Driver, sql: string, params?: unknown[]): Promise<string | undefined> {
    const enabled = policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null;
    if (!enabled || conn.type !== "postgres") return undefined;
    const explain = await driver.explain(sql, params);
    const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
    const estimate = parsePostgresCost(plan);
    if (
      (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
      (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
    ) {
      return `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
    }
    return undefined;
  }

  // Execute a policy-passed statement under the tool timeout + per-query abort.
  // Postgres read-only mode uses a read-only transaction.
  function runStatement<T extends QueryRows>(driver: Driver, sql: string, signal: AbortSignal, label: string, timeoutMs?: number, params?: unknown[]): Promise<T> {
    // In read-only mode every dialect routes through queryReadOnly, which each
    // driver backs with an engine-level guard (Postgres BEGIN READ ONLY, MySQL
    // START TRANSACTION READ ONLY, SQLite query_only / -readonly).
    const useReadOnly = readOnlyMode;
    // Hand the signal to the driver so it cancels the statement server-side
    // (pg_cancel_backend / KILL QUERY); withCancellable still unblocks the caller
    // immediately so the agent isn't left waiting on the round trip.
    const opts = { timeoutMs, signal };
    const work = (useReadOnly ? driver.queryReadOnly(sql, params, opts) : driver.query(sql, params, opts)) as Promise<T>;
    return withToolTimeout(withCancellable(work, signal), label, timeoutMs);
  }

  // Apply the masked-column policy to a row set.
  function mask(rows: unknown[]): unknown[] {
    return maskedColumns.length > 0 ? rows.map((r) => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
  }

  // Shared SQL error handling for the gated query tools: aborted queries are
  // "cancelled" (no driver eviction); everything else evicts + logs.
  const queryGateOpts = {
    classifyError: (msg: string) => (msg.includes("cancelled") ? "cancelled" : "error") as "cancelled" | "error",
    onError: (e: unknown) => {
      if (isSshPending(e)) return;
      evictDriver(sessionIdRef.value, conn.id);
      logError("query tool failed", e, { integration: conn.name, type: conn.type });
    },
    formatError: (e: unknown) => formatSqlError(e),
  };

  // Run a SQL statement through the policy gate + activity log, returning rows
  // (masked + row-capped). Shared by `query` and `run_saved_query`.
  function gatedQuery(sql: string, source: string, timeoutMs?: number, rowCap?: number, database?: string, params?: unknown[]): Promise<ToolResult> {
    const verdict = evaluate(sql, policy, dialect);
    const effectiveCap = policy.maxRows === null ? rowCap ?? null : Math.min(rowCap ?? policy.maxRows, policy.maxRows);
    return runGated(
      conn,
      { category: verdict.categories, action: source, detail: sql, database: database ?? pinnedDb },
      async (logId) => {
        const sid = sessionIdRef.value;
        const queryAc = registerQueryAbort(logId, sid);
        try {
          const driver = await getDriver(sid, conn, database);
          const block = await costBlock(driver, sql, params);
          if (block) return { blocked: block };

          const result = await runStatement<QueryRows>(driver, sql, queryAc.signal, source, timeoutMs, params);
          const { rows, truncated, limit } = capRows(result.rows, effectiveCap);
          const maskedRows = mask(rows);
          const maskedResult: LogSnapshot & QueryRows = { fields: result.fields, rows: maskedRows };
          let text = JSON.stringify(resultWithMeta(result, maskedRows, truncated, limit, database), null, 2);
          if (truncated) {
            text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
          }
          return { text, result: maskedResult };
        } finally {
          clearQueryAbort(logId);
        }
      },
      { precheck: () => switchBlock(sql) ?? (verdict.ok ? undefined : verdict.reason ?? "blocked"), ...queryGateOpts },
    );
  }

  server.prompt(
    "summarize_schema",
    "Generate a concise summary of the database schema and relationships",
    async () => ({
      messages: [
        { role: "user", content: { type: "text", text: "Read the full schema resource, then list the main tables, their purpose, and how they relate to each other." } },
      ],
    })
  );

  server.prompt(
    "investigate_slow_query",
    "Analyze a slow query using EXPLAIN and table stats",
    { sql: z.string().describe("SQL query to investigate") },
    async ({ sql }) => ({
      messages: [
        { role: "user", content: { type: "text", text: `Investigate why this query is slow. Use explain_query and table_stats, then suggest indexes or rewrites.

${sql}` } },
      ],
    })
  );

  server.prompt(
    "find_unused_indexes",
    "Find indexes that may be unused or redundant",
    async () => ({
      messages: [
        { role: "user", content: { type: "text", text: "List all tables and their indexes. Flag any indexes that look redundant or likely unused based on column patterns." } },
      ],
    })
  );

  server.resource(
    "schema",
    "schema://full",
    { mimeType: "text/plain", description: "Full database schema: tables, columns, primary keys, foreign keys" },
    async () => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const text = await driver.getFullSchema();
          return { contents: [{ uri: "schema://full", mimeType: "text/plain", text }] };
        })(), "schema_resource");
      } catch (err) {
        if (!isSshPending(err)) evictDriver(sid, conn.id);
        return { contents: [{ uri: "schema://full", mimeType: "text/plain", text: `Error: ${(err as Error).message}` }] };
      }
    }
  );

  if (on("query")) server.tool(
    "query",
    `Run a SQL query against the database. ${policyDesc}${supportsDbArg ? " This connection has no fixed database — pass `database` to choose one." : ""}`,
    {
      sql: z.string().optional().describe("SQL query to execute"),
      query: z.string().optional().describe("Alias for sql"),
      ...timeoutOption,
      ...dbOption,
      ...paramsOption,
      limit: z.number().int().positive().max(1_000_000).optional().describe("Max rows to return, overriding the default cap (1000)."),
    },
    (args) => {
      const { sql, query, limit } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const database = (args as typeof args & { database?: string }).database;
      const params = (args as typeof args & { params?: unknown[] }).params;
      const statement = sql ?? query;
      if (!statement) return Promise.resolve(missingSqlArg());
      const r = resolveDatabase(database);
      if (!r.ok) return Promise.resolve(err(r.error));
      return gatedQuery(statement, "query", toTimeoutMs(timeout), limit, r.db, params);
    },
  );

  if (on("list_tables")) server.tool("list_tables", "List all tables in the database", { ...dbOption, ...schemaOption }, (args) =>
    introspectScoped("list_tables", args as { database?: string; schema?: string }, async (driver, schema) => (await driver.listTables(schema)).join("\n")));

  if (on("sample_table")) server.tool(
    "sample_table",
    "Preview rows from a table without writing SQL",
    {
      table: z.string().describe("Table name"),
      limit: z.number().int().min(1).max(1000).default(20).describe("Max rows to preview"),
      ...dbOption,
      ...schemaOption,
    },
    ({ table, limit, ...rest }) => {
      const r = resolveDatabase((rest as { database?: string }).database);
      if (!r.ok) return Promise.resolve(err(r.error));
      const s = resolveSchema((rest as { schema?: string }).schema);
      if (!s.ok) return Promise.resolve(err(s.error));
      return introspect("sample_table", async (driver) => {
        const effectiveLimit = policy.maxRows === null ? limit : Math.min(limit, policy.maxRows);
        const result = await driver.sampleTable(table, effectiveLimit, s.schema);
        const { rows, truncated, limit: rowCap } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        let text = JSON.stringify(resultWithMeta(result, maskedRows, truncated, rowCap, r.db), null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${policy.maxRows} of ${result.rows.length} rows.]`;
        }
        return text;
      }, r.db);
    }
  );

  if (on("explain_query")) server.tool(
    "explain_query",
    "Show query execution plan without running the query",
    {
      sql: z.string().optional().describe("SQL query to explain"),
      query: z.string().optional().describe("Alias for sql"),
      ...dbOption,
      ...paramsOption,
    },
    ({ sql, query, ...rest }) => {
      const statement = sql ?? query;
      if (!statement) return Promise.resolve(missingSqlArg());
      const r = resolveDatabase((rest as { database?: string }).database);
      if (!r.ok) return Promise.resolve(err(r.error));
      const params = (rest as { params?: unknown[] }).params;
      // `explain` runs no policy-changing statement, so a passing query is logged
      // by the driver layer (via introspect), not as a gated tool call. A blocked
      // query is still recorded so the audit log shows the denial.
      const verdict = evaluate(statement, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, statement, "blocked", verdict.categories, verdict.reason ?? undefined, undefined, undefined, undefined, conn.viaGroup, r.db ?? pinnedDb);
        return Promise.resolve(err(`Blocked: ${verdict.reason}`));
      }
      return introspect("explain_query", async (driver) => JSON.stringify(await driver.explain(statement, params), null, 2), r.db);
    }
  );

  if (on("describe_table")) server.tool(
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name"), ...dbOption, ...schemaOption },
    ({ table, ...rest }) =>
      introspectScoped("describe_table", rest as { database?: string; schema?: string }, async (driver, schema) => JSON.stringify(await driver.describeTable(table, schema), null, 2))
  );

  if (on("list_relationships")) server.tool(
    "list_relationships",
    "List foreign key relationships between tables",
    { table: z.string().optional().describe("Filter to a specific table (optional)"), ...dbOption, ...schemaOption },
    ({ table, ...rest }) =>
      introspectScoped("list_relationships", rest as { database?: string; schema?: string }, async (driver, schema) => JSON.stringify(await driver.listRelationships(table, schema), null, 2))
  );

  if (on("search_schema")) server.tool(
    "search_schema",
    "Find tables or columns matching a term",
    { term: z.string().describe("Search term (substring match on table or column names)"), ...dbOption, ...schemaOption },
    ({ term, ...rest }) =>
      introspectScoped("search_schema", rest as { database?: string; schema?: string }, async (driver, schema) => JSON.stringify(await driver.searchSchema(term, schema), null, 2))
  );

  if (on("table_stats")) server.tool(
    "table_stats",
    "Get cheap table statistics (estimated rows, size, indexes)",
    { table: z.string().describe("Table name"), ...dbOption, ...schemaOption },
    ({ table, ...rest }) =>
      introspectScoped("table_stats", rest as { database?: string; schema?: string }, async (driver, schema) => JSON.stringify(await driver.tableStats(table, schema), null, 2))
  );

  if (on("list_schemas")) server.tool("list_schemas", "List all schemas or databases", () =>
    introspect("list_schemas", async (driver) => (await driver.listSchemas()).join("\n")));

  if (on("list_databases")) server.tool(
    "list_databases",
    supportsDbArg
      ? "List databases on the server. Pass one of these as `database` on other tools to query it."
      : "List databases on the server.",
    () => introspect("list_databases", async (driver) => (await driver.listDatabases()).join("\n")));

  if (on("export_query")) server.tool(
    "export_query",
    "Run a SQL query and save results to a local CSV or JSON file",
    {
      sql: z.string().optional().describe("SQL query to execute"),
      query: z.string().optional().describe("Alias for sql"),
      format: z.enum(["csv", "json"]).default("csv").describe("Export file format"),
      ...timeoutOption,
      ...dbOption,
      ...paramsOption,
    },
    (args) => {
      const { sql, query, format } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const params = (args as typeof args & { params?: unknown[] }).params;
      const statement = sql ?? query;
      if (!statement) return Promise.resolve(missingSqlArg());
      const r = resolveDatabase((args as typeof args & { database?: string }).database);
      if (!r.ok) return Promise.resolve(err(r.error));
      const verdict = evaluate(statement, policy, dialect);
      const timeoutMs = toTimeoutMs(timeout);
      return runGated(
        conn,
        { category: verdict.categories, action: "export_query", detail: statement, database: r.db ?? pinnedDb },
        async (logId) => {
          const sid = sessionIdRef.value;
          const queryAc = registerQueryAbort(logId, sid);
          try {
            const driver = await getDriver(sid, conn, r.db);
            const result = await runStatement<QueryRows>(driver, statement, queryAc.signal, "export_query", timeoutMs, params);

            const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
            const maskedRows = mask(rows);

            const fields = result.fields ?? (maskedRows[0] ? Object.keys(maskedRows[0] as object) : []);
            const payload = format === "csv"
              ? toCsv(maskedRows, fields)
              : JSON.stringify(resultWithMeta({ ...result, fields }, maskedRows, truncated, limit, r.db), null, 2);
            const filePath = makeExportPath(conn.name, format);
            await Bun.write(filePath, payload);

            return {
              text: `Exported ${maskedRows.length} rows to ${filePath}`,
              result: { rows: maskedRows, fields: result.fields },
            };
          } finally {
            clearQueryAbort(logId);
          }
        },
        { precheck: () => switchBlock(statement) ?? (verdict.ok ? undefined : verdict.reason ?? "blocked"), ...queryGateOpts },
      );
    }
  );

  if (on("run_saved_query")) server.tool(
    "run_saved_query",
    "Run a saved query by name",
    {
      name: z.string().describe("Name of the saved query"),
      ...timeoutOption,
      ...dbOption,
      ...paramsOption,
    },
    (args) => {
      const { name } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const params = (args as typeof args & { params?: unknown[] }).params;
      const r = resolveDatabase((args as typeof args & { database?: string }).database);
      if (!r.ok) return Promise.resolve(err(r.error));
      const saved = getSavedQuery(conn.id, name);
      if (!saved) return Promise.resolve(err(`Saved query "${name}" not found.`));
      return gatedQuery(saved.sql, "run_saved_query", toTimeoutMs(timeout), undefined, r.db, params);
    }
  );

  if (on("list_saved_queries")) server.tool(
    "list_saved_queries",
    "List saved queries for this connection",
    async () => {
      try {
        return ok(JSON.stringify(listSavedQueries(conn.id), null, 2));
      } catch (e) {
        return err(`Error: ${(e as Error).message}`);
      }
    }
  );
}

// ── Export helpers ─────────────────────────────────────────────────────────

const EXPORT_DIR = join(homedir(), ".pluk", "exports");

function sanitizeFilename(input: string): string {
  return input.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
}

function toCsv(rows: unknown[], fields: string[]): string {
  if (rows.length === 0) return fields.join(",") + "\n";
  const escape = (v: unknown) => {
    const s = v === null || v === undefined ? "" : String(v);
    if (/[",\n\r]/.test(s)) return `"${s.replace(/"/g, '""')}"`;
    return s;
  };
  const lines = [fields.join(",")];
  for (const row of rows) {
    const r = row as Record<string, unknown>;
    lines.push(fields.map((f) => escape(r[f])).join(","));
  }
  return lines.join("\n") + "\n";
}

function makeExportPath(connName: string, format: string): string {
  mkdirSync(EXPORT_DIR, { recursive: true });
  const ts = new Date().toISOString().replace(/[:.]/g, "-");
  return join(EXPORT_DIR, `${sanitizeFilename(connName)}_${ts}.${format}`);
}
