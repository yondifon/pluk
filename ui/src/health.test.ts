import { describe, test, expect } from "bun:test";
import { humanizeHealthError, humanizeTestError } from "./health";

describe("humanizeHealthError", () => {
  test("maps known failures to plain language with next step", () => {
    expect(humanizeHealthError("connection refused")).toContain("Couldn’t connect");
    expect(humanizeHealthError("connection refused").toLowerCase()).toContain("try again");
    expect(humanizeHealthError("Unauthorized")).toContain("Authentication failed");
    expect(humanizeHealthError("timeout")).toContain("timed out");
    expect(humanizeHealthError("ssh tunnel error")).toContain("Secure tunnel");
  });

  test("unknown error always appends next step", () => {
    const raw = "something weird happened";
    const msg = humanizeHealthError(raw);
    expect(msg).toContain(raw);
    expect(msg.toLowerCase()).toContain("try again");
    expect(msg.toLowerCase()).toContain("check the setup");
  });

  test("empty returns generic with next step", () => {
    expect(humanizeHealthError(null).toLowerCase()).toContain("try again");
    expect(humanizeHealthError("").toLowerCase()).toContain("try again");
  });

  test("does not duplicate try again", () => {
    const withTry = "Couldn’t connect. Check that the connection is reachable and try again.";
    expect(humanizeHealthError(withTry)).toBe(withTry);
  });

  test("no internal vocab leaks", () => {
    const msg = humanizeHealthError("connection refused").toLowerCase();
    for (const banned of ["adapter", "owner", "manifest", "policy kind", "projection"]) {
      expect(msg).not.toContain(banned);
    }
  });

  test("humanizeTestError aliases health", () => {
    expect(humanizeTestError("auth failed")).toContain("Authentication failed");
  });
});
