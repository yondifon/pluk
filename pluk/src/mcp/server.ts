import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { z } from "zod";
import { mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { Connection } from "../store/connections.js";
import { createDriver, type Driver } from "../db/index.js";
import { parsePolicy, evaluate, capRows, dialectFor, policyDescription, parsePostgresCost } from "./policy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../store/queryLog.js";
import { listSavedQueries, getSavedQuery } from "../store/savedQueries.js";
import { listMaskedColumns, maskRow } from "../store/maskedColumns.js";

// ── Driver pool ──────────────────────────────────────────────────────────────
// One driver per MCP session. Closed after IDLE_MS of inactivity.

const IDLE_MS = 5 * 60 * 1000; // 5 minutes

// Hard wall-clock timeout for any single tool call, including SSH tunnel setup.
// PG's own connectionTimeoutMillis only covers the TCP leg to localhost (the tunnel
// listener), not the PG handshake through the tunnel. This catches the gap.
const TOOL_TIMEOUT_MS = 30_000; // 30 seconds

// Connection/tunnel setup gets its own, larger budget than a single query.
// An SSH proxy (e.g. Cloudflare Access) can require interactive browser auth on
// first use, which easily exceeds the 30s query timeout. Direct connections stay
// tight so a dead host still surfaces fast.
const CONNECT_TIMEOUT_SSH_MS = 180_000; // 3 minutes — room for interactive proxy auth
const CONNECT_TIMEOUT_DIRECT_MS = 30_000;

function withTimeout<T>(work: Promise<T>, ms: number, label: string): Promise<T> {
  return Promise.race([
    work,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`Timed out after ${Math.round(ms / 1000)}s (${label})`)), ms)
    ),
  ]);
}

function withToolTimeout<T>(work: Promise<T>, label: string): Promise<T> {
  return withTimeout(work, TOOL_TIMEOUT_MS, label);
}

function withCancellable<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(new Error("Query cancelled"));
  return Promise.race([
    work,
    new Promise<T>((_, reject) => {
      signal.addEventListener("abort", () => reject(new Error("Query cancelled")), { once: true });
    }),
  ]);
}

interface DriverEntry {
  // In-flight promise, not the resolved driver: concurrent tool calls during
  // setup must share one connection/tunnel instead of each spawning their own.
  driver: Promise<Driver>;
  idleTimer: ReturnType<typeof setTimeout>;
}

const driverPool = new Map<string, DriverEntry>();
const sessionAborts = new Map<string, AbortController>();

// Per-query abort controllers, keyed by log entry id, so the UI can cancel a
// single in-flight query (POST /api/log/:id/cancel) without tearing down the session.
const queryAborts = new Map<number, AbortController>();

/** Abort a single in-flight query by its log id. Returns false if not running. */
export function cancelQuery(logId: number): boolean {
  const ac = queryAborts.get(logId);
  if (!ac) return false;
  ac.abort();
  return true;
}

function getDriver(sessionId: string, conn: Connection): Promise<Driver> {
  const existing = driverPool.get(sessionId);
  if (existing) {
    // Reset idle timer on use
    clearTimeout(existing.idleTimer);
    existing.idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
    return existing.driver;
  }

  // Register the in-flight promise synchronously so any tool call that arrives
  // while the tunnel/connection is still coming up awaits this same setup
  // instead of starting a second one (which, with an SSH proxy, meant a second
  // interactive auth prompt racing the timeout).
  const connectTimeout = conn.use_ssh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  const driver = withTimeout(createDriver(conn), connectTimeout, "connect");
  const idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
  const entry: DriverEntry = { driver, idleTimer };
  driverPool.set(sessionId, entry);

  // If setup fails, drop the entry so the next call retries from scratch.
  driver.catch(() => {
    if (driverPool.get(sessionId) === entry) {
      clearTimeout(idleTimer);
      driverPool.delete(sessionId);
    }
  });

  return driver;
}

function evictDriver(sessionId: string): void {
  const entry = driverPool.get(sessionId);
  if (!entry) return;
  driverPool.delete(sessionId);
  clearTimeout(entry.idleTimer);
  // best-effort close once setup settles; ignore a failed/aborted setup
  entry.driver.then((d) => d.close()).catch(() => {});
}

// ── MCP session registry ─────────────────────────────────────────────────────

interface Session {
  transport: WebStandardStreamableHTTPServerTransport;
  server: McpServer;
}

const sessions = new Map<string, Session>();

// ── Server factory ───────────────────────────────────────────────────────────

function buildMcpServer(conn: Connection, sessionIdRef: { value: string }): McpServer {
  const policy = parsePolicy(conn.query_policy, conn.read_only);
  const dialect = dialectFor(conn.type);
  const policyDesc = policyDescription(policy);

  const server = new McpServer({ name: conn.name, version: "1.0.0" });

  const readOnlyMode = policy.allowed.length === 2 && policy.allowed.includes("select") && policy.allowed.includes("inspect");
  const maskedColumns = listMaskedColumns(conn.id);

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
        evictDriver(sid);
        return { contents: [{ uri: "schema://full", mimeType: "text/plain", text: `Error: ${(err as Error).message}` }] };
      }
    }
  );

  server.tool(
    "query",
    `Run a SQL query against the database. ${policyDesc}`,
    { sql: z.string().describe("SQL query to execute") },
    async ({ sql }) => {
      // Policy check (parse-driven)
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;

      // Create pending log entry before executing so it's visible immediately
      const logId = createLogEntry(conn.id, conn.name, sql, "pending", verdict.categories, undefined, "query");

      // Per-query abort controller; also tripped by a session-wide abort.
      const queryAc = new AbortController();
      queryAborts.set(logId, queryAc);
      const sessionSignal = sessionAborts.get(sid)?.signal;
      if (sessionSignal) {
        if (sessionSignal.aborted) queryAc.abort();
        else sessionSignal.addEventListener("abort", () => queryAc.abort(), { once: true });
      }

      try {
        const costGateEnabled = policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null;
        const driver = await getDriver(sid, conn);

        if (costGateEnabled && conn.type === "postgres") {
          const explain = await driver.explain(sql);
          const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
          const estimate = parsePostgresCost(plan);
          if (
            (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
            (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
          ) {
            const reason = `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
            updateLogEntry(logId, "blocked", reason);
            return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
          }
        }

        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const work = useReadOnly ? driver.queryReadOnly(sql) : driver.query(sql);

        const result = await withToolTimeout(
          withCancellable(work, queryAc.signal),
          "query"
        );

        const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        const maskedResult = { ...result, rows: maskedRows };
        updateLogEntry(logId, "allowed", undefined, maskedResult);

        let text = JSON.stringify(maskedResult, null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
        }
        return { content: [{ type: "text", text }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) evictDriver(sid);
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        queryAborts.delete(logId);
      }
    }
  );

  server.tool("list_tables", "List all tables in the database", async () => {
    const sid = sessionIdRef.value;
    try {
      const driver = await getDriver(sid, conn);
      return await withToolTimeout((async () => {
        const tables = await driver.listTables();
        return { content: [{ type: "text", text: tables.join("\n") }] };
      })(), "list_tables");
    } catch (err) {
      evictDriver(sid);
      return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
    }
  });

  server.tool(
    "sample_table",
    "Preview rows from a table without writing SQL",
    {
      table: z.string().describe("Table name"),
      limit: z.number().int().min(1).max(1000).default(20).describe("Max rows to preview"),
    },
    async ({ table, limit }) => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const effectiveLimit = policy.maxRows === null ? limit : Math.min(limit, policy.maxRows);
          const result = await driver.sampleTable(table, effectiveLimit);
          const { rows, truncated } = capRows(result.rows, policy.maxRows);
          const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
          let text = JSON.stringify({ fields: result.fields ?? [], rows: maskedRows }, null, 2);
          if (truncated) {
            text += `\n\n[Row limit: showing first ${policy.maxRows} of ${result.rows.length} rows.]`;
          }
          return { content: [{ type: "text", text }] };
        })(), "sample_table");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool(
    "explain_query",
    "Show query execution plan without running the query",
    {
      sql: z.string().describe("SQL query to explain"),
    },
    async ({ sql }) => {
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const result = await driver.explain(sql);
          return { content: [{ type: "text", text: JSON.stringify(result, null, 2) }] };
        })(), "explain_query");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool(
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name") },
    async ({ table }) => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const columns = await driver.describeTable(table);
          return { content: [{ type: "text", text: JSON.stringify(columns, null, 2) }] };
        })(), "describe_table");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool(
    "list_relationships",
    "List foreign key relationships between tables",
    { table: z.string().optional().describe("Filter to a specific table (optional)") },
    async ({ table }) => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const relationships = await driver.listRelationships(table);
          return { content: [{ type: "text", text: JSON.stringify(relationships, null, 2) }] };
        })(), "list_relationships");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool(
    "search_schema",
    "Find tables or columns matching a term",
    { term: z.string().describe("Search term (substring match on table or column names)") },
    async ({ term }) => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const matches = await driver.searchSchema(term);
          return { content: [{ type: "text", text: JSON.stringify(matches, null, 2) }] };
        })(), "search_schema");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool(
    "table_stats",
    "Get cheap table statistics (estimated rows, size, indexes)",
    { table: z.string().describe("Table name") },
    async ({ table }) => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const stats = await driver.tableStats(table);
          return { content: [{ type: "text", text: JSON.stringify(stats, null, 2) }] };
        })(), "table_stats");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool("list_schemas", "List all schemas or databases", async () => {
    const sid = sessionIdRef.value;
    try {
      const driver = await getDriver(sid, conn);
      return await withToolTimeout((async () => {
        const schemas = await driver.listSchemas();
        return { content: [{ type: "text", text: schemas.join("\n") }] };
      })(), "list_schemas");
    } catch (err) {
      evictDriver(sid);
      return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
    }
  });

  server.tool(
    "export_query",
    "Run a SQL query and save results to a local CSV or JSON file",
    {
      sql: z.string().describe("SQL query to execute"),
      format: z.enum(["csv", "json"]).default("csv").describe("Export file format"),
    },
    async ({ sql, format }) => {
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;
      const logId = createLogEntry(conn.id, conn.name, sql, "pending", verdict.categories, undefined, "export_query");
      const queryAc = new AbortController();
      queryAborts.set(logId, queryAc);
      const sessionSignal = sessionAborts.get(sid)?.signal;
      if (sessionSignal) {
        if (sessionSignal.aborted) queryAc.abort();
        else sessionSignal.addEventListener("abort", () => queryAc.abort(), { once: true });
      }

      try {
        const driver = await getDriver(sid, conn);
        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const result = await withToolTimeout(
          withCancellable(useReadOnly ? driver.queryReadOnly(sql) : driver.query(sql), queryAc.signal),
          "export_query"
        );

        const { rows } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        updateLogEntry(logId, "allowed", undefined, { rows: maskedRows, fields: result.fields });

        const fields = result.fields ?? (maskedRows[0] ? Object.keys(maskedRows[0] as object) : []);
        const payload = format === "csv" ? toCsv(maskedRows, fields) : JSON.stringify({ fields, rows: maskedRows }, null, 2);
        const filePath = makeExportPath(conn.name, format);
        await Bun.write(filePath, payload);

        return { content: [{ type: "text", text: `Exported ${maskedRows.length} rows to ${filePath}` }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) evictDriver(sid);
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        queryAborts.delete(logId);
      }
    }
  );

  server.tool(
    "run_saved_query",
    "Run a saved query by name",
    { name: z.string().describe("Name of the saved query") },
    async ({ name }) => {
      const saved = getSavedQuery(conn.id, name);
      if (!saved) {
        return { content: [{ type: "text", text: `Saved query "${name}" not found.` }], isError: true };
      }

      const verdict = evaluate(saved.sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, saved.sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;
      const logId = createLogEntry(conn.id, conn.name, saved.sql, "pending", verdict.categories, undefined, "run_saved_query");
      const queryAc = new AbortController();
      queryAborts.set(logId, queryAc);
      const sessionSignal = sessionAborts.get(sid)?.signal;
      if (sessionSignal) {
        if (sessionSignal.aborted) queryAc.abort();
        else sessionSignal.addEventListener("abort", () => queryAc.abort(), { once: true });
      }

      try {
        const driver = await getDriver(sid, conn);

        if ((policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null) && conn.type === "postgres") {
          const explain = await driver.explain(saved.sql);
          const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
          const estimate = parsePostgresCost(plan);
          if (
            (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
            (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
          ) {
            const reason = `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
            updateLogEntry(logId, "blocked", reason);
            return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
          }
        }

        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const work = useReadOnly ? driver.queryReadOnly(saved.sql) : driver.query(saved.sql);
        const result = await withToolTimeout(withCancellable(work, queryAc.signal), "run_saved_query");

        const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        const maskedResult = { ...result, rows: maskedRows };
        updateLogEntry(logId, "allowed", undefined, maskedResult);

        let text = JSON.stringify(maskedResult, null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
        }
        return { content: [{ type: "text", text }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) evictDriver(sid);
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        queryAborts.delete(logId);
      }
    }
  );

  server.tool(
    "list_saved_queries",
    "List saved queries for this connection",
    async () => {
      try {
        const queries = listSavedQueries(conn.id);
        return { content: [{ type: "text", text: JSON.stringify(queries, null, 2) }] };
      } catch (err) {
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  return server;
}

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

// ── Public handler ───────────────────────────────────────────────────────────

export async function handleMcpRequest(conn: Connection, req: Request): Promise<Response> {
  const sessionId = req.headers.get("Mcp-Session-Id");

  // Route to existing session
  if (sessionId) {
    const session = sessions.get(sessionId);
    if (!session) return new Response("Session not found", { status: 404 });
    return session.transport.handleRequest(req);
  }

  // New session — sessionIdRef lets tool handlers look up the pool by session
  const sessionIdRef = { value: "" };
  let session!: Session;

  const transport = new WebStandardStreamableHTTPServerTransport({
    sessionIdGenerator: () => crypto.randomUUID(),
    onsessioninitialized: (sid) => {
      sessionIdRef.value = sid;
      sessions.set(sid, session);
      sessionAborts.set(sid, new AbortController());
    },
    onsessionclosed: (sid) => {
      sessions.delete(sid);
      // Signal cancellation to any in-flight tool calls before closing driver
      const ac = sessionAborts.get(sid);
      ac?.abort();
      sessionAborts.delete(sid);
      evictDriver(sid);
    },
  });

  const server = buildMcpServer(conn, sessionIdRef);
  session = { transport, server };
  await server.connect(transport);

  return transport.handleRequest(req);
}
