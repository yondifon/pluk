import { overviewRows } from "./logic";
import { renderBrowserAccess } from "./browser-access";
import { mountWandePosts } from "./wande-posts";
import { WANDE_TYPE, type AdapterManifest, type Integration } from "./types";

export function renderOverview(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "overview-tab stack-lg";

  let browserAccess: { destroy: () => void } | null = null;
  let posts: { destroy: () => void } | null = null;
  if (integration.type === WANDE_TYPE) {
    const browser = document.createElement("section");
    browserAccess = renderBrowserAccess(browser);
    container.appendChild(browser);

    const outbox = document.createElement("div");
    posts = mountWandePosts(outbox);
    container.appendChild(outbox);
  }

  if (integration.type !== WANDE_TYPE) {
    const rows = overviewRows(integration, manifest ?? null);
    if (rows.length) {
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
  }

  return {
    destroy() {
      browserAccess?.destroy();
      posts?.destroy();
    },
  };
}
