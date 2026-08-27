import { renderHeader, type TestState } from "./header";
import { renderOverview } from "./overview";
import { renderTools } from "./tools";
import { renderClientConfig, type InjectFn } from "./client-config";
import { renderTabs, type TabId } from "./tabs";
import { mountActivityLog } from "../activityLog/activityLog";
import type { AdapterManifest, ConnHealth, Integration } from "./types";

export type DetailActions = {
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onTest: () => Promise<{ ok: boolean; error?: string }>;
  inject: InjectFn;
  onCopyEndpoint?: (copied: boolean) => void;
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
  contentEl.className = "detail-content";
  root.append(headerEl, tabsEl, contentEl);

  let currentHealth: ConnHealth | null | undefined = health ?? null;
  let selectedTab: TabId = "logs";
  let testState: TestState = "idle";
  let resetTimer: ReturnType<typeof setTimeout> | null = null;
  const logsMount = document.createElement("div");
  logsMount.className = "logs-mount";
  let logs: { destroy: () => void } | null = null;

  function render() {
    renderHeader(headerEl, integration, manifest ?? null, currentHealth ?? null, testState, {
      onTest: async () => {
        if (resetTimer) clearTimeout(resetTimer);
        testState = "testing";
        render();
        try {
          const res = await actions.onTest();
          testState = res.ok ? "ok" : { kind: "fail", error: res.error ?? "Unknown error" };
          currentHealth = res.ok
            ? { status: "ok", at: Date.now() }
            : { status: "error", error: res.error ?? "Unknown error", at: Date.now() };
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e);
          testState = { kind: "fail", error: msg };
          currentHealth = { status: "error", error: msg, at: Date.now() };
        }
        render();
        const delay = testState === "ok" ? 3000 : 5000;
        resetTimer = setTimeout(() => {
          testState = "idle";
          render();
        }, delay);
      },
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
      contentEl.appendChild(logsMount);
      logs ??= mountActivityLog(logsMount, {
        scope: { connectionId: integration.id },
        connectionTypes: new Map([[integration.id, integration.type]]),
      });
    } else if (selectedTab === "overview") {
      const overviewWrap = document.createElement("div");
      const clientWrap = document.createElement("div");
      renderOverview(overviewWrap, integration, manifest ?? null, (copied) => actions.onCopyEndpoint?.(copied));
      renderClientConfig(clientWrap, integration, actions.inject);
      contentEl.append(overviewWrap, clientWrap);
    } else {
      renderTools(contentEl, integration, manifest ?? null);
    }
  }

  render();

  return {
    updateHealth(next: ConnHealth | null | undefined) {
      currentHealth = next ?? null;
      render();
    },
    destroy() {
      if (resetTimer) clearTimeout(resetTimer);
      logs?.destroy();
      logs = null;
      root.innerHTML = "";
    },
  };
}

// Re-export pure logic for tests and external use
export * from "./logic";
export * from "./types";
