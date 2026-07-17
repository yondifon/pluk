import type { Integration } from "../../store/integrations.js";
import { createDriver, type Driver } from "../../db/index.js";
import { recordHealth } from "../../mcp/health.js";
import { onSessionClose, sessionSignal } from "../../mcp/pool.js";
import { SSH_CONNECT_RESPAWN_MS, SSH_CONNECT_WAIT_MS, sshPendingError } from "../../ssh/pending.js";
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
  startedAt: number;
  settled: boolean;
  useSsh: boolean;
  validating?: Promise<Driver>;
}

const driverPool = new Map<string, DriverEntry>();
const queryAborts = new Map<number, AbortController>();
const reconnectTimers = new Map<string, ReturnType<typeof setTimeout>>();

// Pool key includes the target database so one connection's databases never
// share a driver: a call for `db=a` must not be served the pool built for
// `db=b` (or the connection's default). Empty segment = the connection's
// configured/default database.
function driverKey(sessionId: string, integrationId: string, database?: string): string {
  return `${sessionId}::${integrationId}::${database ?? ""}`;
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

export async function getDriver(sessionId: string, integration: Integration, database?: string): Promise<Driver> {
  const key = driverKey(sessionId, integration.id, database);
  const existing = driverPool.get(key);
  if (existing) {
    resetIdleTimer(key, existing);

    // An SSH connect still in flight is usually blocked on an interactive
    // approval (1Password confirm, proxy browser login). Don't weld this call
    // — and every retry — to it for the whole connect budget: wait briefly,
    // then surface a "waiting for approval" error while the connect keeps
    // going. A connect pending past the respawn window is doomed (its prompt
    // expired unseen), so kill it and spawn fresh to trigger a fresh prompt.
    if (existing.useSsh && !existing.settled) {
      if (Date.now() - existing.startedAt <= SSH_CONNECT_RESPAWN_MS) {
        existing.lastUsed = Date.now();
        return awaitConnect(existing);
      }
      evictDriverByKey(key);
      return awaitConnect(createDriverEntry(key, sessionId, integration, database));
    }

    const idleFor = Date.now() - existing.lastUsed;
    if (idleFor < STALE_AFTER_MS && !existing.validating) {
      existing.lastUsed = Date.now();
      return existing.driver;
    }

    existing.validating ??= validateOrRebuild(key, sessionId, integration, existing, database);
    return existing.validating;
  }

  return awaitConnect(createDriverEntry(key, sessionId, integration, database));
}

// Bound a tool call's wait on an in-flight SSH connect. The connect itself
// keeps running; once approved it lands in the pool for the next call.
function awaitConnect(entry: DriverEntry): Promise<Driver> {
  if (!entry.useSsh || entry.settled) return entry.driver;
  return Promise.race([
    entry.driver,
    new Promise<Driver>((_, reject) => setTimeout(() => reject(sshPendingError()), SSH_CONNECT_WAIT_MS)),
  ]);
}

function createDriverEntry(key: string, sessionId: string, integration: Integration, database?: string): DriverEntry {
  const useSsh = integration.config.use_ssh === true || integration.config.use_ssh === "true";
  const connectTimeout = useSsh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  const created = createDriver(integration, sessionId, () => {
    if (driverPool.get(key) === entry) {
      evictDriverByKey(key);
      scheduleReconnect(key, sessionId, integration, database);
    }
  }, database);
  const driver = withTimeout(created, connectTimeout, "connect");
  const idleTimer = setTimeout(() => evictDriverByKey(key), IDLE_MS);
  const entry: DriverEntry = { driver, idleTimer, lastUsed: Date.now(), startedAt: Date.now(), settled: false, useSsh };
  driver.then(() => { entry.settled = true; }, () => { entry.settled = true; });
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
  database?: string,
  attempt = 0,
  delayMs?: number
): void {
  if (attempt >= MAX_RECONNECT_ATTEMPTS || reconnectTimers.has(key)) return;
  const delay = delayMs ?? RECONNECT_DELAYS_MS[Math.min(attempt, RECONNECT_DELAYS_MS.length - 1)];
  const timer = setTimeout(() => {
    reconnectTimers.delete(key);
    if (driverPool.has(key)) return; // a query already rebuilt it
    createDriverEntry(key, sessionId, integration, database).driver.then(
      () => console.log(`[pluk] auto-reconnected ${integration.name} after tunnel loss`),
      (e) => {
        const authFailed = classifySqlError(e).category === "auth_failed";
        scheduleReconnect(key, sessionId, integration, database, attempt + 1, authFailed ? RECONNECT_AUTH_DELAY_MS : undefined);
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
  entry: DriverEntry,
  database?: string
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
      const fresh = createDriverEntry(key, sessionId, integration, database);
      // Immediate rebuild counts as attempt 0; keep retrying in the background
      // if it also fails (e.g. agent still locked).
      fresh.driver.catch(() => scheduleReconnect(key, sessionId, integration, database, 1));
      return awaitConnect(fresh);
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

// Force-drop every cached driver and pending reconnect for an integration
// across ALL live sessions. The manual Test button calls this so a stuck or
// pending-approval connection is torn down and the next call connects from
// scratch — re-triggering the 1Password/agent prompt. This is the app's
// equivalent of re-running a git command to force a fresh SSH auth.
// Match a `session::integration::database` key by its integration segment. The
// database segment is validated (`[A-Za-z0-9_$-]`), so a plain split is safe.
function keyIntegrationId(key: string): string | undefined {
  return key.split("::")[1];
}

export function evictDriverEverywhere(integrationId: string): void {
  for (const key of [...driverPool.keys()]) {
    if (keyIntegrationId(key) === integrationId) evictDriverByKey(key);
  }
  for (const key of [...reconnectTimers.keys()]) {
    if (keyIntegrationId(key) === integrationId) cancelReconnect(key);
  }
}

export function evictDriver(sessionId: string, integrationId?: string): void {
  if (integrationId) {
    // Drop every per-database driver for this connection in this session, not
    // just the default-database one.
    const prefix = `${sessionId}::${integrationId}::`;
    for (const key of [...driverPool.keys()]) {
      if (key.startsWith(prefix)) { cancelReconnect(key); evictDriverByKey(key); }
    }
    for (const key of [...reconnectTimers.keys()]) {
      if (key.startsWith(prefix)) cancelReconnect(key);
    }
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
