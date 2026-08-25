import { test, expect } from "bun:test";
import type { Integration } from "../../store/integrations.js";
import type { ToolHost } from "../../mcp/namespace.js";
import { createSavedCommand, deleteSavedCommand } from "../../store/savedCommands.js";

const { registerSshServer } = await import("./server.js");

const conn = {
  id: "ssh-only-test",
  name: "ssh-only-test",
  type: "ssh",
  config: {},
  query_policy: JSON.stringify({ tools: { list_saved_commands: { enabled: true } } }),
} as unknown as Integration;

function capture(): Map<string, (args: unknown) => Promise<{ content: { type: "text"; text: string }[]; isError?: boolean }>> {
  const tools = new Map<string, (args: unknown) => Promise<{ content: { type: "text"; text: string }[]; isError?: boolean }>>();
  const host: ToolHost = {
    tool: ((name: string, _description: string, ...rest: unknown[]) => {
      tools.set(name, rest[rest.length - 1] as (args: unknown) => Promise<{ content: { type: "text"; text: string }[]; isError?: boolean }>);
    }) as ToolHost["tool"],
    prompt: (() => undefined) as ToolHost["prompt"],
    resource: (() => undefined) as ToolHost["resource"],
  };
  registerSshServer(host, conn, "owner1");
  return tools;
}

async function withSavedCommand(only: string[] | undefined) {
  const saved = createSavedCommand({ connection_id: conn.id, name: "logs", command: "journalctl", working_dir: "/srv/app" });
  try {
    const result = await capture().get("list_saved_commands")!({ only });
    return JSON.parse(result.content[0]!.text) as Record<string, unknown>[];
  } finally {
    deleteSavedCommand(conn.id, saved.name);
  }
}

test("saved commands default to name and command", async () => {
  expect(await withSavedCommand(undefined)).toEqual([{ name: "logs", command: "journalctl" }]);
});

test("saved command preset exposes location", async () => {
  expect(await withSavedCommand(["location"])).toEqual([{ working_dir: "/srv/app" }]);
});

test("saved commands support the full payload escape hatch", async () => {
  expect(await withSavedCommand(["*"])).toEqual([{ name: "logs", command: "journalctl", working_dir: "/srv/app" }]);
});

test("saved commands reject unknown only fields", async () => {
  const saved = createSavedCommand({ connection_id: conn.id, name: "logs", command: "journalctl", working_dir: null });
  try {
    await expect(capture().get("list_saved_commands")!({ only: ["bogus"] })).rejects.toThrow(
      'Unknown "only" field "bogus"',
    );
  } finally {
    deleteSavedCommand(conn.id, saved.name);
  }
});
