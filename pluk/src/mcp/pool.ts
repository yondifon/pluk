import type { Integration } from "../store/integrations.js";
import { createDriver, type Driver } from "../db/index.js";

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
const CONNECT_TIMEOUT_SSH_MS = 180_000; // 3 minutes — room for interactive proxy auth
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
}

const driverPool = new Map<string, DriverEntry>();
const sessionAborts = new Map<string, AbortController>();

// Per-query abort controllers, keyed by log entry id, so the UI can cancel a
// single in-flight query (POST /api/log/:id/cancel) without tearing down the session.
const queryAborts = new Map<number, AbortController>();

// ── Session lifecycle (called by the MCP transport) ──────────────────────────

export function openSession(sessionId: string): void {
  sessionAborts.set(sessionId, new AbortController());
}

export function closeSession(sessionId: string): void {
  // Signal cancellation to any in-flight tool calls before closing the driver.
  sessionAborts.get(sessionId)?.abort();
  sessionAborts.delete(sessionId);
  evictDriver(sessionId);
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

export function getDriver(sessionId: string, integration: Integration): Promise<Driver> {
  const existing = driverPool.get(sessionId);
  if (existing) {
    // Reset idle timer on use
    clearTimeout(existing.idleTimer);
    existing.idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
    return existing.driver;
  }

  // Register the in-flight promise synchronously so any tool call that arrives
  // while the tunnel/connection is still coming up awaits this same setup
  // instead of starting a second one (which, with an SSH proxy, meant a second
  // interactive auth prompt racing the timeout).
  const useSsh = Boolean(integration.config.use_ssh);
  const connectTimeout = useSsh ? CONNECT_TIMEOUT_SSH_MS : CONNECT_TIMEOUT_DIRECT_MS;
  const created = createDriver(integration);
  const driver = withTimeout(created, connectTimeout, "connect");
  const idleTimer = setTimeout(() => evictDriver(sessionId), IDLE_MS);
  const entry: DriverEntry = { driver, idleTimer };
  driverPool.set(sessionId, entry);

  // If setup fails, drop the entry so the next call retries from scratch.
  driver.catch(() => {
    if (driverPool.get(sessionId) === entry) {
      clearTimeout(idleTimer);
      driverPool.delete(sessionId);
    }
  });

  // If the connect timeout fired (or the entry was otherwise evicted) but the
  // underlying setup later succeeds, close the orphaned driver so its pool and
  // SSH tunnel don't leak.
  created.then((d) => {
    if (driverPool.get(sessionId) !== entry) d.close().catch(() => {});
  }).catch(() => {});

  return driver;
}

export function evictDriver(sessionId: string): void {
  const entry = driverPool.get(sessionId);
  if (!entry) return;
  driverPool.delete(sessionId);
  clearTimeout(entry.idleTimer);
  // best-effort close once setup settles; ignore a failed/aborted setup
  entry.driver.then((d) => d.close()).catch(() => {});
}
