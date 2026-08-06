import { test, expect, afterAll } from "bun:test";
import { createServer, type Server } from "net";
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { probeAgentSocket, pickLiveAgent, agentSocketCandidates, type LiveAgent } from "./agent.js";

const dir = mkdtempSync(join(tmpdir(), "pluk-agent-"));
const servers: Server[] = [];

function fakeAgent(name: string, reply?: Buffer): Promise<string> {
  const path = join(dir, name);
  const server = createServer((sock) => {
    sock.on("data", () => { if (reply) sock.write(reply); });
  });
  servers.push(server);
  return new Promise((resolve, reject) => {
    server.on("error", reject);
    server.listen(path, () => resolve(path));
  });
}

afterAll(() => {
  for (const s of servers) s.close();
});

// SSH_AGENT_IDENTITIES_ANSWER (12) with a key count.
function identitiesAnswer(keys: number): Buffer {
  const buf = Buffer.from([0, 0, 0, 5, 12, 0, 0, 0, 0]);
  buf.writeUInt32BE(keys, 5);
  return buf;
}

test("agent that lists keys -> keys", async () => {
  const path = await fakeAgent("keys.sock", identitiesAnswer(2));
  expect(await probeAgentSocket(path)).toEqual({ state: "keys", keys: 2 });
});

test("agent with no keys -> empty", async () => {
  const path = await fakeAgent("empty.sock", identitiesAnswer(0));
  expect(await probeAgentSocket(path)).toEqual({ state: "empty" });
});

test("agent that answers failure -> mute", async () => {
  const path = await fakeAgent("refuse.sock", Buffer.from([0, 0, 0, 1, 5]));
  expect(await probeAgentSocket(path)).toEqual({ state: "mute" });
});

test("agent that never answers -> mute", async () => {
  const path = await fakeAgent("mute.sock");
  expect(await probeAgentSocket(path, 200)).toEqual({ state: "mute" });
});

test("missing socket -> dead", async () => {
  const probe = await probeAgentSocket(join(dir, "gone.sock"), 200);
  expect(probe.state).toBe("dead");
});

test("an agent with no keys is never picked", () => {
  const empty: LiveAgent = { socket: "a", probe: { state: "empty" } };
  expect(pickLiveAgent([empty])).toBeUndefined();
});

test("a mute agent is picked over nothing", () => {
  const empty: LiveAgent = { socket: "a", probe: { state: "empty" } };
  const mute: LiveAgent = { socket: "b", probe: { state: "mute" } };
  expect(pickLiveAgent([empty, mute])).toEqual(mute);
});

test("SSH_AUTH_SOCK is probed after the well-known 1Password sockets", () => {
  const original = process.env.SSH_AUTH_SOCK;
  process.env.SSH_AUTH_SOCK = "/tmp/pluk-test-env-agent.sock";
  const candidates = agentSocketCandidates("pluk-test-host-with-no-ssh-config-entry");
  if (original === undefined) delete process.env.SSH_AUTH_SOCK;
  else process.env.SSH_AUTH_SOCK = original;

  expect(candidates.at(-1)).toBe("/tmp/pluk-test-env-agent.sock");
});
