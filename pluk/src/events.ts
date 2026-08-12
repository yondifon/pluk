import { subscribeLogActivity, logHighWater, logRowsAfter } from "./store/queryLog.js";

// Server-Sent Events for the activity log (GET /api/events?after=<cursor>).
// Every insert/update in query_log reaches subscribers through queryLog's feed;
// this module turns that feed into one held-open HTTP stream per client. A
// reconnecting client replays the rows written after its cursor — the cursor is
// a query_log row id, so replay is exact: no gap, no duplicates.

const encoder = new TextEncoder();
const KEEPALIVE_MS = 15_000;

interface Subscriber {
  controller: ReadableStreamDefaultController<Uint8Array>;
}

const subscribers = new Set<Subscriber>();
let keepaliveTimer: ReturnType<typeof setInterval> | null = null;

function frame(name: string, data: string): Uint8Array {
  return encoder.encode(`event: ${name}\ndata: ${data}\n\n`);
}

function stopKeepaliveIfIdle(): void {
  if (subscribers.size === 0 && keepaliveTimer) {
    clearInterval(keepaliveTimer);
    keepaliveTimer = null;
  }
}

function drop(sub: Subscriber): void {
  subscribers.delete(sub);
  try {
    sub.controller.error(new Error("subscriber dropped"));
  } catch {
    // controller already closed
  }
  stopKeepaliveIfIdle();
}

function enqueue(sub: Subscriber, payload: Uint8Array): void {
  try {
    sub.controller.enqueue(payload);
  } catch {
    drop(sub);
  }
}

function ensureKeepalive(): void {
  if (keepaliveTimer) return;
  keepaliveTimer = setInterval(() => {
    const payload = frame("keepalive", JSON.stringify({ cursor: logHighWater() }));
    // Drop clients that have fallen more than the stream's high-water mark
    // behind (dead or too slow to read); buffering them would grow memory.
    for (const sub of [...subscribers]) {
      const desired = sub.controller.desiredSize;
      if (desired !== null && desired < 0) drop(sub);
      else enqueue(sub, payload);
    }
  }, KEEPALIVE_MS);
}

subscribeLogActivity((row) => {
  if (subscribers.size === 0) return;
  const payload = frame("event", JSON.stringify(row));
  for (const sub of [...subscribers]) enqueue(sub, payload);
});

/** `after` query param → cursor. Absent means 0 (fresh client); anything that is
 *  not a non-negative integer is rejected, never coerced. */
export function parseAfter(raw: string | null): number | null {
  if (raw === null) return 0;
  if (!/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  return Number.isSafeInteger(n) ? n : null;
}

/** Route for the events endpoint: rejects a malformed cursor, else opens a stream. */
export function handleActivityEvents(req: Request, url: URL): Response | null {
  if (req.method !== "GET" || url.pathname !== "/api/events") return null;
  const after = parseAfter(url.searchParams.get("after"));
  if (after === null) {
    return Response.json({ ok: false, error: "Invalid cursor" }, { status: 400 });
  }
  return createEventStream(after);
}

/** Open an event stream for a client at `after`. Replays the rows written since
 *  the cursor, then sends `ready` with the current high-water. The connection
 *  stays open, fed `keepalive` frames, until the client disconnects. */
export function createEventStream(after: number): Response {
  let sub: Subscriber | null = null;
  const stream = new ReadableStream<Uint8Array>(
    {
      start(controller: ReadableStreamDefaultController<Uint8Array>) {
        sub = { controller };
        subscribers.add(sub);
        for (const row of logRowsAfter(after)) controller.enqueue(frame("event", JSON.stringify(row)));
        controller.enqueue(frame("ready", JSON.stringify({ cursor: logHighWater() })));
        ensureKeepalive();
      },
      cancel() {
        if (sub) subscribers.delete(sub);
        stopKeepaliveIfIdle();
      },
    },
    { highWaterMark: 256 },
  );
  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    },
  });
}
