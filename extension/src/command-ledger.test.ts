import { expect, test } from "bun:test";
import { findCommandLedgerEntry, recordCommand } from "./state";

test("records a submitted command so a duplicate cannot run again", async () => {
  const globals = globalThis as unknown as Record<string, unknown>;
  const previousChrome = globals.chrome;
  const values = new Map<string, unknown>();
  const chromeValue = {
    storage: {
      local: {
        async get(keys: readonly string[]) {
          return Object.fromEntries(
            keys
              .filter((key) => values.has(key))
              .map((key) => [key, values.get(key)]),
          );
        },
        async set(items: Record<string, unknown>) {
          for (const [key, value] of Object.entries(items)) {
            values.set(key, value);
          }
        },
      },
    },
  };
  Object.defineProperty(globals, "chrome", {
    configurable: true,
    value: chromeValue,
  });
  try {
    await recordCommand("submit-command", "submit_reply", "completed");
    const entry = await findCommandLedgerEntry("submit-command");
    expect(entry).toMatchObject({
      commandId: "submit-command",
      action: "submit_reply",
      state: "completed",
    });
    expect(await findCommandLedgerEntry("submit-command")).not.toBeNull();
  } finally {
    if (previousChrome === undefined) {
      delete globals.chrome;
    } else {
      Object.defineProperty(globals, "chrome", {
        configurable: true,
        value: previousChrome,
      });
    }
  }
});
