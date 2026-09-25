import { describe, expect, test } from "bun:test";
import { canRestart, canStop, isLocalMcp, restartLabel, stateLabel, stateNote, stateTone } from "./local-mcp";

describe("isLocalMcp", () => {
  test("only a connection of local counts, everything else is remote", () => {
    expect(isLocalMcp({ config: { connection: "local" } })).toBe(true);
    expect(isLocalMcp({ config: { connection: "remote" } })).toBe(false);
    expect(isLocalMcp({ config: {} })).toBe(false);
  });
});

describe("server state words and controls", () => {
  test("each state has its own label", () => {
    expect(stateLabel("idle")).toBe("Not started");
    expect(stateLabel("starting")).toBe("Starting…");
    expect(stateLabel("running")).toBe("Running");
    expect(stateLabel("stopped")).toBe("Stopped");
    expect(stateLabel("crashed")).toBe("Keeps stopping");
  });

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

  test("a server that is not up offers Start rather than Restart", () => {
    expect(restartLabel("idle")).toBe("Start");
    expect(restartLabel("stopped")).toBe("Start");
    expect(restartLabel("running")).toBe("Restart");
    expect(restartLabel("crashed")).toBe("Restart");
  });

  test("tone follows whether the server is good, off, or wrong", () => {
    expect(stateTone("running")).toBe("on");
    expect(stateTone("starting")).toBe("on");
    expect(stateTone("stopped")).toBe("off");
    expect(stateTone("crashed")).toBe("warn");
  });
});
