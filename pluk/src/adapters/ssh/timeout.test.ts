import { test, expect, afterEach } from "bun:test";
import { Server, type Channel, type Session, utils as sshUtils } from "ssh2";
import type { AddressInfo } from "net";
import type { Integration } from "../../store/integrations.js";
import { runCommand, closeOwnerClients, CommandTimeoutError } from "./client.js";
import { evictSharedSSHClient } from "../../ssh/client.js";
import { humanizeSshError } from "./errors.js";

// Proves a command that exceeds its timeout is rejected with the timeout error
// (never the connection-failure wording) and that the remote side is killed:
// sshd is asked to SIGKILL the process and the session channel is closed.

let server: Server | undefined;
let lastPort: number | undefined;
const OWNER = "timeout-test-owner";
const events: { sessionClosed: boolean; signal: string | null } = { sessionClosed: false, signal: null };

afterEach(async () => {
  closeOwnerClients(OWNER);
  if (lastPort !== undefined) {
    evictSharedSSHClient(OWNER, { host: "127.0.0.1", port: lastPort, user: "tester", authType: "password", password: "x" });
  }
  await new Promise<void>((r) => (server ? server.close(() => r()) : r()));
  server = undefined;
  lastPort = undefined;
  events.sessionClosed = false;
  events.signal = null;
});

function startServer(onExec: (stream: Channel) => void, onSession?: (session: Session) => void): Promise<number> {
  const { private: hostKey } = sshUtils.generateKeyPairSync("ed25519");
  return new Promise((resolve) => {
    server = new Server({ hostKeys: [hostKey] }, (client) => {
      client.on("authentication", (ctx) => ctx.accept());
      client.on("session", (accept) => {
        const session = accept();
        onSession?.(session);
        session.on("exec", (acceptExec) => onExec(acceptExec()));
        session.on("signal", (acceptSignal, _reject, info) => {
          events.signal = info.name;
          if (acceptSignal) acceptSignal();
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
    id: "ssh-timeout-test",
    name: "timeout-test",
    type: "ssh",
    config: { host: "127.0.0.1", port, user: "tester", auth_type: "password", password: "x" },
  } as unknown as Integration;
}

async function until(cond: () => boolean, timeoutMs = 2000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (!cond() && Date.now() < deadline) await Bun.sleep(10);
  return cond();
}

test("command exceeding its timeout rejects with the timeout error, not the connection-failure wording", async () => {
  const port = await startServer(
    (stream) => {
      // Never finish: the remote command keeps running until the timeout kills it.
      stream.on("error", () => {});
    },
    (session) => session.on("close", () => { events.sessionClosed = true; }),
  );
  lastPort = port;
  const conn = makeConn(port);

  const err = await runCommand(OWNER, conn, "sleep 60", 600).catch((e) => e);

  expect(err).toBeInstanceOf(CommandTimeoutError);
  expect((err as Error).message).toBe("Command timed out after 1s");
  const text = humanizeSshError(err);
  expect(text).toMatch(/Command timed out after 1s/);
  expect(text).toMatch(/exceeded the timeout/);
  expect(text).toMatch(/higher `timeout`/);
  expect(text).not.toMatch(/host, port|firewall|tunnel/i);

  expect(await until(() => events.signal !== null)).toBe(true);
  expect(events.signal).toBe("KILL");
  expect(await until(() => events.sessionClosed)).toBe(true);
});

test("a genuine connection timeout keeps the connection-failure wording", () => {
  const text = humanizeSshError(new Error("Timed out while waiting for handshake"));
  expect(text).toMatch(/Check host, port, SSH tunnel, and firewall\/VPC rules\./);
});
