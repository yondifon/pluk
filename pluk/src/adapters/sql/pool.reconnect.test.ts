import { test, expect, mock, afterAll } from "bun:test";
import type { Integration } from "../../store/integrations.js";
import type { Driver } from "../../db/index.js";

// Regression: a tunnel-backed driver that dies after being healthy (laptop
// sleep, locked 1Password agent) must be rebuilt in the background instead of
// sitting broken until a manual retry. Agent/auth failures must retry on the
// gentle 60s interval (each SSH attempt can pop a 1Password prompt), and once
// the agent answers again the pool must reconnect on its own.

interface FakeTimer { fn: () => void; ms: number; id: number }
const timers: FakeTimer[] = [];
const cleared: number[] = [];
let nextTimerId = 1;

const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
globalThis.setTimeout = ((fn: () => void, ms?: number) => {
  const id = nextTimerId++;
  timers.push({ fn, ms: ms ?? 0, id });
  return id;
}) as unknown as typeof setTimeout;
globalThis.clearTimeout = ((id: number) => {
  cleared.push(id);
}) as unknown as typeof clearTimeout;

afterAll(() => {
  globalThis.setTimeout = realSetTimeout;
  globalThis.clearTimeout = realClearTimeout;
});

let connectBehavior: "ok" | "agent-error" = "ok";
let connectCalls = 0;
let capturedOnFatal: (() => void) | undefined;

function fakeDriver(): Driver {
  return {
    query: async () => ({ rows: [] }),
    queryReadOnly: async () => ({ rows: [] }),
    explain: async () => ({ rows: [] }),
    listTables: async () => [],
    describeTable: async () => [],
    sampleTable: async () => ({ rows: [] }),
    listRelationships: async () => [],
    searchSchema: async () => [],
    tableStats: async () => ({ table: "t", estimatedRows: null, sizeBytes: null, indexes: [] }),
    listSchemas: async () => [],
    getFullSchema: async () => "",
    testConnection: async () => {},
    close: async () => {},
  };
}

mock.module("../../db/index.js", () => ({
  createDriver: async (_integration: Integration, _sessionId?: string, onFatal?: () => void) => {
    connectCalls++;
    capturedOnFatal = onFatal;
    if (connectBehavior === "agent-error") throw new Error("communication with agent failed");
    return fakeDriver();
  },
}));

const { getDriver } = await import("./pool.js");

const integration: Integration = {
  id: "pg1",
  name: "test-pg",
  type: "postgres",
  config: { use_ssh: true, ssh_host: "bastion" },
  read_only: 0,
  token: "t",
  created_at: "2026-01-01",
};

function takeNewTimers(after: number): FakeTimer[] {
  return timers.filter((t) => t.id > after && !cleared.includes(t.id));
}

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

test("dead tunnel auto-reconnects, backing off to 60s while the agent is locked", async () => {
  await getDriver("s1", integration);
  expect(connectCalls).toBe(1);
  expect(capturedOnFatal).toBeDefined();

  // Tunnel dies (ssh process exited) → first reconnect scheduled at 2s.
  let mark = nextTimerId - 1;
  capturedOnFatal!();
  const first = takeNewTimers(mark).filter((t) => t.ms === 2_000);
  expect(first).toHaveLength(1);

  // Agent locked: rebuild fails with an agent error → next retry at 60s.
  connectBehavior = "agent-error";
  mark = nextTimerId - 1;
  first[0]!.fn();
  await settle();
  expect(connectCalls).toBe(2);
  const authRetry = takeNewTimers(mark).filter((t) => t.ms === 60_000);
  expect(authRetry).toHaveLength(1);

  // User unlocks 1Password → next attempt reconnects without manual retry.
  connectBehavior = "ok";
  authRetry[0]!.fn();
  await settle();
  expect(connectCalls).toBe(3);

  // Pool is live again: getDriver reuses the rebuilt driver.
  await getDriver("s1", integration);
  expect(connectCalls).toBe(3);
});
