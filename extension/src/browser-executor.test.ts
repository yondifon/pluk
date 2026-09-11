import { beforeEach, expect, test } from "bun:test";
import { type CommandEnvelope, PROTOCOL_VERSION } from "./protocol";
import type { DriverPageResult } from "./drivers/types";

interface MockTab {
  id: number;
  windowId: number;
  status: chrome.tabs.TabStatus;
  url: string;
  pendingUrl?: string;
  active: boolean;
}

let tab: MockTab = {
  id: 1,
  windowId: 1,
  status: "complete",
  url: "",
  active: true,
};
let captureVisibleTabImpl: () => Promise<string> = () =>
  Promise.resolve("data:image/png;base64,AA==");
let captureVisibleTabCalls = 0;
let windowFocusCalls: number[] = [];
let windowFocused = false;
let executeScriptImpl: (targetUrl: string) => DriverPageResult = (
  targetUrl,
) => ({
  state: "ready",
  kind: "x_home",
  url: targetUrl,
  title: "Home / X",
});

const storage = new Map<string, unknown>();

const mockChrome = {
  storage: {
    local: {
      get: async (keys: readonly string[]) => {
        const out: Record<string, unknown> = {};
        for (const key of keys) {
          if (storage.has(key)) {
            out[key] = storage.get(key);
          }
        }
        return out;
      },
      set: async (items: Record<string, unknown>) => {
        for (const [key, value] of Object.entries(items)) {
          storage.set(key, value);
        }
      },
    },
  },
  permissions: {
    contains: async () => true,
  },
  windows: {
    create: async (data: { readonly url?: string }) => {
      windowFocused = false;
      tab = {
        id: 1,
        windowId: 1,
        status: "complete",
        url: data.url ?? "",
        active: true,
      };
      return { id: 1, type: "normal", incognito: false, focused: false };
    },
    get: async (id: number) => ({
      id,
      type: "normal" as const,
      incognito: false,
      focused: windowFocused,
      tabs: [{ id: tab.id, windowId: tab.windowId, active: tab.active }],
    }),
    update: async (id: number, properties: { readonly focused?: boolean }) => {
      if (properties.focused === true) {
        windowFocused = true;
        windowFocusCalls.push(id);
      }
      return {
        id,
        type: "normal" as const,
        incognito: false,
        focused: windowFocused,
      };
    },
  },
  tabs: {
    get: async (id: number) => ({ ...tab, id }),
    query: async () => [{ ...tab }],
    update: async (
      id: number,
      properties: { readonly active?: boolean; readonly url?: string },
    ) => {
      tab = {
        ...tab,
        id,
        url: properties.url ?? tab.url,
        pendingUrl: undefined,
        status: "complete",
        active: properties.active ?? tab.active,
      };
      return { ...tab };
    },
    reload: async () => {},
    captureVisibleTab: async () => {
      captureVisibleTabCalls += 1;
      return captureVisibleTabImpl();
    },
    onUpdated: { addListener: () => {}, removeListener: () => {} },
  },
  scripting: {
    executeScript: async (details: {
      readonly args: readonly [{ readonly targetUrl: string }];
    }) => {
      const [options] = details.args;
      return [{ frameId: 0, result: executeScriptImpl(options.targetUrl) }];
    },
  },
};

(globalThis as unknown as { chrome: unknown }).chrome = mockChrome;

const { BrowserExecutor } = await import("./browser-executor");

beforeEach(() => {
  storage.clear();
  tab = { id: 1, windowId: 1, status: "complete", url: "", active: true };
  captureVisibleTabCalls = 0;
  windowFocusCalls = [];
  windowFocused = false;
  captureVisibleTabImpl = () => Promise.resolve("data:image/png;base64,AA==");
  executeScriptImpl = (targetUrl) => ({
    state: "ready",
    kind: "x_home",
    url: targetUrl,
    title: "Home / X",
  });
});

function makeCommand(
  action: CommandEnvelope["action"],
  targetUrl: string,
): CommandEnvelope {
  const now = Date.now();
  return {
    version: PROTOCOL_VERSION,
    type: "command",
    jobId: "job-1",
    commandId: "command-1",
    platform: "x",
    action,
    targetUrl,
    issuedAt: now,
    expiresAt: now + 60_000,
    payload: { kind: "empty" },
  };
}

function makeSink() {
  const uploads: { kind: string; contentType: string }[] = [];
  return {
    uploads,
    upload: async (
      _jobId: string,
      kind: "screenshot" | "extract",
      contentType: string,
    ) => {
      uploads.push({ kind, contentType });
      return `${kind}-artifact`;
    },
  };
}

test("a read succeeds and returns no image field even when screenshot capture is unavailable", async () => {
  captureVisibleTabImpl = () =>
    Promise.reject(
      new Error("MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND quota exceeded"),
    );
  const executor = new BrowserExecutor();
  const sink = makeSink();

  const result = await executor.run(
    makeCommand("inspect", "https://x.com/home"),
    sink,
  );

  expect(captureVisibleTabCalls).toBe(0);
  expect(windowFocusCalls).toEqual([]);
  expect(result.extractArtifactId).toBe("extract-artifact");
  expect(result).not.toHaveProperty("screenshotArtifactId");
  expect(sink.uploads).toEqual([
    { kind: "extract", contentType: "application/json" },
  ]);
});

test("capture still attaches a screenshot", async () => {
  const executor = new BrowserExecutor();
  const sink = makeSink();

  const result = await executor.run(
    makeCommand("capture", "https://x.com/home"),
    sink,
  );

  expect(captureVisibleTabCalls).toBe(1);
  expect(result.extractArtifactId).toBe("extract-artifact");
  expect(result.screenshotArtifactId).toBe("screenshot-artifact");
});

test("submit_reply focuses the window and succeeds without a screenshot even when capture would fail", async () => {
  captureVisibleTabImpl = () => Promise.reject(new Error("quota exceeded"));
  const executor = new BrowserExecutor();
  const sink = makeSink();

  executeScriptImpl = (targetUrl) => ({
    state: "submission_succeeded",
    kind: "submission",
    url: targetUrl,
    title: "Post / X",
  });
  const submitCommand: CommandEnvelope = {
    ...makeCommand("submit_reply", "https://x.com/status/42"),
    payload: {
      kind: "submission",
      draftId: "draft-1",
      postId: "42",
      text: "Thanks for sharing this.",
    },
  };
  const submission = await executor.run(submitCommand, sink);
  expect(captureVisibleTabCalls).toBe(0);
  expect(windowFocusCalls).toEqual([1]);
  expect(submission).not.toHaveProperty("screenshotArtifactId");
});

test("a real DOM read failure still surfaces honestly when the site itself denies access", async () => {
  executeScriptImpl = () => ({
    state: "login_required",
    message: "X needs an active sign-in.",
  });
  const executor = new BrowserExecutor();
  const sink = makeSink();

  return expect(
    executor.run(makeCommand("inspect", "https://x.com/home"), sink),
  ).rejects.toThrow("X needs an active sign-in.");
});

test("a pre-submit failure stays a failure instead of becoming uncertain", async () => {
  executeScriptImpl = () => ({
    state: "unsupported",
    message: "X did not expose the post control.",
  });
  const executor = new BrowserExecutor();
  const sink = makeSink();
  const command: CommandEnvelope = {
    ...makeCommand("submit_post", "https://x.com/compose/post"),
    payload: {
      kind: "post_submission",
      draftId: "draft-1",
      text: "Do not send",
    },
  };

  await expect(executor.run(command, sink)).rejects.toMatchObject({
    code: "site_markup_changed",
  });
  expect(sink.uploads).toEqual([]);
});
