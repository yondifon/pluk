import { describe, test, expect, beforeEach, afterEach, vi } from "bun:test";
import { renderGroupDetail } from "./groupDetail";
import type { LogEntry } from "./activityLog/types";

// The group screen opens on Logs, which talks to the host before Overview shows.
beforeEach(() => {
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: async () => 30 },
    event: { listen: async () => () => {} },
  } as never;
});

const group = {
  id: "g1",
  name: "Marketing",
  environment: "production",
  token: "grouptok",
  members: [],
};

function mountOverview(): HTMLElement {
  const root = document.createElement("div");
  renderGroupDetail(root, {
    group,
    integrations: [],
    onEdit: () => {},
    onDelete: () => {},
    onEditIntegration: () => {},
    inject: async () => ({ status: "added", path: "" }),
  });
  root.querySelector<HTMLButtonElement>("#tab-overview")!.click();
  return root;
}

const pendingEntry: LogEntry = {
  id: 1,
  connectionId: "c1",
  connectionName: "Pg",
  sql: "SELECT 1",
  verdict: "pending",
  reason: null,
  categories: null,
  source: null,
  resultJson: null,
  rowCount: null,
  responseText: null,
  groupId: "g1",
  groupName: "Marketing",
  createdAt: "2026-08-26 12:00:00",
};

function installHost(): { liveSubscriptions: () => number } {
  let live = 0;
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: async (cmd: string) => {
        if (cmd === "get_logs") return { entries: [pendingEntry], nextCursor: null, hasMore: false };
        return 30;
      },
    },
    event: {
      listen: async (event: string) => {
        if (event !== "pluk://log-activity") return () => {};
        live += 1;
        return () => {
          live -= 1;
        };
      },
    },
  } as never;
  return { liveSubscriptions: () => live };
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("group activity log lifecycle", () => {
  afterEach(() => vi.useRealTimers());

  test("repeated Logs and Overview switches keep one live subscription and timer, and teardown releases both", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const host = installHost();
    const root = document.createElement("div");
    const handle = renderGroupDetail(root, {
      group,
      integrations: [],
      onEdit: () => {},
      onDelete: () => {},
      onEditIntegration: () => {},
      inject: async () => ({ status: "added", path: "" }),
    });
    await flush();

    expect(host.liveSubscriptions()).toBe(1);
    expect(vi.getTimerCount()).toBe(1);

    for (let i = 0; i < 3; i += 1) {
      root.querySelector<HTMLButtonElement>("#tab-overview")!.click();
      await flush();
      expect(host.liveSubscriptions()).toBe(0);
      expect(vi.getTimerCount()).toBe(0);

      root.querySelector<HTMLButtonElement>("#tab-logs")!.click();
      await flush();
      expect(host.liveSubscriptions()).toBe(1);
      expect(vi.getTimerCount()).toBe(1);
    }

    handle.destroy();
    await flush();
    expect(host.liveSubscriptions()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("group overview", () => {
  test("shows one MCP section holding the endpoint and Install", () => {
    const root = mountOverview();

    const cards = [...root.querySelectorAll(".ui-card")];
    expect(cards.map((c) => c.querySelector(".ui-card-title")!.textContent)).toEqual([
      "MCP endpoint",
      "Integrations",
    ]);
    expect(cards[0].querySelector(".mono")!.textContent).toBe("http://localhost:4242/mcp/grouptok");
    expect(cards[0].querySelector('button[aria-label="Install into selected clients"]')).not.toBeNull();
  });

  test("installs the group under its own server name", async () => {
    const root = mountOverview();
    const snippet = root.querySelector(".snippet")!;
    const client = root.querySelector<HTMLSelectElement>("#pluk-client-select")!;
    client.value = "cursor";
    client.dispatchEvent(new Event("change"));

    expect(snippet.textContent).toContain('"marketing-production"');
    expect(snippet.textContent).toContain("http://localhost:4242/mcp/grouptok");
  });
});
