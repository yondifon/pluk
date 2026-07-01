import { test, expect, mock, afterEach } from "bun:test";
import { createServer, type Server } from "net";
import { EventEmitter } from "events";
import { PassThrough } from "stream";
import type { Tunnel } from "./ssh.js";

// Regression: agent/key DB tunnels must forward through the system `ssh` binary.
// The in-process ssh2 forwardOut channel opens but silently fails to pass data
// under Bun, so the driver connected to a live-looking local port that never
// delivered a byte and died on the connect timeout. This test locks the routing
// by mocking child_process: an agent-auth tunnel MUST spawn `ssh`.

const spawnCalls: { cmd: string; args: string[] }[] = [];
const listeners: Server[] = [];

mock.module("child_process", () => ({
  spawn: (cmd: string, args: string[]) => {
    spawnCalls.push({ cmd, args });
    // openOpenSSHTunnel waits until the -L local port accepts connections. The
    // real ssh opens that listener; here the fake child stands one up so the
    // readiness probe resolves.
    const localPort = Number(args[args.indexOf("-L") + 1]?.split(":")[1]);
    const child = new EventEmitter() as EventEmitter & {
      stderr: PassThrough;
      kill: () => void;
    };
    child.stderr = new PassThrough();
    const srv = createServer();
    listeners.push(srv);
    srv.listen(localPort, "127.0.0.1");
    child.kill = () => { srv.close(); child.emit("close"); };
    return child;
  },
}));

const { openSSHTunnel } = await import("./ssh.js");

let tunnel: Tunnel | undefined;
afterEach(() => {
  tunnel?.close();
  tunnel = undefined;
  listeners.splice(0).forEach((s) => s.close());
  spawnCalls.length = 0;
});

test("agent-auth DB tunnel forwards via the OpenSSH binary, not ssh2", async () => {
  tunnel = await openSSHTunnel({
    host: "db.example.internal",
    port: 22,
    user: "root",
    authType: "agent",
    remoteHost: "127.0.0.1",
    remotePort: 5432,
  });

  expect(tunnel.localPort).toBeGreaterThan(0);
  expect(spawnCalls).toHaveLength(1);
  const call = spawnCalls[0]!;
  expect(call.cmd).toBe("ssh");

  const { args } = call;
  expect(args).toContain("-N");
  const forward = args[args.indexOf("-L") + 1];
  expect(forward).toMatch(/^127\.0\.0\.1:\d+:127\.0\.0\.1:5432$/);
});
