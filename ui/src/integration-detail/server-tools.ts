import { humanizeHealthError } from "../health";
import { integrationApi, invoke } from "../host";
import { confirmModal } from "../modal";
import { createBadge, createButton, createCard } from "../primitives";
import { toast } from "../toast";
import {
  awaitingSignIn,
  canEnable,
  orderedProxyTools,
  signInView,
  stateBadge,
  stateNote,
  type ProxyToolRow,
  type SignIn,
  type SignInStatus,
} from "./proxy-tools";
import type { Integration } from "./types";

const SIGN_IN_POLL_MS = 2000;
const SIGN_IN_GIVE_UP_MS = 10 * 60 * 1000;

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
 * the server lets Pluk in. Each tool then carries one tick, which pins the
 * definition the server offers now and switches the tool on together.
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
  let auth: SignIn | null = null;
  let authError: string | null = null;
  let rows: ProxyToolRow[] | null = null;
  let toolsError: string | null = null;
  let busy = false;
  let waitingForBrowser = false;
  let discovered = false;
  let stopPoll: (() => void) | null = null;

  async function loadAuth(): Promise<void> {
    const result = await call<{ auth: SignIn }>(integration.id, "GET", "/proxy/auth");
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
  }

  /**
   * A server Pluk can already reach, with nothing listed, is asked without a
   * click. Once per panel: a server that keeps answering with nothing must not
   * turn the screen into a loop.
   */
  async function discoverOnce(): Promise<void> {
    if (discovered || !rows || rows.length) return;
    if (!auth || awaitingSignIn(auth)) return;
    discovered = true;
    await loadTools("/proxy/refresh", "POST");
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
        rows = null;
        render();
        await loadTools();
        await discoverOnce();
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
   * One tick, one call. The route answers with the list as it now stands, and
   * `toolConfig` is the same object the shell holds for this integration, so
   * writing into it keeps the list behind the detail screen in step without a
   * reload that would tear this panel down.
   */
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
    // The list is rebuilt around the answer, so the tick the user just pressed
    // is a new element. Keyboard focus follows it rather than falling to the top.
    [...tools.body.querySelectorAll<HTMLInputElement>("input[data-tool]")]
      .find((tick) => tick.dataset.tool === name)
      ?.focus();
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

    const view = signInView(auth);
    if (view.status) {
      const statusLine = actionRow(statusBadge(view.status));
      statusLine.setAttribute("role", "status");
      signIn.body.appendChild(statusLine);
    }
    signIn.body.appendChild(line(view.message, "hint"));

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
    if (!view.action) return;
    const action =
      view.action === "sign-out"
        ? createButton("Sign out", {
            onClick: () =>
              confirmModal({
                title: "Sign out",
                message:
                  "Your agents lose this server’s tools until you sign in again. What you turned on is remembered.",
                confirmLabel: "Sign out",
                onConfirm: () => void working(signOut),
              }),
          })
        : createButton(view.action === "sign-in-again" ? "Sign in again" : "Sign in", {
            variant: "primary",
            onClick: () => void working(startSignIn),
          });
    action.disabled = busy;
    signIn.body.appendChild(actionRow(action));
  }

  /** A tool an agent can reach: pinned as the server describes it now, and on. */
  function isOn(row: ProxyToolRow): boolean {
    return canEnable(row.state) && (integration.toolConfig[row.name]?.enabled ?? false);
  }

  function toolControl(row: ProxyToolRow): HTMLElement | null {
    if (row.state === "missing") return null;
    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.checked = isOn(row);
    toggle.dataset.tool = row.name;
    toggle.setAttribute("aria-label", row.label);
    toggle.setAttribute("aria-describedby", `tool-desc-${row.name}`);
    toggle.addEventListener("change", () => void setEnabled(row.name, toggle.checked));
    return toggle;
  }

  function toolRow(row: ProxyToolRow): HTMLElement {
    const on = isOn(row);
    const el = document.createElement("div");
    el.className = "tool-row";
    // A tool still waiting on the user keeps full contrast. Only a settled one
    // dims when it is switched off, and a withdrawn one dims for good.
    if (row.state === "missing") el.classList.add("tool-gone");
    else if (canEnable(row.state)) el.classList.add(on ? "tool-on" : "tool-off");

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

  /** What an empty list is waiting for, in the words of the step above it. */
  function waitingLine(): string {
    const action = auth ? signInView(auth).action : null;
    return action === "sign-in" || action === "sign-in-again"
      ? "Sign in above to see what this server offers."
      : "Finish the step above to see what this server offers.";
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
      const waiting = auth ? awaitingSignIn(auth) : false;
      const empty = waiting ? waitingLine() : "This server offers no tools yet.";
      tools.body.appendChild(line(empty, "empty"));
      if (!waiting) tools.body.appendChild(actionRow(refreshButton("Check for new tools")));
      return;
    }

    const live = rows.filter(isOn).length;
    tools.body.appendChild(line(`${live} of ${rows.length} tools available to the agent.`, "hint"));

    for (const row of orderedProxyTools(rows)) tools.body.appendChild(toolRow(row));
    tools.body.appendChild(actionRow(refreshButton("Check for new tools")));
  }

  function render(): void {
    if (!alive) return;
    renderSignIn();
    renderTools();
  }

  render();
  void working(async () => {
    await Promise.all([loadAuth(), loadTools()]);
    await discoverOnce();
  });

  return {
    destroy() {
      alive = false;
      stopPoll?.();
      container.innerHTML = "";
    },
  };
}
