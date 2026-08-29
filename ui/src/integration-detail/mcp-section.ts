import { formatFanOutMessage } from "./logic";
import type { ConfigScope, FanOutResult, McpClientId } from "./types";
import { createButton } from "../primitives";
import { hasHost, invoke, pickDirectory } from "../host";

const CLIENTS: Array<{ id: McpClientId; label: string; supportsProject: boolean; globalPath: string; projectPath: string | null; language: string }> = [
  { id: "opencode", label: "OpenCode", supportsProject: true, globalPath: "~/.config/opencode/opencode.json", projectPath: "opencode.json", language: "json" },
  { id: "codex", label: "Codex", supportsProject: false, globalPath: "~/.codex/config.toml", projectPath: null, language: "toml" },
  { id: "claudeCode", label: "Claude Code", supportsProject: true, globalPath: "~/.mcp.json", projectPath: ".mcp.json", language: "json" },
  { id: "cursor", label: "Cursor", supportsProject: true, globalPath: "~/.cursor/mcp.json", projectPath: ".cursor/mcp.json", language: "json" },
  { id: "windsurf", label: "Windsurf", supportsProject: false, globalPath: "~/.codeium/windsurf/mcp_config.json", projectPath: null, language: "json" },
  { id: "antigravity", label: "Antigravity", supportsProject: false, globalPath: "~/.gemini/config/mcp_config.json", projectPath: null, language: "json" },
];

export type InjectFn = (args: { client: McpClientId; scope: ConfigScope; projectDir: string | null; key: string; url: string }) => Promise<{ status: "added" | "skipped"; path: string }>;

/**
 * What the section is about: the endpoint agents call, the server name they
 * will see it under, and any adapter advice that belongs beside it.
 */
export type McpSectionSpec = {
  key: string;
  url: string;
  agentHint?: string | null;
  onCopyConfirm?: (copied: boolean) => void;
};

const TITLE_ID = "mcp-section-title";

function snippetFor(client: McpClientId, key: string, url: string): string {
  switch (client) {
    case "opencode":
      return `{\n  "mcp": {\n    "${key}": {\n      "type": "remote",\n      "enabled": true,\n      "url": "${url}",\n      "oauth": false\n    }\n  }\n}`;
    case "codex":
      return `[mcp_servers.${key}]\nurl = "${url}"`;
    case "claudeCode":
      return `{\n  "mcpServers": {\n    "${key}": {\n      "type": "http",\n      "url": "${url}"\n    }\n  }\n}`;
    case "cursor":
      return `{\n  "mcpServers": {\n    "${key}": {\n      "command": "bunx",\n      "args": ["mcp-remote", "${url}"]\n    }\n  }\n}`;
    case "windsurf":
    case "antigravity":
      return `{\n  "mcpServers": {\n    "${key}": {\n      "serverUrl": "${url}"\n    }\n  }\n}`;
  }
}

function configPathFor(client: McpClientId, scope: ConfigScope): string {
  const c = CLIENTS.find((x) => x.id === client)!;
  if (scope === "project" && c.projectPath) return c.projectPath;
  return c.globalPath;
}

function endpointRow({ url, onCopyConfirm }: McpSectionSpec): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const label = document.createElement("span");
  label.className = "inspector-label";
  label.textContent = "URL";
  const urlText = document.createElement("code");
  urlText.className = "mono";
  urlText.textContent = url;
  urlText.title = url;
  const copyBtn = createButton("Copy", { variant: "primary", size: "sm", ariaLabel: "Copy endpoint URL" });
  const live = document.createElement("span");
  live.className = "sr-only";
  live.setAttribute("role", "status");
  live.setAttribute("aria-live", "polite");

  let resetTimer: ReturnType<typeof setTimeout> | null = null;
  copyBtn.addEventListener("click", async () => {
    await copyText(url);
    copyBtn.replaceChildren(document.createTextNode("Copied!"));
    copyBtn.classList.add("copied");
    live.textContent = "Endpoint URL copied.";
    onCopyConfirm?.(true);
    if (resetTimer) clearTimeout(resetTimer);
    resetTimer = setTimeout(() => {
      copyBtn.replaceChildren(document.createTextNode("Copy"));
      copyBtn.classList.remove("copied");
      onCopyConfirm?.(false);
    }, 1500);
  });

  row.append(label, urlText, copyBtn, live);
  return row;
}

function agentHintRow(hint: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const label = document.createElement("span");
  label.className = "inspector-label";
  label.textContent = "Agent hint";
  const value = document.createElement("span");
  value.className = "hint";
  value.textContent = hint;
  row.append(label, value);
  return row;
}

/** Host errors arrive as plain strings, not Error objects. */
function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}

/**
 * The endpoint and the install controls are one job — pointing an agent at this
 * server — so they share one card and one heading.
 */
export function renderMcpSection(
  container: HTMLElement,
  target: McpSectionSpec,
  inject: InjectFn,
  opts?: { installed?: McpClientId[]; pickProjectDir?: (title: string) => Promise<string | null> },
): void {
  container.innerHTML = "";
  container.className = "ui-card";
  container.setAttribute("aria-labelledby", TITLE_ID);
  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.textContent = "MCP endpoint";
  title.id = TITLE_ID;
  container.append(title, endpointRow(target));
  if (target.agentHint) container.appendChild(agentHintRow(target.agentHint));

  let selectedClient: McpClientId | "all" = "all";
  let selectedScope: ConfigScope = "project";
  let installed: McpClientId[] = opts?.installed ?? (CLIENTS.map((c) => c.id) as McpClientId[]);
  const chooseDir = opts?.pickProjectDir ?? pickDirectory;

  const controls = document.createElement("div");
  controls.className = "client-controls";
  controls.setAttribute("role", "group");
  controls.setAttribute("aria-labelledby", TITLE_ID);

  const clientLabel = document.createElement("label");
  clientLabel.textContent = "Client";
  clientLabel.htmlFor = "pluk-client-select";
  const clientSelect = document.createElement("select");
  clientSelect.className = "ui-select";
  clientSelect.id = "pluk-client-select";
  clientSelect.setAttribute("aria-label", "AI client");
  const allOpt = document.createElement("option");
  allOpt.value = "all";
  allOpt.textContent = "All detected";
  clientSelect.appendChild(allOpt);
  for (const c of CLIENTS) {
    const o = document.createElement("option");
    o.value = c.id;
    o.textContent = c.label;
    clientSelect.appendChild(o);
  }
  clientSelect.value = selectedClient;

  const scopeLabel = document.createElement("label");
  scopeLabel.textContent = "Scope";
  scopeLabel.htmlFor = "pluk-scope-select";
  scopeLabel.id = "pluk-scope-label";
  const scopeSelect = document.createElement("select");
  scopeSelect.className = "ui-select";
  scopeSelect.id = "pluk-scope-select";
  scopeSelect.setAttribute("aria-labelledby", "pluk-scope-label");
  for (const s of ["project", "global"] as const) {
    const o = document.createElement("option");
    o.value = s;
    o.textContent = s === "project" ? "Project" : "Global";
    scopeSelect.appendChild(o);
  }
  scopeSelect.value = selectedScope;

  function targets(): McpClientId[] {
    if (selectedClient !== "all") return [selectedClient as McpClientId];
    return installed.filter((id) => {
      const c = CLIENTS.find((x) => x.id === id)!;
      return selectedScope === "global" || c.supportsProject;
    });
  }

  const addBtn = createButton("Install", { variant: "secondary", size: "sm", ariaLabel: "Install into selected clients" });
  const copyBtn = createButton("Copy", { variant: "secondary", size: "sm", ariaLabel: "Copy snippet to clipboard" });
  const snippetPre = document.createElement("pre");
  snippetPre.className = "snippet";
  snippetPre.tabIndex = 0;
  snippetPre.setAttribute("aria-label", "Configuration snippet");
  const hint = document.createElement("div");
  hint.className = "hint";
  hint.id = "pluk-config-hint";
  snippetPre.setAttribute("aria-describedby", "pluk-config-hint");
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.setAttribute("role", "status");
  toast.setAttribute("aria-live", "polite");
  toast.setAttribute("aria-atomic", "true");
  const outcome = document.createElement("div");
  outcome.className = "install-outcome";

  function syncScopeOptions() {
    const single = selectedClient !== "all" ? CLIENTS.find((x) => x.id === selectedClient) : null;
    if (single && !single.supportsProject && selectedScope === "project") {
      selectedScope = "global";
      scopeSelect.value = "global";
    }
    const hideScope = Boolean(single && !single.supportsProject);
    scopeSelect.style.display = hideScope ? "none" : "";
    scopeLabel.style.display = hideScope ? "none" : "";
    scopeSelect.disabled = hideScope;
    scopeSelect.setAttribute("aria-hidden", hideScope ? "true" : "false");
    if (hideScope) scopeSelect.setAttribute("tabindex", "-1");
    else scopeSelect.removeAttribute("tabindex");
    if (selectedClient === "all") {
      scopeSelect.style.display = "";
      scopeLabel.style.display = "";
      scopeSelect.disabled = false;
      scopeSelect.setAttribute("aria-hidden", "false");
      scopeSelect.removeAttribute("tabindex");
    }
    addBtn.setAttribute("aria-disabled", targets().length === 0 ? "true" : "false");
  }

  function renderPreview() {
    const t = targets();
    addBtn.disabled = t.length === 0;
    copyBtn.style.display = selectedClient === "all" ? "none" : "";
    if (selectedClient === "all") {
      const list = document.createElement("div");
      list.className = "all-target-list";
      if (t.length === 0) {
        list.textContent = "No MCP client found. Paste the snippet manually.";
      } else {
        for (const id of t) {
          const row = document.createElement("div");
          row.className = "target-row";
          const label = CLIENTS.find((x) => x.id === id)!.label;
          row.textContent = `${label} — ${configPathFor(id, selectedScope)}`;
          list.appendChild(row);
        }
      }
      snippetPre.replaceChildren(list);
      hint.textContent = "";
    } else {
      const id = selectedClient as McpClientId;
      snippetPre.textContent = snippetFor(id, target.key, target.url);
      hint.textContent = `Install to ${configPathFor(id, selectedScope)}`;
    }
  }

  clientSelect.addEventListener("change", () => {
    selectedClient = clientSelect.value as McpClientId | "all";
    syncScopeOptions();
    renderPreview();
  });
  scopeSelect.addEventListener("change", () => {
    selectedScope = scopeSelect.value as ConfigScope;
    renderPreview();
  });

  controls.append(clientLabel, clientSelect, scopeLabel, scopeSelect, addBtn, copyBtn);
  container.append(controls, hint, snippetPre, toast, outcome);

  if (!opts?.installed && hasHost()) {
    invoke<McpClientId[]>("list_installed_mcp_clients")
      .then((res) => {
        if (Array.isArray(res) && res.length) {
          installed = res;
          syncScopeOptions();
          renderPreview();
        }
      })
      .catch(() => {});
  }

  copyBtn.addEventListener("click", async () => {
    if (selectedClient === "all") return;
    await copyText(snippetFor(selectedClient as McpClientId, target.key, target.url));
    const prev = copyBtn.textContent;
    copyBtn.textContent = "Copied!";
    setTimeout(() => (copyBtn.textContent = prev), 1500);
  });

  function outcomeRow(client: string, detail: string): HTMLElement {
    const row = document.createElement("div");
    row.className = "target-row";
    row.textContent = `${client} — ${detail}`;
    return row;
  }

  // One row per client, so the fan-out never hides which file it touched.
  function showOutcome({ added, skipped, failed }: FanOutResult) {
    outcome.replaceChildren(
      ...added.map((r) => outcomeRow(r.client, `added to ${r.path}`)),
      ...skipped.map((r) => outcomeRow(r.client, `already set up in ${r.path}`)),
      ...failed.map((r) => outcomeRow(r.client, r.reason)),
    );
  }

  // Every exit from Install lands here, so no click ends without a word.
  function say(message: string, kind: "" | "success" | "error" = "") {
    toast.textContent = message;
    toast.className = kind ? `toast toast-${kind}` : "toast";
    outcome.replaceChildren();
  }

  addBtn.addEventListener("click", async () => {
    const t = targets();
    if (!t.length) {
      say("No AI client found to install into. Copy the snippet instead.", "error");
      return;
    }
    let projectDir: string | null = null;
    if (selectedScope === "project") {
      try {
        projectDir = await chooseDir("Choose the project folder");
      } catch (e) {
        say(`Couldn’t open the folder chooser: ${errorText(e)}`, "error");
        return;
      }
      if (!projectDir) {
        say("Nothing installed — choose a project folder first.");
        return;
      }
    }
    addBtn.disabled = true;
    addBtn.setAttribute("aria-busy", "true");
    addBtn.textContent = "Installing…";
    const result: FanOutResult = { added: [], skipped: [], failed: [] };
    for (const cid of t) {
      const client = CLIENTS.find((x) => x.id === cid)!.label;
      try {
        const res = await inject({ client: cid, scope: selectedScope, projectDir, key: target.key, url: target.url });
        if (res.status === "added") result.added.push({ client, path: res.path });
        else result.skipped.push({ client, path: res.path });
      } catch (e) {
        result.failed.push({ client, reason: errorText(e) });
      }
    }
    const { kind, message } = formatFanOutMessage(target.key, result);
    say(message, kind);
    showOutcome(result);
    addBtn.disabled = targets().length === 0;
    addBtn.removeAttribute("aria-busy");
    addBtn.textContent = "Install";
    addBtn.focus();
  });

  syncScopeOptions();
  renderPreview();
}
