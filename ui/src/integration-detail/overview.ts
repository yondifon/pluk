import { mcpKey, mcpUrl, overviewRows } from "./logic";
import { renderBrowserAccess } from "./browser-access";
import { renderMcpSection, type InjectFn } from "./mcp-section";
import { WANDE_TYPE, type AdapterManifest, type Integration } from "./types";

export function renderOverview(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  deps: { inject: InjectFn },
): void {
  container.innerHTML = "";
  container.className = "overview-tab stack-lg";

  // Where the outside connects. Every integration has an MCP endpoint;
  // Wande also needs the value Chrome pairs with, so it shows both.
  const endpoint = document.createElement("section");
  renderMcpSection(
    endpoint,
    {
      key: mcpKey(integration.name, integration.environment ?? "development"),
      url: mcpUrl(integration.token),
      agentHint: manifest?.agentHint,
    },
    deps.inject,
  );
  container.appendChild(endpoint);

  if (integration.type === WANDE_TYPE) {
    const browserAccess = document.createElement("section");
    renderBrowserAccess(browserAccess);
    container.appendChild(browserAccess);
  }

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
