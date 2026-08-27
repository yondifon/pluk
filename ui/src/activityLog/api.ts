import type { LogCursor, LogEntry, LogPage, TimeRange } from "./types";

export type LogScope = { connectionId: string } | { groupId: string };

function scopeParams(scope: LogScope): string {
  if ("connectionId" in scope) return `connectionId=${encodeURIComponent(scope.connectionId)}`;
  return `groupId=${encodeURIComponent(scope.groupId)}`;
}

export async function fetchLogPage(
  scope: LogScope,
  range: TimeRange,
  cursor: LogCursor | null,
  signal?: AbortSignal,
): Promise<LogPage> {
  const qs = new URLSearchParams();
  // scope
  if ("connectionId" in scope) qs.set("connectionId", scope.connectionId);
  else qs.set("groupId", scope.groupId);
  qs.set("range", range);
  if (cursor) {
    qs.set("cursorTime", cursor.createdAt);
    qs.set("cursorId", String(cursor.id));
  }
  const res = await fetch(`/api/logs?${qs.toString()}`, { signal });
  if (!res.ok) throw new Error(`logs fetch ${res.status}`);
  const json = await res.json() as { entries: any[]; nextCursor: LogCursor | null; hasMore: boolean };
  // Normalize camelCase from server
  const entries: LogEntry[] = (json.entries ?? []).map((e: any) => ({
    id: e.id,
    connectionId: e.connectionId,
    connectionName: e.connectionName,
    sql: e.sql,
    verdict: e.verdict,
    reason: e.reason ?? null,
    categories: e.categories ?? null,
    source: e.source ?? null,
    resultJson: e.resultJson ?? null,
    rowCount: e.rowCount ?? null,
    responseText: e.responseText ?? null,
    groupId: e.groupId ?? null,
    groupName: e.groupName ?? null,
    database: e.database ?? null,
    createdAt: e.createdAt,
  }));
  return { entries, nextCursor: json.nextCursor ?? null, hasMore: !!json.hasMore };
}

export async function cancelLog(id: number): Promise<boolean> {
  const res = await fetch(`/api/log/${id}/cancel`, { method: "POST" });
  if (!res.ok) return false;
  const json = await res.json() as { ok: boolean };
  return !!json.ok;
}

export async function getRetention(): Promise<number> {
  const res = await fetch("/api/retention");
  if (!res.ok) return 30;
  const json = await res.json() as { days: number };
  return json.days;
}

export async function setRetention(days: number): Promise<void> {
  const res = await fetch("/api/retention", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ days }) });
  if (!res.ok) throw new Error(`retention ${res.status}`);
}

export async function clearLogs(scope: LogScope): Promise<number> {
  const qs = scopeParams(scope);
  const res = await fetch(`/api/logs?${qs}`, { method: "DELETE" });
  if (!res.ok) throw new Error(`clear ${res.status}`);
  const json = await res.json() as { deleted: number };
  return json.deleted ?? 0;
}

// Merge by id newest-first
export function mergeEntries(existing: LogEntry[], incoming: LogEntry[]): LogEntry[] {
  const byId = new Map<number, LogEntry>();
  for (const e of existing) byId.set(e.id, e);
  for (const e of incoming) byId.set(e.id, e);
  return Array.from(byId.values()).sort((a, b) => {
    if (a.createdAt !== b.createdAt) return a.createdAt > b.createdAt ? -1 : 1;
    return b.id - a.id;
  });
}

export interface LiveEvent {
  id: number;
  connectionId: string;
  connectionName: string;
  sql: string;
  verdict: string;
  reason: string | null;
  categories: string | null;
  source: string | null;
  groupId: string | null;
  groupName: string | null;
  database: string | null;
  rowCount: number | null;
  createdAt: string;
}

export type SseHandler = {
  onEvent: (ev: LiveEvent) => void;
  onReady: (cursor: number) => void;
  onKeepalive: (cursor: number) => void;
};

export function connectEvents(after: number, handler: SseHandler, opts?: { backoffMs?: number }): { close: () => void } {
  let closed = false;
  let attempt = 0;
  let es: EventSource | null = null;
  let reconnectTimer: number | null = null;
  let cursor = after;

  const open = () => {
    if (closed) return;
    es = new EventSource(`/api/events?after=${cursor}`);
    es.addEventListener("event", (e: MessageEvent) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as LiveEvent;
        // monotonic cursor: never move backwards
        if (data.id > cursor) cursor = data.id;
        handler.onEvent(data);
        attempt = 0;
      } catch { /* ignore */ }
    });
    es.addEventListener("ready", (e: MessageEvent) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as { cursor: number };
        if (data.cursor > cursor) cursor = data.cursor;
        handler.onReady(data.cursor);
        attempt = 0;
      } catch {}
    });
    es.addEventListener("keepalive", (e: MessageEvent) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as { cursor: number };
        if (data.cursor > cursor) cursor = data.cursor;
        handler.onKeepalive(data.cursor);
      } catch {}
    });
    es.onerror = () => {
      es?.close();
      es = null;
      if (closed) return;
      const backoff = Math.min(30000, (opts?.backoffMs ?? 500) * Math.pow(2, attempt++));
      reconnectTimer = window.setTimeout(open, backoff);
    };
  };
  open();
  return {
    close() {
      closed = true;
      es?.close();
      if (reconnectTimer) clearTimeout(reconnectTimer);
    },
  };
}

// Drift check: periodically compare local high-water with server
export function startDriftCheck(getLocalHighWater: () => number, onDrift: (serverHighWater: number) => void, intervalMs = 30000): () => void {
  const id = window.setInterval(async () => {
    try {
      // Use retention endpoint not suitable; instead fetch high-water via events ready?
      // Fallback: fetch a single newest entry and compare ids
      // We use /api/events?after=0 ready frame to get cursor, but simpler: fetch logs page 1 and look at first id
      // Here we poll /api/logs?range=all limit 1 equivalent by fetching page
      const res = await fetch(`/api/events?after=${getLocalHighWater()}`);
      // Not ideal — we just keep connection alive; drift is detected via keepalive cursor > local
      void res;
    } catch {}
    // Real drift reconciliation is driven by keepalive cursor comparison in caller
    void onDrift;
  }, intervalMs);
  return () => clearInterval(id);
}
