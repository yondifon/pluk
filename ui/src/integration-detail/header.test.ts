import { describe, test, expect } from "bun:test";
import { renderHeader, type TestState } from "./header";
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

function render(state: TestState, health: unknown = null) {
  const container = document.createElement("div");
  renderHeader(container, baseIntegration, { id: "postgres", label: "PostgreSQL", category: "database", agentHint: "", tools: [], configFields: [] }, health as never, state, {
    onTest: () => {},
    onEdit: () => {},
    onDuplicate: () => {},
    onDelete: () => {},
  });
  return container;
}

describe("header Test control", () => {
  test("renders Test button with accessible name", () => {
    const c = render("idle");
    const btn = c.querySelector("button");
    expect(btn).not.toBeNull();
    expect(btn!.textContent).toBe("Test");
    expect(btn!.getAttribute("aria-label")).toBe("Test connection");
    expect(btn!.type).toBe("button");
  });

  test("testing state disables button and shows live region", () => {
    const c = render("testing");
    const btn = c.querySelector("button")!;
    expect(btn.disabled).toBe(true);
    expect(btn.getAttribute("aria-busy")).toBe("true");
    expect(btn.textContent).toBe("Testing…");
    const live = c.querySelector("[role='status']");
    expect(live).not.toBeNull();
    expect(live!.textContent).toContain("Testing connection");
    expect(live!.getAttribute("aria-live")).toBe("polite");
  });

  test("ok state shows success message and check icon", () => {
    const c = render("ok");
    const live = c.querySelector("[role='status']");
    expect(live!.textContent).toContain("Connected");
    expect(live!.className).toContain("test-result-ok");
    const glyph = c.querySelector(".test-glyph")!;
    expect(glyph.getAttribute("data-icon")).toBe("check");
    expect(glyph.getAttribute("aria-hidden")).toBe("true");
  });

  test("fail state shows humanized error with next step", () => {
    const c = render({ kind: "fail", error: "connection refused" });
    const live = c.querySelector("[role='status']")!;
    expect(live.textContent).toContain("Couldn’t connect");
    expect(live.textContent).toContain("try again");
    expect(live.className).toContain("test-result-fail");
  });

  test("fail with raw message still includes next step", () => {
    const c = render({ kind: "fail", error: "something broke" });
    const live = c.querySelector("[role='status']")!;
    expect(live.textContent).toContain("something broke");
    expect(live.textContent!.toLowerCase()).toContain("try again");
  });

  test("idle has no live region", () => {
    const c = render("idle");
    expect(c.querySelector("[role='status']")).toBeNull();
  });

  test("health chip reflects status with accessible label", () => {
    const c = render("idle", { status: "error", error: "refused", at: Date.now() });
    const chip = c.querySelector(".status-chip")!;
    expect(chip.getAttribute("aria-label")).toContain("Failing");
    const c2 = render("idle", { status: "ok", at: Date.now() });
    expect(c2.querySelector(".status-chip")!.getAttribute("aria-label")).toContain("Healthy");
  });

  test("button is keyboard reachable (tabbable)", () => {
    const c = render("idle");
    const btn = c.querySelector("button")!;
    expect(btn.tabIndex).not.toBe(-1);
    // disabled testing button not reachable, idle is
    expect(btn.disabled).toBe(false);
  });
});
