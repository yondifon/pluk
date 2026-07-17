import { test, expect } from "bun:test";
import { createSqliteDriver } from "./sqlite.js";

// list_databases is the multi-db discovery tool. On SQLite it reports the
// connection's databases (main + any ATTACHed), so it stays useful and uniform
// across engines rather than erroring on a single-file database.

test("listDatabases reports main for a fresh SQLite connection", async () => {
  const driver = createSqliteDriver(":memory:");
  try {
    expect(await driver.listDatabases()).toEqual(["main"]);
  } finally {
    await driver.close();
  }
});

test("listDatabases includes ATTACHed databases", async () => {
  const driver = createSqliteDriver(":memory:");
  try {
    await driver.query("ATTACH DATABASE ':memory:' AS extra");
    expect(await driver.listDatabases()).toEqual(["main", "extra"]);
  } finally {
    await driver.close();
  }
});

test("queryReadOnly makes the engine refuse writes, then resets query_only", async () => {
  const driver = createSqliteDriver(":memory:");
  try {
    await driver.query("CREATE TABLE t (id INTEGER)");
    // A write smuggled through the read-only path is rejected by SQLite itself,
    // not the policy layer.
    await expect(driver.queryReadOnly("INSERT INTO t VALUES (1)")).rejects.toThrow();
    // query_only was reset in finally, so a legitimate write still works.
    await driver.query("INSERT INTO t VALUES (2)");
    expect((await driver.query("SELECT id FROM t")).rows).toEqual([{ id: 2 }]);
  } finally {
    await driver.close();
  }
});

test("query binds ? placeholders from params instead of dropping them", async () => {
  const driver = createSqliteDriver(":memory:");
  try {
    await driver.query("CREATE TABLE t (id INTEGER, name TEXT)");
    await driver.query("INSERT INTO t VALUES (?, ?)", [1, "alice"]);
    await driver.query("INSERT INTO t VALUES (?, ?)", [2, "bob"]);
    // The bound value selects exactly one row — proof params reached the driver.
    const res = await driver.queryReadOnly("SELECT name FROM t WHERE id = ?", [2]);
    expect(res.rows).toEqual([{ name: "bob" }]);
  } finally {
    await driver.close();
  }
});
