import { humanizeHealthError } from "../health";
import { integrationApi, invoke } from "../host";
import { confirmModal } from "../modal";
import { createBadge, createButton, createCard } from "../primitives";
import { toast } from "../toast";
import {
  attentionCount,
  canEnable,
  orderedProxyTools,
  signInMessage,
  stateBadge,
  stateNote,
  type ProxyToolRow,
  type SignInKind,
  type SignInStatus,
} from "./proxy-tools";
import type { Integration } from "./types";

const SIGN_IN_POLL_MS = 2000;
const SIGN_IN_GIVE_UP_MS = 10 * 60 * 1000;

type Auth = { kind: SignInKind; status: SignInStatus };

type Answer<T> = { ok: true; value: T } | { ok: false; error: string };

/**
 * One call to a route the server's adapter serves. A refusal the route itself
 * chose comes back with its own wording; only a host that could not carry the
 * request at all throws.
 */
async function call<T>(
  integrationId: string,
  method: string,
  subpath: string,
  body?: unknown,
): Promise<Answer<T>> {
  try {
    const answer = await integrationApi<T & { error?: string }>({
      integrationId,
      method,
      subpath,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (answer.status >= 200 && answer.status < 300) return { ok: true, value: answer.body };
    return { ok: false, error: humanizeHealthError(answer.body?.error) };
  } catch (e) {
    return { ok: false, error: humanizeHealthError(e instanceof Error ? e.message : String(e)) };
  }
}

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

/**
 * The tools an MCP server offers, and the sign-in that reveals them.
 *
 * The two sit together because one causes the other: nothing is listed until
 * the server lets Pluk in, and a tool reaches an agent only once the user has
 * approved it and switched it on.
 */
export function mountServerTools(
  container: HTMLElement,
  integration: Integration,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "tools-tab stack-lg";

  const signIn = card("Server");
  const tools = card("Tools");
  container.append(signIn.el, tools.el);

  let alive = true;
  let auth: Auth | null = null;
  let authError: string | null = null;
  let rows: ProxyToolRow[] | null = null;
  let toolsError: string | null = null;
  let busy = false;
  let waitingForBrowser = false;
  /** What the user ticked on a first run, before anything has been approved. */
  let picked: Set<string> | null = null;
  let stopPoll: (() => void) | null = null;

  async function loadAuth(): Promise<void> {
    const result = await call<{ auth: Auth }>(integration.id, "GET", "/proxy/auth");
    if (!alive) return;
    auth = result.ok ? result.value.auth : null;
    authError = result.ok ? null : result.error;
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
    const untouched = rows.length > 0 && rows.every((row) => row.state === "new");
    picked = untouched ? new Set<string>() : null;
  }

  async function working(run: () => Promise<void>): Promise<void> {
    busy = true;
    render();
    await run();
    if (!alive) return;
    busy = false;
    render();
  }

  function pollUntilSignedIn(): void {
    stopPoll?.();
    const startedAt = Date.now();
    waitingForBrowser = true;
    const timer = setInterval(() => {
      if (Date.now() - startedAt > SIGN_IN_GIVE_UP_MS) {
        stopPoll?.();
        return render();
      }
      void (async () => {
        await loadAuth();
        if (!alive) return;
        if (auth?.status !== "connected") return render();
        stopPoll?.();
        await loadTools();
        render();
      })();
    }, SIGN_IN_POLL_MS);
    stopPoll = () => {
      clearInterval(timer);
      stopPoll = null;
      waitingForBrowser = false;
    };
  }

  async function startSignIn(): Promise<void> {
    const started = await call<{ authorizeUrl: string }>(
      integration.id,
      "POST",
      "/proxy/oauth/start",
    );
    if (!alive) return;
    if (!started.ok) {
      toast.error("Pluk could not start the sign-in", { description: started.error });
      return;
    }
    await invoke("open_external", { url: started.value.authorizeUrl });
    if (alive) pollUntilSignedIn();
  }

  async function approve(names: string[]): Promise<void> {
    if (!names.length) return;
    const result = await call<{ tools: ProxyToolRow[] }>(integration.id, "POST", "/proxy/approve", {
      names,
    });
    if (!alive) return;
    if (!result.ok) {
      toast.error("Pluk could not approve that", { description: result.error });
      return;
    }
    rows = result.value.tools;
    picked = null;
    toast.success(names.length === 1 ? "Tool approved" : `${names.length} tools approved`);
  }

  async function signOut(): Promise<void> {
    const result = await call(integration.id, "POST", "/proxy/disconnect");
    if (!alive) return;
    if (!result.ok) {
      toast.error("Pluk could not sign you out", { description: result.error });
      return;
    }
    await Promise.all([loadAuth(), loadTools()]);
  }

  /**
   * `toolConfig` is the same object the shell holds for this integration, so
   * writing into it keeps the list behind the detail screen in step without a
   * reload that would tear this panel down.
   */
  async function setEnabled(name: string, on: boolean): Promise<void> {
    const before = integration.toolConfig[name];
    integration.toolConfig[name] = { enabled: on, settings: before?.settings ?? {} };
    render();
    try {
      await invoke("update_integration", {
        id: integration.id,
        payload: {
          name: integration.name,
          type: integration.type,
          config: integration.config,
          environment: integration.environment,
          toolConfig: integration.toolConfig,
          approvals: integration.approvals,
        },
      });
    } catch (e) {
      if (before) integration.toolConfig[name] = before;
      else delete integration.toolConfig[name];
      toast.error("Pluk could not save that", {
        description: humanizeHealthError(e instanceof Error ? e.message : String(e)),
      });
      render();
    }
  }

  function statusBadge(status: SignInStatus): HTMLElement {
    const settled = status === "connected";
    const badge = createBadge(settled ? "Signed in" : "Not signed in");
    badge.classList.add(
      "browser-status",
      settled ? "browser-status-connected" : "browser-status-disconnected",
    );
    const dot = document.createElement("span");
    dot.className = "browser-status-dot";
    dot.setAttribute("aria-hidden", "true");
    badge.prepend(dot);
    return badge;
  }

  function renderSignIn(): void {
    signIn.body.innerHTML = "";
    if (authError) {
      signIn.body.append(line("Pluk could not reach this server.", "empty"), line(authError, "hint"));
      return;
    }
    if (!auth) {
      signIn.body.appendChild(line("Loading…", "hint"));
      return;
    }

    const statusLine = actionRow(statusBadge(auth.status));
    statusLine.setAttribute("role", "status");
    signIn.body.append(statusLine, line(signInMessage(auth.kind, auth.status), "hint"));

    if (waitingForBrowser) {
      const stop = createButton("Cancel", {
        onClick: () => {
          stopPoll?.();
          render();
        },
      });
      signIn.body.append(
        line("Finish signing in, then come back here.", "hint"),
        actionRow(stop),
      );
      return;
    }
    const action =
      auth.status === "connected"
        ? createButton("Sign out", {
            onClick: () =>
              confirmModal({
                title: "Sign out",
                message:
                  "Your agents lose this server’s tools until you sign in again. What you approved is remembered.",
                confirmLabel: "Sign out",
                onConfirm: () => void working(signOut),
              }),
          })
        : createButton(auth.status === "reconnect_needed" ? "Sign in again" : "Sign in", {
            variant: "primary",
            onClick: () => void working(startSignIn),
          });
    action.disabled = busy;
    signIn.body.appendChild(actionRow(action));
  }

  function toolControl(row: ProxyToolRow): HTMLElement | null {
    if (picked) {
      const tick = document.createElement("input");
      tick.type = "checkbox";
      tick.checked = picked.has(row.name);
      tick.setAttribute("aria-label", `Approve ${row.label}`);
      tick.addEventListener("change", () => {
        if (tick.checked) picked?.add(row.name);
        else picked?.delete(row.name);
      });
      return tick;
    }
    if (row.state === "missing") return null;
    const approved = canEnable(row.state);
    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.checked = approved && (integration.toolConfig[row.name]?.enabled ?? false);
    toggle.disabled = !approved;
    toggle.setAttribute("aria-label", row.label);
    toggle.setAttribute("aria-describedby", `tool-desc-${row.name}`);
    if (!approved) toggle.title = "Approve this tool before switching it on.";
    toggle.addEventListener("change", () => void setEnabled(row.name, toggle.checked));
    return toggle;
  }

  function toolRow(row: ProxyToolRow): HTMLElement {
    const approved = canEnable(row.state);
    const on = approved && (integration.toolConfig[row.name]?.enabled ?? false);
    const el = document.createElement("div");
    el.className = "tool-row";
    // A tool still waiting on the user keeps full contrast. Only a settled one
    // dims when it is switched off, and a withdrawn one dims for good.
    if (row.state === "missing") el.classList.add("tool-gone");
    else if (approved) el.classList.add(on ? "tool-on" : "tool-off");

    const control = toolControl(row);
    const head = document.createElement(control ? "label" : "div");
    head.className = "tool-head";
    if (control) head.appendChild(control);

    const name = document.createElement("span");
    name.className = "tool-name";
    name.textContent = row.label;
    const id = document.createElement("code");
    id.className = "tool-id tool-category mono";
    id.textContent = row.name;
    const category = document.createElement("span");
    category.className = "tool-category";
    category.textContent = row.category;
    head.append(name, id, category);
    const badge = stateBadge(row.state);
    if (badge) head.appendChild(createBadge(badge, row.state));
    el.appendChild(head);

    const body = document.createElement("div");
    body.className = "tool-body";
    const desc = document.createElement("div");
    desc.className = "tool-summary";
    desc.id = `tool-desc-${row.name}`;
    desc.textContent = row.description;
    body.appendChild(desc);
    const note = stateNote(row.state);
    if (note) body.appendChild(line(note, "tool-summary"));
    if (!picked && (row.state === "new" || row.state === "changed")) {
      const one = createButton("Approve", {
        size: "sm",
        onClick: () => void working(() => approve([row.name])),
      });
      one.disabled = busy;
      body.appendChild(one);
    }
    el.appendChild(body);
    return el;
  }

  function refreshButton(label: string): HTMLButtonElement {
    const button = createButton(label, {
      onClick: () => void working(() => loadTools("/proxy/refresh", "POST")),
    });
    button.disabled = busy;
    return button;
  }

  function actionRow(...controls: HTMLElement[]): HTMLElement {
    const el = document.createElement("div");
    el.className = "browser-status-line";
    el.append(...controls);
    return el;
  }

  function renderTools(): void {
    tools.body.innerHTML = "";
    if (toolsError) {
      tools.body.append(
        line("Pluk could not reach this server.", "empty"),
        line(toolsError, "hint"),
        actionRow(refreshButton("Try again")),
      );
      return;
    }
    if (!rows) {
      tools.body.appendChild(line("Loading…", "hint"));
      return;
    }
    if (!rows.length) {
      const waitingOnSignIn = auth?.kind === "oauth" && auth.status !== "connected";
      tools.body.append(
        line(
          waitingOnSignIn
            ? "Sign in above to see what this server offers."
            : "This server offers no tools yet.",
          "empty",
        ),
      );
      if (!waitingOnSignIn) tools.body.appendChild(actionRow(refreshButton("Check for new tools")));
      return;
    }

    const waiting = attentionCount(rows);
    if (picked) {
      tools.body.appendChild(
        line(
          `Pluk found ${rows.length} ${rows.length === 1 ? "tool" : "tools"}. Tick the ones you want, then approve them. Nothing reaches your agents until you do.`,
          "hint",
        ),
      );
    } else if (waiting) {
      tools.body.appendChild(
        line(`${waiting} of ${rows.length} tools need your attention.`, "hint"),
      );
    }

    for (const row of orderedProxyTools(rows)) tools.body.appendChild(toolRow(row));

    const actions = [refreshButton("Check for new tools")];
    if (picked) {
      const confirm = createButton("Approve ticked tools", {
        variant: "primary",
        onClick: () => void working(() => approve([...(picked ?? [])])),
      });
      confirm.disabled = busy;
      actions.push(confirm);
    } else if (waiting) {
      const all = createButton("Approve all", {
        onClick: () =>
          void working(() =>
            approve(
              (rows ?? [])
                .filter((row) => row.state === "new" || row.state === "changed")
                .map((row) => row.name),
            ),
          ),
      });
      all.disabled = busy;
      actions.push(all);
    }
    tools.body.appendChild(actionRow(...actions));
  }

  function render(): void {
    if (!alive) return;
    renderSignIn();
    renderTools();
  }

  render();
  void working(async () => {
    await Promise.all([loadAuth(), loadTools()]);
  });

  return {
    destroy() {
      alive = false;
      stopPoll?.();
      container.innerHTML = "";
    },
  };
}
