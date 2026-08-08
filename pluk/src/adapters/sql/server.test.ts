import { test, expect, mock } from "bun:test";
import type { Integration } from "../../store/integrations.js";
import type { Driver } from "../../db/index.js";
import type { ToolHost } from "../../mcp/namespace.js";
import type { ToolResult } from "../kit.js";

// Serialized result metadata sent to the LLM carries no host: the connection's
// configured host stays internal (connecting, logging) and is dropped from the
// envelope.

function fakeDriver(): Driver {
  return {
    query: async () => ({ rows: [{ id: 1 }], fields: ["id"] }),
    queryReadOnly: async () => ({ rows: [{ id: 1 }], fields: ["id"] }),
    explain: async () => ({ rows: [] }),
    listTables: async () => [],
    describeTable: async () => [],
    sampleTable: async () => ({ rows: [{ id: 1 }], fields: ["id"] }),
    listRelationships: async () => [],
    searchSchema: async () => [],
    tableStats: async () => ({ table: "t", estimatedRows: null, sizeBytes: null, indexes: [] }),
    listSchemas: async () => [],
    listDatabases: async () => [],
    getFullSchema: async () => "",
    testConnection: async () => {},
    close: async () => {},
  };
}

mock.module("./pool.js", () => ({
  getDriver: async () => fakeDriver(),
  evictDriver: () => {},
  withToolTimeout: async <T>(work: Promise<T>): Promise<T> => work,
  withCancellable: async <T>(work: Promise<T>): Promise<T> => work,
  registerQueryAbort: () => new AbortController(),
  clearQueryAbort: () => {},
}));

const { registerSqlServer } = await import("./server.js");

const conn: Integration = {
  id: "pg1",
  name: "test-pg",
  type: "postgres",
  config: { host: "10.0.1.50", database: "app" },
  read_only: 0,
  token: "t",
  created_at: "2026-01-01",
};

function captureHost(): { tools: Map<string, (args: unknown) => Promise<ToolResult>>; host: ToolHost } {
  const tools = new Map<string, (args: unknown) => Promise<ToolResult>>();
  const host: ToolHost = {
    tool: ((name: string, _desc: string, ...rest: unknown[]) => {
      const cb = rest[rest.length - 1] as (args: unknown) => Promise<ToolResult>;
      tools.set(name, cb);
      return undefined;
    }) as ToolHost["tool"],
    prompt: (() => undefined) as ToolHost["prompt"],
    resource: (() => undefined) as ToolHost["resource"],
  };
  return { tools, host };
}

async function serialize(toolName: string, args: unknown): Promise<Record<string, unknown>> {
  const { tools, host } = captureHost();
  registerSqlServer(host, conn, "owner1");
  const result = await (tools.get(toolName)! as (a: unknown) => Promise<ToolResult>)(args);
  const text = result.content[0]!.text;
  return JSON.parse(text) as Record<string, unknown>;
}

test("query result omits host but keeps env/connection/type/database", async () => {
  const parsed = await serialize("query", { sql: "SELECT 1" });
  expect(parsed).not.toHaveProperty("host");
  expect(parsed.host).toBeUndefined();
  expect(parsed.env).not.toBeUndefined();
  expect(parsed.connection).toBe("test-pg");
  expect(parsed.type).toBe("postgres");
  expect(parsed.database).toBe("app");
});

test("sample_table result omits host", async () => {
  const parsed = await serialize("sample_table", { table: "users" });
  expect(parsed).not.toHaveProperty("host");
  expect(parsed.connection).toBe("test-pg");
  expect(parsed.database).toBe("app");
});