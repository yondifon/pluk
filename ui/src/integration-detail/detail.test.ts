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
    });
    expect(root.querySelector(".status-unknown")).not.toBeNull();
    handle.updateHealth({ status: "ok", at: Date.now() });
    expect(root.querySelector(".status-ok")).not.toBeNull();
    expect(root.querySelector(".status-unknown")).toBeNull();
    handle.updateHealth({ status: "error", error: "refused", at: Date.now() });
    expect(root.querySelector(".status-failing")).not.toBeNull();
    handle.destroy();
  });

  test("overview shows one MCP section holding the endpoint and Install", () => {
    const root = document.createElement("div");
    mountIntegrationDetail(root, integration, null, null, {
      onEdit: () => {},
      onDuplicate: () => {},
      onDelete: () => {},
      onTest: async () => ({ ok: true }),
      inject: async () => ({ status: "added", path: "" }),
    });
    root.querySelector<HTMLButtonElement>("#tab-overview")!.click();

    const cards = [...root.querySelectorAll(".ui-card")];
    expect(cards.map((c) => c.querySelector(".ui-card-title")!.textContent)).toEqual([
      "MCP endpoint",
      "Configuration",
    ]);
    expect(cards[0].querySelector('button[aria-label="Copy endpoint URL"]')).not.toBeNull();
    expect(cards[0].querySelector('button[aria-label="Install into selected clients"]')).not.toBeNull();
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
