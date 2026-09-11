import { mcpKey, mcpUrl } from "./logic";
import { renderMcpSection, type InjectFn } from "./mcp-section";
import { renderPlukIdCard } from "./browser-access";
import { WANDE_TYPE, type AdapterManifest, type Integration, type McpClientId } from "./types";
import { wizardStepFooter, wizardStepHeader } from "../primitives";

export function renderAgentSetup(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  inject: InjectFn,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "agent-setup-tab";

  let browserAccess: { destroy: () => void } | null = null;
  if (integration.type === WANDE_TYPE) {
    const browser = document.createElement("section");
    browserAccess = renderPlukIdCard(browser, integration.id);
    container.appendChild(browser);
  }

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

  return {
    destroy() {
      browserAccess?.destroy();
      container.innerHTML = "";
    },
  };
}

/** The wizard's last screen: the same install controls, reached once the integration is saved. */
export function renderInstallStep(
  integration: { name: string; environment?: string | null; token: string },
  manifest: AdapterManifest | null | undefined,
  inject: InjectFn,
  stepIndex: number,
  totalSteps: number,
  handlers: { onBack: (() => void) | null; onDone: () => void },
  opts?: { installed?: McpClientId[] },
): HTMLElement {
  const wrap = wizardStepHeader(
    stepIndex,
    totalSteps,
    "Install into your agent",
    `Add this to the AI tool you use, so it can reach ${manifest?.label ?? "it"}.`,
  );
  const body = document.createElement("div");
  body.className = "wizard-body";
  const endpoint = document.createElement("section");
  renderMcpSection(
    endpoint,
    {
      key: mcpKey(integration.name, integration.environment ?? "development"),
      url: mcpUrl(integration.token),
      agentHint: manifest?.agentHint,
      title: "Endpoint",
    },
    inject,
    opts,
  );
  body.appendChild(endpoint);
  wrap.appendChild(body);

  const { el: footer } = wizardStepFooter({
    onBack: handlers.onBack,
    onCancel: null,
    primaryLabel: "Done",
    onPrimary: handlers.onDone,
  });
  wrap.appendChild(footer);
  return wrap;
}
