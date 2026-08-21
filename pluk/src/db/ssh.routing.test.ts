import { test, expect, mock, afterEach } from "bun:test";
import { createServer, type Server } from "net";
import { homedir } from "os";
import { EventEmitter } from "events";
import { PassThrough } from "stream";
import type { Tunnel } from "./ssh.js";

// Regression: agent/key DB tunnels must forward through the system `ssh` binary.
// The in-process ssh2 forwardOut channel opens but silently fails to pass data
// under Bun, so the driver connected to a live-looking local port that never
// delivered a byte and died on the connect timeout. This test locks the routing
// by mocking child_process: an agent-auth tunnel MUST spawn `ssh`.
//
// It also locks the multiplexed shape: one persistent master carries the auth,
// and the forward is registered on it with `-O forward` rather than owned by a
// child process — killing a pid no longer removes a forward.

const spawnCalls: { cmd: string; args: string[] }[] = [];
const listeners: Server[] = [];

mock.module("child_process", () => ({
  spawn: (cmd: string, args: string[]) => {
    spawnCalls.push({ cmd, args });
    const child = new EventEmitter() as EventEmitter & {
      stderr: PassThrough;
      kill: () => void;
    };
    child.stderr = new PassThrough();
    child.kill = () => child.emit("close", 1);

    // `-O check` reports no master yet (exit 1) so the master gets started;
    // everything else succeeds. On `-O forward` the fake stands up the local
    // listener the real master would open, so the readiness probe resolves.
    const control = args.includes("-O") ? args[args.indexOf("-O") + 1] : undefined;
    if (control === "forward") {
      const localPort = Number(args[args.indexOf("-L") + 1]?.split(":")[1]);
      const srv = createServer();
      listeners.push(srv);
      srv.listen(localPort, "127.0.0.1");
    }
    queueMicrotask(() => child.emit("close", control === "check" ? 1 : 0));
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
  expect(spawnCalls.every((c) => c.cmd === "ssh")).toBe(true);

  const [check, master, forward] = spawnCalls;
  expect(check!.args.slice(0, 2)).toEqual(["-O", "check"]);

  // The master is the only step that authenticates, and it must outlive this
  // tunnel — that is what stops every connect re-signing with the agent.
  expect(master!.args).toContain("-N");
  expect(master!.args).toContain("-f");
  expect(master!.args.some((a) => a.startsWith("ControlPersist="))).toBe(true);
  // pluk keeps its own control socket: joining ~/.ssh/control-* would let it
  // tear down the user's interactive sessions.
  const path = master!.args.find((a) => a.startsWith("ControlPath="))!;
  expect(path).toContain("/.pluk/");
  expect(check!.args).toContain(path);

  expect(forward!.args.slice(0, 2)).toEqual(["-O", "forward"]);
  expect(forward!.args[forward!.args.indexOf("-L") + 1]).toMatch(/^127\.0\.0\.1:\d+:127\.0\.0\.1:5432$/);

  // Closing removes just this forward; the master stays for the next tunnel.
  tunnel.close();
  tunnel = undefined;
  await Bun.sleep(10);
  const cancel = spawnCalls.at(-1)!;
  expect(cancel.args.slice(0, 2)).toEqual(["-O", "cancel"]);
});

// A key tunnel carries its own credential, so the agent has no part in it: a
// locked or refusing 1Password must not be able to fail it, and no default
// ~/.ssh key may be offered beside the configured one.
test("key-auth DB tunnel pins the key file and shuts the agent out", async () => {
  tunnel = await openSSHTunnel({
    host: "legacy.example.internal",
    port: 22,
    user: "root",
    authType: "key",
    keyPath: "~/.ssh/id_legacy",
    remoteHost: "127.0.0.1",
    remotePort: 5432,
  });

  const master = spawnCalls[1]!;
  expect(master.args[master.args.indexOf("-i") + 1]).toBe(`${homedir()}/.ssh/id_legacy`);
  expect(master.args).toContain("IdentitiesOnly=yes");
  expect(master.args).toContain("IdentityAgent=none");
});
