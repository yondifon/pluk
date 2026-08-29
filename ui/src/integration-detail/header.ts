import { deriveStatus, formatMetaLine, formatRelativeTime, statusLabel } from "./logic";
import type { AdapterManifest, ConnHealth, Integration } from "./types";
import { createButton, createBadge, openMenu } from "../primitives";
import { typeBadge } from "../glyph";

export type HeaderActions = {
  onTest: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
};

export function renderHeader(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  health: ConnHealth | null | undefined,
  testing: boolean,
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

  const testBtn = createButton(testing ? "Testing…" : "Test", {
    variant: "secondary",
    size: "sm",
    ariaLabel: "Test connection",
    onClick: actions.onTest,
  });
  testBtn.classList.add("test-button");
  testBtn.disabled = testing;
  if (testing) testBtn.setAttribute("aria-busy", "true");

  const menu = createButton("", { icon: "more", ariaLabel: "More actions" });
  menu.classList.add("icon-button");
  menu.setAttribute("aria-haspopup", "menu");
  menu.addEventListener("click", () => openMenu(menu, [
    { label: "Edit…", icon: "edit", onSelect: actions.onEdit },
    { label: "Duplicate", icon: "copy", onSelect: actions.onDuplicate },
    { separator: true },
    { label: "Delete…", icon: "trash", danger: true, onSelect: actions.onDelete },
  ]));

  titleRow.append(title, chip, testBtn, menu);

  const metaRow = document.createElement("div");
  metaRow.className = "detail-meta";
  metaRow.textContent = formatMetaLine(integration, manifest ?? null);
  if (integration.readOnly) {
    const tag = createBadge("Read-only", "readonly");
    metaRow.appendChild(tag);
  }

  stack.append(titleRow, metaRow);

  top.append(badge, stack);
  container.appendChild(top);
}
