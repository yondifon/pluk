import { test, expect } from "bun:test";
import { postgresDateTypesAsText } from "./postgres.js";
import { mysqlDateStrings } from "./mysql.js";

test("postgres date and timestamp values stay as database text", () => {
  const timestamp = "2026-06-26 11:00:00";

  expect(postgresDateTypesAsText.getTypeParser(1114)(timestamp)).toBe(timestamp);
  expect(postgresDateTypesAsText.getTypeParser(1184)(timestamp)).toBe(timestamp);
  expect(postgresDateTypesAsText.getTypeParser(1082)("2026-06-26")).toBe("2026-06-26");
});

test("mysql date and timestamp values stay as database text", () => {
  expect(mysqlDateStrings).toBe(true);
});
