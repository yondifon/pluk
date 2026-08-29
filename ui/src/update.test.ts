import { describe, test, expect } from "bun:test";
import { noticeFor, type UpdateState } from "./update";

describe("noticeFor", () => {
  test("announces an available version", () => {
    const state: UpdateState = { type: "available", version: "0.2.0", notes: null };
    expect(noticeFor(state)).toEqual({ kind: "available", version: "0.2.0" });
  });

  test("reports a failure the person can act on", () => {
    const state: UpdateState = { type: "failed", kind: "signature", message: "bad signature" };
    expect(noticeFor(state)).toEqual({ kind: "failed", message: "bad signature" });
  });

  test("stays quiet when the endpoint is unreachable", () => {
    const state: UpdateState = { type: "failed", kind: "unreachable", message: "dns failure" };
    expect(noticeFor(state)).toBeNull();
  });

  test("stays quiet while checking, when up to date, and when disabled", () => {
    const quiet: UpdateState[] = [
      { type: "checking" },
      { type: "idle" },
      { type: "upToDate" },
      { type: "downloading", progress: 40 },
      { type: "disabled", reason: "updater not configured" },
    ];
    for (const state of quiet) expect(noticeFor(state)).toBeNull();
  });
});
