import { test, expect, afterAll } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtempSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

// The store modules resolve their DB path at import, so point them at a scratch
// dir before importing queryLog (which creates the schema) and events.
const scratch = mkdtempSync(join(tmpdir(), "pluk-events-"));
process.env.PLUK_DATA_DIR = scratch;

const {
  createLogEntry,
  updateLogEntry,
  logHighWater,
  LOG_PAGE_SIZE,
  readLogPage,
  setRetentionDays,
} = await import("./store/queryLog.js");
const { createEventStream, parseAfter, handleActivityEvents } = await import("./events.js");
const { handleLogRequest, parseLogRange } = await import("./logs.js");
const db = new Database(join(scratch, "pluk.db"));

setRetentionDays(0);

afterAll(() => {
  db.close();
  rmSync(scratch, { recursive: true, force: true });
});

type Frame = { name: string; data: string };

function parseFrame(raw: string): Frame {
  let name = "";
  const data: string[] = [];
  for (const line of raw.split("\n")) {
    if (line.startsWith("event:")) name = line.slice("event:".length).trim();
    else if (line.startsWith("data:")) data.push(line.slice("data:".length).trim());
  }
  return { name, data: data.join("\n") };
}

function makeReader(response: Response) {
  const reader = response.body!.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  return {
    async next(): Promise<Frame> {
      while (true) {
        const idx = buffer.indexOf("\n\n");
        if (idx !== -1) {
          const raw = buffer.slice(0, idx);
          buffer = buffer.slice(idx + 2);
          return parseFrame(raw);
        }
        const { done, value } = await reader.read();
        if (done) throw new Error("stream closed before frame");
        buffer += decoder.decode(value, { stream: true });
      }
    },
    cancel() {
      return reader.cancel();
    },
  };
}

test("parseAfter: absent is 0, malformed is rejected", () => {
  expect(parseAfter(null)).toBe(0);
  expect(parseAfter("")).toBeNull();
  expect(parseAfter("abc")).toBeNull();
  expect(parseAfter("-1")).toBeNull();
  expect(parseAfter("1.5")).toBeNull();
  expect(parseAfter("12abc")).toBeNull();
  expect(parseAfter("99999999999999999999")).toBeNull(); // not a safe integer
  expect(parseAfter("42")).toBe(42);
});

test("reconnect with a cursor replays exactly the missed rows", async () => {
  const ids = [1, 2, 3, 4, 5].map(() =>
    createLogEntry("conn-a", "Alpha", "select 1", "allowed", "read")
  );
  const reader = makeReader(createEventStream(ids[2]!));
  try {
    const replay = [await reader.next(), await reader.next()];
    expect(replay.map((f) => f.name)).toEqual(["event", "event"]);
    expect(replay.map((f) => (JSON.parse(f.data) as { id: number }).id)).toEqual([ids[3]!, ids[4]!]);

    const ready = await reader.next();
    expect(ready.name).toBe("ready");
    expect((JSON.parse(ready.data) as { cursor: number }).cursor).toBe(ids[4]!);
  } finally {
    await reader.cancel();
  }
});

test("a client at the high-water mark receives nothing but ready", async () => {
  createLogEntry("conn-a", "Alpha", "select 1", "allowed", "read");
  const high = logHighWater();
  const reader = makeReader(createEventStream(high));
  try {
    const first = await reader.next();
    expect(first.name).toBe("ready");
    expect((JSON.parse(first.data) as { cursor: number }).cursor).toBe(high);
  } finally {
    await reader.cancel();
  }
});

test("a row written after connect is pushed to live subscribers", async () => {
  const reader = makeReader(createEventStream(logHighWater()));
  try {
    const ready = await reader.next();
    expect(ready.name).toBe("ready");

    const id = createLogEntry("conn-b", "Beta", "insert into t values (1)", "blocked", "write", "read-only");
    const event = await reader.next();
    expect(event.name).toBe("event");
    expect(JSON.parse(event.data)).toMatchObject({ id, connectionId: "conn-b", verdict: "blocked" });
  } finally {
    await reader.cancel();
  }
});

test("an update to an existing row is pushed", async () => {
  const id = createLogEntry("conn-c", "Gamma", "select slow", "pending", "read");
  const reader = makeReader(createEventStream(logHighWater()));
  try {
    await reader.next(); // ready
    updateLogEntry(id, "allowed", undefined, { rows: [{ a: 1 }], fields: ["a"] });
    const event = await reader.next();
    expect(event.name).toBe("event");
    expect(JSON.parse(event.data)).toMatchObject({ id, verdict: "allowed" });
  } finally {
    await reader.cancel();
  }
});

test("the /api/events endpoint streams over HTTP", async () => {
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: (req) => handleActivityEvents(req, new URL(req.url)) ?? new Response("not found", { status: 404 }),
  });

  try {
    const base = `http://127.0.0.1:${server.port}`;

    const malformed = await fetch(`${base}/api/events?after=nope`);
    expect(malformed.status).toBe(400);

    const res = await fetch(`${base}/api/events?after=${logHighWater()}`);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toContain("text/event-stream");

    const reader = res.body!.getReader();
    const dec = new TextDecoder();
    const next = async () => {
      const { value, done } = await reader.read();
      return done ? null : dec.decode(value);
    };

    const ready = parseFrame((await next())!);
    expect(ready.name).toBe("ready");
    expect((JSON.parse(ready.data) as { cursor: number }).cursor).toBe(logHighWater());

    createLogEntry("conn-1", "Connection 1", "SELECT 1", "allowed", "read");
    const live = parseFrame((await next())!);
    expect(live.name).toBe("event");
    expect(JSON.parse(live.data)).toMatchObject({
      id: logHighWater(),
      connectionId: "conn-1",
      verdict: "allowed",
    });

    await reader.cancel();
  } finally {
    server.stop(true);
  }
});

function addRows(count: number, connectionId: string): number[] {
  return Array.from({ length: count }, (_, index) => createLogEntry(
    connectionId,
    connectionId,
    `select ${index}`,
    "allowed",
    "read",
  ));
}

function setCreatedAt(ids: number[], date: Date): void {
  const value = date.toISOString().slice(0, 19).replace("T", " ");
  for (const id of ids) db.query("UPDATE query_log SET created_at = ? WHERE id = ?").run(value, id);
}

function idsOf(page: { entries: { id: number }[] }): number[] {
  return page.entries.map((entry) => entry.id);
}

test("the first page uses the fixed size and returns a cursor", () => {
  const ids = addRows(LOG_PAGE_SIZE + 1, "page-first");
  setCreatedAt(ids, new Date("2026-01-01T00:00:00Z"));

  const page = readLogPage({ connectionId: "page-first" }, "all");

  expect(page.entries).toHaveLength(LOG_PAGE_SIZE);
  expect(idsOf(page)).toEqual(ids.slice().reverse().slice(0, LOG_PAGE_SIZE));
  expect(page.hasMore).toBe(true);
  expect(page.nextCursor).toEqual({
    createdAt: "2026-01-01 00:00:00",
    id: ids.at(ids.length - LOG_PAGE_SIZE)!,
  });
});

test("the cursor reads the next page without repeating the boundary row", () => {
  const ids = addRows(LOG_PAGE_SIZE + 1, "page-next");
  setCreatedAt(ids, new Date("2026-01-02T00:00:00Z"));

  const first = readLogPage({ connectionId: "page-next" }, "all");
  const second = readLogPage({ connectionId: "page-next" }, "all", first.nextCursor ?? undefined);

  expect(idsOf(second)).toEqual([ids.at(0)!]);
  expect(new Set(idsOf(first)).intersection(new Set(idsOf(second))).size).toBe(0);
});

test("paging reaches the last row with no gap or overlap", () => {
  const ids = addRows(LOG_PAGE_SIZE * 2 + 3, "page-all");
  setCreatedAt(ids, new Date("2026-01-03T00:00:00Z"));

  const pages: { entries: { id: number }[]; nextCursor: { createdAt: string; id: number } | null; hasMore: boolean }[] = [];
  let cursor: { createdAt: string; id: number } | undefined;
  while (true) {
    const page = readLogPage({ connectionId: "page-all" }, "all", cursor);
    pages.push(page);
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }

  const allIds = pages.flatMap(idsOf);
  expect(allIds).toEqual(ids.slice().reverse());
  expect(new Set(allIds).size).toBe(ids.length);
  expect(pages.at(-1)?.hasMore).toBe(false);
  expect(pages.at(-1)?.nextCursor).toBeNull();
});

test("each time range includes only its matching records", () => {
  const now = new Date();
  const cutoffRow = db.query("SELECT datetime('now', 'localtime', 'start of day', 'utc') AS cutoff").get() as { cutoff: string };
  const todayStart = new Date(`${cutoffRow.cutoff.replace(" ", "T")}Z`);
  const halfHourDate = new Date(Math.min(now.getTime(), todayStart.getTime() + 60 * 1000));
  const todayMarker = new Date(Math.min(now.getTime(), todayStart.getTime() + 2 * 60 * 60 * 1000));

  const current = createLogEntry("range-bounds", "range-bounds", "current", "allowed", "read");
  const halfHour = createLogEntry("range-bounds", "range-bounds", "half hour", "allowed", "read");
  const today = createLogEntry("range-bounds", "range-bounds", "today", "allowed", "read");
  const twoDays = createLogEntry("range-bounds", "range-bounds", "two days", "allowed", "read");
  const eightDays = createLogEntry("range-bounds", "range-bounds", "eight days", "allowed", "read");
  const thirtyOneDays = createLogEntry("range-bounds", "range-bounds", "thirty one days", "allowed", "read");

  setCreatedAt([current], now);
  setCreatedAt([halfHour], halfHourDate);
  setCreatedAt([today], todayMarker);
  setCreatedAt([twoDays], new Date(now.getTime() - 2 * 24 * 60 * 60 * 1000));
  setCreatedAt([eightDays], new Date(now.getTime() - 8 * 24 * 60 * 60 * 1000));
  setCreatedAt([thirtyOneDays], new Date(now.getTime() - 31 * 24 * 60 * 60 * 1000));

  const idsFor = (range: "hour" | "today" | "7d" | "30d" | "all") =>
    new Set(idsOf(readLogPage({ connectionId: "range-bounds" }, range)));

  const hourIds = [current, halfHour];
  if (todayMarker.getTime() >= now.getTime() - 60 * 60 * 1000) hourIds.push(today);
  expect(idsFor("hour")).toEqual(new Set(hourIds));
  expect(idsFor("today")).toEqual(new Set([current, halfHour, today]));
  expect(idsFor("7d")).toEqual(new Set([current, halfHour, today, twoDays]));
  expect(idsFor("30d")).toEqual(new Set([current, halfHour, today, twoDays, eightDays]));
  expect(idsFor("all")).toEqual(new Set([current, halfHour, today, twoDays, eightDays, thirtyOneDays]));
});

test("range filtering stays active while paging", () => {
  const ids = addRows(LOG_PAGE_SIZE + 3, "range-pages");
  const old = createLogEntry("range-pages", "range-pages", "old", "allowed", "read");
  setCreatedAt(ids, new Date());
  setCreatedAt([old], new Date(Date.now() - 8 * 24 * 60 * 60 * 1000));

  const first = readLogPage({ connectionId: "range-pages" }, "7d");
  const second = readLogPage({ connectionId: "range-pages" }, "7d", first.nextCursor ?? undefined);

  expect(first.entries).toHaveLength(LOG_PAGE_SIZE);
  expect(second.entries).toHaveLength(3);
  expect(second.entries.some((entry) => entry.id === old)).toBe(false);
  expect(second.hasMore).toBe(false);
  expect(second.nextCursor).toBeNull();
});

test("the HTTP read validates the scope, range, and cursor", async () => {
  const invalidScope = handleLogRequest(
    new Request("http://localhost/api/logs"),
    new URL("http://localhost/api/logs"),
  );
  expect(invalidScope?.status).toBe(400);

  const invalidRange = handleLogRequest(
    new Request("http://localhost/api/logs?connectionId=test&range=tomorrow"),
    new URL("http://localhost/api/logs?connectionId=test&range=tomorrow"),
  );
  expect(invalidRange?.status).toBe(400);

  const invalidCursor = handleLogRequest(
    new Request("http://localhost/api/logs?connectionId=test&cursorTime=nope&cursorId=1"),
    new URL("http://localhost/api/logs?connectionId=test&cursorTime=nope&cursorId=1"),
  );
  expect(invalidCursor?.status).toBe(400);
  expect(parseLogRange(null)).toBe("all");
  expect(parseLogRange("30d")).toBe("30d");

  const id = createLogEntry("api-read", "API read", "select 1", "allowed", "read");
  const valid = handleLogRequest(
    new Request("http://localhost/api/logs?connectionId=api-read&range=all"),
    new URL("http://localhost/api/logs?connectionId=api-read&range=all"),
  );
  expect(valid?.status).toBe(200);
  expect(await valid?.json()).toMatchObject({
    entries: [{ id, connectionId: "api-read" }],
    hasMore: false,
    nextCursor: null,
  });

  const groupId = createLogEntry(
    "member",
    "Member",
    "select 2",
    "allowed",
    "read",
    undefined,
    undefined,
    { id: "group-a", name: "Group A" },
  );
  const groupValid = handleLogRequest(
    new Request("http://localhost/api/logs?groupId=group-a&range=all"),
    new URL("http://localhost/api/logs?groupId=group-a&range=all"),
  );
  expect(groupValid?.status).toBe(200);
  expect(await groupValid?.json()).toMatchObject({ entries: [{ id: groupId, groupId: "group-a" }] });
});
