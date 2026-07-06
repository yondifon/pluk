import type { Integration } from "../../store/integrations.js";
import { createDriver, type Driver } from "../../db/index.js";
import { recordHealth } from "../../mcp/health.js";
import { onSessionClose, sessionSignal } from "../../mcp/pool.js";
import { classifySqlError, humanizeSqlError } from "./errors.js";

const IDLE_MS = 5 * 60 * 1000;
const TOOL_TIMEOUT_MS = 30_000;
const CONNECT_TIMEOUT_SSH_MS = 195_000;
const CONNECT_TIMEOUT_DIRECT_MS = 30_000;
const STALE_AFTER_MS = 30_000;
const HEALTHCHECK_TIMEOUT_MS = 5_000;
const RECONNECT_DELAYS_MS = [2_000, 5_000, 15_000, 30_000, 60_000];
const RECONNECT_AUTH_DELAY_MS = 60_000;
const MAX_RECONNECT_ATTEMPTS = 12;

function withTimeout<T>(work: Promise<T>, ms: number, label: string): Promise<T> {
  return Promise.race([
    work,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`Timed out after ${Math.round(ms / 1000)}s (${label})`)), ms)
    ),
  ]);
}

export function withToolTimeout<T>(work: Promise<T>, label: string, ms: number = TOOL_TIMEOUT_MS): Promise<T> {
  return withTimeout(work, ms, label);
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
const reconnectTimers = new Map<string, ReturnType<typeof setTimeout>>();

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
  const useSsh = integration.config.use_ssh === true || integration.config.use_ssh === "true";
  const connectTimeout = useSsh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  const created = createDriver(integration, sessionId, () => {
    if (driverPool.get(key) === entry) {
      evictDriverByKey(key);
      scheduleReconnect(key, sessionId, integration);
    }
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

// A driver that dies after being healthy (laptop sleep, network drop, locked
// SSH agent) had a working config, so the failure is transient — rebuild it in
// the background instead of waiting for a manual retry. Auth/agent failures
// retry on a gentle fixed interval, since each SSH attempt can pop a 1Password
// prompt; once the user unlocks the agent, the next attempt reconnects and
// health flips back to ok. Never used for first-time connects (misconfig would
// retry forever).
function scheduleReconnect(
  key: string,
  sessionId: string,
  integration: Integration,
  attempt = 0,
  delayMs?: number
): void {
  if (attempt >= MAX_RECONNECT_ATTEMPTS || reconnectTimers.has(key)) return;
  const delay = delayMs ?? RECONNECT_DELAYS_MS[Math.min(attempt, RECONNECT_DELAYS_MS.length - 1)];
  const timer = setTimeout(() => {
    reconnectTimers.delete(key);
    if (driverPool.has(key)) return; // a query already rebuilt it
    createDriverEntry(key, sessionId, integration).driver.then(
      () => console.log(`[pluk] auto-reconnected ${integration.name} after tunnel loss`),
      (e) => {
        const authFailed = classifySqlError(e).category === "auth_failed";
        scheduleReconnect(key, sessionId, integration, attempt + 1, authFailed ? RECONNECT_AUTH_DELAY_MS : undefined);
      }
    );
  }, delay);
  reconnectTimers.set(key, timer);
}

function cancelReconnect(key: string): void {
  const timer = reconnectTimers.get(key);
  if (timer === undefined) return;
  clearTimeout(timer);
  reconnectTimers.delete(key);
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
      const fresh = createDriverEntry(key, sessionId, integration);
      // Immediate rebuild counts as attempt 0; keep retrying in the background
      // if it also fails (e.g. agent still locked).
      fresh.driver.catch(() => scheduleReconnect(key, sessionId, integration, 1));
      return fresh.driver;
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
    const key = driverKey(sessionId, integrationId);
    cancelReconnect(key);
    evictDriverByKey(key);
    return;
  }
  for (const key of [...driverPool.keys()]) {
    if (key.startsWith(`${sessionId}::`)) evictDriverByKey(key);
  }
  for (const key of [...reconnectTimers.keys()]) {
    if (key.startsWith(`${sessionId}::`)) cancelReconnect(key);
  }
}

onSessionClose((sessionId) => evictDriver(sessionId));
