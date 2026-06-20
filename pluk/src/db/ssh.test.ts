import { test, expect, afterEach } from "bun:test";
import { Server, utils as sshUtils } from "ssh2";
import type { Connection as SSHServerConnection } from "ssh2";
import type { AddressInfo } from "net";
import { openSSHTunnel, type Tunnel } from "./ssh.js";

// These tests reproduce the two SSH failure modes behind the reported timeouts
// using an in-process ssh2 server — no real host or 1Password tap needed.

let server: Server | undefined;
let tunnel: Tunnel | undefined;

afterEach(async () => {
  tunnel?.close();
  tunnel = undefined;
  await new Promise<void>((r) => (server ? server.close(() => r()) : r()));
  server = undefined;
});

interface TestServer {
  port: number;
  /** Drop the live SSH connection, simulating a server-side idle disconnect. */
  drop: () => void;
}

function startServer(opts: { authDelayMs?: number } = {}): Promise<TestServer> {
  const { private: hostKey } = sshUtils.generateKeyPairSync("ed25519");
  let live: SSHServerConnection | undefined;

  return new Promise((resolve) => {
    server = new Server({ hostKeys: [hostKey] }, (client) => {
      live = client;
      client.on("authentication", (ctx) => {
        if (ctx.method !== "password") {
          ctx.reject(["password"]);
          return;
        }
        if (opts.authDelayMs) setTimeout(() => ctx.accept(), opts.authDelayMs);
        else ctx.accept();
      });
      client.on("ready", () => {});
      client.on("error", () => {});
    });

    server.listen(0, "127.0.0.1", () => {
      const { port } = server!.address() as AddressInfo;
      resolve({ port, drop: () => live?.end() });
    });
  });
}

function connect(port: number, onFatal?: () => void): Promise<Tunnel> {
  return openSSHTunnel(
    {
      host: "127.0.0.1",
      port,
      user: "tester",
      authType: "password",
      passphrase: "irrelevant",
      remoteHost: "127.0.0.1",
      remotePort: 5432,
    },
    onFatal
  );
}

// Bug 1: ssh2's readyTimeout defaults to 20s, but agent auth (1Password SSH
// agent) blocks the handshake on an interactive confirm. A confirm landing after
// 20s used to abort the handshake. The fix raises readyTimeout to 180s.
test(
  "handshake slower than ssh2's old 20s default still connects",
  async () => {
    const { port } = await startServer({ authDelayMs: 22_000 });
    const started = Date.now();
    tunnel = await connect(port);
    expect(tunnel.localPort).toBeGreaterThan(0);
    expect(Date.now() - started).toBeGreaterThan(20_000); // outlived the old ceiling
  },
  60_000
);

// Bug 2: after a long-lived session the SSH connection can drop (idle
// disconnect, NAT timeout). The local listener used to linger, so every later
// query hung against a dead tunnel. The fix fires onFatal so the pool rebuilds.
test(
  "a dropped SSH connection notifies onFatal so the driver can be rebuilt",
  async () => {
    const srv = await startServer();
    let fatal = false;
    const fatalFired = new Promise<void>((resolve) => {
      tunnel = undefined;
      connect(srv.port, () => { fatal = true; resolve(); }).then((t) => {
        tunnel = t;
        // Tunnel is up; now simulate the server dropping the idle connection.
        srv.drop();
      });
    });

    await Promise.race([
      fatalFired,
      new Promise<void>((_, reject) => setTimeout(() => reject(new Error("onFatal never fired")), 10_000)),
    ]);
    expect(fatal).toBe(true);
  },
  20_000
);
