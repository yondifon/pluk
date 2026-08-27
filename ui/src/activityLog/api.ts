import type { LogCursor, LogEntry, LogPage, TimeRange } from "./types";
import { invoke, listen } from "../host";

export type LogScope = { connectionId: string } | { groupId: string };

function scopeArgs(scope: LogScope): { scope: string; scopeId: string } {
  return "connectionId" in scope
    ? { scope: "connection", scopeId: scope.connectionId }
    : { scope: "group", scopeId: scope.groupId };
}

export async function fetchLogPage(
  scope: LogScope,
  range: TimeRange,
  cursor: LogCursor | null,
): Promise<LogPage> {
  return invoke<LogPage>("get_logs", {
    ...scopeArgs(scope),
    range,
    cursorTime: cursor?.createdAt ?? null,
    cursorId: cursor?.id ?? null,
  });
}

export async function cancelLog(id: number): Promise<boolean> {
  return invoke<boolean>("cancel_query", { id });
}

export async function getRetention(): Promise<number> {
  return invoke<number>("get_retention");
}

export async function setRetention(days: number): Promise<void> {
  await invoke<void>("set_retention", { days });
}

export async function clearLogs(scope: LogScope): Promise<number> {
  return invoke<number>("clear_logs", scopeArgs(scope));
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

/**
 * Live rows from the host. Every written log row is emitted, so the caller
 * filters to its own scope.
 */
export function connectEvents(onEvent: (ev: LiveEvent) => void): { close: () => void } {
  let unlisten: (() => void) | null = null;
  let closed = false;
  void listen<LiveEvent>("pluk://log-activity", onEvent).then((off) => {
    if (closed) off();
    else unlisten = off;
  });
  return {
    close() {
      closed = true;
      unlisten?.();
    },
  };
}
