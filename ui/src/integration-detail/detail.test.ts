import { describe, test, expect, beforeEach } from "bun:test";
import { mountIntegrationDetail } from "./index";
import type { Integration } from "./types";

beforeEach(() => {
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: async () => 30 },
    event: { listen: async () => () => {} },
  } as never;
});

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

  test("test updates health optimistically", async () => {
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
    await new Promise((r) => setTimeout(r, 10));
    // after click, testing then fail state should show error with health failing
    // wait for async handler
    await new Promise((r) => setTimeout(r, 20));
    const live = root.querySelector("[role='status']");
    expect(live).not.toBeNull();
  });
});
