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

describe("mountIntegrationDetail health update", () => {
  test("updateHealth refreshes chip without recreating detail", () => {
    const root = document.createElement("div");
    const handle = mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    }, "overview");
    expect(root.querySelector(".status-unknown")).not.toBeNull();
    handle.updateHealth({ status: "ok", at: Date.now() });
    expect(root.querySelector(".status-ok")).not.toBeNull();
    expect(root.querySelector(".status-unknown")).toBeNull();
    handle.updateHealth({ status: "error", error: "refused", at: Date.now() });
    expect(root.querySelector(".status-failing")).not.toBeNull();
    handle.destroy();
  });

  test("an MCP server opens on Tools, and other integrations do not", async () => {
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
      core: { invoke: async () => ({ status: 200, body: { tools: [] } }) },
      event: { listen: async () => () => {} },
    } as never;
    const actions = {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added" as const, path: "" }),
    };

    const server = document.createElement("div");
    const serverHandle = mountIntegrationDetail(
      server,
      { ...integration, id: "m1", type: "mcp" },
      null,
      null,
      actions,
    );
    expect(server.querySelector<HTMLButtonElement>("#tab-tools")!.getAttribute("aria-selected")).toBe("true");

    const database = document.createElement("div");
    const databaseHandle = mountIntegrationDetail(database, integration, null, null, actions);
    expect(database.querySelector<HTMLButtonElement>("#tab-tools")!.getAttribute("aria-selected")).toBe("false");
    expect(database.querySelector<HTMLButtonElement>("#tab-logs")!.getAttribute("aria-selected")).toBe("true");

    await new Promise((resolve) => setTimeout(resolve, 0));
    serverHandle.destroy();
    databaseHandle.destroy();
  });

  test("an MCP server awaiting sign-in loads auth once and stays on Tools", async () => {
    let authCalls = 0;
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
      core: {
        invoke: async (cmd: string, args?: { subpath?: string }) => {
          if (cmd !== "integration_api") return undefined;
          if (args?.subpath === "/proxy/auth") {
            authCalls += 1;
            return { status: 200, body: { auth: { kind: "oauth", status: "not_connected", required: "oauth" } } };
          }
          return { status: 200, body: { tools: [] } };
        },
      },
      event: { listen: async () => () => {} },
    } as never;
    const root = document.createElement("div");
    const handle = mountIntegrationDetail(root, { ...integration, type: "mcp" }, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added" as const, path: "" }),
    });

    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(root.querySelector<HTMLButtonElement>("#tab-tools")!.getAttribute("aria-selected")).toBe("true");
    expect(authCalls).toBe(1);
    handle.destroy();
  });

  test("a signed-in MCP server with discovered tools settles on Logs", async () => {
    let authCalls = 0;
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
      core: {
        invoke: async (cmd: string, args?: { subpath?: string }) => {
          if (cmd !== "integration_api") return undefined;
          if (args?.subpath === "/proxy/auth") {
            authCalls += 1;
            return { status: 200, body: { auth: { kind: "oauth", status: "connected", required: "oauth" } } };
          }
          return {
            status: 200,
            body: { tools: [{ name: "search", label: "Search", description: "", category: "read", state: "approved", present: true, updatedAt: "" }] },
          };
        },
      },
      event: { listen: async () => () => {} },
    } as never;
    const root = document.createElement("div");
    const handle = mountIntegrationDetail(root, { ...integration, type: "mcp" }, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added" as const, path: "" }),
    });

    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(root.querySelector<HTMLButtonElement>("#tab-logs")!.getAttribute("aria-selected")).toBe("true");
    expect(authCalls).toBe(1);
    handle.destroy();
  });

  test("Overview shows adapter configuration without MCP setup", () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    }, "overview");

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
    }, "overview");

    expect(root.querySelector(".endpoint-url")).toBeNull();
    root.querySelector<HTMLButtonElement>("#tab-agentSetup")!.click();
    expect(root.querySelector(".endpoint-url")!.textContent).toBe("http://localhost:4242/mcp/tok");
    expect(root.querySelector('button[aria-label="Copy endpoint URL"]')).not.toBeNull();
    expect(root.querySelector('button[aria-label="Install into selected clients"]')).not.toBeNull();
    root.querySelector<HTMLButtonElement>("#tab-overview")!.click();
    expect(root.querySelector(".endpoint-url")).toBeNull();
  });

  test("a failed test resolves its own toast and marks the connection failing", async () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, { status: "unknown" as never, at: Date.now() }, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: false, error: "connection refused" }),
      inject: async () => ({ status: "added", path: "" }),
    }, "overview");
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
    }, "overview");

    (root.querySelector("button") as HTMLButtonElement).click();
    await new Promise((r) => setTimeout(r, 0));

    expect(toaster.querySelectorAll(".toast:not([data-exit])")).toHaveLength(1);
    expect(currentToast().dataset.variant).toBe("success");
    expect(currentToast().querySelector(".toast-title")!.textContent).toBe("Prod DB");
    expect(root.querySelector(".status-ok")).not.toBeNull();
  });
});
