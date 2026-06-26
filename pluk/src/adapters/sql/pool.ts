import type { Integration } from "../../store/integrations.js";
import { createDriver, type Driver } from "../../db/index.js";
import { recordHealth } from "../../mcp/health.js";
import { onSessionClose, sessionSignal } from "../../mcp/pool.js";
import { humanizeSqlError } from "./errors.js";

const IDLE_MS = 5 * 60 * 1000;
const TOOL_TIMEOUT_MS = 30_000;
const CONNECT_TIMEOUT_SSH_MS = 195_000;
const CONNECT_TIMEOUT_DIRECT_MS = 30_000;
const STALE_AFTER_MS = 30_000;
const HEALTHCHECK_TIMEOUT_MS = 5_000;

function withTimeout<T>(work: Promise<T>, ms: number, label: string): Promise<T> {
  return Promise.race([
    work,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`Timed out after ${Math.round(ms / 1000)}s (${label})`)), ms)
    ),
  ]);
}

export function withToolTimeout<T>(work: Promise<T>, label: string): Promise<T> {
  return withTimeout(work, TOOL_TIMEOUT_MS, label);
}

export function withCancellable<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(new Error("Query cancelled"));
  return Promise.race([
    work,
    new Promise<T>((_, reject) => {
      signal.addEventListener("abort", () => reject(new Error("Query cancelled")), { once: true });
    }),
  ]);
}

interface DriverEntry {
  driver: Promise<Driver>;
  idleTimer: ReturnType<typeof setTimeout>;
  lastUsed: number;
  validating?: Promise<Driver>;
}

const driverPool = new Map<string, DriverEntry>();
const queryAborts = new Map<number, AbortController>();

function driverKey(sessionId: string, integrationId: string): string {
  return `${sessionId}::${integrationId}`;
}

export function registerQueryAbort(logId: number, sessionId: string): AbortController {
  const ac = new AbortController();
  queryAborts.set(logId, ac);
  const signal = sessionSignal(sessionId);
  if (signal) {
    if (signal.aborted) ac.abort();
    else signal.addEventListener("abort", () => ac.abort(), { once: true });
  }
  return ac;
}

export function clearQueryAbort(logId: number): void {
  queryAborts.delete(logId);
}

export function cancelQuery(logId: number): boolean {
  const ac = queryAborts.get(logId);
  if (!ac) return false;
  ac.abort();
  return true;
}

export async function getDriver(sessionId: string, integration: Integration): Promise<Driver> {
  const key = driverKey(sessionId, integration.id);
  const existing = driverPool.get(key);
  if (existing) {
    resetIdleTimer(key, existing);
    const idleFor = Date.now() - existing.lastUsed;
    if (idleFor < STALE_AFTER_MS && !existing.validating) {
      existing.lastUsed = Date.now();
      return existing.driver;
    }

    existing.validating ??= validateOrRebuild(key, sessionId, integration, existing);
    return existing.validating;
  }

  return createDriverEntry(key, sessionId, integration).driver;
}

function createDriverEntry(key: string, sessionId: string, integration: Integration): DriverEntry {
  const useSsh = Boolean(integration.config.use_ssh);
  const connectTimeout = useSsh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  const created = createDriver(integration, sessionId, () => {
    if (driverPool.get(key) === entry) evictDriverByKey(key);
  });
  const driver = withTimeout(created, connectTimeout, "connect");
  const idleTimer = setTimeout(() => evictDriverByKey(key), IDLE_MS);
  const entry: DriverEntry = { driver, idleTimer, lastUsed: Date.now() };
  driverPool.set(key, entry);

  driver.catch(() => {
    if (driverPool.get(key) === entry) {
      clearTimeout(idleTimer);
      driverPool.delete(key);
    }
  });

  created.then((d) => {
    if (driverPool.get(key) !== entry) d.close().catch(() => {});
  }).catch(() => {});

  created.then(
    () => recordHealth(integration.id, "ok"),
    (e) => recordHealth(integration.id, "error", humanizeSqlError(e)),
  );

  return entry;
}

function resetIdleTimer(key: string, entry: DriverEntry): void {
  clearTimeout(entry.idleTimer);
  entry.idleTimer = setTimeout(() => evictDriverByKey(key), IDLE_MS);
}

async function validateOrRebuild(
  key: string,
  sessionId: string,
  integration: Integration,
  entry: DriverEntry
): Promise<Driver> {
  try {
    const d = await entry.driver;
    await withTimeout(d.testConnection(), HEALTHCHECK_TIMEOUT_MS, "healthcheck");
    if (driverPool.get(key) === entry) {
      entry.lastUsed = Date.now();
      entry.validating = undefined;
    }
    return d;
  } catch {
    const current = driverPool.get(key);
    if (current === entry) {
      evictDriverByKey(key);
      return createDriverEntry(key, sessionId, integration).driver;
    }
    if (current) return current.driver;
    throw new Error("Driver evicted during healthcheck");
  } finally {
    if (driverPool.get(key) === entry) entry.validating = undefined;
  }
}

function evictDriverByKey(key: string): void {
  const entry = driverPool.get(key);
  if (!entry) return;
  driverPool.delete(key);
  clearTimeout(entry.idleTimer);
  entry.driver.then((d) => d.close()).catch(() => {});
}

export function evictDriver(sessionId: string, integrationId?: string): void {
  if (integrationId) {
    evictDriverByKey(driverKey(sessionId, integrationId));
    return;
  }
  for (const key of [...driverPool.keys()]) {
    if (key.startsWith(`${sessionId}::`)) evictDriverByKey(key);
  }
}

onSessionClose((sessionId) => evictDriver(sessionId));
