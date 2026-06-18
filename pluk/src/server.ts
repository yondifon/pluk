import { getConnectionByToken, getConnectionById } from "./store/connections.js";
import { listSavedQueries, createSavedQuery, deleteSavedQuery } from "./store/savedQueries.js";
import { handleMcpRequest, cancelQuery } from "./mcp/server.js";
import { createDriver } from "./db/index.js";

const PORT = Number(process.env.PORT ?? 4242);

const server = Bun.serve({
  port: PORT,
  idleTimeout: 255,
  async fetch(req) {
    const url = new URL(req.url);
    const path = url.pathname;

    // POST /api/connections/:id/test — called by the Swift UI
    const testId = path.match(/^\/api\/connections\/([^/]+)\/test$/)?.[1];
    if (testId && req.method === "POST") {
      const conn = getConnectionById(testId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });
      let driver: Awaited<ReturnType<typeof createDriver>> | undefined;
      try {
        driver = await createDriver(conn);
        await driver.testConnection();
        return Response.json({ ok: true });
      } catch (err) {
        return Response.json({ ok: false, error: formatTestError(err as Error) });
      } finally {
        try {
          await driver?.close();
        } catch (err) {
          console.error(`[pluk] test cleanup error: ${(err as Error).message}`);
        }
      }
    }

    // POST /api/log/:id/cancel — cancel an in-flight query, called by the Swift UI
    const cancelId = path.match(/^\/api\/log\/(\d+)\/cancel$/)?.[1];
    if (cancelId && req.method === "POST") {
      const ok = cancelQuery(Number(cancelId));
      return Response.json({ ok });
    }

    // Saved queries REST endpoints (consumed by the Swift UI later)
    const savedMatch = path.match(/^\/api\/connections\/([^/]+)\/saved_queries(?:\/([^/]+))?$/);
    if (savedMatch) {
      const connectionId = savedMatch[1]!;
      const savedName = savedMatch[2];
      const conn = getConnectionById(connectionId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });

      if (req.method === "GET") {
        return Response.json({ ok: true, queries: listSavedQueries(connectionId) });
      }

      if (req.method === "POST") {
        const body = await req.json() as { name?: string; sql?: string };
        if (!body.name || !body.sql) {
          return Response.json({ ok: false, error: "name and sql required" }, { status: 400 });
        }
        const q = createSavedQuery({ connection_id: connectionId, name: body.name, sql: body.sql });
        return Response.json({ ok: true, query: q });
      }

      if (req.method === "DELETE" && savedName) {
        const ok = deleteSavedQuery(connectionId, savedName);
        return Response.json({ ok });
      }

      return new Response("Method not allowed", { status: 405 });
    }

    // /mcp/:token — MCP streamable HTTP endpoint for AI agents
    const token = path.match(/^\/mcp\/([^/]+)/)?.[1];
    if (token) {
      const conn = getConnectionByToken(token);
      if (!conn) return new Response("Connection not found", { status: 404 });
      return handleMcpRequest(conn, req);
    }

    if (path === "/health") return new Response("ok");

    return new Response("Not found", { status: 404 });
  },
});

function formatTestError(err: Error): string {
  if (/connection timeout|timeout expired/i.test(err.message)) {
    return "Postgres timed out after SSH connected. Check DB host/port from the SSH host and firewall/VPC rules.";
  }

  if (/no such host|name or service not known/i.test(err.message)) {
    return `SSH proxy (cloudflared?) failed DNS lookup. This is usually transient — try again. Detail: ${err.message}`;
  }

  return err.message;
}

console.log(`pluk MCP server on http://localhost:${PORT}`);
console.log(`MCP endpoint: http://localhost:${PORT}/mcp/<token>`);

async function shutdown() {
  await server.stop(true); // true = drain in-flight requests
  process.exit(0);
}

process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
