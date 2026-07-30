import { test, expect } from "bun:test";
import { z } from "zod";
import { McpServer } from "@modelcontextprotocol/server";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { handleMcpRequest, resetOwners } from "./server.js";
import { toolHost } from "./namespace.js";

// The endpoint serves protocol revision 2026-07-28 only. These tests pin the two
// halves of that promise: a modern client works end to end (through the positional
// ToolHost shim the adapters register with), and a 2025-era client is refused
// rather than quietly served a different protocol.

const OWNER = "owner-under-test";

function makeServer(): McpServer {
  const server = new McpServer({ name: "Test Integration", version: "1.0.0" });
  const host = toolHost(server);
  host.tool("echo", "Echo a value back", { value: z.string() }, ({ value }) => ({
    content: [{ type: "text", text: value }],
  }));
  host.tool("ping", "Take no arguments", { readOnlyHint: true }, () => ({
    content: [{ type: "text", text: "pong" }],
  }));
  return server;
}

const serve = (req: Request) => handleMcpRequest(req, OWNER, makeServer);

async function connect(): Promise<Client> {
  const client = new Client(
    { name: "test-client", version: "1.0.0" },
    { versionNegotiation: { mode: { pin: "2026-07-28" } } },
  );
  await client.connect(
    new StreamableHTTPClientTransport(new URL("http://test.local/mcp"), {
      fetch: (url, init) => serve(new Request(url, init)),
    }),
  );
  return client;
}

test("serves the 2026-07-28 revision: tools registered through ToolHost are listed and callable", async () => {
  const client = await connect();
  try {
    expect(client.getProtocolEra()).toBe("modern");

    const { tools } = await client.listTools();
    const names = tools.map((t) => t.name).sort();
    expect(names).toEqual(["echo", "ping"]);

    // The shim must forward the description and the zod shape, not just the name.
    const echo = tools.find((t) => t.name === "echo")!;
    expect(echo.description).toBe("Echo a value back");
    expect(echo.inputSchema.properties).toHaveProperty("value");

    // A tool declared with annotations and no schema keeps its hints.
    expect(tools.find((t) => t.name === "ping")!.annotations?.readOnlyHint).toBe(true);

    const result = await client.callTool({ name: "echo", arguments: { value: "hi" } });
    expect(result.content).toEqual([{ type: "text", text: "hi" }]);
  } finally {
    await client.close();
    await resetOwners(OWNER);
  }
});

test("a 2025-era request is rejected, not served a different protocol", async () => {
  const response = await serve(
    new Request("http://test.local/mcp", {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json, text/event-stream" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2025-06-18",
          capabilities: {},
          clientInfo: { name: "legacy-client", version: "1.0.0" },
        },
      }),
    }),
  );

  const body = await response.json() as { result?: unknown; error?: { message?: string } };
  expect(body.result).toBeUndefined();
  expect(body.error).toBeDefined();

  await resetOwners(OWNER);
});
