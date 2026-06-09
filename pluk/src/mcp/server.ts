import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { z } from "zod";
import type { Connection } from "../store/connections.js";
import { createDriver } from "../db/index.js";

interface Session {
  transport: WebStandardStreamableHTTPServerTransport;
  server: McpServer;
}

const sessions = new Map<string, Session>();

function buildMcpServer(conn: Connection): McpServer {
  const server = new McpServer({ name: conn.name, version: "1.0.0" });

  server.tool(
    "query",
    "Run a SQL query against the database. For production data, prefer SELECT-only queries, add explicit LIMIT clauses, and avoid broad scans or long-running statements.",
    { sql: z.string().describe("SQL query to execute") },
    async ({ sql }) => {
      const driver = await createDriver(conn);
      try {
        if (conn.read_only && /^\s*(insert|update|delete|drop|truncate|alter|create)/i.test(sql)) {
          return { content: [{ type: "text", text: "Error: connection is read-only" }], isError: true };
        }
        const result = await driver.query(sql);
        return { content: [{ type: "text", text: JSON.stringify(result, null, 2) }] };
      } catch (err) {
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
      } finally {
        await driver.close();
      }
    }
  );

  server.tool("list_tables", "List all tables in the database", async () => {
    const driver = await createDriver(conn);
    try {
      const tables = await driver.listTables();
      return { content: [{ type: "text", text: tables.join("\n") }] };
    } finally {
      await driver.close();
    }
  });

  server.tool(
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name") },
    async ({ table }) => {
      const driver = await createDriver(conn);
      try {
        const columns = await driver.describeTable(table);
        return { content: [{ type: "text", text: JSON.stringify(columns, null, 2) }] };
      } finally {
        await driver.close();
      }
    }
  );

  server.tool("list_schemas", "List all schemas or databases", async () => {
    const driver = await createDriver(conn);
    try {
      const schemas = await driver.listSchemas();
      return { content: [{ type: "text", text: schemas.join("\n") }] };
    } finally {
      await driver.close();
    }
  });

  return server;
}

export async function handleMcpRequest(conn: Connection, req: Request): Promise<Response> {
  const sessionId = req.headers.get("Mcp-Session-Id");

  // Route to existing session
  if (sessionId) {
    const session = sessions.get(sessionId);
    if (!session) return new Response("Session not found", { status: 404 });
    return session.transport.handleRequest(req);
  }

  // New session — SSE stream, stateful, survives long-running queries
  let session!: Session;

  const transport = new WebStandardStreamableHTTPServerTransport({
    sessionIdGenerator: () => crypto.randomUUID(),
    onsessioninitialized: (sid) => {
      sessions.set(sid, session);
    },
    onsessionclosed: (sid) => {
      sessions.delete(sid);
    },
  });

  const server = buildMcpServer(conn);
  session = { transport, server };
  await server.connect(transport);

  return transport.handleRequest(req);
}
