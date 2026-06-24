import type { Integration } from "../store/integrations.js";
import { createDriver, type Driver } from "../db/index.js";
import { recordHealth } from "./health.js";
import { humanizeConnError } from "./errors.js";

// ── Driver pool ──────────────────────────────────────────────────────────────
// One driver per MCP session, closed after IDLE_MS of inactivity. This layer is
// adapter-neutral plumbing: session lifecycle, timeouts, and cancellation. The
// driver pool itself is used only by the SQL adapter family (API adapters open
// no long-lived connection), but the session/abort bookkeeping is shared.

const IDLE_MS = 5 * 60 * 1000; // 5 minutes

// Hard wall-clock timeout for any single tool call, including SSH tunnel setup.
// PG's own connectionTimeoutMillis only covers the TCP leg to localhost (the tunnel
// listener), not the PG handshake through the tunnel. This catches the gap.
const TOOL_TIMEOUT_MS = 30_000; // 30 seconds

// Connection/tunnel setup gets its own, larger budget than a single query.
// An SSH proxy (e.g. Cloudflare Access) can require interactive browser auth on
// first use, which easily exceeds the 30s query timeout. Direct connections stay
// tight so a dead host still surfaces fast.
const CONNECT_TIMEOUT_SSH_MS = 195_000; // SSH has 180s; this watchdog should not hide its error
const CONNECT_TIMEOUT_DIRECT_MS = 30_000;

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
  // In-flight promise, not the resolved driver: concurrent tool calls during
  // setup must share one connection/tunnel instead of each spawning their own.
  driver: Promise<Driver>;
  idleTimer: ReturnType<typeof setTimeout>;
  lastUsed: number;
  validating?: Promise<Driver>;
}

// A driver idle longer than this is re-validated before reuse: while the laptop
// slept (or NAT dropped the mapping) the SSH tunnel can die silently without the
// keepalive having fired yet, so the first query would otherwise hang on a dead
// connection until its query timeout. The probe is bounded so the worst case on
// return is a few seconds + a rebuild, not a full 20s query timeout.
const STALE_AFTER_MS = 30_000;
const HEALTHCHECK_TIMEOUT_MS = 5_000;

const driverPool = new Map<string, DriverEntry>();
const sessionAborts = new Map<string, AbortController>();

// Per-query abort controllers, keyed by log entry id, so the UI can cancel a
// single in-flight query (POST /api/log/:id/cancel) without tearing down the session.
const queryAborts = new Map<number, AbortController>();

// ── Session lifecycle (called by the MCP transport) ──────────────────────────

export function openSession(sessionId: string): void {
  sessionAborts.set(sessionId, new AbortController());
}

// Adapters that hold their own session-scoped connections (e.g. SSH) register a
// cleanup hook so a closing/reloading session tears those down too — the DB
// driver pool above only knows about DB drivers.
const sessionCloseHooks = new Set<(sessionId: string) => void>();
export function onSessionClose(fn: (sessionId: string) => void): void {
  sessionCloseHooks.add(fn);
}

export function closeSession(sessionId: string): void {
  // Signal cancellation to any in-flight tool calls before closing the driver.
  sessionAborts.get(sessionId)?.abort();
  sessionAborts.delete(sessionId);
  evictDriver(sessionId); // no integration id → evict every driver in the session
  for (const hook of sessionCloseHooks) {
    try { hook(sessionId); } catch { /* best-effort */ }
  }
}

// ── Per-query cancellation (used by tool handlers) ────────────────────────────

/** Create a query abort controller wired to the session-wide abort. */
export function registerQueryAbort(logId: number, sessionId: string): AbortController {
  const ac = new AbortController();
  queryAborts.set(logId, ac);
  const sessionSignal = sessionAborts.get(sessionId)?.signal;
  if (sessionSignal) {
    if (sessionSignal.aborted) ac.abort();
    else sessionSignal.addEventListener("abort", () => ac.abort(), { once: true });
  }
  return ac;
}

export function clearQueryAbort(logId: number): void {
  queryAborts.delete(logId);
}

/** Abort a single in-flight query by its log id. Returns false if not running. */
export function cancelQuery(logId: number): boolean {
  const ac = queryAborts.get(logId);
  if (!ac) return false;
  ac.abort();
  return true;
}

// ── Driver acquisition ────────────────────────────────────────────────────────

// Drivers are keyed by session AND integration: a group endpoint serves several
// integrations under one MCP session, so each needs its own driver/tunnel.
function driverKey(sessionId: string, integrationId: string): string {
  return `${sessionId}::${integrationId}`;
}

export async function getDriver(sessionId: string, integration: Integration): Promise<Driver> {
  const key = driverKey(sessionId, integration.id);
  const existing = driverPool.get(key);
  if (existing) {
    // Reset idle timer on use
    resetIdleTimer(key, existing);
    const idleFor = Date.now() - existing.lastUsed;
    if (idleFor < STALE_AFTER_MS && !existing.validating) {
      existing.lastUsed = Date.now();
      return existing.driver;
    }

    // Idle long enough that the tunnel may be dead. Probe before handing it
    // back; on failure, evict and fall through to a fresh rebuild rather than
    // letting the caller hang on a dead connection.
    existing.validating ??= validateOrRebuild(key, integration, existing);
    return existing.validating;
  }

  return createDriverEntry(key, integration).driver;
}

function createDriverEntry(key: string, integration: Integration): DriverEntry {
  // Register the in-flight promise synchronously so any tool call that arrives
  // while the tunnel/connection is still coming up awaits this same setup
  // instead of starting a second one (which, with an SSH proxy, meant a second
  // interactive auth prompt racing the timeout).
  const useSsh = Boolean(integration.config.use_ssh);
  const connectTimeout = useSsh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  // Self-heal: if the SSH tunnel drops mid-session, evict this entry so the next
  // tool call rebuilds the tunnel/driver instead of hanging on a dead listener.
  const created = createDriver(integration, () => {
    if (driverPool.get(key) === entry) evictDriverByKey(key);
  });
  const driver = withTimeout(created, connectTimeout, "connect");
  const idleTimer = setTimeout(() => evictDriverByKey(key), IDLE_MS);
  const entry: DriverEntry = { driver, idleTimer, lastUsed: Date.now() };
  driverPool.set(key, entry);

  // If setup fails, drop the entry so the next call retries from scratch.
  driver.catch(() => {
    if (driverPool.get(key) === entry) {
      clearTimeout(idleTimer);
      driverPool.delete(key);
    }
  });

  // If the connect timeout fired (or the entry was otherwise evicted) but the
  // underlying setup later succeeds, close the orphaned driver so its pool and
  // SSH tunnel don't leak.
  created.then((d) => {
    if (driverPool.get(key) !== entry) d.close().catch(() => {});
  }).catch(() => {});

  // Surface connect/tunnel/auth health to the UI. Uses the real setup promise
  // (not the timeout-wrapped one) so a genuine failure is recorded with its
  // actual message rather than the watchdog's "Timed out".
  created.then(
    () => recordHealth(integration.id, "ok"),
    (e) => recordHealth(integration.id, "error", humanizeConnError(e)),
  );

  return entry;
}

function resetIdleTimer(key: string, entry: DriverEntry): void {
  clearTimeout(entry.idleTimer);
  entry.idleTimer = setTimeout(() => evictDriverByKey(key), IDLE_MS);
}

async function validateOrRebuild(
  key: string,
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
      return createDriverEntry(key, integration).driver;
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
  // best-effort close once setup settles; ignore a failed/aborted setup
  entry.driver.then((d) => d.close()).catch(() => {});
}

/**
 * Evict pooled drivers. With an integration id, evicts just that one; without,
 * evicts every driver in the session (used when the whole session closes).
 */
export function evictDriver(sessionId: string, integrationId?: string): void {
  if (integrationId) {
    evictDriverByKey(driverKey(sessionId, integrationId));
    return;
  }
  for (const key of [...driverPool.keys()]) {
    if (key.startsWith(`${sessionId}::`)) evictDriverByKey(key);
  }
}
