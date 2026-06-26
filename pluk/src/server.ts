import { getIntegrationByToken, getIntegrationById } from "./store/integrations.js";
import { getGroupByToken } from "./store/groups.js";
import { handleMcpRequest, resetSessions } from "./mcp/server.js";
import { buildGroupServer } from "./mcp/group.js";
import { allHealth, recordHealth } from "./mcp/health.js";
import { getAdapter, listAdapters, buildAdapterServer } from "./adapters/index.js";
import { logInfo, logError, LOG_PATH } from "./log.js";

const PORT = Number(process.env.PORT ?? 4242);

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
        tools: a.toolSpecs,
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
        recordHealth(integration.id, "ok");
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
        const reason = adapter.humanizeError?.(err) ?? ((err as Error).message || String(err));
        recordHealth(integration.id, "error", reason);
        return Response.json({ ok: false, error: reason });
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

    for (const adapter of listAdapters()) {
      const response = await adapter.handleGlobalApi?.(req, path);
      if (response) return response;
    }

    const adapterApiMatch = path.match(/^\/api\/integrations\/([^/]+)(\/.+)$/);
    if (adapterApiMatch) {
      const connectionId = adapterApiMatch[1]!;
      const subpath = adapterApiMatch[2]!;
      const conn = getIntegrationById(connectionId);
      if (!conn) return Response.json({ ok: false, error: "Not found" }, { status: 404 });
      const adapter = getAdapter(conn.type);
      const response = await adapter?.handleApi?.(conn, req, subpath);
      if (response) return response;
    }

    // /mcp/:token — MCP streamable HTTP endpoint for AI agents. A token resolves
    // to a single integration or a group (one endpoint fronting many).
    const token = path.match(/^\/mcp\/([^/]+)/)?.[1];
    if (token) {
      const conn = getIntegrationByToken(token);
      if (conn) {
        const adapter = getAdapter(conn.type);
        if (!adapter) return new Response(`No adapter for type: ${conn.type}`, { status: 400 });
        return handleMcpRequest(req, conn.id, (ref) => buildAdapterServer(adapter, conn, ref));
      }

      const group = getGroupByToken(token);
      if (group) return handleMcpRequest(req, group.id, (ref) => buildGroupServer(group, ref));

      return new Response("Integration not found", { status: 404 });
    }

    if (path === "/health") return new Response("ok");

    // GET /api/health — per-connection health for the UI, so a failing
    // connection shows red without the user manually testing it.
    if (path === "/api/health" && req.method === "GET") {
      return Response.json({ health: allHealth() });
    }

    return new Response("Not found", { status: 404 });
  },
});


logInfo(`pluk MCP server on http://localhost:${PORT}`, { logFile: LOG_PATH });
console.log(`MCP endpoint: http://localhost:${PORT}/mcp/<token>`);
console.log(`Debug log: ${LOG_PATH}`);

async function shutdown() {
  await server.stop(true); // true = drain in-flight requests
  process.exit(0);
}

process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
