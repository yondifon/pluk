import { mcpKey, mcpUrl, overviewRows } from "./logic";
import { renderMcpSection, type InjectFn } from "./mcp-section";
import type { AdapterManifest, Integration } from "./types";

export function renderOverview(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  deps: { inject: InjectFn },
): void {
  container.innerHTML = "";
  container.className = "overview-tab stack-lg";

  const mcp = document.createElement("section");
  renderMcpSection(
    mcp,
    {
      key: mcpKey(integration.name, integration.environment ?? "development"),
      url: mcpUrl(integration.token),
      agentHint: manifest?.agentHint,
    },
    deps.inject,
  );

  const config = document.createElement("section");
  config.className = "ui-card";
  const cfgTitle = document.createElement("h2");
  cfgTitle.className = "ui-card-title";
  cfgTitle.textContent = "Configuration";
  config.appendChild(cfgTitle);

  for (const [label, value] of overviewRows(integration, manifest ?? null)) {
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

  container.append(mcp, config);
}
