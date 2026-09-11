import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { mountIntegrationDetail } from "./index";
import { toast, mountToaster } from "../toast";
import type { Integration } from "./types";

let toaster: HTMLElement;
let unmountToaster: () => void;

beforeEach(() => {
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: async () => 30 },
    event: { listen: async () => () => {} },
  } as never;
  toaster = document.createElement("div");
  document.body.appendChild(toaster);
  unmountToaster = mountToaster(toaster);
});

afterEach(() => {
  toast.clear();
  unmountToaster();
  toaster.remove();
});

function currentToast(): HTMLElement {
  return toaster.querySelector<HTMLElement>(".toast:not([data-exit])")!;
}

const integration: Integration = {
  id: "1",
  name: "Prod DB",
  type: "postgres",
  config: {},
  toolConfig: {},
  token: "tok",
  createdAt: "",
};

const wande: Integration = {
  id: "w1",
  name: "Wande",
  type: "wande",
  config: {},
  toolConfig: {},
  token: "wandetok",
  createdAt: "",
};

const wandeManifest = {
  id: "wande",
  label: "Wande",
  category: "browser",
  agentHint: "The agent can write posts for your review.",
  tools: [],
  configFields: [],
};

async function mountWande(connected: boolean): Promise<{ root: HTMLElement; destroy: () => void }> {
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: async (cmd: string) => {
        if (cmd === "get_pluk_id") return { id: "pluk-test" };
        if (cmd === "list_wande_posts") return { chromeConnected: connected, sending: false, waiting: [], queued: [] };
        if (cmd === "list_installed_mcp_clients") return [];
        return undefined;
      },
    },
    event: { listen: async () => () => {} },
  } as never;
  const root = document.createElement("div");
  const handle = mountIntegrationDetail(root, wande, wandeManifest, null, {
    onEdit: () => {},
    onDuplicate: () => {},
    onDelete: () => {},
    onTest: async () => ({ ok: true }),
    inject: async () => ({ status: "added", path: "" }),
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  return { root, destroy: handle.destroy };
}

describe("mountIntegrationDetail health update", () => {
  test("updateHealth refreshes chip without recreating detail", () => {
    const root = document.createElement("div");
    const handle = mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });
    expect(root.querySelector(".status-unknown")).not.toBeNull();
    handle.updateHealth({ status: "ok", at: Date.now() });
    expect(root.querySelector(".status-ok")).not.toBeNull();
    expect(root.querySelector(".status-unknown")).toBeNull();
    handle.updateHealth({ status: "error", error: "refused", at: Date.now() });
    expect(root.querySelector(".status-failing")).not.toBeNull();
    handle.destroy();
  });

  test("detail tabs are ordered Overview, Agent setup, Logs, Tools", () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });

    expect([...root.querySelectorAll(".ui-tab")].map((tab) => tab.textContent)).toEqual([
      "Overview",
      "Agent setup",
      "Logs",
      "Tools",
    ]);
    expect(root.querySelector<HTMLButtonElement>("#tab-overview")!.getAttribute("aria-selected")).toBe("true");
  });

  test("Overview shows adapter configuration without MCP setup", () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });

    const cards = [...root.querySelectorAll(".ui-card")];
    expect(cards.map((c) => c.querySelector(".ui-card-title")!.textContent)).toEqual(["Configuration"]);
    expect(root.querySelector(".endpoint-url")).toBeNull();
    expect(root.querySelector('button[aria-label="Install into selected clients"]')).toBeNull();
  });

  test("MCP setup renders only under Agent setup", () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });

    expect(root.querySelector(".endpoint-url")).toBeNull();
    root.querySelector<HTMLButtonElement>("#tab-agentSetup")!.click();
    expect(root.querySelector(".endpoint-url")!.textContent).toBe("http://localhost:4242/mcp/tok");
    expect(root.querySelector('button[aria-label="Copy endpoint URL"]')).not.toBeNull();
    expect(root.querySelector('button[aria-label="Install into selected clients"]')).not.toBeNull();
    root.querySelector<HTMLButtonElement>("#tab-overview")!.click();
    expect(root.querySelector(".endpoint-url")).toBeNull();
  });

  test("Wande Overview contains Chrome and posts only", async () => {
    const { root, destroy } = await mountWande(true);
    const cards = [...root.querySelectorAll(".ui-card:not([hidden])")];

    expect(cards.map((card) => card.querySelector(".ui-card-title")!.textContent)).toEqual([
      "Chrome",
      "Waiting for you",
    ]);
    expect(root.querySelector('button[aria-label="Copy Pluk ID"]')).not.toBeNull();
    expect(root.querySelector(".agent-setup-tab")).toBeNull();
    root.querySelector<HTMLButtonElement>("#tab-agentSetup")!.click();
    expect(root.querySelector(".agent-setup-tab .endpoint-url")).not.toBeNull();
    expect((root.querySelector(".agent-hint-disclosure") as HTMLDetailsElement).open).toBe(false);
    destroy();
  });

  test("Chrome card shows a Connected badge and hides the steps", async () => {
    const { root, destroy } = await mountWande(true);

    expect(root.querySelector(".browser-status")!.textContent).toBe("Connected");
    expect((root.querySelector(".browser-steps") as HTMLOListElement).hidden).toBe(true);
    destroy();
  });

  test("Chrome card shows a Not connected badge and the steps", async () => {
    const { root, destroy } = await mountWande(false);

    expect(root.querySelector(".browser-status")!.textContent).toBe("Not connected");
    expect((root.querySelector(".browser-steps") as HTMLOListElement).hidden).toBe(false);
    expect([...root.querySelectorAll(".browser-steps li")].map((step) => step.textContent)).toEqual([
      "Open Wande in Chrome.",
      "Paste the Pluk ID into Wande.",
    ]);
    destroy();
  });

  test("a failed test resolves its own toast and marks the connection failing", async () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, { status: "unknown" as never, at: Date.now() }, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: false, error: "connection refused" }),
      inject: async () => ({ status: "added", path: "" }),
    });
    const btn = root.querySelector("button") as HTMLButtonElement;
    expect(btn.textContent).toBe("Test");

    btn.click();
    expect(currentToast().dataset.variant).toBe("pending");
    expect(currentToast().querySelector(".toast-description")!.textContent).toBe("Testing connection…");
    await new Promise((r) => setTimeout(r, 0));

    expect(toaster.querySelectorAll(".toast:not([data-exit])")).toHaveLength(1);
    expect(currentToast().dataset.variant).toBe("error");
    expect(currentToast().querySelector(".toast-description")!.textContent).toContain("Couldn’t connect");
    expect(root.querySelector(".status-failing")).not.toBeNull();
    expect(root.querySelector(".detail-header [role='status']")).toBeNull();
  });

  test("a passing test resolves the same toast into a success", async () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });

    (root.querySelector("button") as HTMLButtonElement).click();
    await new Promise((r) => setTimeout(r, 0));

    expect(toaster.querySelectorAll(".toast:not([data-exit])")).toHaveLength(1);
    expect(currentToast().dataset.variant).toBe("success");
    expect(currentToast().querySelector(".toast-title")!.textContent).toBe("Prod DB");
    expect(root.querySelector(".status-ok")).not.toBeNull();
  });
});
