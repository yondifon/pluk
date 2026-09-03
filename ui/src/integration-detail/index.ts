import { renderHeader } from "./header";
import { renderOverview } from "./overview";
import { renderTools } from "./tools";
import { type InjectFn } from "./mcp-section";
import { renderTabs, type TabId } from "./tabs";
import { mountActivityLog } from "../activityLog/activityLog";
import { humanizeHealthError } from "../health";
import { toast, type PendingToast } from "../toast";
import type { AdapterManifest, ConnHealth, Integration } from "./types";

export type DetailActions = {
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onTest: () => Promise<{ ok: boolean; error?: string }>;
  inject: InjectFn;
};

export function mountIntegrationDetail(
  root: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  health: ConnHealth | null | undefined,
  actions: DetailActions,
): { destroy: () => void; updateHealth: (next: ConnHealth | null | undefined) => void } {
  root.innerHTML = "";
  root.className = "integration-detail";

  const headerEl = document.createElement("div");
  const tabsEl = document.createElement("div");
  const contentEl = document.createElement("div");
  contentEl.className = "detail-content stack-lg";
  root.append(headerEl, tabsEl, contentEl);

  let currentHealth: ConnHealth | null | undefined = health ?? null;
  let selectedTab: TabId = "logs";
  let testing = false;
  const logsMount = document.createElement("div");
  logsMount.className = "logs-mount";
  let logs: { destroy: () => void } | null = null;

  function reportTestFailure(error: string, pending: PendingToast): void {
    currentHealth = { status: "error", error, at: Date.now() };
    const description = humanizeHealthError(error);
    pending.error(integration.name, {
      description,
      detail: description.includes(error.trim()) ? undefined : error,
      action: { label: "Try again", onClick: () => void runTest() },
    });
  }

  async function runTest(): Promise<void> {
    if (testing) return;
    testing = true;
    render();
    const pending = toast.pending(integration.name, { description: "Testing connection…" });
    try {
      const result = await actions.onTest();
      if (result.ok) {
        currentHealth = { status: "ok", at: Date.now() };
        pending.success(integration.name, { description: "Connected." });
      } else {
        reportTestFailure(result.error ?? "Unknown error", pending);
      }
    } catch (e) {
      reportTestFailure(e instanceof Error ? e.message : String(e), pending);
    }
    testing = false;
    render();
  }

  function render() {
    renderHeader(headerEl, integration, manifest ?? null, currentHealth ?? null, testing, {
      onTest: () => void runTest(),
      onEdit: actions.onEdit,
      onDuplicate: actions.onDuplicate,
      onDelete: actions.onDelete,
    });

    renderTabs(tabsEl, selectedTab, (id) => {
      selectedTab = id;
      render();
    });

    contentEl.innerHTML = "";
    if (selectedTab === "logs") {
      const panel = document.createElement("div");
      panel.setAttribute("role", "tabpanel");
       panel.setAttribute("aria-labelledby", "tab-logs");
      panel.appendChild(logsMount);
      contentEl.appendChild(panel);
      logs ??= mountActivityLog(logsMount, {
        scope: { connectionId: integration.id },
        connectionTypes: new Map([[integration.id, integration.type]]),
      });
    } else if (selectedTab === "overview") {
      const overviewWrap = document.createElement("div");
      overviewWrap.setAttribute("role", "tabpanel");
      overviewWrap.setAttribute("aria-labelledby", "tab-overview");
      renderOverview(overviewWrap, integration, manifest ?? null, { inject: actions.inject });
      contentEl.appendChild(overviewWrap);
    } else {
      const panel = document.createElement("div");
      panel.setAttribute("role", "tabpanel");
       panel.setAttribute("aria-labelledby", "tab-tools");
      renderTools(panel, integration, manifest ?? null);
      contentEl.appendChild(panel);
    }
  }

  render();

  return {
    updateHealth(next: ConnHealth | null | undefined) {
      currentHealth = next ?? null;
      render();
    },
    destroy() {
      logs?.destroy();
      logs = null;
      root.innerHTML = "";
    },
  };
}

// Re-export pure logic for tests and external use
export * from "./logic";
export * from "./types";
