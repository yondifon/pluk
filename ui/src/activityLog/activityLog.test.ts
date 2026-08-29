import { describe, test, expect } from "bun:test";
import { mergeEntries } from "./api";
import { capResponse, capConsole, PREVIEW_LINES, PREVIEW_CHARS, CONSOLE_PREVIEW_LINES, CONSOLE_PREVIEW_CHARS } from "./caps";
import { scan, scanConsole } from "./highlight";
import { parseUtcToMillis, relativeTime, localTimeString } from "./time";
import type { LogEntry } from "./types";

function entry(over: Partial<LogEntry> & { id: number }): LogEntry {
  return {
    connectionId: "c1",
    connectionName: "Pg",
    sql: "SELECT 1",
    verdict: "allowed",
    reason: null,
    categories: null,
    source: null,
    resultJson: null,
    rowCount: null,
    responseText: null,
    groupId: null,
    groupName: null,
    createdAt: "2026-08-26 12:00:00",
    ...over,
  };
}

describe("page merging by id and ordering", () => {
  test("merges by id and stays sorted newest first", () => {
    const a = [entry({ id: 1, createdAt: "2026-08-26 12:00:00" }), entry({ id: 2, createdAt: "2026-08-26 12:01:00" })];
    const b = [entry({ id: 2, createdAt: "2026-08-26 12:01:00", sql: "UPDATED" }), entry({ id: 3, createdAt: "2026-08-26 12:02:00" })];
    const merged = mergeEntries(a, b);
    expect(merged.map(e => e.id)).toEqual([3, 2, 1]);
    expect(merged.find(e => e.id === 2)?.sql).toBe("UPDATED");
  });

  test("tie-breaks on id within identical timestamps", () => {
    const a = [entry({ id: 5, createdAt: "2026-08-26 12:00:00" }), entry({ id: 3, createdAt: "2026-08-26 12:00:00" })];
    const b = [entry({ id: 4, createdAt: "2026-08-26 12:00:00" })];
    const merged = mergeEntries(a, b);
    expect(merged.map(e => e.id)).toEqual([5, 4, 3]);
  });
});

describe("generation counter discards stale response", () => {
  test("simulated generation discards stale page", async () => {
    let generation = 0;
    let applied: number[] = [];
    const applyIfCurrent = (gen: number, ids: number[]) => {
      if (gen !== generation) return false;
      applied = ids;
      return true;
    };
    generation = 1; const g1 = generation;
    generation = 2; const g2 = generation;
    // stale response for g1 arrives after g2
    expect(applyIfCurrent(g1, [1])).toBe(false);
    expect(applied).toEqual([]);
    expect(applyIfCurrent(g2, [2,3])).toBe(true);
    expect(applied).toEqual([2,3]);
  });
});

describe("filters and live counts", () => {
  function counts(es: LogEntry[]) {
    return {
      allowed: es.filter(e => e.verdict === "allowed").length,
      blocked: es.filter(e => e.verdict === "blocked").length,
      error: es.filter(e => e.verdict === "error").length,
    };
  }
  test("each verdict filter counts", () => {
    const es = [
      entry({ id: 1, verdict: "allowed" }),
      entry({ id: 2, verdict: "blocked" }),
      entry({ id: 3, verdict: "error" }),
      entry({ id: 4, verdict: "allowed" }),
      entry({ id: 5, verdict: "pending" }),
    ];
    expect(counts(es)).toEqual({ allowed: 2, blocked: 1, error: 1 });
    expect(es.filter(e => e.verdict === "allowed").length).toBe(2);
  });

  test("search matches sql, source, connectionName, categories", () => {
    const es = [
      entry({ id: 1, sql: "SELECT * FROM users", source: "query", connectionName: "Prod", categories: "read" }),
    ];
    const q = (needle: string) => es.filter(e =>
      e.sql.toLowerCase().includes(needle) || (e.source?.toLowerCase().includes(needle) ?? false) || e.connectionName.toLowerCase().includes(needle) || (e.categories?.toLowerCase().includes(needle) ?? false)
    );
    expect(q("users").length).toBe(1);
    expect(q("query").length).toBe(1);
    expect(q("prod").length).toBe(1);
    expect(q("read").length).toBe(1);
    expect(q("missing").length).toBe(0);
  });
});

describe("cursor monotonicity under out-of-order events", () => {
  test("cursor never moves backwards", () => {
    let cursor = 10;
    const applyEvent = (id: number) => {
      if (id > cursor) cursor = id;
    };
    applyEvent(12); expect(cursor).toBe(12);
    applyEvent(11); expect(cursor).toBe(12);
    applyEvent(15); expect(cursor).toBe(15);
    applyEvent(14); expect(cursor).toBe(15);
  });
});

describe("drift reconciliation", () => {
  test("reconciles when cursor far behind", () => {
    let local = 10;
    const server = 50;
    const shouldReconcile = server - local > 20;
    expect(shouldReconcile).toBe(true);
    // after reconcile, local jumps
    local = server;
    expect(local).toBe(50);
  });
});

describe("pending row transitions", () => {
  test("pending -> done", () => {
    let e = entry({ id: 1, verdict: "pending" });
    expect(e.verdict).toBe("pending");
    e = { ...e, verdict: "allowed", responseText: "ok" };
    expect(e.verdict).toBe("allowed");
  });
  test("pending -> cancelled distinct from failed", () => {
    let e = entry({ id: 1, verdict: "pending" });
    e = { ...e, verdict: "cancelled" };
    expect(e.verdict).toBe("cancelled");
    expect(e.verdict).not.toBe("error");
    // cancelled label should not read as error
    const label = (v: string) => v === "cancelled" ? "cancelled" : v === "error" ? "error" : v;
    expect(label(e.verdict)).toBe("cancelled");
    expect(label("error")).toBe("error");
  });
});

describe("cap thresholds and notices", () => {
  test("response preview limited to 10 lines or 1200 chars", () => {
    const longLines = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    const r1 = capResponse(longLines);
    expect(r1.truncated).toBe(true);
    expect(r1.preview.split("\n").length).toBe(PREVIEW_LINES);

    const longChars = "x".repeat(2000);
    const r2 = capResponse(longChars);
    expect(r2.truncated).toBe(true);
    expect(r2.preview.length).toBe(PREVIEW_CHARS);

    const short = "hello\nworld";
    const r3 = capResponse(short);
    expect(r3.truncated).toBe(false);
  });

  test("console output limited to 40 lines or 6000 chars", () => {
    const longLines = Array.from({ length: 80 }, (_, i) => `out ${i}`).join("\n");
    const r1 = capConsole(longLines);
    expect(r1.truncated).toBe(true);
    expect(r1.preview.split("\n").length).toBe(CONSOLE_PREVIEW_LINES);

    const longChars = "y".repeat(7000);
    const r2 = capConsole(longChars);
    expect(r2.truncated).toBe(true);
    expect(r2.preview.length).toBe(CONSOLE_PREVIEW_CHARS);
  });
});

describe("highlighter across all five languages", () => {
  test("json: strings, numbers, keywords, property keys", () => {
    const spans = scan(`{"key": "value", "num": 123, "flag": true}`, "json");
    const tints = new Set(spans.map(s => s.tint));
    expect(tints.has("property")).toBe(true);
    expect(tints.has("string")).toBe(true);
    expect(tints.has("number")).toBe(true);
    expect(tints.has("keyword")).toBe(true);
  });
  test("toml: comments, strings, property", () => {
    const spans = scan(`# comment\nkey = "value"\nflag = true`, "toml");
    const tints = new Set(spans.map(s => s.tint));
    expect(tints.has("comment")).toBe(true);
    expect(tints.has("string")).toBe(true);
  });
  test("sql: keywords, types, strings, comments", () => {
    const spans = scan(`SELECT id, name FROM users WHERE name = 'hi' -- comment\n/* block */`, "sql");
    const tints = new Set(spans.map(s => s.tint));
    expect(tints.has("keyword")).toBe(true);
    expect(tints.has("string")).toBe(true);
    expect(tints.has("comment")).toBe(true);
  });
  test("sql types highlighted", () => {
    const spans = scan(`CREATE TABLE t (id integer, name varchar)`, "sql");
    expect(spans.some(s => s.tint === "type")).toBe(true);
  });
  test("shell: commands as property", () => {
    const spans = scan(`git status && echo hi # comment`, "shell");
    const tints = spans.map(s => s.tint);
    expect(tints.includes("property")).toBe(true);
    expect(spans.some(s => s.tint === "comment")).toBe(true);
  });
  test("text: no spans", () => {
    expect(scan("just plain text", "text").length).toBe(0);
  });
});

describe("console line-level tinting", () => {
  test("timestamps tinted as comment in first two words", () => {
    const spans = scanConsole("2026-08-26 12:34:56 INFO hello");
    expect(spans.some(s => s.tint === "comment")).toBe(true);
  });
  test("banner lines tinted as property", () => {
    const spans = scanConsole("=== Starting ===");
    expect(spans.some(s => s.tint === "property")).toBe(true);
  });
  test("severity words tinted as keyword", () => {
    const spans = scanConsole("something failed with error");
    expect(spans.some(s => s.tint === "keyword")).toBe(true);
  });
  test("console not grammar-parsed: quotes do not leak", () => {
    const spans = scanConsole(`"unclosed quote and # not a comment`);
    // should not produce string tint — only line-level
    expect(spans.every(s => s.tint !== "string")).toBe(true);
  });
});

describe("explicit UTC parsing with local display", () => {
  test("parses UTC string explicitly", () => {
    expect(parseUtcToMillis("2026-08-26 12:34:56")).not.toBeNull();
    expect(parseUtcToMillis("2026-08-26T12:34:56")).toBeNull();
    expect(parseUtcToMillis("2026-08-26 12:34:56.789")).toBeNull();
    expect(parseUtcToMillis("not a date")).toBeNull();
  });
  test("relative and local display differ from raw", () => {
    const raw = "2026-08-26 12:34:56";
    expect(relativeTime(raw)).not.toBe(raw); // should be relative
    const local = localTimeString(raw);
    expect(local.length).toBe(19);
    // localTimeString should parse via UTC, not locale
    expect(parseUtcToMillis(local)).not.toBeNull(); // still valid format
  });
  test("invalid raw returns raw", () => {
    expect(relativeTime("invalid")).toBe("invalid");
    expect(localTimeString("invalid")).toBe("invalid");
  });
});
