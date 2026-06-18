import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { z } from "zod";
import type { Connection } from "../store/connections.js";
import { createDriver, type Driver } from "../db/index.js";
import { parsePolicy, evaluate, capRows, dialectFor, policyDescription } from "./policy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../store/queryLog.js";

// ── Driver pool ──────────────────────────────────────────────────────────────
// One driver per MCP session. Closed after IDLE_MS of inactivity.

const IDLE_MS = 5 * 60 * 1000; // 5 minutes

// Hard wall-clock timeout for any single tool call, including SSH tunnel setup.
// PG's own connectionTimeoutMillis only covers the TCP leg to localhost (the tunnel
// listener), not the PG handshake through the tunnel. This catches the gap.
const TOOL_TIMEOUT_MS = 30_000; // 30 seconds

function withToolTimeout<T>(work: Promise<T>, label: string): Promise<T> {
  return Promise.race([
    work,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`Timed out after 30s (${label})`)), TOOL_TIMEOUT_MS)
    ),
  ]);
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
  driver: Driver;
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

async function getDriver(sessionId: string, conn: Connection): Promise<Driver> {
  const existing = driverPool.get(sessionId);
  if (existing) {
    // Reset idle timer on use
    clearTimeout(existing.idleTimer);
    existing.idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
    return existing.driver;
  }

  const driver = await createDriver(conn);
  const idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
  driverPool.set(sessionId, { driver, idleTimer });
  return driver;
}

function evictDriver(sessionId: string): void {
  const entry = driverPool.get(sessionId);
  if (!entry) return;
  driverPool.delete(sessionId);
  entry.driver.close().catch(() => {}); // best-effort close
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
      const logId = createLogEntry(conn.id, conn.name, sql, "pending", verdict.categories);

      // Per-query abort controller; also tripped by a session-wide abort.
      const queryAc = new AbortController();
      queryAborts.set(logId, queryAc);
      const sessionSignal = sessionAborts.get(sid)?.signal;
      if (sessionSignal) {
        if (sessionSignal.aborted) queryAc.abort();
        else sessionSignal.addEventListener("abort", () => queryAc.abort(), { once: true });
      }

      try {
        const work = (async () => {
          const driver = await getDriver(sid, conn);
          return driver.query(sql);
        })();

        const result = await withToolTimeout(
          withCancellable(work, queryAc.signal),
          "query"
        );

        const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
        updateLogEntry(logId, "allowed", undefined, result);

        let text = JSON.stringify({ ...result, rows }, null, 2);
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
      return await withToolTimeout((async () => {
        const driver = await getDriver(sid, conn);
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
        return await withToolTimeout((async () => {
          const driver = await getDriver(sid, conn);
          const effectiveLimit = policy.maxRows === null ? limit : Math.min(limit, policy.maxRows);
          const result = await driver.sampleTable(table, effectiveLimit);
          const { rows, truncated } = capRows(result.rows, policy.maxRows);
          let text = JSON.stringify({ fields: result.fields ?? [], rows }, null, 2);
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
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name") },
    async ({ table }) => {
      const sid = sessionIdRef.value;
      try {
        return await withToolTimeout((async () => {
          const driver = await getDriver(sid, conn);
          const columns = await driver.describeTable(table);
          return { content: [{ type: "text", text: JSON.stringify(columns, null, 2) }] };
        })(), "describe_table");
      } catch (err) {
        evictDriver(sid);
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      }
    }
  );

  server.tool("list_schemas", "List all schemas or databases", async () => {
    const sid = sessionIdRef.value;
    try {
      return await withToolTimeout((async () => {
        const driver = await getDriver(sid, conn);
        const schemas = await driver.listSchemas();
        return { content: [{ type: "text", text: schemas.join("\n") }] };
      })(), "list_schemas");
    } catch (err) {
      evictDriver(sid);
      return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
    }
  });

  return server;
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
