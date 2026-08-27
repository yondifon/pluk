import "./shell.css";

export type BannerState = {
  update?: { commit?: string; updating: boolean };
  serverStatus: "running" | "starting" | "stopped";
};

export function createShell(
  sidebarEl: HTMLElement,
  detailEl: HTMLElement,
): { root: HTMLElement; detailMount: HTMLElement; bottomMount: HTMLElement; toastMount: HTMLElement } {
  const root = document.createElement("div");
  root.className = "shell";

  const sidebarWrap = document.createElement("div");
  sidebarWrap.className = "shell-sidebar";
  sidebarWrap.appendChild(sidebarEl);

  const resizer = document.createElement("div");
  resizer.className = "shell-resizer";
  // simple drag resize
  let startX = 0;
  let startW = 0;
  resizer.addEventListener("mousedown", (e) => {
    startX = e.clientX;
    startW = sidebarWrap.getBoundingClientRect().width;
    const onMove = (ev: MouseEvent) => {
      const delta = ev.clientX - startX;
      const next = Math.max(220, Math.min(320, startW + delta));
      sidebarWrap.style.width = `${next}px`;
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });

  const main = document.createElement("div");
  main.className = "shell-main";

  const detail = document.createElement("div");
  detail.className = "shell-detail";
  detail.appendChild(detailEl);

  const bottom = document.createElement("div");
  bottom.className = "shell-bottom";

  const toastMount = document.createElement("div");
  toastMount.className = "toast-mount";
  toastMount.id = "toast-mount";
  toastMount.setAttribute("aria-live", "polite");

  main.append(detail, bottom, toastMount);
  root.append(sidebarWrap, resizer, main);

  return { root, detailMount: detail, bottomMount: bottom, toastMount };
}

export function renderBanners(mount: HTMLElement, state: BannerState, onRestart: () => void, onUpdate: () => void): void {
  mount.innerHTML = "";
  if (state.update) {
    const banner = document.createElement("div");
    banner.className = "banner";
    if (state.update.updating) {
      banner.textContent = "Updating — rebuilding from source, app will relaunch";
    } else {
      const short = state.update.commit ? state.update.commit.slice(0, 7) : "new commit";
      banner.textContent = `Update available — ${short} on remote`;
      const btn = document.createElement("button");
      btn.textContent = "Update & Relaunch";
      btn.onclick = onUpdate;
      banner.appendChild(btn);
    }
    mount.appendChild(banner);
  }
  if (state.serverStatus !== "running") {
    const banner = document.createElement("div");
    banner.className = "banner";
    banner.textContent = state.serverStatus === "starting" ? "Server starting…" : "Server not running";
    if (state.serverStatus === "stopped") {
      const btn = document.createElement("button");
      btn.textContent = "Restart";
      btn.onclick = onRestart;
      banner.appendChild(btn);
    }
    mount.appendChild(banner);
  }
}
