import { getConnectionByToken, getConnectionById } from "./store/connections.js";
import { handleMcpRequest } from "./mcp/server.js";
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
      try {
        const driver = await createDriver(conn);
        await driver.testConnection();
        await driver.close();
        return Response.json({ ok: true });
      } catch (err) {
        return Response.json({ ok: false, error: (err as Error).message });
      }
    }

    // /mcp/:token — MCP streamable HTTP endpoint for AI agents
    const token = path.match(/^\/mcp\/([^/]+)/)?.[1];
    if (token) {
      const conn = getConnectionByToken(token);
      if (!conn) return new Response("Connection not found", { status: 404 });
      return handleMcpRequest(conn, req);
    }

    return new Response("Not found", { status: 404 });
  },
});

console.log(`pluk MCP server on http://localhost:${PORT}`);
console.log(`MCP endpoint: http://localhost:${PORT}/mcp/<token>`);

async function shutdown() {
  await server.stop(true); // true = drain in-flight requests
  process.exit(0);
}

process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
