import { copyText } from "../clipboard";
import { invoke } from "../host";
import { createBadge, createButton } from "../primitives";
import { toast } from "../toast";

const TITLE_ID = "browser-access-title";
const STATUS_REFRESH_MS = 5000;

export type PlukId = { id: string };

function idRow(id: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row endpoint-row";
  const name = document.createElement("span");
  name.className = "inspector-label";
  name.textContent = "Pluk ID";
  const text = document.createElement("code");
  text.className = "mono endpoint-url";
  text.textContent = id;
  text.title = id;
  const copy = createButton("Copy", {
    variant: "secondary",
    size: "sm",
    ariaLabel: "Copy Pluk ID",
    onClick: async () => {
      await copyText(id);
      toast.success("Pluk ID copied");
    },
  });
  row.append(name, text, copy);
  return row;
}

/**
 * The one value Wande asks for. Read from the running surface rather than
 * stored on the integration, so what is shown is always what Chrome can
 * actually connect with.
 */
export function renderBrowserAccess(container: HTMLElement): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "ui-card";
  container.setAttribute("aria-labelledby", TITLE_ID);

  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.id = TITLE_ID;
  title.textContent = "Chrome";

  const statusLine = document.createElement("div");
  statusLine.className = "browser-status-line";
  const status = createBadge("");
  status.classList.add("browser-status");
  status.setAttribute("role", "status");
  const dot = document.createElement("span");
  dot.className = "browser-status-dot";
  dot.setAttribute("aria-hidden", "true");
  const statusText = document.createElement("span");
  status.append(dot, statusText);
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
    statusText.textContent = connected ? "Connected" : "Not connected";
    status.classList.toggle("browser-status-connected", connected);
    status.classList.toggle("browser-status-disconnected", !connected);
    steps.hidden = connected;
  }

  async function refreshStatus(): Promise<void> {
    try {
      const result = await invoke<{ chromeConnected: boolean }>("list_wande_posts");
      if (alive) setConnectionStatus(result.chromeConnected);
    } catch {
      if (alive) setConnectionStatus(false);
    }
  }

  setConnectionStatus(false);

  void invoke<PlukId>("get_pluk_id").then(
    (access) => {
      if (!alive) return;
      idMount.replaceChildren(idRow(access.id));
    },
    () => {
      if (!alive) return;
      idMount.innerHTML = "";
      const failed = document.createElement("p");
      failed.className = "empty";
      failed.setAttribute("role", "alert");
      failed.textContent = "Pluk can’t show this right now. Restart Pluk and try again.";
      idMount.appendChild(failed);
    },
  );

  void refreshStatus();
  const poll = setInterval(() => void refreshStatus(), STATUS_REFRESH_MS);

  return {
    destroy() {
      alive = false;
      clearInterval(poll);
      container.innerHTML = "";
    },
  };
}
