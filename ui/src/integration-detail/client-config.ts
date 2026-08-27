import { formatFanOutMessage, mcpUrl } from "./logic";
import type { ConfigScope, Integration, McpClientId } from "./types";

const CLIENTS: Array<{ id: McpClientId; label: string; supportsProject: boolean; globalPath: string; projectPath: string | null; language: string }> = [
  { id: "opencode", label: "opencode", supportsProject: true, globalPath: "~/.config/opencode/opencode.json", projectPath: "opencode.json", language: "json" },
  { id: "codex", label: "Codex", supportsProject: false, globalPath: "~/.codex/config.toml", projectPath: null, language: "toml" },
  { id: "claudeCode", label: "Claude Code", supportsProject: true, globalPath: "~/.mcp.json", projectPath: ".mcp.json", language: "json" },
  { id: "cursor", label: "Cursor", supportsProject: true, globalPath: "~/.cursor/mcp.json", projectPath: ".cursor/mcp.json", language: "json" },
  { id: "windsurf", label: "Windsurf", supportsProject: false, globalPath: "~/.codeium/windsurf/mcp_config.json", projectPath: null, language: "json" },
  { id: "antigravity", label: "Antigravity", supportsProject: false, globalPath: "~/.gemini/config/mcp_config.json", projectPath: null, language: "json" },
];

export type InjectFn = (args: { client: McpClientId; scope: ConfigScope; projectDir: string | null; key: string; url: string }) => Promise<{ status: "added" | "skipped"; path: string }>;

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

export function renderClientConfig(
  container: HTMLElement,
  integration: Integration,
  inject: InjectFn,
  opts?: { installed?: McpClientId[] },
): void {
  container.innerHTML = "";
  container.className = "card";
  const title = document.createElement("h2");
  title.className = "card-title";
  title.textContent = "Agent setup";
  container.appendChild(title);

  let selectedClient: McpClientId | "all" = "all";
  let selectedScope: ConfigScope = "project";
  const installed = opts?.installed ?? (CLIENTS.map((c) => c.id) as McpClientId[]);

  const controls = document.createElement("div");
  controls.className = "client-controls";

  const clientLabel = document.createElement("span");
  clientLabel.textContent = "Client";
  const clientSelect = document.createElement("select");
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

  const scopeSelect = document.createElement("select");
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

  const addBtn = document.createElement("button");
  addBtn.className = "btn btn-secondary btn-sm";
  addBtn.textContent = "Add";
  const copyBtn = document.createElement("button");
  copyBtn.className = "btn btn-secondary btn-sm";
  copyBtn.textContent = "Copy";
  const snippetPre = document.createElement("pre");
  snippetPre.className = "snippet";
  const hint = document.createElement("div");
  hint.className = "hint";
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.setAttribute("role", "status");

  function syncScopeOptions() {
    const single = selectedClient !== "all" ? CLIENTS.find((x) => x.id === selectedClient) : null;
    if (single && !single.supportsProject && selectedScope === "project") {
      selectedScope = "global";
      scopeSelect.value = "global";
    }
    scopeSelect.style.display = single && !single.supportsProject ? "none" : "";
    // When all, both scopes available
    if (selectedClient === "all") scopeSelect.style.display = "";
  }

  function renderPreview() {
    const t = targets();
    addBtn.disabled = t.length === 0;
    copyBtn.style.display = selectedClient === "all" ? "none" : "";
    if (selectedClient === "all") {
      const list = document.createElement("div");
      list.className = "all-target-list";
      if (t.length === 0) {
        list.textContent = "No clients detected";
      } else {
        for (const id of t) {
          const row = document.createElement("div");
          row.className = "target-row";
          const label = CLIENTS.find((x) => x.id === id)!.label;
          const path = configPathFor(id, selectedScope);
          row.textContent = `${label} — ${path}`;
          list.appendChild(row);
        }
      }
      snippetPre.replaceChildren(list);
      hint.textContent = "";
    } else {
      const id = selectedClient as McpClientId;
      const url = mcpUrl(integration.token);
      const key = integration.id;
      snippetPre.textContent = snippetFor(id, key, url);
      hint.textContent = `Add to ${configPathFor(id, selectedScope)}`;
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

  controls.append(clientLabel, clientSelect, scopeSelect, addBtn, copyBtn);
  container.append(controls, hint, snippetPre, toast);

  copyBtn.addEventListener("click", async () => {
    if (selectedClient === "all") return;
    const id = selectedClient as McpClientId;
    const text = snippetFor(id, integration.id, mcpUrl(integration.token));
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
    const prev = copyBtn.textContent;
    copyBtn.textContent = "Copied!";
    setTimeout(() => (copyBtn.textContent = prev), 1500);
  });

  // Project scope needs directory picker via Tauri dialog
  async function pickProjectDir(): Promise<string | null> {
    const anyWindow = window as unknown as { __TAURI__?: { dialog: { open: (o: unknown) => Promise<string | null> } } };
    if (anyWindow.__TAURI__?.dialog) {
      const picked = await anyWindow.__TAURI__.dialog.open({ directory: true, multiple: false, title: "Choose project folder" });
      return picked;
    }
    // Fallback: prompt
    const p = prompt("Project folder path:");
    return p && p.trim() ? p.trim() : null;
  }

  addBtn.addEventListener("click", async () => {
    const t = targets();
    if (!t.length) return;
    let projectDir: string | null = null;
    if (selectedScope === "project") {
      projectDir = await pickProjectDir();
      if (!projectDir) return;
    }
    const key = integration.id;
    const url = mcpUrl(integration.token);
    const added: string[] = [];
    const skipped: string[] = [];
    const failed: Array<{ client: string; reason: string }> = [];
    for (const cid of t) {
      const label = CLIENTS.find((x) => x.id === cid)!.label;
      try {
        const res = await inject({ client: cid, scope: selectedScope, projectDir, key, url });
        if (res.status === "added") added.push(label);
        else skipped.push(label);
      } catch (e) {
        const reason = e instanceof Error ? e.message : String(e);
        failed.push({ client: label, reason });
      }
    }
    const { kind, message } = formatFanOutMessage(key, { added, skipped, failed });
    toast.textContent = message;
    toast.className = `toast toast-${kind}`;
  });

  syncScopeOptions();
  renderPreview();
}
