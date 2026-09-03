import { describe, test, expect, beforeEach } from "bun:test";
import { renderGroupDetail } from "./groupDetail";

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
