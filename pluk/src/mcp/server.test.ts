import { test, expect } from "bun:test";
import { z } from "zod";
import { McpServer } from "@modelcontextprotocol/server";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { handleMcpRequest, resetOwners } from "./server.js";
import { toolHost } from "./namespace.js";

// The endpoint prefers protocol revision 2026-07-28 and also serves the 2025 era.
// These tests pin both halves: a modern client works end to end (through the
// positional ToolHost shim the adapters register with), and a 2025-era client
// negotiates its own revision and reaches the same tools.

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

test("a 2025-era client negotiates its own era and reaches the same tools", async () => {
  const client = new Client(
    { name: "legacy-client", version: "1.0.0" },
    { versionNegotiation: { mode: "legacy" } },
  );
  await client.connect(
    new StreamableHTTPClientTransport(new URL("http://test.local/mcp"), {
      fetch: (url, init) => serve(new Request(url, init)),
    }),
  );
  try {
    expect(client.getProtocolEra()).toBe("legacy");

    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name).sort()).toEqual(["echo", "ping"]);

    const result = await client.callTool({ name: "echo", arguments: { value: "hi" } });
    expect(result.content).toEqual([{ type: "text", text: "hi" }]);
  } finally {
    await client.close();
    await resetOwners(OWNER);
  }
});
