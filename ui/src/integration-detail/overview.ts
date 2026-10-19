import { overviewRows } from "./logic";
import type { AdapterManifest, Integration } from "./types";

export function renderOverview(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
): void {
  container.innerHTML = "";
  container.className = "overview-tab stack-lg";

  const rows = overviewRows(integration, manifest ?? null);
  if (!rows.length) return;

  const config = document.createElement("section");
  config.className = "ui-card";
  const cfgTitle = document.createElement("h2");
  cfgTitle.className = "ui-card-title";
  cfgTitle.textContent = "Configuration";
  config.appendChild(cfgTitle);

  for (const [label, value] of rows) {
    const row = document.createElement("div");
    row.className = "inspector-row";
    const l = document.createElement("span");
    l.className = "inspector-label";
    l.textContent = label;
    const v = document.createElement("span");
    v.className = "mono";
    v.textContent = value;
    row.append(l, v);
    config.appendChild(row);
  }
  container.appendChild(config);
}
