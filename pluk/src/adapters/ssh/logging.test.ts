import { test, expect, afterEach, afterAll } from "bun:test";
import { Server, utils as sshUtils } from "ssh2";
import type { AddressInfo } from "net";
import { mkdtempSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import type { Integration } from "../../store/integrations.js";
import type { ToolHost } from "../../mcp/namespace.js";

// The store resolves its DB path at import, so point it at a scratch dir first.
const scratch = mkdtempSync(join(tmpdir(), "pluk-ssh-log-"));
process.env.PLUK_DATA_DIR = scratch;

const { readLogPage } = await import("../../store/queryLog.js");
const { registerSshServer } = await import("./server.js");

// Exercises run_command's log lifecycle against an in-process ssh2 server whose
// exec stream returns output for the first call and silence for the second.

let server: Server | undefined;
const OWNER = "log-test-owner";
let execCount = 0;

afterEach(() => {
  execCount = 0;
  server?.close();
  server = undefined;
});

afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function startServer(): Promise<number> {
  const { private: hostKey } = sshUtils.generateKeyPairSync("ed25519");
  return new Promise((resolve) => {
    server = new Server({ hostKeys: [hostKey] }, (client) => {
      client.on("authentication", (ctx) => ctx.accept());
      client.on("ready", () => {});
      client.on("session", (accept) => {
        const session = accept();
        session.on("exec", (accept2) => {
          const stream = accept2();
          execCount += 1;
          setTimeout(() => {
            if (execCount === 1) stream.write("hello world\n");
            stream.exit(0);
            stream.end();
          }, 20);
        });
        session.on("error", () => {});
      });
      client.on("error", () => {});
    });
    server.listen(0, "127.0.0.1", () => resolve((server!.address() as AddressInfo).port));
  });
}

function makeConn(port: number): Integration {
  return {
    id: "ssh-log-test",
    name: "log-test",
    type: "ssh",
    config: { host: "127.0.0.1", port, user: "tester", auth_type: "password", password: "x" },
    read_only: 0,
    token: "t",
    created_at: "",
  } as unknown as Integration;
}

test("run_command logs a response even when the command produces no output", async () => {
  const port = await startServer();
  const conn = makeConn(port);
  const handlers = new Map<string, (args: Record<string, unknown>) => unknown>();
  const host = {
    tool(name: string, _desc: string, ...rest: unknown[]) {
      handlers.set(name, rest[rest.length - 1] as (args: Record<string, unknown>) => unknown);
    },
    prompt: (() => undefined) as unknown as ToolHost["prompt"],
    resource: (() => undefined) as unknown as ToolHost["resource"],
  } as ToolHost;

  registerSshServer(host, conn, OWNER);
  const run = handlers.get("run_command") as (args: Record<string, unknown>) => Promise<unknown>;

  await run({ command: "echo hi", timeout: 10 });
  await run({ command: "silent", timeout: 10 });

  const entries = readLogPage({ connectionId: conn.id }, "all").entries;
  expect(entries).toHaveLength(2);
  for (const entry of entries) {
    expect(entry.responseText?.trim().length).toBeGreaterThan(0);
  }
  expect(entries[0]?.responseText).toContain("no output");
});
