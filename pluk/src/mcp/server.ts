import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { openSession, closeSession } from "./pool.js";

// MCP streamable-HTTP transport + session registry. Target-agnostic: the caller
// supplies a factory that builds the McpServer (a single integration's adapter
// server, or a group's aggregated server). Per-query cancellation lives in
// pool.ts (see cancelQuery).

interface Session {
  transport: WebStandardStreamableHTTPServerTransport;
  server: McpServer;
  // The integration or group id this session serves, so edits to it can reset
  // just its sessions (a standalone integration and the same integration inside
  // a group are different owners → different sessions → isolated connections).
  ownerId: string;
}

const sessions = new Map<string, Session>();

/** Build the MCP server for a new session. `sessionIdRef` is filled once the
 *  session id is assigned, so tool handlers can key the driver pool by it. */
export type ServerFactory = (sessionIdRef: { value: string }) => McpServer;

/**
 * Drop live MCP sessions so config edits take effect (servers bake in config —
 * incl. group member overrides — and policy at build time; the next client
 * request re-initializes from current DB state). With `ownerId`, resets only the
 * sessions for that integration/group; without it, resets all. Returns the count.
 */
export async function resetSessions(ownerId?: string): Promise<number> {
  const ids = [...sessions.keys()].filter(
    (sid) => !ownerId || sessions.get(sid)?.ownerId === ownerId
  );
  for (const sid of ids) {
    const session = sessions.get(sid);
    sessions.delete(sid);
    closeSession(sid); // abort in-flight calls + evict pooled drivers/tunnels
    try { await session?.server.close(); } catch { /* best-effort */ }
  }
  return ids.length;
}

export async function handleMcpRequest(req: Request, ownerId: string, makeServer: ServerFactory): Promise<Response> {
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
      openSession(sid);
    },
    onsessionclosed: (sid) => {
      sessions.delete(sid);
      closeSession(sid);
    },
  });

  const server = makeServer(sessionIdRef);
  session = { transport, server, ownerId };
  await server.connect(transport);

  return transport.handleRequest(req);
}
