import { deriveStatus, formatMetaLine, formatRelativeTime, statusLabel } from "./logic";
import type { AdapterManifest, ConnHealth, Integration } from "./types";

export type HeaderActions = {
  onTest: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
};

export type TestState = "idle" | "testing" | "ok" | { kind: "fail"; error: string };

function humanize(raw: string): string {
  if (!raw) return "Connection failed. Check the setup and try again.";
  const low = raw.toLowerCase();
  let msg: string;
  if (low.includes("refused") || low.includes("connection")) msg = "Couldn’t connect. Check that the service is reachable and try again.";
  else if (low.includes("auth") || low.includes("unauthorized") || low.includes("forbidden")) msg = "Authentication failed. Check the credentials and try again.";
  else if (low.includes("timeout")) msg = "Connection timed out. Check the network and try again.";
  else if (low.includes("tunnel") || low.includes("ssh")) msg = "Secure tunnel failed. Check SSH settings and try again.";
  else msg = raw.trim();
  if (!msg.toLowerCase().includes("try again")) msg = msg.replace(/\.?$/, ".") + " Check the setup and try again.";
  return msg;
}

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

  const glyph = document.createElement("span");
  glyph.className = "test-glyph";
  glyph.setAttribute("aria-hidden", "true");
  if (testState === "testing") glyph.textContent = "…";
  else if (testState === "ok") glyph.textContent = "✓";
  else if (typeof testState === "object" && testState.kind === "fail") glyph.textContent = "✕";
  if (glyph.textContent) testWrap.appendChild(glyph);

  const testBtn = document.createElement("button");
  testBtn.type = "button";
  testBtn.className = "btn btn-secondary btn-sm";
  testBtn.setAttribute("aria-label", "Test connection");
  if (testState === "testing") {
    testBtn.textContent = "Testing…";
    testBtn.disabled = true;
    testBtn.setAttribute("aria-busy", "true");
  } else {
    testBtn.textContent = "Test";
    testBtn.disabled = false;
    testBtn.removeAttribute("aria-busy");
  }
  testBtn.addEventListener("click", actions.onTest);
  testWrap.appendChild(testBtn);

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

  if (testState !== "idle") {
    const live = document.createElement("div");
    live.className = `test-result test-result-${testState === "testing" ? "testing" : testState === "ok" ? "ok" : "fail"}`;
    live.setAttribute("role", "status");
    live.setAttribute("aria-live", "polite");
    live.setAttribute("aria-atomic", "true");
    if (testState === "testing") live.textContent = "Testing connection…";
    else if (testState === "ok") live.textContent = "Connected — your integration is working.";
    else live.textContent = humanize(testState.error);
    stack.appendChild(live);
  }

  top.append(badge, stack);
  container.appendChild(top);
}
