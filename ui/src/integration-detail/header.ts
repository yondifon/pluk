import { deriveStatus, formatMetaLine, formatRelativeTime, statusLabel } from "./logic";
import type { AdapterManifest, ConnHealth, Integration } from "./types";

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

  const badge = document.createElement("div");
  badge.className = "type-badge";
  badge.textContent = (manifest?.label ?? integration.type).slice(0, 2).toUpperCase();
  badge.setAttribute("aria-hidden", "true");

  const stack = document.createElement("div");
  stack.className = "detail-header-stack";

  const titleRow = document.createElement("div");
  titleRow.className = "detail-title-row";

  const title = document.createElement("h1");
  title.className = "detail-title";
  title.textContent = integration.name;

  const status = deriveStatus(health ?? null);
  const chip = document.createElement("span");
  chip.className = `status-chip status-${status}`;
  const dot = document.createElement("span");
  dot.className = "status-dot";
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

  // Test button
  const testWrap = document.createElement("span");
  testWrap.className = "test-wrap";
  const testGlyph = document.createElement("span");
  testGlyph.className = "test-glyph";
  if (testState === "testing") testGlyph.textContent = "…";
  else if (testState === "ok") testGlyph.textContent = "✓";
  else if (typeof testState === "object" && testState.kind === "fail") testGlyph.textContent = "✕";
  if (testGlyph.textContent) testWrap.appendChild(testGlyph);

  const testBtn = document.createElement("button");
  testBtn.className = "btn btn-secondary btn-sm";
  testBtn.textContent = "Test";
  testBtn.disabled = testState === "testing";
  testBtn.addEventListener("click", actions.onTest);
  testWrap.appendChild(testBtn);

  // Overflow menu
  const menu = document.createElement("details");
  menu.className = "overflow-menu";
  const summary = document.createElement("summary");
  summary.textContent = "⋯";
  summary.setAttribute("aria-label", "More actions");
  menu.appendChild(summary);
  const menuList = document.createElement("div");
  menuList.className = "overflow-menu-list";
  const editBtn = document.createElement("button");
  editBtn.textContent = "Edit…";
  editBtn.addEventListener("click", () => {
    menu.removeAttribute("open");
    actions.onEdit();
  });
  const dupBtn = document.createElement("button");
  dupBtn.textContent = "Duplicate";
  dupBtn.addEventListener("click", () => {
    menu.removeAttribute("open");
    actions.onDuplicate();
  });
  const delBtn = document.createElement("button");
  delBtn.textContent = "Delete…";
  delBtn.className = "danger";
  delBtn.addEventListener("click", () => {
    menu.removeAttribute("open");
    actions.onDelete();
  });
  menuList.append(editBtn, dupBtn, document.createElement("hr"), delBtn);
  menu.appendChild(menuList);

  titleRow.append(title, chip, testWrap, menu);

  const metaRow = document.createElement("div");
  metaRow.className = "detail-meta";
  metaRow.textContent = formatMetaLine(integration, manifest ?? null);
  if (integration.readOnly) {
    const tag = document.createElement("span");
    tag.className = "tag tag-readonly";
    tag.textContent = "Read-only";
    metaRow.appendChild(tag);
  }

  stack.append(titleRow, metaRow);
  top.append(badge, stack);
  container.appendChild(top);
}
