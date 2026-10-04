import { afterEach, describe, expect, test, vi } from "bun:test";
import { WindowVisibility, pollWhileVisible } from "./windowVisibility";

describe("pollWhileVisible", () => {
  afterEach(() => vi.useRealTimers());

  test("stays quiet while hidden, refreshes once on show, and stops for good", () => {
    vi.useFakeTimers();
    const visibility = new WindowVisibility();
    visibility.setVisible(false);

    let calls = 0;
    const stop = pollWhileVisible(() => {
      calls += 1;
    }, 1000, visibility);

    vi.advanceTimersByTime(5000);
    expect(calls).toBe(0);

    visibility.setVisible(true);
    expect(calls).toBe(1);

    vi.advanceTimersByTime(2000);
    expect(calls).toBe(3);

    stop();
    vi.advanceTimersByTime(5000);
    expect(calls).toBe(3);

    visibility.setVisible(false);
    visibility.setVisible(true);
    vi.advanceTimersByTime(5000);
    expect(calls).toBe(3);
  });
});
