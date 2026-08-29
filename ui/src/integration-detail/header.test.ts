import { describe, test, expect } from "bun:test";
import { renderHeader } from "./header";
import type { Integration } from "./types";

const baseIntegration: Integration = {
  id: "1",
  name: "Prod DB",
  type: "postgres",
  config: {},
  toolConfig: {},
  token: "tok",
  createdAt: "",
};

function render(testing: boolean, health: unknown = null) {
  const container = document.createElement("div");
  renderHeader(container, baseIntegration, { id: "postgres", label: "PostgreSQL", category: "database", agentHint: "", tools: [], configFields: [] }, health as never, testing, {
    onTest: () => {},
    onEdit: () => {},
    onDuplicate: () => {},
    onDelete: () => {},
  });
  return container;
}

describe("header Test control", () => {
  test("renders Test button with accessible name", () => {
    const c = render(false);
    const btn = c.querySelector("button");
    expect(btn).not.toBeNull();
    expect(btn!.textContent).toBe("Test");
    expect(btn!.getAttribute("aria-label")).toBe("Test connection");
    expect(btn!.type).toBe("button");
  });

  test("testing state disables the button and says so", () => {
    const c = render(true);
    const btn = c.querySelector("button")!;
    expect(btn.disabled).toBe(true);
    expect(btn.getAttribute("aria-busy")).toBe("true");
    expect(btn.textContent).toBe("Testing…");
  });

  // The result belongs to the toast now, so the header never grows a result panel.
  test("the header reports no result of its own", () => {
    expect(render(false).querySelector("[role='status']")).toBeNull();
    expect(render(true).querySelector("[role='status']")).toBeNull();
  });

  test("health chip reflects status with accessible label", () => {
    const c = render(false, { status: "error", error: "refused", at: Date.now() });
    const chip = c.querySelector(".status-chip")!;
    expect(chip.getAttribute("aria-label")).toContain("Failing");
    const c2 = render(false, { status: "ok", at: Date.now() });
    expect(c2.querySelector(".status-chip")!.getAttribute("aria-label")).toContain("Healthy");
  });

  test("button is keyboard reachable (tabbable)", () => {
    const c = render(false);
    const btn = c.querySelector("button")!;
    expect(btn.tabIndex).not.toBe(-1);
    // disabled testing button not reachable, idle is
    expect(btn.disabled).toBe(false);
  });
});
