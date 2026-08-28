import { deriveStatus, formatMetaLine, formatRelativeTime, statusLabel } from "./logic";
import type { AdapterManifest, ConnHealth, Integration } from "./types";
import { createIcon } from "../icon";
import { createButton, createBadge, openMenu } from "../primitives";
import { humanizeHealthError } from "../health";
import { typeBadge } from "../glyph";

export type HeaderActions = {
  onTest: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
};

export type TestState = "idle" | "testing" | "ok" | { kind: "fail"; error: string };

export function renderHeader(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  health: ConnHealth | null | undefined,
  testState: TestState,
  actions: HeaderActions,
): void {
  container.innerHTML = "";
  container.className = "detail-header";

  const top = document.createElement("div");
  top.className = "detail-header-top";

  const badge = typeBadge(integration.type, manifest?.label ?? integration.type);

  const stack = document.createElement("div");
  stack.className = "detail-header-stack";

  const titleRow = document.createElement("div");
  titleRow.className = "detail-title-row";

  const title = document.createElement("h1");
  title.className = "detail-title";
  title.textContent = integration.name;
  title.title = integration.name;

  const status = deriveStatus(health ?? null);
  const chip = document.createElement("span");
  chip.className = `status-chip status-${status}`;
  chip.setAttribute("aria-label", `Status: ${statusLabel(status)}`);
  const dot = document.createElement("span");
  dot.className = "status-dot";
  dot.setAttribute("aria-hidden", "true");
  chip.appendChild(dot);
  const label = document.createElement("span");
  label.textContent = statusLabel(status);
  chip.appendChild(label);
  const rel = formatRelativeTime(health?.at);
  if (rel) {
    const ago = document.createElement("span");
    ago.className = "status-ago";
    ago.textContent = rel;
    chip.appendChild(ago);
  }
  if (health?.error) chip.title = health.error;

  const testWrap = document.createElement("span");
  testWrap.className = "test-wrap";

  const glyph = testState === "testing"
    ? createIcon("spinner")
    : testState === "ok"
      ? createIcon("check")
      : typeof testState === "object" && testState.kind === "fail"
      ? createIcon("close")
        : null;
  if (glyph) {
    glyph.classList.add("test-glyph");
    testWrap.appendChild(glyph);
  }

  const testBtn = createButton("Test", { variant: "secondary", size: "sm", ariaLabel: "Test connection", onClick: actions.onTest });
  testBtn.classList.add("test-button");
  if (testState === "testing") {
    testBtn.replaceChildren(document.createTextNode("Testing…"));
    testBtn.disabled = true;
    testBtn.setAttribute("aria-busy", "true");
  } else {
    testBtn.replaceChildren(document.createTextNode("Test"));
    testBtn.disabled = false;
    testBtn.removeAttribute("aria-busy");
  }
  testWrap.appendChild(testBtn);

  const menu = createButton("", { icon: "more", ariaLabel: "More actions" });
  menu.classList.add("icon-button");
  menu.setAttribute("aria-haspopup", "menu");
  menu.addEventListener("click", () => openMenu(menu, [
    { label: "Edit…", icon: "edit", onSelect: actions.onEdit },
    { label: "Duplicate", icon: "copy", onSelect: actions.onDuplicate },
    { separator: true },
    { label: "Delete…", icon: "trash", danger: true, onSelect: actions.onDelete },
  ]));

  titleRow.append(title, chip, testWrap, menu);

  const metaRow = document.createElement("div");
  metaRow.className = "detail-meta";
  metaRow.textContent = formatMetaLine(integration, manifest ?? null);
  if (integration.readOnly) {
    const tag = createBadge("Read-only", "readonly");
    metaRow.appendChild(tag);
  }

  stack.append(titleRow, metaRow);

  if (testState !== "idle") {
    const live = document.createElement("div");
    live.className = `test-result test-result-${testState === "testing" ? "testing" : testState === "ok" ? "ok" : "fail"}`;
    live.setAttribute("role", "status");
    live.setAttribute("aria-live", "polite");
    live.setAttribute("aria-atomic", "true");
    if (testState === "testing") live.textContent = "Testing connection…";
    else if (testState === "ok") live.textContent = "Connected — your integration is working.";
    else live.textContent = humanizeHealthError(testState.error);
    stack.appendChild(live);
  }

  top.append(badge, stack);
  container.appendChild(top);
}
