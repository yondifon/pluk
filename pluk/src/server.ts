import { getIntegrationByToken, getIntegrationById } from "./store/integrations.js";
import { getGroupByToken } from "./store/groups.js";
import { listSavedQueries, createSavedQuery, deleteSavedQuery } from "./store/savedQueries.js";
import { listSavedCommands, createSavedCommand, deleteSavedCommand } from "./store/savedCommands.js";
import { listMaskedColumns, addMaskedColumn, removeMaskedColumn } from "./store/maskedColumns.js";
import { handleMcpRequest, resetSessions } from "./mcp/server.js";
import { buildGroupServer } from "./mcp/group.js";
import { cancelQuery } from "./mcp/pool.js";
import { getAdapter, listAdapters } from "./adapters/index.js";
import { logInfo, logError, LOG_PATH } from "./log.js";

const PORT = Number(process.env.PORT ?? 4242);

// Parse a JSON request body, returning null on malformed/empty input so routes
// can answer with a clean { ok: false } 400 instead of throwing a raw 500.
async function readJsonBody<T>(req: Request): Promise<T | null> {
  try {
    return (await req.json()) as T;
  } catch {
    return null;
  }
}

// Translate a duplicate-name UNIQUE violation into a 409 the UI can show;
// rethrow anything else so genuine failures still surface as a 500.
function conflictOrThrow(err: unknown, message: string): Response {
  if (/UNIQUE constraint/i.test((err as Error).message)) {
    return Response.json({ ok: false, error: message }, { status: 409 });
  }
  throw err;
}

const server = Bun.serve({
  port: PORT,
  // Loopback only: the REST/MCP surface is unauthenticated beyond the per-conn
  // token, and the product promise is that nothing leaves the laptop. Without
  // this, Bun.serve defaults to 0.0.0.0 and exposes it to the whole LAN.
  hostname: "127.0.0.1",
  idleTimeout: 255,
  async fetch(req) {
    const url = new URL(req.url);
    const path = url.pathname;

    // GET /api/adapters — adapter catalog for the UI to render forms dynamically.
    // configFields are definitions only (no secret values), so safe to expose.
    if (path === "/api/adapters" && req.method === "GET") {
      const adapters = listAdapters().map((a) => ({
        id: a.id,
        label: a.label,
        category: a.category,
        policyKind: a.policyKind,
        agentHint: a.agentHint,
        configFields: a.configFields,
      }));
      return Response.json({ adapters });
    }

    // POST /api/integrations/:id/test — called by the Swift UI
    const testId = path.match(/^\/api\/integrations\/([^/]+)\/test$/)?.[1];
    if (testId && req.method === "POST") {
      const integration = getIntegrationById(testId);
      if (!integration) return Response.json({ ok: false, error: "Not found" }, { status: 404 });
      const adapter = getAdapter(integration.type);
      if (!adapter) return Response.json({ ok: false, error: `No adapter for type: ${integration.type}` }, { status: 400 });
      try {
        await adapter.testConnection(integration);
        logInfo("connection test ok", { id: integration.id, name: integration.name, type: integration.type });
        return Response.json({ ok: true });
      } catch (err) {
        logError("connection test failed", err, {
          id: integration.id,
          name: integration.name,
          type: integration.type,
          host: integration.config.host,
          port: integration.config.port,
          use_ssh: integration.config.use_ssh ?? false,
          use_ssl: integration.config.use_ssl ?? false,
        });
        return Response.json({ ok: false, error: formatTestError(err as Error) });
      }
    }

    // POST /api/reload?id=<integration|group id> — drop live MCP sessions so
    // config/override edits in the UI take effect on the next agent request
    // (sessions bake in config at build time). Scoped to the given owner id when
    // provided, so editing one group/integration doesn't disturb the others.
    if (path === "/api/reload" && req.method === "POST") {
      const id = url.searchParams.get("id") ?? undefined;
      const count = await resetSessions(id);
      logInfo("reloaded MCP sessions", { count, id: id ?? "all" });
      return Response.json({ ok: true, count });
    }

    // POST /api/log/:id/cancel — cancel an in-flight query, called by the Swift UI
    const cancelId = path.match(/^\/api\/log\/(\d+)\/cancel$/)?.[1];
    if (cancelId && req.method === "POST") {
      const ok = cancelQuery(Number(cancelId));
      return Response.json({ ok });
    }

    // Saved queries REST endpoints (consumed by the Swift UI later)
    const savedMatch = path.match(/^\/api\/integrations\/([^/]+)\/saved_queries(?:\/([^/]+))?$/);
    if (savedMatch) {
      const connectionId = savedMatch[1]!;
      const savedName = savedMatch[2] ? decodeURIComponent(savedMatch[2]) : undefined;
      const conn = getIntegrationById(connectionId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });

      if (req.method === "GET") {
        return Response.json({ ok: true, queries: listSavedQueries(connectionId) });
      }

      if (req.method === "POST") {
        const body = await readJsonBody<{ name?: string; sql?: string }>(req);
        if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
        if (!body.name?.trim() || !body.sql?.trim()) {
          return Response.json({ ok: false, error: "name and sql required" }, { status: 400 });
        }
        try {
          const q = createSavedQuery({ connection_id: connectionId, name: body.name.trim(), sql: body.sql });
          return Response.json({ ok: true, query: q });
        } catch (err) {
          return conflictOrThrow(err, "A saved query with that name already exists.");
        }
      }

      if (req.method === "DELETE" && savedName) {
        const ok = deleteSavedQuery(connectionId, savedName);
        return Response.json({ ok });
      }

      return new Response("Method not allowed", { status: 405 });
    }

    // Saved commands REST endpoints (SSH pre-selected commands; consumed by the Swift UI later)
    const savedCmdMatch = path.match(/^\/api\/integrations\/([^/]+)\/saved_commands(?:\/([^/]+))?$/);
    if (savedCmdMatch) {
      const connectionId = savedCmdMatch[1]!;
      const savedName = savedCmdMatch[2] ? decodeURIComponent(savedCmdMatch[2]) : undefined;
      const conn = getIntegrationById(connectionId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });

      if (req.method === "GET") {
        return Response.json({ ok: true, commands: listSavedCommands(connectionId) });
      }

      if (req.method === "POST") {
        const body = await readJsonBody<{ name?: string; command?: string; working_dir?: string }>(req);
        if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
        if (!body.name?.trim() || !body.command?.trim()) {
          return Response.json({ ok: false, error: "name and command required" }, { status: 400 });
        }
        try {
          const c = createSavedCommand({
            connection_id: connectionId,
            name: body.name.trim(),
            command: body.command,
            working_dir: body.working_dir?.trim() || null,
          });
          return Response.json({ ok: true, command: c });
        } catch (err) {
          return conflictOrThrow(err, "A saved command with that name already exists.");
        }
      }

      if (req.method === "DELETE" && savedName) {
        const ok = deleteSavedCommand(connectionId, savedName);
        return Response.json({ ok });
      }

      return new Response("Method not allowed", { status: 405 });
    }

    // Masked columns REST endpoints (consumed by the Swift UI later)
    const maskMatch = path.match(/^\/api\/integrations\/([^/]+)\/masked_columns(?:\/([^/]+))?$/);
    if (maskMatch) {
      const connectionId = maskMatch[1]!;
      const columnName = maskMatch[2] ? decodeURIComponent(maskMatch[2]) : undefined;
      const conn = getIntegrationById(connectionId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });

      if (req.method === "GET") {
        return Response.json({ ok: true, columns: listMaskedColumns(connectionId) });
      }

      if (req.method === "POST") {
        const body = await readJsonBody<{ column_name?: string }>(req);
        if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
        if (!body.column_name?.trim()) {
          return Response.json({ ok: false, error: "column_name required" }, { status: 400 });
        }
        const c = addMaskedColumn(connectionId, body.column_name.trim());
        return Response.json({ ok: true, column: c });
      }

      if (req.method === "DELETE" && columnName) {
        const ok = removeMaskedColumn(connectionId, columnName);
        return Response.json({ ok });
      }

      return new Response("Method not allowed", { status: 405 });
    }

    // /mcp/:token — MCP streamable HTTP endpoint for AI agents. A token resolves
    // to a single integration or a group (one endpoint fronting many).
    const token = path.match(/^\/mcp\/([^/]+)/)?.[1];
    if (token) {
      const conn = getIntegrationByToken(token);
      if (conn) {
        const adapter = getAdapter(conn.type);
        if (!adapter) return new Response(`No adapter for type: ${conn.type}`, { status: 400 });
        return handleMcpRequest(req, conn.id, (ref) => adapter.buildServer(conn, ref));
      }

      const group = getGroupByToken(token);
      if (group) return handleMcpRequest(req, group.id, (ref) => buildGroupServer(group, ref));

      return new Response("Integration not found", { status: 404 });
    }

    if (path === "/health") return new Response("ok");

    return new Response("Not found", { status: 404 });
  },
});

// Map low-level driver errors to a short, actionable message for the UI. Full
// detail (code + stack) is always in the debug log; this is what the user sees.
function formatTestError(err: Error): string {
  const msg = err.message;
  const code = (err as { code?: string }).code;

  // Authentication — covers Postgres (28P01/28000) and PgBouncer, which reports
  // a bad SCRAM password as a protocol violation ("SASL authentication failed").
  if (code === "28P01" || code === "28000" || /password authentication failed|SASL authentication failed/i.test(msg)) {
    return "Authentication failed — check the username and password.";
  }

  // Database / schema missing
  if (code === "3D000" || /database .* does not exist/i.test(msg)) {
    return "Database not found — check the database name.";
  }

  // Host unreachable / refused
  if (code === "ECONNREFUSED" || /ECONNREFUSED/i.test(msg)) {
    return "Connection refused — check the host and port, and that the server is reachable (through the SSH tunnel, if used).";
  }
  if (code === "ENOTFOUND" || /no such host|name or service not known/i.test(msg)) {
    return `Host not found — check the host name. If using an SSH proxy (cloudflared), this is often transient — try again. Detail: ${msg}`;
  }

  // SSL / TLS
  if (/self.signed|certificate|\bssl\b|\btls\b/i.test(msg)) {
    return `SSL error — try a different SSL mode, or disable SSL if the server doesn't require it. Detail: ${msg}`;
  }

  // Timeouts (direct or post-tunnel)
  if (/timed out|connection timeout|timeout expired/i.test(msg)) {
    return "Timed out — the server didn't respond. Check the host/port, the SSH tunnel, and any firewall/VPC rules.";
  }

  // SSH (ssh2 driver) — auth, host key, and key parsing surface as raw strings.
  if (/All configured authentication methods failed/i.test(msg)) {
    return "SSH authentication failed — check the user and that your key or agent is set up for this host.";
  }
  if (/no usable private key|cannot parse privatekey|encrypted.*passphrase|bad passphrase/i.test(msg)) {
    return "SSH key problem — the key couldn't be read or the passphrase is wrong. Check the key path and passphrase.";
  }
  if (/handshake|host key|hostkey/i.test(msg)) {
    return `SSH host key error — the server's host key was rejected. Detail: ${msg}`;
  }

  // Unknown — surface the raw message and point at the log for the full trace.
  return `${msg} (see Logs, or ~/.pluk/pluk.log, for details)`;
}

logInfo(`pluk MCP server on http://localhost:${PORT}`, { logFile: LOG_PATH });
console.log(`MCP endpoint: http://localhost:${PORT}/mcp/<token>`);
console.log(`Debug log: ${LOG_PATH}`);

async function shutdown() {
  await server.stop(true); // true = drain in-flight requests
  process.exit(0);
}

process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
