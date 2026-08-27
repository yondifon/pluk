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
  card.className = "card";

  if (!tools.length) {
    const title = document.createElement("h2");
    title.className = "card-title";
    title.textContent = "Tools";
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "Tool list unavailable — the local service isn’t responding.";
    card.append(title, empty);
    container.appendChild(card);
    return;
  }

  const enabled = tools.filter((t) => isToolEnabled(t, integration.toolConfig)).length;
  const title = document.createElement("h2");
  title.className = "card-title";
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
    const main = document.createElement("div");
    main.className = "tool-main";
    const nameRow = document.createElement("div");
    nameRow.className = "tool-name-row";
    const name = document.createElement("code");
    name.className = "mono";
    name.textContent = tool.name;
    const cat = document.createElement("span");
    cat.className = "tool-category";
    cat.textContent = tool.category;
    nameRow.append(name, cat);
    main.appendChild(nameRow);
    if (on) {
      const summary = settingsSummary(tool, integration.toolConfig);
      if (summary) {
        const s = document.createElement("div");
        s.className = "tool-summary";
        s.textContent = summary;
        main.appendChild(s);
      }
    }
    row.append(dot, main);
    card.appendChild(row);
  }

  container.appendChild(card);
}
