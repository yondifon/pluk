import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { openSession, closeSession } from "./pool.js";
import { logError } from "../log.js";

// MCP streamable-HTTP transport + session registry. Target-agnostic: the caller
// supplies a factory that builds the McpServer (a single integration's adapter
// server, or a group's aggregated server). Adapter-owned pools subscribe to
// session close hooks in pool.ts.

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
    closeSession(sid); // abort in-flight calls + notify adapter-owned pools
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
    try {
      return await session.transport.handleRequest(req);
    } catch (err) {
      logError("MCP request failed", err, { ownerId: session.ownerId, sessionId });
      return new Response("MCP request failed; session kept alive", { status: 500 });
    }
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

  try {
    return await transport.handleRequest(req);
  } catch (err) {
    logError("MCP request failed during session init", err, { ownerId });
    return new Response("MCP request failed during session init", { status: 500 });
  }
}
