import { beforeEach, expect, test } from "bun:test";
import { PROMPT_COPY } from "./post-prompt";
import {
  CHOICES,
  makeAnswerEnvelope,
  parseServerMessage,
  PROTOCOL_VERSION,
} from "./protocol";

const storage = new Map<string, unknown>();
let injected: Array<Record<string, unknown>> = [];
let injectionResult: unknown = "postNow";
let injectionThrows = false;

const mockChrome = {
  alarms: { create: async () => {} },
  runtime: { getManifest: () => ({ version: "test" }) },
  storage: {
    local: {
      get: async (keys: readonly string[]) => {
        const out: Record<string, unknown> = {};
        for (const key of keys) if (storage.has(key)) out[key] = storage.get(key);
        return out;
      },
      set: async (items: Record<string, unknown>) => {
        for (const [key, value] of Object.entries(items)) storage.set(key, value);
      },
    },
  },
  scripting: {
    executeScript: async (details: Record<string, unknown>) => {
      injected.push(details);
      if (injectionThrows) throw new Error("no such tab");
      return [{ result: injectionResult }];
    },
  },
};

(globalThis as unknown as { chrome: unknown }).chrome = mockChrome;

const { BrowserExecutor } = await import("./browser-executor");
const { BrowserBridge } = await import("./connection");

const prompt = {
  account: "@owner",
  text: "Exact post text",
  replyingTo: null,
  canQueue: true,
  closesAt: Date.now() + 60_000,
};

beforeEach(() => {
  storage.clear();
  storage.set("automationContext", { windowId: 1, tabId: 1 });
  injected = [];
  injectionResult = "postNow";
  injectionThrows = false;
});

test("the overlay says what it is about to do, in Pluk's words", () => {
  expect(PROMPT_COPY).toEqual({
    post: "Post this?",
    reply: "Send this reply?",
    from: "Posting as",
    postNow: "Post now",
    queue: "Queue for later",
    discard: "Discard",
    dismissal: "Press Esc to decide later — it waits for you in Pluk.",
  });
});

test("the answer is whatever the overlay returned to Chrome", async () => {
  const executor = new BrowserExecutor();
  for (const choice of CHOICES) {
    injectionResult = choice;
    expect(await executor.askAboutPost(prompt)).toBe(choice);
  }
});

// The overlay is injected, so what comes back is Chrome's, not the page's.
// Anything that is not one of the four answers is no answer at all — which
// sends nothing and discards nothing.
test("a result that is not one of the four answers leaves the post waiting", async () => {
  const executor = new BrowserExecutor();
  for (const forged of [
    "POSTNOW",
    "post_now",
    "",
    null,
    undefined,
    0,
    { choice: "postNow" },
    ["postNow"],
  ]) {
    injectionResult = forged;
    expect(await executor.askAboutPost(prompt)).toBe("later");
  }
});

test("a tab that is gone, or refuses the overlay, is not an answer either", async () => {
  const executor = new BrowserExecutor();
  injectionThrows = true;
  expect(await executor.askAboutPost(prompt)).toBe("later");

  storage.delete("automationContext");
  injectionThrows = false;
  expect(await executor.askAboutPost(prompt)).toBe("later");
  expect(injected).toHaveLength(1);
});

// Everything a page or content script can reach goes through the extension's
// runtime message bus. There is no answer on it — the only way in is a
// question Pluk asked over its own socket.
test("nothing on the extension's message bus can answer a question", async () => {
  const bridge = new BrowserBridge();
  for (const forged of [
    { type: "answer", questionId: "q1", choice: "postNow" },
    { type: "wande_post_answer", choice: "postNow" },
    { type: "ask", questionId: "q1" },
    { type: "post_now" },
  ]) {
    expect(await bridge.handleMessage(forged)).toEqual({
      ok: false,
      error: "This request is not supported.",
    });
  }
});

// A question only counts when it arrives shaped the way Pluk sends it, and
// the answer names that question and nothing else.
test("a malformed question is refused, and an answer names the question it came from", () => {
  const now = Date.now();
  const ask = {
    version: PROTOCOL_VERSION,
    type: "ask",
    questionId: "question-1",
    account: "@owner",
    text: "Exact post text",
    replyingTo: null,
    canQueue: true,
    closesAt: now + 600_000,
    issuedAt: now,
    expiresAt: now + 30_000,
  };
  expect(parseServerMessage(ask)).toMatchObject({ ok: true });
  for (const broken of [
    { ...ask, questionId: "" },
    { ...ask, canQueue: "yes" },
    { ...ask, closesAt: "soon" },
    { ...ask, draftId: "draft-1" },
    { ...ask, text: 42 },
  ]) {
    expect(parseServerMessage(broken)).toMatchObject({ ok: false });
  }

  expect(makeAnswerEnvelope("question-1", "postNow", now)).toMatchObject({
    type: "answer",
    questionId: "question-1",
    choice: "postNow",
  });
});
