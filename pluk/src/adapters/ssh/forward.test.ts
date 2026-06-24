import { test, expect, afterEach } from "bun:test";
import { Server, utils as sshUtils } from "ssh2";
import type { AddressInfo } from "net";
import { createConnection } from "net";
import type { Integration } from "../../store/integrations.js";
import { openForward, listForwards, closeForward, closeSessionClients } from "./client.js";

// Exercises the ssh -L forwarding the adapter exposes, against an in-process
// ssh2 server that echoes any direct-tcpip (forwardOut) channel — no real host.

let server: Server | undefined;
const SESSION = "fwd-test-session";

afterEach(async () => {
  closeSessionClients(SESSION);
  await new Promise<void>((r) => (server ? server.close(() => r()) : r()));
  server = undefined;
});

function startServer(): Promise<number> {
  const { private: hostKey } = sshUtils.generateKeyPairSync("ed25519");
  return new Promise((resolve) => {
    server = new Server({ hostKeys: [hostKey] }, (client) => {
      client.on("authentication", (ctx) => ctx.accept());
      client.on("ready", () => {});
      // direct-tcpip: a forwardOut from the adapter. Echo it straight back so a
      // round-trip through the local listener proves the tunnel carries bytes.
      client.on("tcpip", (accept) => {
        const stream = accept();
        stream.on("data", (d: Buffer) => stream.write(d));
      });
      client.on("error", () => {});
    });
    server.listen(0, "127.0.0.1", () => resolve((server!.address() as AddressInfo).port));
  });
}

function makeConn(port: number): Integration {
  return {
    id: "ssh-fwd-test",
    name: "fwd-test",
    type: "ssh",
    config: { host: "127.0.0.1", port, user: "tester", auth_type: "password", password: "x" },
  } as unknown as Integration;
}

function roundTrip(localPort: number, payload: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const sock = createConnection({ host: "127.0.0.1", port: localPort }, () => sock.write(payload));
    let buf = "";
    sock.on("data", (d) => { buf += d.toString(); sock.end(); });
    sock.on("end", () => resolve(buf));
    sock.on("error", reject);
  });
}

test("open_forward tunnels a local port to the remote target and carries bytes", async () => {
  const conn = makeConn(await startServer());
  const fwd = await openForward(SESSION, conn, "localhost", 5432);

  expect(fwd.id).toBe("localhost:5432");
  expect(fwd.localPort).toBeGreaterThan(0);
  expect(await roundTrip(fwd.localPort, "ping")).toBe("ping");
  expect(listForwards(SESSION, conn).map((f) => f.id)).toEqual(["localhost:5432"]);
});

test("open_forward is idempotent per remote target — reuses the same local port", async () => {
  const conn = makeConn(await startServer());
  const a = await openForward(SESSION, conn, "localhost", 6379);
  const b = await openForward(SESSION, conn, "localhost", 6379);

  expect(b.localPort).toBe(a.localPort);
  expect(listForwards(SESSION, conn)).toHaveLength(1);
});

test("close_forward tears the listener down; unknown id returns false", async () => {
  const conn = makeConn(await startServer());
  const fwd = await openForward(SESSION, conn, "localhost", 5432);

  expect(closeForward(SESSION, conn, fwd.id)).toBe(true);
  expect(listForwards(SESSION, conn)).toHaveLength(0);
  await expect(roundTrip(fwd.localPort, "ping")).rejects.toBeDefined();
  expect(closeForward(SESSION, conn, "nope:1")).toBe(false);
});
