import { describe, test, expect, beforeEach, afterEach, vi } from "bun:test";
import { countdown, mountWandePosts, queueStatus, slotLabel, type WandePosts } from "./wande-posts";
import { toast } from "../toast";

const NOW = new Date("2026-09-11T12:00:00").getTime();

let calls: Array<{ cmd: string; args?: Record<string, unknown> }>;
let posts: WandePosts;

function waitingPost(overrides: Partial<WandePosts["waiting"][number]> = {}) {
  return {
    id: "draft-1",
    text: "First line\nSecond line",
    replyingTo: null,
    expiresAt: NOW + 90_000,
    canQueue: true,
    ...overrides,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  calls = [];
  posts = { chromeConnected: true, sending: false, waiting: [], queued: [] };
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args });
        return cmd === "list_wande_posts" ? posts : undefined;
      },
    },
  };
});

afterEach(() => {
  toast.clear();
  vi.useRealTimers();
});

async function mount(): Promise<{ root: HTMLElement; destroy: () => void }> {
  const root = document.createElement("div");
  const panel = mountWandePosts(root);
  await vi.advanceTimersByTimeAsync(0);
  return { root, destroy: panel.destroy };
}

describe("the posts waiting on a person", () => {
  test("with nothing waiting, it says so instead of going blank", async () => {
    const { root, destroy } = await mount();
    expect(root.querySelector(".empty")!.textContent).toBe(
      "No posts waiting. Anything written through Wande lands here for you to send.",
    );
    destroy();
  });

  test("a waiting post shows its exact text and how long is left", async () => {
    posts.waiting = [waitingPost()];
    const { root, destroy } = await mount();
    expect(root.querySelector(".wande-post-text")!.textContent).toBe("First line\nSecond line");
    expect(root.querySelector(".wande-post-countdown")!.textContent).toBe("1:30 left to post");
    destroy();
  });

  test("Post now sends that one post and nothing else", async () => {
    posts.waiting = [waitingPost()];
    const { root, destroy } = await mount();
    const send = [...root.querySelectorAll("button")].find((b) => b.textContent === "Post now")!;
    posts.waiting = [];
    send.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.filter((c) => c.cmd === "send_wande_post")).toEqual([
      { cmd: "send_wande_post", args: { draftId: "draft-1", queue: false } },
    ]);
    destroy();
  });

  test("Add to queue is offered only when the post can take a slot", async () => {
    posts.waiting = [waitingPost({ canQueue: false })];
    const { root, destroy } = await mount();
    expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Post now", "Discard"]);
    destroy();
  });

  test("while a post is going out, the next one can only join the queue", async () => {
    posts.sending = true;
    posts.waiting = [waitingPost()];
    const { root, destroy } = await mount();
    expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Add to queue", "Discard"]);
    expect(root.querySelector(".wande-post-note")!.textContent).toBe(
      "A post is going out now. The next one can wait in the queue.",
    );
    destroy();
  });

  test("a post that runs out of time stays on screen saying so", async () => {
    posts.waiting = [waitingPost({ expiresAt: NOW + 2000 })];
    const { root, destroy } = await mount();
    posts.waiting = [];
    await vi.advanceTimersByTimeAsync(3000);
    expect(root.querySelector(".wande-post-note")!.textContent).toBe(
      "Time ran out, so this never went out. Ask for it again to post it.",
    );
    expect(root.querySelector(".wande-post-text")!.textContent).toBe("First line\nSecond line");
    destroy();
  });
});

describe("the posts going out later", () => {
  test("each slot shows when it goes out, how it ended, and can be taken back", async () => {
    posts.queued = [
      { id: "draft-2", text: "Later", scheduledAt: NOW + 3_600_000, status: "reserved" },
    ];
    const { root, destroy } = await mount();
    expect(root.querySelector(".wande-post-slot")!.textContent).toBe(slotLabel(NOW + 3_600_000, NOW));
    expect(root.querySelectorAll(".wande-post .hint")[0].textContent).toBe("Waiting");
    [...root.querySelectorAll("button")].find((b) => b.textContent === "Take out of the queue")!.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.filter((c) => c.cmd === "cancel_queued_wande_post")).toEqual([
      { cmd: "cancel_queued_wande_post", args: { draftId: "draft-2" } },
    ]);
    destroy();
  });

  test("a slot that already went out offers nothing to cancel", async () => {
    posts.queued = [
      { id: "draft-3", text: "Gone", scheduledAt: NOW - 60_000, status: "committed" },
    ];
    const { root, destroy } = await mount();
    expect(root.querySelectorAll(".wande-post .hint")[0].textContent).toBe("Posted");
    expect(root.querySelector(".wande-post-actions")).toBeNull();
    destroy();
  });
});

describe("the words on the clock and the slots", () => {
  test("a deadline counts down in minutes and seconds and stops at zero", () => {
    expect(countdown(107_000)).toBe("1:47");
    expect(countdown(9_400)).toBe("0:10");
    expect(countdown(-5_000)).toBe("0:00");
  });

  test("every slot outcome has a word for it", () => {
    expect(queueStatus("reserved")).toBe("Waiting");
    expect(queueStatus("committed")).toBe("Posted");
    expect(queueStatus("released")).toBe("Not posted");
    expect(queueStatus("unknown")).toBe("Outcome unknown");
  });

  test("a slot today reads as today", () => {
    expect(slotLabel(NOW + 3_600_000, NOW)).toContain("Today at");
    expect(slotLabel(NOW + 86_400_000, NOW)).toContain("Tomorrow at");
  });
});
