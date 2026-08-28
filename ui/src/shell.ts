import "./shell.css";
import { createButton } from "./primitives";

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
  const reduce = typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
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
    if (!reduce) banner.style.transition = "transform 200ms ease, opacity 200ms ease";
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
    if (!reduce) banner.style.transition = "transform 200ms ease, opacity 200ms ease";
    mount.appendChild(banner);
  }
}
