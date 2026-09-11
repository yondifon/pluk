import { copyText } from "../clipboard";
import { invoke } from "../host";
import { createBadge, createButton, wizardStepFooter, wizardStepHeader } from "../primitives";
import { toast } from "../toast";

const TITLE_ID = "browser-access-title";
const STATUS_REFRESH_MS = 5000;

export type PlukId = { id: string };

/** The one call every pairing status watcher polls, shared so nothing opens a second interval. */
export function watchChromeConnection(
  integrationId: string,
  onChange: (connected: boolean) => void,
): () => void {
  let alive = true;
  async function refresh(): Promise<void> {
    try {
      const result = await invoke<{ chromeConnected: boolean }>("list_wande_posts", { integrationId });
      if (alive) onChange(result.chromeConnected);
    } catch {
      if (alive) onChange(false);
    }
  }
  void refresh();
  const poll = setInterval(() => void refresh(), STATUS_REFRESH_MS);
  return () => {
    alive = false;
    clearInterval(poll);
  };
}

/** The dot-and-text badge every pairing screen shows, wired to flip between its two states. */
function connectionStatusBadge(): { el: HTMLElement; setConnected: (connected: boolean) => void } {
  const el = createBadge("");
  el.classList.add("browser-status");
  el.setAttribute("role", "status");
  const dot = document.createElement("span");
  dot.className = "browser-status-dot";
  dot.setAttribute("aria-hidden", "true");
  const text = document.createElement("span");
  el.append(dot, text);
  return {
    el,
    setConnected(connected) {
      text.textContent = connected ? "Connected" : "Not connected";
      el.classList.toggle("browser-status-connected", connected);
      el.classList.toggle("browser-status-disconnected", !connected);
    },
  };
}

function renderPlukIdFailure(idMount: HTMLElement): void {
  idMount.innerHTML = "";
  const failed = document.createElement("p");
  failed.className = "empty";
  failed.setAttribute("role", "alert");
  failed.textContent = "Pluk can’t show this right now. Restart Pluk and try again.";
  idMount.appendChild(failed);
}

export function idRow(id: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row endpoint-row";
  const name = document.createElement("span");
  name.className = "inspector-label";
  name.textContent = "Pluk ID";
  const text = document.createElement("code");
  text.className = "mono endpoint-url";
  text.textContent = id;
  text.title = id;
  const copy = createButton("", {
    icon: "copy",
    ariaLabel: "Copy Pluk ID",
    onClick: async () => {
      await copyText(id);
      toast.success("Pluk ID copied");
    },
  });
  copy.classList.add("icon-button");
  row.append(name, text, copy);
  return row;
}

export function renderPlukIdCard(
  container: HTMLElement,
  integrationId: string,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "ui-card";
  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.textContent = "Chrome";
  const idMount = document.createElement("div");
  idMount.className = "browser-id-mount";
  const pending = document.createElement("p");
  pending.className = "hint";
  pending.textContent = "Loading…";
  idMount.appendChild(pending);
  container.append(title, idMount);

  let alive = true;
  void invoke<PlukId>("get_pluk_id", { integrationId }).then(
    (access) => {
      if (alive) idMount.replaceChildren(idRow(access.id));
    },
    () => {
      if (alive) renderPlukIdFailure(idMount);
    },
  );

  return {
    destroy() {
      alive = false;
      container.innerHTML = "";
    },
  };
}

/**
 * The one value Wande asks for. Read from the running surface rather than
 * stored on the integration, so what is shown is always what Chrome can
 * actually connect with.
 */
export function renderBrowserAccess(
  container: HTMLElement,
  integrationId: string,
): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "ui-card";
  container.setAttribute("aria-labelledby", TITLE_ID);

  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.id = TITLE_ID;
  title.textContent = "Chrome";

  const statusLine = document.createElement("div");
  statusLine.className = "browser-status-line";
  const { el: status, setConnected } = connectionStatusBadge();
  statusLine.appendChild(status);

  const body = document.createElement("div");
  const idMount = document.createElement("div");
  idMount.className = "browser-id-mount";
  const pending = document.createElement("p");
  pending.className = "hint";
  pending.textContent = "Loading…";
  idMount.appendChild(pending);

  const steps = document.createElement("ol");
  steps.className = "browser-steps";
  const openStep = document.createElement("li");
  openStep.textContent = "Open Wande in Chrome.";
  const pasteStep = document.createElement("li");
  pasteStep.textContent = "Paste the Pluk ID into Wande.";
  steps.append(openStep, pasteStep);
  body.append(statusLine, idMount, steps);

  container.append(title, body);

  let alive = true;

  function setConnectionStatus(connected: boolean): void {
    setConnected(connected);
    steps.hidden = connected;
  }

  setConnectionStatus(false);

  void invoke<PlukId>("get_pluk_id", { integrationId }).then(
    (access) => {
      if (!alive) return;
      idMount.replaceChildren(idRow(access.id));
    },
    () => {
      if (alive) renderPlukIdFailure(idMount);
    },
  );

  const stopWatching = watchChromeConnection(integrationId, (connected) => {
    if (alive) setConnectionStatus(connected);
  });

  return {
    destroy() {
      alive = false;
      stopWatching();
      container.innerHTML = "";
    },
  };
}

/**
 * Step 3 of the new-integration wizard for adapters shaped like Wande: no
 * config fields, just a Pluk ID to paste into the Chrome extension. Continue
 * unlocks once the same poll used by the Overview tab's card reports back.
 */
export function renderConnectChromeStep(
  integrationId: string,
  stepIndex: number,
  totalSteps: number,
  handlers: { onBack: (() => void) | null; onCancel: () => void; onContinue: () => void; onSkip: () => void },
  deps?: { fetchPlukId?: () => Promise<PlukId>; watch?: (onChange: (connected: boolean) => void) => () => void },
): { el: HTMLElement; destroy: () => void } {
  const wrap = wizardStepHeader(
    stepIndex,
    totalSteps,
    "Connect Chrome",
    "Paste this Pluk ID into the Wande extension in Chrome, then come back here.",
  );

  const body = document.createElement("div");
  body.className = "wizard-body";

  const statusLine = document.createElement("div");
  statusLine.className = "browser-status-line";
  const { el: status, setConnected } = connectionStatusBadge();
  statusLine.appendChild(status);

  const idMount = document.createElement("div");
  idMount.className = "browser-id-mount";
  const pending = document.createElement("p");
  pending.className = "hint";
  pending.textContent = "Loading…";
  idMount.appendChild(pending);

  body.append(statusLine, idMount);
  wrap.appendChild(body);

  const skip = createButton("Skip for now", { onClick: handlers.onSkip });
  skip.classList.add("wizard-skip");
  const { el: footer, primaryButton } = wizardStepFooter({
    onBack: handlers.onBack,
    onCancel: handlers.onCancel,
    primaryLabel: "Continue",
    onPrimary: handlers.onContinue,
    extra: skip,
  });
  wrap.appendChild(footer);

  let alive = true;
  function setConnectionStatus(connected: boolean): void {
    if (!alive) return;
    setConnected(connected);
    primaryButton.disabled = !connected;
  }
  setConnectionStatus(false);

  const fetchPlukId = deps?.fetchPlukId ?? (() => invoke<PlukId>("get_pluk_id", { integrationId }));
  void fetchPlukId().then(
    (access) => {
      if (alive) idMount.replaceChildren(idRow(access.id));
    },
    () => {
      if (alive) renderPlukIdFailure(idMount);
    },
  );

  const watch = deps?.watch ?? ((onChange: (connected: boolean) => void) => watchChromeConnection(integrationId, onChange));
  const stop = watch(setConnectionStatus);

  return {
    el: wrap,
    destroy() {
      alive = false;
      stop();
    },
  };
}
