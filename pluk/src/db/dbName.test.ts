import { test, expect } from "bun:test";
import { isValidDatabaseName, resolveOverrideDatabase } from "./dbName.js";

// The multi-database feature's security boundary at the driver layer: a hostile
// identifier must be rejected, and a connection pinned to a database at setup
// must never be pointed at another. Both fail closed (throw) before any pool is
// built.

test("isValidDatabaseName accepts plain identifiers, rejects injection", () => {
  for (const ok of ["app", "app_prod", "billing-2", "db$x", "A1"]) {
    expect(isValidDatabaseName(ok)).toBe(true);
  }
  for (const bad of ["", "a b", "a;b", "a`b", 'a"b', "a.b", "a'b", "a/*x*/", "a\nb", "x".repeat(129)]) {
    expect(isValidDatabaseName(bad)).toBe(false);
  }
});

test("resolveOverrideDatabase: no override falls back to the configured database", () => {
  expect(resolveOverrideDatabase("appdb", undefined)).toBe("appdb");
  expect(resolveOverrideDatabase(undefined, undefined)).toBeUndefined();
});

test("resolveOverrideDatabase: an unpinned connection may target any valid database", () => {
  expect(resolveOverrideDatabase(undefined, "analytics")).toBe("analytics");
});

test("resolveOverrideDatabase: a pinned connection is locked to its database", () => {
  // Same database is fine; a different one is refused.
  expect(resolveOverrideDatabase("appdb", "appdb")).toBe("appdb");
  expect(() => resolveOverrideDatabase("appdb", "otherdb")).toThrow(/locked to database "appdb"/);
});

test("resolveOverrideDatabase: a hostile identifier is rejected", () => {
  expect(() => resolveOverrideDatabase(undefined, "evil; DROP")).toThrow(/Invalid database name/);
  expect(() => resolveOverrideDatabase(undefined, "a.b")).toThrow(/Invalid database name/);
});
