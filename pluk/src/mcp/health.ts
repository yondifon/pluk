// Per-connection health, surfaced to the Swift UI so a failing connection shows
// red instead of silently looking fine. Updated wherever a connection is
// actually exercised — driver/tunnel setup (pool.ts) and the manual test
// endpoint — so connect/auth/tunnel failures (the silent ones) are visible
// without the user clicking Test.

export type HealthStatus = "ok" | "error";

export interface ConnHealth {
  status: HealthStatus;
  error?: string;
  at: number; // epoch ms of last observation
}

const health = new Map<string, ConnHealth>();

export function recordHealth(id: string, status: HealthStatus, error?: string): void {
  health.set(id, { status, error, at: Date.now() });
}

export function allHealth(): Record<string, ConnHealth> {
  return Object.fromEntries(health);
}
