import { mcpUrl, overviewRows } from "./logic";
import type { AdapterManifest, Integration } from "./types";
import { createButton } from "../primitives";

export function renderOverview(
  container: HTMLElement,
  integration: Integration,
  manifest: AdapterManifest | null | undefined,
  onCopyConfirm: (copied: boolean) => void,
): void {
  container.innerHTML = "";
  container.className = "overview-tab stack-lg";

  // Endpoint card
  const endpoint = document.createElement("section");
   endpoint.className = "ui-card";
  const epTitle = document.createElement("h2");
   epTitle.className = "ui-card-title";
  epTitle.textContent = "MCP endpoint";
  const epRow = document.createElement("div");
  epRow.className = "inspector-row";
  const epLabel = document.createElement("span");
  epLabel.className = "inspector-label";
  epLabel.textContent = "URL";
  const url = mcpUrl(integration.token);
  const urlText = document.createElement("code");
  urlText.className = "mono";
  urlText.textContent = url;
  urlText.title = url;
  const copyBtn = createButton("Copy", { variant: "primary", size: "sm", ariaLabel: "Copy endpoint URL" });
  const live = document.createElement("span");
  live.className = "sr-only";
  live.setAttribute("role", "status");
  live.setAttribute("aria-live", "polite");
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;
  copyBtn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      // fallback
      const ta = document.createElement("textarea");
      ta.value = url;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
     copyBtn.replaceChildren(document.createTextNode("Copied!"));
     live.textContent = "Endpoint URL copied.";
    copyBtn.classList.add("copied");
    onCopyConfirm(true);
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
       copyBtn.replaceChildren(document.createTextNode("Copy"));
       copyBtn.classList.remove("copied");
       onCopyConfirm(false);
    }, 1500);
  });
   epRow.append(epLabel, urlText, copyBtn, live);
  endpoint.append(epTitle, epRow);
  if (manifest?.agentHint) {
    const hintRow = document.createElement("div");
    hintRow.className = "inspector-row";
    const hintLabel = document.createElement("span");
    hintLabel.className = "inspector-label";
    hintLabel.textContent = "Agent hint";
    const hintVal = document.createElement("span");
    hintVal.className = "hint";
    hintVal.textContent = manifest.agentHint;
    hintRow.append(hintLabel, hintVal);
    endpoint.appendChild(hintRow);
  }

  // Configuration card
  const config = document.createElement("section");
   config.className = "ui-card";
  const cfgTitle = document.createElement("h2");
   cfgTitle.className = "ui-card-title";
  cfgTitle.textContent = "Configuration";
  config.appendChild(cfgTitle);

  const rows = overviewRows(integration, manifest ?? null);
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

  container.append(endpoint, config);
}
