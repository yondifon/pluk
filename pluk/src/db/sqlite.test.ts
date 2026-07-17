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
