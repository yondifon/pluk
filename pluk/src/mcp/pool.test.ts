import { test, expect, mock, setSystemTime, afterEach } from "bun:test";

// Drives the pool's stale-reuse health check (the "+20s on return" fix) with a
// fake createDriver and a controllable clock — no real DB or tunnel needed.

let healthOk = true;
let holdHealthcheck = false;
let releaseHealthcheck: (() => void) | undefined;
let healthcheckCount = 0;
let createCount = 0;

function makeFakeDriver() {
  return {
    id: createCount,
    testConnection: async () => {
      healthcheckCount++;
      if (holdHealthcheck) await new Promise<void>((resolve) => { releaseHealthcheck = resolve; });
      if (!healthOk) throw new Error("server closed the connection");
    },
    close: async () => {},
  };
}

mock.module("../db/index.js", () => ({
  createDriver: async () => {
    createCount++;
    return makeFakeDriver();
  },
}));

const { getDriver, evictDriver } = await import("../adapters/sql/pool.js");
const { closeSession } = await import("./pool.js");

const integration = { id: "i1", name: "DB", type: "postgres", config: {} } as never;

afterEach(() => {
  closeSession("s1");
  setSystemTime(); // restore real clock
  healthOk = true;
  holdHealthcheck = false;
  releaseHealthcheck = undefined;
  healthcheckCount = 0;
  createCount = 0;
});

test("fresh and recently-used drivers are reused without a probe", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);

  const d1 = (await getDriver("s1", integration)) as unknown as { id: number };
  expect(createCount).toBe(1);

  // Used again well within STALE_AFTER_MS — same driver, no rebuild.
  setSystemTime(new Date(t0.getTime() + 10_000));
  const d2 = (await getDriver("s1", integration)) as unknown as { id: number };
  expect(d2.id).toBe(d1.id);
  expect(createCount).toBe(1);
});

test("a live but long-idle driver passes the probe and is reused", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);
  const d1 = (await getDriver("s1", integration)) as unknown as { id: number };

  setSystemTime(new Date(t0.getTime() + 31_000)); // > STALE_AFTER_MS
  healthOk = true;
  const d2 = (await getDriver("s1", integration)) as unknown as { id: number };
  expect(d2.id).toBe(d1.id); // probe ok → reused
  expect(createCount).toBe(1);
});

test("a dead long-idle driver fails the probe and is rebuilt", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);
  const d1 = (await getDriver("s1", integration)) as unknown as { id: number };

  setSystemTime(new Date(t0.getTime() + 31_000)); // > STALE_AFTER_MS
  healthOk = false; // tunnel died while idle
  const d2 = (await getDriver("s1", integration)) as unknown as { id: number };
  expect(d2.id).not.toBe(d1.id); // probe failed → fresh driver
  expect(createCount).toBe(2);
});

test("each target database gets its own isolated driver; same database reuses", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);

  // Two different databases on one connection must never share a pool: a call
  // for `analytics` must not be served the driver built for `billing`.
  const a = (await getDriver("s1", integration, "billing")) as unknown as { id: number };
  const b = (await getDriver("s1", integration, "analytics")) as unknown as { id: number };
  expect(b.id).not.toBe(a.id);
  expect(createCount).toBe(2);

  // Same (session, connection, database) reuses its driver — no rebuild.
  const a2 = (await getDriver("s1", integration, "billing")) as unknown as { id: number };
  expect(a2.id).toBe(a.id);
  expect(createCount).toBe(2);

  // The connection's default database ("" segment) is its own pool too.
  await getDriver("s1", integration);
  expect(createCount).toBe(3);
});

test("evictDriver drops every per-database driver for a connection", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);
  await getDriver("s1", integration, "billing");
  await getDriver("s1", integration, "analytics");
  expect(createCount).toBe(2);

  evictDriver("s1", "i1");

  // Both databases were dropped → each rebuilds on next use.
  await getDriver("s1", integration, "billing");
  await getDriver("s1", integration, "analytics");
  expect(createCount).toBe(4);
});

test("concurrent stale callers share the probe and rebuilt driver", async () => {
  const t0 = new Date("2026-06-20T00:00:00Z");
  setSystemTime(t0);
  await getDriver("s1", integration);

  setSystemTime(new Date(t0.getTime() + 31_000)); // > STALE_AFTER_MS
  holdHealthcheck = true;
  healthOk = false;

  const p1 = getDriver("s1", integration) as unknown as Promise<{ id: number }>;
  await Promise.resolve();
  expect(healthcheckCount).toBe(1);

  const p2 = getDriver("s1", integration) as unknown as Promise<{ id: number }>;
  releaseHealthcheck?.();

  const [d1, d2] = await Promise.all([p1, p2]);
  expect(d1.id).toBe(2);
  expect(d2.id).toBe(2);
  expect(createCount).toBe(2);
  expect(healthcheckCount).toBe(1);
});
