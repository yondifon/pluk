import { test, expect, describe } from "bun:test";
import {
  classify,
  evaluate,
  parsePolicy,
  capRows,
  PRESETS,
  defaultPolicyFor,
  type StatementCategory,
} from "./policy.js";

// ── classify ─────────────────────────────────────────────────────────────────

describe("classify - PostgreSQL", () => {
  test("SELECT", () => {
    const r = classify("SELECT * FROM t", "PostgreSQL");
    expect(r.categories).toEqual(["select"]);
    expect(r.statementCount).toBe(1);
    expect(r.hasUpdateOrDeleteWithoutWhere).toBe(false);
  });

  test("CTE select", () => {
    const r = classify("WITH x AS (SELECT 1) SELECT * FROM x", "PostgreSQL");
    expect(r.categories).toEqual(["select"]);
  });

  test("INSERT", () => {
    expect(classify("INSERT INTO t VALUES (1)", "PostgreSQL").categories).toEqual(["insert"]);
  });

  test("UPDATE with WHERE", () => {
    const r = classify("UPDATE t SET x=1 WHERE id=1", "PostgreSQL");
    expect(r.categories).toEqual(["update"]);
    expect(r.hasUpdateOrDeleteWithoutWhere).toBe(false);
  });

  test("UPDATE without WHERE", () => {
    const r = classify("UPDATE t SET x=1", "PostgreSQL");
    expect(r.categories).toEqual(["update"]);
    expect(r.hasUpdateOrDeleteWithoutWhere).toBe(true);
  });

  test("DELETE without WHERE", () => {
    const r = classify("DELETE FROM t", "PostgreSQL");
    expect(r.hasUpdateOrDeleteWithoutWhere).toBe(true);
  });

  test("DROP TABLE", () => {
    expect(classify("DROP TABLE t", "PostgreSQL").categories).toEqual(["drop"]);
  });

  test("ALTER TABLE", () => {
    expect(classify("ALTER TABLE t ADD COLUMN x INT", "PostgreSQL").categories).toEqual(["alter"]);
  });

  test("TRUNCATE", () => {
    expect(classify("TRUNCATE TABLE t", "PostgreSQL").categories).toEqual(["truncate"]);
  });

  test("stacked SELECT;DROP", () => {
    const r = classify("SELECT 1; DROP TABLE t", "PostgreSQL");
    expect(r.statementCount).toBe(2);
    expect(r.categories).toContain("select");
    expect(r.categories).toContain("drop");
  });

  test("EXPLAIN falls back to keyword classifier -> inspect", () => {
    const r = classify("EXPLAIN SELECT 1", "PostgreSQL");
    expect(r.categories).toEqual(["inspect"]);
  });

  test("BEGIN -> transaction", () => {
    expect(classify("BEGIN", "PostgreSQL").categories).toEqual(["transaction"]);
  });

  test("GRANT -> grant", () => {
    expect(classify("GRANT SELECT ON t TO u", "PostgreSQL").categories).toEqual(["grant"]);
  });

  test("REVOKE -> grant", () => {
    expect(classify("REVOKE SELECT ON t FROM u", "PostgreSQL").categories).toEqual(["grant"]);
  });

  test("SET -> session", () => {
    expect(classify("SET search_path = public", "PostgreSQL").categories).toEqual(["session"]);
  });

  test("CALL -> procedure", () => {
    expect(classify("CALL my_proc()", "PostgreSQL").categories).toEqual(["procedure"]);
  });

  test("VACUUM falls back -> maintenance", () => {
    expect(classify("VACUUM", "PostgreSQL").categories).toEqual(["maintenance"]);
  });

  test("SHOW -> inspect", () => {
    expect(classify("SHOW search_path", "PostgreSQL").categories).toEqual(["inspect"]);
  });

  test("comment-prefix bypass blocked (fail-closed or correct category)", () => {
    // /*c*/DELETE should still be classified as delete (keyword fallback strips comments)
    const r = classify("/*c*/DELETE FROM t", "PostgreSQL");
    expect(r.categories).toContain("delete");
  });

  test("garbage SQL -> fail-closed (null category if completely unknown)", () => {
    // Truly unparseable + no keyword match
    const r = classify("XYZZY FROBNICATOR 42", "PostgreSQL");
    // keyword fallback returns null for unknown; categories may contain null
    expect(r.categories.some(c => c === null)).toBe(true);
  });
});

describe("classify - MySQL", () => {
  test("REPLACE -> merge", () => {
    expect(classify("REPLACE INTO t VALUES (1)", "MySQL").categories).toEqual(["merge"]);
  });

  test("SHOW TABLES -> inspect", () => {
    expect(classify("SHOW TABLES", "MySQL").categories).toEqual(["inspect"]);
  });
});

describe("classify - SQLite", () => {
  test("PRAGMA falls back -> inspect", () => {
    expect(classify("PRAGMA table_info(t)", "SQLite").categories).toEqual(["inspect"]);
  });

  test("SELECT", () => {
    expect(classify("SELECT 1", "SQLite").categories).toEqual(["select"]);
  });
});

// ── dangerous constructs ─────────────────────────────────────────────────────

describe("classify - dangerous constructs", () => {
  test("COPY FROM PROGRAM detected", () => {
    expect(classify("COPY t FROM PROGRAM 'ls'", "PostgreSQL").dangerous).toBe("copy-program");
  });

  test("COPY TO PROGRAM detected", () => {
    expect(classify("COPY t TO PROGRAM 'ls'", "PostgreSQL").dangerous).toBe("copy-program");
  });

  test("INTO OUTFILE detected", () => {
    expect(classify("SELECT * FROM t INTO OUTFILE '/tmp/x'", "MySQL").dangerous).toBe("into-outfile");
  });

  test("LOAD DATA detected", () => {
    expect(classify("LOAD DATA INFILE '/tmp/x' INTO TABLE t", "MySQL").dangerous).toBe("load-data");
  });

  test("ATTACH DATABASE detected", () => {
    expect(classify("ATTACH DATABASE '/tmp/x' AS aux", "SQLite").dangerous).toBe("attach-database");
  });

  test("pg_read_file detected", () => {
    expect(classify("SELECT pg_read_file('/etc/passwd')", "PostgreSQL").dangerous).toBe("pg-read-file");
  });

  test("safe SELECT has no dangerous flag", () => {
    expect(classify("SELECT 1", "PostgreSQL").dangerous).toBeNull();
  });
});

// ── evaluate ─────────────────────────────────────────────────────────────────

describe("evaluate - read-only preset", () => {
  const policy = PRESETS["read-only"];

  test("SELECT allowed", () => {
    expect(evaluate("SELECT 1", policy, "PostgreSQL").ok).toBe(true);
  });

  test("INSERT blocked", () => {
    const r = evaluate("INSERT INTO t VALUES (1)", policy, "PostgreSQL");
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/insert/);
  });

  test("UPDATE blocked", () => {
    expect(evaluate("UPDATE t SET x=1 WHERE id=1", policy, "PostgreSQL").ok).toBe(false);
  });

  test("DELETE blocked", () => {
    expect(evaluate("DELETE FROM t WHERE id=1", policy, "PostgreSQL").ok).toBe(false);
  });

  test("DROP blocked", () => {
    expect(evaluate("DROP TABLE t", policy, "PostgreSQL").ok).toBe(false);
  });

  test("EXPLAIN allowed (inspect)", () => {
    expect(evaluate("EXPLAIN SELECT 1", policy, "PostgreSQL").ok).toBe(true);
  });

  test("PRAGMA allowed (inspect via SQLite)", () => {
    expect(evaluate("PRAGMA table_info(t)", policy, "SQLite").ok).toBe(true);
  });
});

describe("evaluate - stacked statements", () => {
  test("blocked when blockStacked=true", () => {
    const policy = { ...PRESETS["read-only"], blockStacked: true };
    const r = evaluate("SELECT 1; DROP TABLE t", policy, "PostgreSQL");
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/stacked/i);
  });

  test("allowed when blockStacked=false (if cats allowed)", () => {
    const policy = { ...PRESETS["migrations"], blockStacked: false };
    const r = evaluate("SELECT 1; SELECT 2", policy, "PostgreSQL");
    expect(r.ok).toBe(true);
  });
});

describe("evaluate - requireWhere", () => {
  const policy = { ...PRESETS["read-write"], requireWhere: true };

  test("UPDATE without WHERE blocked", () => {
    const r = evaluate("UPDATE t SET x=1", policy, "PostgreSQL");
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/WHERE/i);
  });

  test("UPDATE with WHERE allowed", () => {
    expect(evaluate("UPDATE t SET x=1 WHERE id=1", policy, "PostgreSQL").ok).toBe(true);
  });

  test("DELETE without WHERE blocked", () => {
    const r = evaluate("DELETE FROM t", policy, "PostgreSQL");
    expect(r.ok).toBe(false);
  });
});

describe("evaluate - filesystem", () => {
  test("COPY PROGRAM blocked by default", () => {
    const r = evaluate("COPY t FROM PROGRAM 'ls'", PRESETS["unrestricted"], "PostgreSQL");
    // unrestricted has allowFilesystem=true, should pass category check
    expect(r.ok).toBe(true);
  });

  test("COPY PROGRAM blocked when allowFilesystem=false", () => {
    const policy = { ...PRESETS["read-only"], allowed: ["select" as StatementCategory], allowFilesystem: false };
    const r = evaluate("COPY t FROM PROGRAM 'ls'", policy, "PostgreSQL");
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/filesystem/i);
  });

  test("INTO OUTFILE blocked when allowFilesystem=false", () => {
    const policy = { ...PRESETS["read-only"], allowFilesystem: false };
    const r = evaluate("SELECT * FROM t INTO OUTFILE '/tmp/x'", policy, "MySQL");
    expect(r.ok).toBe(false);
  });
});

describe("evaluate - fail-closed on unknown", () => {
  test("garbage SQL blocked", () => {
    const r = evaluate("XYZZY FROBNICATOR 42", PRESETS["unrestricted"], "PostgreSQL");
    expect(r.ok).toBe(false);
    expect(r.reason).toMatch(/could not be identified/i);
  });
});

// ── parsePolicy ───────────────────────────────────────────────────────────────

describe("parsePolicy", () => {
  test("null policy + read_only=1 -> read-only preset", () => {
    const p = parsePolicy(null, 1);
    expect(p.preset).toBe("read-only");
    expect(p.allowed).toContain("select");
    expect(p.allowed).not.toContain("insert");
  });

  test("null policy + read_only=0 -> unrestricted", () => {
    const p = parsePolicy(null, 0);
    expect(p.preset).toBe("unrestricted");
  });

  test("valid JSON parsed correctly", () => {
    const raw = JSON.stringify({ preset: "read-write", allowed: ["select", "insert"], blockStacked: false, requireWhere: true, allowFilesystem: false, maxRows: 500 });
    const p = parsePolicy(raw, 0);
    expect(p.preset).toBe("read-write");
    expect(p.allowed).toEqual(["select", "insert"]);
    expect(p.blockStacked).toBe(false);
    expect(p.requireWhere).toBe(true);
    expect(p.maxRows).toBe(500);
  });

  test("invalid JSON falls back to read_only flag", () => {
    const p = parsePolicy("not-json", 1);
    expect(p.preset).toBe("read-only");
  });

  test("unknown allowed categories filtered out", () => {
    const raw = JSON.stringify({ preset: "custom", allowed: ["select", "foobar"], blockStacked: true, requireWhere: false, allowFilesystem: false, maxRows: null });
    const p = parsePolicy(raw, 0);
    expect(p.allowed).not.toContain("foobar");
    expect(p.allowed).toContain("select");
  });
});

// ── defaultPolicyFor ──────────────────────────────────────────────────────────

describe("defaultPolicyFor", () => {
  test("production -> read-only", () => {
    expect(defaultPolicyFor("production").preset).toBe("read-only");
  });
  test("staging -> read-only", () => {
    expect(defaultPolicyFor("staging").preset).toBe("read-only");
  });
  test("development -> read-write", () => {
    expect(defaultPolicyFor("development").preset).toBe("read-write");
  });
  test("local -> read-write", () => {
    expect(defaultPolicyFor("local").preset).toBe("read-write");
  });
});

// ── capRows ───────────────────────────────────────────────────────────────────

describe("capRows", () => {
  const rows = Array.from({ length: 50 }, (_, i) => ({ id: i }));

  test("no cap when maxRows=null", () => {
    const r = capRows(rows, null);
    expect(r.rows.length).toBe(50);
    expect(r.truncated).toBe(false);
  });

  test("no cap when rows <= maxRows", () => {
    const r = capRows(rows, 100);
    expect(r.truncated).toBe(false);
  });

  test("truncates and flags when rows > maxRows", () => {
    const r = capRows(rows, 10);
    expect(r.rows.length).toBe(10);
    expect(r.truncated).toBe(true);
    expect(r.limit).toBe(10);
  });
});
