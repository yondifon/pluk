import { copyText } from "../clipboard";
import { invoke } from "../host";
import { createButton } from "../primitives";
import { toast } from "../toast";

const TITLE_ID = "browser-access-title";

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
export function renderBrowserAccess(container: HTMLElement): void {
  container.innerHTML = "";
  container.className = "ui-card";
  container.setAttribute("aria-labelledby", TITLE_ID);

  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.id = TITLE_ID;
  title.textContent = "Wande in Chrome";

  const lede = document.createElement("p");
  lede.className = "hint";
  lede.textContent = "Open Wande in Chrome and paste this in.";

  const body = document.createElement("div");
  const pending = document.createElement("p");
  pending.className = "hint";
  pending.setAttribute("role", "status");
  pending.textContent = "Loading…";
  body.appendChild(pending);

  container.append(title, lede, body);

  void invoke<PlukId>("get_pluk_id").then(
    (access) => {
      body.innerHTML = "";
      body.append(idRow(access.id));
    },
    () => {
      body.innerHTML = "";
      const failed = document.createElement("p");
      failed.className = "empty";
      failed.setAttribute("role", "alert");
      failed.textContent = "Pluk can’t show this right now. Restart Pluk and try again.";
      body.appendChild(failed);
    },
  );
}
