import "./shell.css";
import { createButton } from "./primitives";

const COLLAPSED_KEY = "pluk.sidebar.collapsed";

let shellRoot: HTMLElement | null = null;

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

function applyCollapsed(collapsed: boolean): void {
  shellRoot?.classList.toggle("sidebar-collapsed", collapsed);
  const label = collapsed ? "Show sidebar" : "Hide sidebar";
  for (const btn of shellRoot?.querySelectorAll<HTMLElement>(".sidebar-toggle") ?? []) {
    btn.setAttribute("aria-expanded", collapsed ? "false" : "true");
    btn.setAttribute("aria-label", label);
    btn.title = label;
  }
}

/** Both toggles — the one in the sidebar and the one that replaces it — call this. */
export function toggleSidebar(): void {
  const collapsed = !shellRoot?.classList.contains("sidebar-collapsed");
  applyCollapsed(collapsed);
  try {
    localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
  } catch {
    // A viewer with site data blocked still gets the toggle, just not the memory of it.
  }
}

export function createSidebarToggle(): HTMLButtonElement {
  const btn = createButton("", { icon: "sidebar", ariaLabel: "Hide sidebar", onClick: toggleSidebar });
  btn.classList.add("icon-button", "sidebar-toggle");
  btn.title = "Hide sidebar";
  btn.setAttribute("aria-controls", "pluk-sidebar");
  return btn;
}

export type BannerState = {
  update?: { commit?: string; updating: boolean };
  serverStatus: "running" | "starting" | "stopped";
};

export function createShell(
  sidebarEl: HTMLElement,
  detailEl: HTMLElement,
): { root: HTMLElement; detailMount: HTMLElement; bottomMount: HTMLElement; toasterMount: HTMLElement } {
  const root = document.createElement("div");
  root.className = "shell";
  shellRoot = root;

  const sidebarWrap = document.createElement("div");
  sidebarWrap.className = "shell-sidebar";
  sidebarWrap.id = "pluk-sidebar";
  sidebarWrap.appendChild(sidebarEl);

  const resizer = document.createElement("div");
  resizer.className = "shell-resizer";
  resizer.setAttribute("role", "separator");
  resizer.setAttribute("aria-label", "Resize sidebar");
  resizer.setAttribute("aria-orientation", "vertical");
  resizer.tabIndex = 0;
  // simple drag resize
  let startX = 0;
  let startW = 0;
  resizer.addEventListener("mousedown", (e) => {
    startX = e.clientX;
    startW = sidebarWrap.getBoundingClientRect().width;
    // Width relayouts the whole shell, so coalesce to one write per frame and
    // hold the cursor for the drag — it otherwise reverts the moment the
    // pointer leaves the resizer's hit area.
    const previousCursor = document.body.style.cursor;
    document.body.style.cursor = "col-resize";
    let pending = 0;
    let latestX = startX;
    const onMove = (ev: MouseEvent) => {
      latestX = ev.clientX;
      if (pending) return;
      pending = requestAnimationFrame(() => {
        pending = 0;
        const next = Math.max(220, Math.min(320, startW + (latestX - startX)));
        sidebarWrap.style.width = `${next}px`;
      });
    };
    const onUp = () => {
      if (pending) cancelAnimationFrame(pending);
      document.body.style.cursor = previousCursor;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  resizer.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight" && e.key !== "Home") return;
    e.preventDefault();
    const current = sidebarWrap.getBoundingClientRect().width;
    const next = e.key === "Home" ? 244 : current + (e.key === "ArrowRight" ? 16 : -16);
    sidebarWrap.style.width = `${Math.max(220, Math.min(320, next))}px`;
    resizer.setAttribute("aria-valuenow", String(Math.round(Math.max(220, Math.min(320, next)))));
  });
  resizer.addEventListener("dblclick", () => {
    sidebarWrap.style.width = "244px";
    resizer.setAttribute("aria-valuenow", "244");
  });

  const main = document.createElement("div");
  main.className = "shell-main";

  // With the sidebar hidden there is nothing left holding the window's drag
  // region or the space the traffic lights need, so this strip takes over.
  const mainTopbar = document.createElement("div");
  mainTopbar.className = "shell-topbar";
  mainTopbar.setAttribute("data-tauri-drag-region", "");
  mainTopbar.appendChild(createSidebarToggle());

  const detail = document.createElement("div");
  detail.className = "shell-detail";
  detail.appendChild(detailEl);

  const bottom = document.createElement("div");
  bottom.className = "shell-bottom";

  const toasterMount = document.createElement("div");

  const stage = document.createElement("div");
  stage.className = "shell-stage";
  stage.append(detail, toasterMount);

  main.append(mainTopbar, stage, bottom);
  root.append(sidebarWrap, resizer, main);
  applyCollapsed(readCollapsed());

  return { root, detailMount: detail, bottomMount: bottom, toasterMount };
}

export function renderBanners(mount: HTMLElement, state: BannerState, onRestart: () => void, onUpdate: () => void): void {
  mount.innerHTML = "";
  if (state.update) {
    const banner = document.createElement("div");
    banner.className = "banner";
    banner.setAttribute("role", "status");
    banner.setAttribute("aria-live", "polite");
    if (state.update.updating) {
      banner.textContent = "Updating — rebuilding from source, app will relaunch";
      banner.setAttribute("aria-label", "Updating");
    } else {
      const short = state.update.commit ? state.update.commit.slice(0, 7) : "new commit";
      banner.textContent = `Update available — ${short} on remote`;
      banner.setAttribute("aria-label", `Update available ${short}`);
      const btn = createButton("Update & Relaunch", { size: "sm", ariaLabel: "Update and relaunch app", onClick: onUpdate });
      banner.appendChild(btn);
    }
    mount.appendChild(banner);
  }
  if (state.serverStatus !== "running") {
    const banner = document.createElement("div");
    banner.className = "banner";
    banner.setAttribute("role", state.serverStatus === "stopped" ? "alert" : "status");
    banner.setAttribute("aria-live", state.serverStatus === "stopped" ? "assertive" : "polite");
    banner.textContent = state.serverStatus === "starting" ? "Server starting…" : "Server not running";
    if (state.serverStatus === "stopped") {
      const btn = createButton("Restart", { size: "sm", ariaLabel: "Restart server", onClick: onRestart });
      banner.appendChild(btn);
    }
    mount.appendChild(banner);
  }
}
