// Standalone diagnostic harness: drives the real BrowserBridge/connection.ts
// transport against a real running Rust server over a real loopback
// WebSocket. Only chrome.scripting.executeScript (site DOM reading, out of
// transport scope) and the browser chrome.* surface are mocked; everything
// else (hello/ready/heartbeat/command/result framing, reconnection, the
// command ledger) is the production extension code, unmodified.
//
// Usage: bun run extension/src/transport-repro.ts <serverUrl> <token> [slowMs]

export {};

interface MockTab {
  id: number;
  windowId: number;
  status: chrome.tabs.TabStatus;
  url: string;
  pendingUrl?: string;
  active: boolean;
}

const [, , serverUrlArg, tokenArg, slowMsArg] = process.argv;
if (!serverUrlArg || !tokenArg) {
  console.error("usage: transport-repro.ts <serverUrl> <token> [slowMs]");
  process.exit(2);
}
const serverUrl = serverUrlArg;
const token = tokenArg;
const slowMs = Number(slowMsArg ?? "45000");

const storage = new Map<string, unknown>();
let tab: MockTab = {
  id: 1,
  windowId: 1,
  status: "complete",
  url: "",
  active: true,
};
let executeScriptCalls = 0;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function report(event: Record<string, unknown>): void {
  console.log(`REPRO_EVENT ${JSON.stringify(event)}`);
}

const mockChrome = {
  alarms: {
    create: async () => {},
  },
  runtime: {
    getManifest: () => ({ version: "transport-repro" }),
  },
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
      focused: false,
      tabs: [{ id: tab.id, windowId: tab.windowId, active: tab.active }],
    }),
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
    captureVisibleTab: async () => "data:image/png;base64,AA==",
    onUpdated: { addListener: () => {}, removeListener: () => {} },
  },
  scripting: {
    executeScript: async (details: {
      readonly args: readonly [{ readonly targetUrl: string }];
    }) => {
      executeScriptCalls += 1;
      if (executeScriptCalls === 1 && slowMs > 0) {
        await sleep(slowMs);
      }
      const [options] = details.args;
      return [
        {
          frameId: 0,
          result: {
            state: "ready",
            kind: "x_home",
            url: options.targetUrl,
            title: "Home / X",
          },
        },
      ];
    },
  },
};

(globalThis as unknown as { chrome: unknown }).chrome = mockChrome;

const { BrowserBridge } = await import("./connection");
const { readStatus } = await import("./state");

async function waitForStatus(
  predicate: (state: string) => boolean,
  timeoutMs: number,
): Promise<{ readonly state: string; readonly message: string }> {
  const deadline = Date.now() + timeoutMs;
  let last = await readStatus();
  while (Date.now() < deadline) {
    last = await readStatus();
    if (predicate(last.state)) {
      return last;
    }
    await sleep(150);
  }
  throw new Error(
    `timed out waiting for status; last was ${JSON.stringify(last)}`,
  );
}

async function createJob(): Promise<string> {
  const response = await fetch(`${serverUrl}/wande/jobs`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      platform: "x",
      action: "inspect",
      targetUrl: "https://x.com/home",
      payload: {},
    }),
  });
  if (!response.ok) {
    throw new Error(
      `create job failed: ${response.status} ${await response.text()}`,
    );
  }
  const body = (await response.json()) as {
    readonly job: { readonly id: string };
  };
  return body.job.id;
}

async function waitForJob(
  jobId: string,
  timeoutMs: number,
): Promise<{ readonly status: string; readonly error: unknown }> {
  const deadline = Date.now() + timeoutMs;
  let last: { readonly status: string; readonly error: unknown } = {
    status: "unknown",
    error: null,
  };
  while (Date.now() < deadline) {
    const response = await fetch(`${serverUrl}/wande/jobs/${jobId}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    const body = (await response.json()) as {
      readonly job: { readonly status: string; readonly error: unknown };
    };
    last = body.job;
    if (last.status !== "queued" && last.status !== "running") {
      return last;
    }
    await sleep(500);
  }
  throw new Error(
    `job ${jobId} did not finish in time; last was ${JSON.stringify(last)}`,
  );
}

const bridge = new BrowserBridge();
await bridge.initialize();
await bridge.handleMessage({
  type: "save_settings",
  settings: { serverUrl, token, enabled: true },
});

const connected = await waitForStatus((state) => state === "connected", 5_000);
report({ phase: "connected", status: connected });

const job1 = await createJob();
report({ phase: "job1-created", jobId: job1 });
const finished1 = await waitForJob(job1, slowMs + 30_000);
report({ phase: "job1-finished", ...finished1 });

await bridge.handleMessage({
  type: "save_settings",
  settings: { serverUrl, token, enabled: false },
});
await waitForStatus((state) => state === "disabled", 5_000);
await bridge.handleMessage({
  type: "save_settings",
  settings: { serverUrl, token, enabled: true },
});
const reconnected = await waitForStatus(
  (state) => state === "connected",
  5_000,
);
report({ phase: "reconnected", status: reconnected });

const job2 = await createJob();
const finished2 = await waitForJob(job2, 15_000);
report({ phase: "job2-finished", ...finished2 });

process.exit(0);
