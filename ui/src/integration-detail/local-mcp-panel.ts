/** A local MCP server's approval, status and tools. Nothing asks the server for tools before approval. */

import {
  approveMcpLaunch,
  mcpLaunchPreview,
  mcpServerOutput,
  mcpServerStatus,
  restartMcpServer,
  stopMcpServer,
  type LaunchPreview,
  type McpServerStatus,
} from "../host";
import { createBadge, createButton, createCard } from "../primitives";
import { toast } from "../toast";
import { callProxyApi as call } from "./proxy-api";
import {
  canEnable,
  orderedProxyTools,
  pendingOffNote,
  stateBadge as toolStateBadge,
  stateNote as toolStateNote,
  type ProxyToolRow,
} from "./proxy-tools";
import { canRestart, canStop, restartLabel, RUNS_WITH_FULL_ACCESS, stateLabel, stateNote, stateTone } from "./local-mcp";
import type { Integration } from "./types";

const STATUS_POLL_MS = 2000;

function line(text: string, className: string): HTMLParagraphElement {
  const el = document.createElement("p");
  el.className = className;
  el.textContent = text;
  return el;
}

function card(title: string): { el: HTMLElement; body: HTMLElement } {
  const el = createCard(title);
  const body = document.createElement("div");
  el.appendChild(body);
  return { el, body };
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function actionRow(...controls: HTMLElement[]): HTMLElement {
  const el = document.createElement("div");
  el.className = "browser-status-line";
  el.append(...controls);
  return el;
}

function commandDetails(preview: LaunchPreview): HTMLElement {
  const list = document.createElement("dl");
  list.className = "launch-preview";

  const row = (term: string, value: string) => {
    const dt = document.createElement("dt");
    dt.textContent = term;
    const dd = document.createElement("dd");
    dd.className = "mono";
    dd.textContent = value;
    list.append(dt, dd);
  };
  row("Program", preview.program);
  row("Working folder", preview.cwd);

  if (preview.args.length) {
    const argsTerm = document.createElement("dt");
    argsTerm.textContent = "Arguments";
    const argsValue = document.createElement("dd");
    const ol = document.createElement("ol");
    ol.className = "launch-preview-args mono";
    for (const arg of preview.args) {
      const li = document.createElement("li");
      li.textContent = arg;
      ol.appendChild(li);
    }
    argsValue.appendChild(ol);
    list.append(argsTerm, argsValue);
  }

  if (preview.env.length) {
    const envTerm = document.createElement("dt");
    envTerm.textContent = "Environment";
    const envValue = document.createElement("dd");
    const envList = document.createElement("ul");
    envList.className = "launch-preview-env";
    for (const variable of preview.env) {
      const li = document.createElement("li");
      li.className = "mono";
      li.textContent = variable.name;
      if (variable.secret) li.appendChild(createBadge("Secret"));
      envList.appendChild(li);
    }
    envValue.appendChild(envList);
    list.append(envTerm, envValue);
  }

  return list;
}

export function mountLocalMcp(
  container: HTMLElement,
  integration: Integration,
  onStatusChange?: (status: { signedIn: boolean; toolCount: number }) => void,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "tools-tab stack-lg";

  const approval = card("Local server");
  const status = card("Status");
  const tools = card("Tools");
  const pendingNote = line("", "hint");
  pendingNote.hidden = true;
  tools.el.appendChild(pendingNote);
  container.append(approval.el, status.el, tools.el);

  let alive = true;
  let preview: LaunchPreview | null = null;
  let previewError: string | null = null;
  let approving = false;
  let serverStatus: McpServerStatus | null = null;
  let output: string[] | null = null;
  let outputOpen = false;
  let rows: ProxyToolRow[] | null = null;
  let toolsError: string | null = null;
  let busy = false;
  let discovered = false;
  let statusTimer: ReturnType<typeof setInterval> | null = null;

  async function loadPreview(): Promise<void> {
    try {
      const shown = await mcpLaunchPreview(integration.id);
      if (!alive) return;
      preview = shown;
      previewError = null;
    } catch (e) {
      if (!alive) return;
      preview = null;
      previewError = errorText(e);
    }
  }

  async function loadStatus(): Promise<void> {
    try {
      const next = await mcpServerStatus(integration.id);
      if (alive) serverStatus = next;
    } catch {
      // The status card keeps showing what it last knew.
    }
  }

  async function loadOutput(): Promise<void> {
    try {
      const lines = await mcpServerOutput(integration.id);
      if (alive) output = lines;
    } catch {
      // Leave the output panel showing what it already has.
    }
  }

  async function loadTools(subpath = "/proxy/tools", method = "GET"): Promise<void> {
    const result = await call<{ tools: ProxyToolRow[] }>(integration.id, method, subpath);
    if (!alive) return;
    if (!result.ok) {
      toolsError = result.error;
      return;
    }
    rows = result.value.tools;
    toolsError = null;
  }

  /** Once approved, ask the server what it offers, but only once per panel. */
  async function discoverOnce(): Promise<void> {
    if (discovered || !preview?.approved || (rows && rows.length)) return;
    discovered = true;
    await loadTools("/proxy/refresh", "POST");
  }

  function startPolling(): void {
    if (statusTimer) return;
    statusTimer = setInterval(() => {
      void (async () => {
        await loadStatus();
        if (outputOpen) await loadOutput();
        if (!alive) return;
        render();
      })();
    }, STATUS_POLL_MS);
  }

  function stopPolling(): void {
    if (statusTimer) clearInterval(statusTimer);
    statusTimer = null;
  }

  async function working(run: () => Promise<void>): Promise<void> {
    busy = true;
    render();
    await run();
    if (!alive) return;
    busy = false;
    render();
  }

  async function approve(): Promise<void> {
    if (!preview) return;
    approving = true;
    render();
    try {
      await approveMcpLaunch(integration.id, preview.launchHash);
      await loadPreview();
      await loadStatus();
      startPolling();
      discovered = false;
      await discoverOnce();
    } catch (e) {
      toast.error("Pluk could not approve this command", { description: errorText(e) });
      await loadPreview();
    }
    if (!alive) return;
    approving = false;
    render();
  }

  async function stop(): Promise<void> {
    try {
      await stopMcpServer(integration.id);
    } catch (e) {
      toast.error("Pluk could not stop this server", { description: errorText(e) });
    }
    await loadStatus();
  }

  async function restart(): Promise<void> {
    try {
      await restartMcpServer(integration.id);
    } catch (e) {
      toast.error("Pluk could not restart this server", { description: errorText(e) });
    }
    await Promise.all([loadStatus(), loadTools("/proxy/tools")]);
  }

  function statusDot(tone: "on" | "off" | "warn"): HTMLElement {
    const badge = createBadge(serverStatus ? stateLabel(serverStatus.state) : "");
    badge.classList.add(
      "browser-status",
      tone === "on" ? "browser-status-connected" : tone === "warn" ? "browser-status-warn" : "browser-status-disconnected",
    );
    const dot = document.createElement("span");
    dot.className = "browser-status-dot";
    dot.setAttribute("aria-hidden", "true");
    badge.prepend(dot);
    return badge;
  }

  function renderApproval(): void {
    approval.body.innerHTML = "";
    if (previewError) {
      approval.body.append(line("Pluk could not prepare this command.", "empty"), line(previewError, "hint"));
      approval.body.appendChild(actionRow(createButton("Try again", { onClick: () => void working(loadPreview) })));
      return;
    }
    if (!preview) {
      approval.body.appendChild(line("Loading…", "hint"));
      return;
    }
    approval.body.appendChild(commandDetails(preview));
    approval.body.appendChild(line(RUNS_WITH_FULL_ACCESS, "hint"));
    for (const warning of preview.warnings) approval.body.appendChild(line(warning, "hint danger-copy"));

    if (preview.approved) {
      approval.body.appendChild(line("Approved. Pluk starts it the first time it's needed.", "hint"));
      return;
    }
    const approveBtn = createButton("Approve and start", { variant: "primary", onClick: () => void approve() });
    approveBtn.disabled = approving;
    approval.body.appendChild(actionRow(approveBtn));
  }

  function renderStatus(): void {
    status.body.innerHTML = "";
    if (!preview?.approved) {
      status.body.appendChild(line("Approve the command above to see whether it's running.", "empty"));
      return;
    }
    if (!serverStatus) {
      status.body.appendChild(line("Loading…", "hint"));
      return;
    }
    status.body.appendChild(actionRow(statusDot(stateTone(serverStatus.state))));
    if (serverStatus.pid != null) {
      status.body.appendChild(line(`Process ID ${serverStatus.pid}`, "hint mono"));
    }
    const note = stateNote(serverStatus.state);
    if (note) status.body.appendChild(line(note, "hint"));

    const stopBtn = createButton("Stop", { onClick: () => void working(stop) });
    stopBtn.disabled = busy || !canStop(serverStatus.state);
    const restartBtn = createButton(restartLabel(serverStatus.state), { onClick: () => void working(restart) });
    restartBtn.disabled = busy || !canRestart(serverStatus.state);
    status.body.appendChild(actionRow(stopBtn, restartBtn));

    const details = document.createElement("details");
    details.className = "output-disclosure";
    details.open = outputOpen;
    const summary = document.createElement("summary");
    summary.textContent = "Recent output";
    details.appendChild(summary);
    const pre = document.createElement("pre");
    pre.className = "mono output-lines";
    pre.textContent = output && output.length ? output.join("\n") : "Nothing printed yet.";
    details.appendChild(pre);
    details.addEventListener("toggle", () => {
      outputOpen = details.open;
      if (outputOpen) void working(async () => loadOutput());
    });
    status.body.appendChild(details);
  }

  function toolRow(row: ProxyToolRow): HTMLElement {
    const on = canEnable(row.state) && (integration.toolConfig[row.name]?.enabled ?? false);
    const el = document.createElement("div");
    el.className = "tool-row";
    if (row.state === "missing") el.classList.add("tool-gone");
    else if (canEnable(row.state)) el.classList.add(on ? "tool-on" : "tool-off");

    const head = document.createElement(row.state === "missing" ? "div" : "label");
    head.className = "tool-head";
    if (row.state !== "missing") {
      const toggle = document.createElement("input");
      toggle.type = "checkbox";
      toggle.checked = on;
      toggle.setAttribute("aria-label", row.label);
      toggle.addEventListener("change", () => void setEnabled(row.name, toggle.checked));
      head.appendChild(toggle);
    }
    const name = document.createElement("span");
    name.className = "tool-name";
    name.textContent = row.label;
    const id = document.createElement("code");
    id.className = "tool-id tool-category mono";
    id.textContent = row.name;
    head.append(name, id);
    const badge = toolStateBadge(row.state);
    if (badge) head.appendChild(createBadge(badge, row.state));
    el.appendChild(head);

    const body = document.createElement("div");
    body.className = "tool-body";
    body.appendChild(line(row.description, "tool-summary"));
    const note = toolStateNote(row.state);
    if (note) body.appendChild(line(note, "tool-summary"));
    el.appendChild(body);
    return el;
  }

  async function setEnabled(name: string, on: boolean): Promise<void> {
    const result = await call<{ tools: ProxyToolRow[] }>(integration.id, "POST", "/proxy/enable", {
      names: [name],
      enabled: on,
    });
    if (!alive) return;
    if (result.ok) {
      rows = result.value.tools;
      const before = integration.toolConfig[name];
      integration.toolConfig[name] = { enabled: on, settings: before?.settings ?? {} };
    } else {
      toast.error("Pluk could not save that", { description: result.error });
    }
    render();
  }

  function refreshToolsButton(label: string): HTMLButtonElement {
    const button = createButton(label, {
      onClick: () => void working(() => loadTools("/proxy/refresh", "POST")),
    });
    button.disabled = busy;
    return button;
  }

  function renderTools(): void {
    tools.body.innerHTML = "";
    const pending = pendingOffNote(integration.pendingToolsOff, rows);
    pendingNote.textContent = pending ?? "";
    pendingNote.hidden = pending === null;
    if (!preview?.approved) {
      tools.body.appendChild(line("Approve the command above to see what it offers.", "empty"));
      return;
    }
    if (toolsError) {
      tools.body.append(
        line("Pluk could not reach this server.", "empty"),
        line(toolsError, "hint"),
        actionRow(refreshToolsButton("Try again")),
      );
      return;
    }
    if (!rows) {
      tools.body.appendChild(line("Loading…", "hint"));
      return;
    }
    if (!rows.length) {
      tools.body.appendChild(line("This server offers no tools yet.", "empty"));
      tools.body.appendChild(actionRow(refreshToolsButton("Check for new tools")));
      return;
    }
    const live = rows.filter((row) => canEnable(row.state) && (integration.toolConfig[row.name]?.enabled ?? false)).length;
    tools.body.appendChild(line(`${live} of ${rows.length} tools available to the agent.`, "hint"));
    for (const row of orderedProxyTools(rows)) tools.body.appendChild(toolRow(row));
    tools.body.appendChild(actionRow(refreshToolsButton("Check for new tools")));
  }

  function render(): void {
    if (!alive) return;
    renderApproval();
    renderStatus();
    renderTools();
  }

  function reportStatus(): void {
    onStatusChange?.({ signedIn: preview?.approved ?? false, toolCount: rows?.length ?? 0 });
  }

  render();
  void working(async () => {
    await loadPreview();
    if (preview?.approved) {
      await loadStatus();
      startPolling();
    }
    await loadTools();
    await discoverOnce();
    reportStatus();
  });

  return {
    destroy() {
      alive = false;
      stopPolling();
      container.innerHTML = "";
    },
  };
}
