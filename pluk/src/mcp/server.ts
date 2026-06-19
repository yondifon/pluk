import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import type { Integration } from "../store/integrations.js";
import { getAdapter } from "../adapters/index.js";
import { openSession, closeSession } from "./pool.js";

// MCP streamable-HTTP transport + session registry. Adapter-agnostic: it routes
// requests and delegates server construction to the integration's adapter. Per-
// query cancellation lives in pool.ts (see cancelQuery).

interface Session {
  transport: WebStandardStreamableHTTPServerTransport;
  server: McpServer;
}

const sessions = new Map<string, Session>();

export async function handleMcpRequest(integration: Integration, req: Request): Promise<Response> {
  const sessionId = req.headers.get("Mcp-Session-Id");

  // Route to existing session
  if (sessionId) {
    const session = sessions.get(sessionId);
    if (!session) return new Response("Session not found", { status: 404 });
    return session.transport.handleRequest(req);
  }

  const adapter = getAdapter(integration.type);
  if (!adapter) return new Response(`No adapter for type: ${integration.type}`, { status: 400 });

  // New session — sessionIdRef lets tool handlers look up the pool by session
  const sessionIdRef = { value: "" };
  let session!: Session;

  const transport = new WebStandardStreamableHTTPServerTransport({
    sessionIdGenerator: () => crypto.randomUUID(),
    onsessioninitialized: (sid) => {
      sessionIdRef.value = sid;
      sessions.set(sid, session);
      openSession(sid);
    },
    onsessionclosed: (sid) => {
      sessions.delete(sid);
      closeSession(sid);
    },
  });

  const server = adapter.buildServer(integration, sessionIdRef);
  session = { transport, server };
  await server.connect(transport);

  return transport.handleRequest(req);
}
