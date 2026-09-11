import { mcpKey, mcpUrl } from "./logic";
import { renderMcpSection, type InjectFn } from "./mcp-section";
import { WANDE_TYPE, type AdapterManifest, type Integration } from "./types";

export function renderAgentSetup(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  inject: InjectFn,
): void {
  container.innerHTML = "";
  container.className = "agent-setup-tab";

  const endpoint = document.createElement("section");
  renderMcpSection(
    endpoint,
    {
      key: mcpKey(integration.name, integration.environment ?? "development"),
      url: mcpUrl(integration.token),
      agentHint: manifest?.agentHint,
      title: integration.type === WANDE_TYPE ? "Agent setup" : undefined,
    },
    inject,
  );
  container.appendChild(endpoint);
}
