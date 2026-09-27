import { describe, expect, test } from "bun:test";
import { canRestart, canStop, stateNote } from "./local-mcp";

describe("server state words and controls", () => {
  test("only a crashed server explains itself", () => {
    expect(stateNote("crashed")).not.toBeNull();
    expect(stateNote("running")).toBeNull();
    expect(stateNote("stopped")).toBeNull();
    expect(stateNote("starting")).toBeNull();
  });

  test("stop is offered while it is up or coming up, restart except mid-start", () => {
    expect(canStop("running")).toBe(true);
    expect(canStop("starting")).toBe(true);
    expect(canStop("stopped")).toBe(false);
    expect(canStop("crashed")).toBe(false);

    expect(canRestart("starting")).toBe(false);
    expect(canRestart("running")).toBe(true);
    expect(canRestart("stopped")).toBe(true);
    expect(canRestart("crashed")).toBe(true);
  });
});
