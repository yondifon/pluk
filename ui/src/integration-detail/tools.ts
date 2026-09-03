import { isToolEnabled, orderedTools, settingsSummary } from "./logic";
import type { AdapterManifest, Integration } from "./types";

export function renderTools(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
): void {
  container.innerHTML = "";
  container.className = "tools-tab";

  const tools = manifest?.tools ?? [];
  const card = document.createElement("section");
  card.className = "ui-card";

  if (!tools.length) {
    const title = document.createElement("h2");
    title.className = "ui-card-title";
    title.textContent = "Tools";
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "Tool list unavailable — the local connection isn’t responding.";
    card.append(title, empty);
    container.appendChild(card);
    return;
  }

  const enabled = tools.filter((t) => isToolEnabled(t, integration.toolConfig)).length;
  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.textContent = `${enabled} of ${tools.length} tools available to the agent`;
  card.appendChild(title);

  const ordered = orderedTools(tools, integration.toolConfig);
  for (const tool of ordered) {
    const on = isToolEnabled(tool, integration.toolConfig);
    const row = document.createElement("div");
    row.className = `tool-row ${on ? "tool-on" : "tool-off"}`;
    const dot = document.createElement("span");
    dot.className = "tool-dot";
    dot.setAttribute("aria-label", on ? "Enabled" : "Disabled");
    dot.title = on ? "Available to the agent" : "Not available";
    const head = document.createElement("div");
    head.className = "tool-head";
    const name = document.createElement("code");
    name.className = "tool-name mono";
    name.textContent = tool.name;
    const category = document.createElement("span");
    category.className = "tool-category";
    category.textContent = tool.category;
    head.append(dot, name, category);
    row.appendChild(head);
    const summary = on ? settingsSummary(tool, integration.toolConfig) : "Off — enable in Edit.";
    if (summary) {
      const body = document.createElement("div");
      body.className = "tool-body";
      const s = document.createElement("div");
      s.className = "tool-summary";
      s.textContent = summary;
      body.appendChild(s);
      row.appendChild(body);
    }
    card.appendChild(row);
  }

  container.appendChild(card);
}
