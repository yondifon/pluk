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

let connectBehavior: "ok" | "agent-error" | "hang" = "ok";
let connectCalls = 0;
let capturedOnFatal: (() => void) | undefined;
let resolveHang: ((d: Driver) => void) | undefined;

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
    if (connectBehavior === "hang") return new Promise<Driver>((res) => { resolveHang = res; });
    return fakeDriver();
  },
}));

const { getDriver, evictDriverEverywhere } = await import("./pool.js");

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

// Regression: retries during an in-flight SSH connect (blocked on a 1Password
// approval) must not weld onto it for the whole connect budget — they get a
// bounded wait and a "waiting for approval" error, while the connect keeps
// running so the approval still lands in the pool.
test("in-flight connect: bounded wait, no duplicate spawn, approval lands for the next call", async () => {
  connectBehavior = "hang";
  const mark = nextTimerId - 1;

  const first = getDriver("s2", integration);
  first.catch(() => {});
  await settle();
  const calls = connectCalls;

  // Retry while the connect is in flight: reuses it, no second ssh spawn.
  const second = getDriver("s2", integration);
  second.catch(() => {});
  await settle();
  expect(connectCalls).toBe(calls);

  // Bounded wait fires → both calls surface the pending-approval error.
  const waits = takeNewTimers(mark).filter((t) => t.ms === 25_000);
  expect(waits.length).toBe(2);
  for (const t of waits) t.fn();
  expect(((await first.catch((e) => e)) as { code?: string }).code).toBe("SSH_CONNECT_PENDING");
  expect(((await second.catch((e) => e)) as { code?: string }).code).toBe("SSH_CONNECT_PENDING");

  // User approves → the still-running connect completes and stays pooled.
  resolveHang!(fakeDriver());
  await settle();
  await getDriver("s2", integration);
  expect(connectCalls).toBe(calls);
});

// Force-refresh (the Test button): tear down a live pooled driver across
// sessions so the next call reconnects from scratch — re-triggering the SSH
// prompt instead of reusing a poisoned/pending entry.
test("evictDriverEverywhere forces the next call to reconnect fresh", async () => {
  connectBehavior = "ok";
  await getDriver("s-refresh", integration);
  const calls = connectCalls;

  // Same session reuses the pooled driver — no new connect.
  await getDriver("s-refresh", integration);
  expect(connectCalls).toBe(calls);

  // Force-refresh drops it; the next call must build a brand-new connection.
  evictDriverEverywhere(integration.id);
  await getDriver("s-refresh", integration);
  expect(connectCalls).toBe(calls + 1);
});

// Regression: a connect stuck past the respawn window (its prompt expired
// unseen) must be killed and respawned so a fresh prompt can appear.
test("connect stuck past the respawn window is replaced by a fresh attempt", async () => {
  connectBehavior = "hang";
  const first = getDriver("s3", integration);
  first.catch(() => {});
  await settle();
  const calls = connectCalls;

  const realNow = Date.now;
  Date.now = () => realNow() + 91_000;
  try {
    const retry = getDriver("s3", integration);
    retry.catch(() => {});
    await settle();
    expect(connectCalls).toBe(calls + 1);
  } finally {
    Date.now = realNow;
  }
});
