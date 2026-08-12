import {
  readLogPage,
  type LogCursor,
  type LogRange,
  type LogScope,
} from "./store/queryLog.js";

const CURSOR_TIME = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/;
const RANGES = new Set<string>(["hour", "today", "7d", "30d", "all"]);

function isLogRange(value: string): value is LogRange {
  return RANGES.has(value);
}

export function parseLogRange(raw: string | null): LogRange | null {
  if (raw === null) return "all";
  return isLogRange(raw) ? raw : null;
}

function parseLogCursor(time: string | null, id: string | null): LogCursor | null | undefined {
  if (time === null && id === null) return undefined;
  if (time === null || id === null || !CURSOR_TIME.test(time) || !/^\d+$/.test(id)) return null;
  const parsedId = Number(id);
  if (!Number.isSafeInteger(parsedId) || parsedId <= 0) return null;
  return { createdAt: time, id: parsedId };
}

function parseLogScope(connectionId: string | null, groupId: string | null): LogScope | null {
  if (connectionId !== null) {
    return groupId === null && connectionId !== "" ? { connectionId } : null;
  }
  return groupId !== null && groupId !== "" ? { groupId } : null;
}

export function handleLogRequest(req: Request, url: URL): Response | null {
  if (req.method !== "GET" || url.pathname !== "/api/logs") return null;

  const scope = parseLogScope(
    url.searchParams.get("connectionId"),
    url.searchParams.get("groupId"),
  );
  if (scope === null) {
    return Response.json({ ok: false, error: "Exactly one log scope is required" }, { status: 400 });
  }

  const range = parseLogRange(url.searchParams.get("range"));
  if (range === null) return Response.json({ ok: false, error: "Invalid range" }, { status: 400 });

  const cursor = parseLogCursor(
    url.searchParams.get("cursorTime"),
    url.searchParams.get("cursorId"),
  );
  if (cursor === null) return Response.json({ ok: false, error: "Invalid cursor" }, { status: 400 });

  return Response.json(readLogPage(scope, range, cursor));
}
