/**
 * Toast system — one toast at a time per integration, newer replaces previous.
 * Errors persist longer than successes and also raise a system notification.
 * Animations respect `prefers-reduced-motion`.
 * Mirrors `swift/Sources/Toast.swift#ToastCenter`.
 */

export type ToastKind = "error" | "success";

export type Toast = {
  id: string;
  integrationId: string;
  title: string;
  message: string;
  kind: ToastKind;
};

export const ERROR_LIFETIME_MS = 8000;
export const SUCCESS_LIFETIME_MS = 3000;

function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export type ToastListener = (toasts: Toast[]) => void;

export class ToastCenter {
  private toasts: Toast[] = [];
  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  private listeners = new Set<ToastListener>();
  private _onRetry: (integrationId: string) => void;

  constructor(onRetry?: (integrationId: string) => void) {
    this._onRetry = onRetry ?? (() => {});
  }

  set onRetry(fn: (id: string) => void) {
    this._onRetry = fn;
  }

  get all(): Toast[] {
    return [...this.toasts];
  }

  subscribe(fn: ToastListener): () => void {
    this.listeners.add(fn);
    fn(this.all);
    return () => this.listeners.delete(fn);
  }

  private notify(): void {
    const snap = this.all;
    for (const fn of this.listeners) fn(snap);
  }

  private lifetime(kind: ToastKind): number {
    return kind === "error" ? ERROR_LIFETIME_MS : SUCCESS_LIFETIME_MS;
  }

  present(toast: Omit<Toast, "id"> & { id?: string }): Toast {
    const full: Toast = {
      id: toast.id ?? `${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      ...toast,
    };

    // Replace any existing toast for same integration
    const existingIdx = this.toasts.findIndex((t) => t.integrationId === full.integrationId);
    if (existingIdx !== -1) {
      const existing = this.toasts[existingIdx];
      const timer = this.timers.get(existing.id);
      if (timer) clearTimeout(timer);
      this.timers.delete(existing.id);
      this.toasts.splice(existingIdx, 1);
    }

    this.toasts.push(full);

    if (full.kind === "error") this.postNotification(full);

    const lifetime = this.lifetime(full.kind);
    const timer = setTimeout(() => this.dismiss(full.id), lifetime);
    this.timers.set(full.id, timer);

    this.notify();
    return full;
  }

  dismiss(id: string): void {
    const idx = this.toasts.findIndex((t) => t.id === id);
    if (idx === -1) return;
    this.toasts.splice(idx, 1);
    const timer = this.timers.get(id);
    if (timer) clearTimeout(timer);
    this.timers.delete(id);
    this.notify();
  }

  clear(): void {
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
    this.toasts = [];
    this.notify();
  }

  private postNotification(toast: Toast): void {
    try {
      // Web Notifications API (Tauri maps to native notification when permitted)
      if (typeof Notification !== "undefined" && Notification.permission === "granted") {
        new Notification(toast.title, { body: toast.message });
        return;
      }
      // Tauri notification plugin fallback
      const tauri = (window as unknown as { __TAURI__?: { notification?: { sendNotification: (o: unknown) => void } } }).__TAURI__;
      if (tauri?.notification?.sendNotification) {
        tauri.notification.sendNotification({ title: toast.title, body: toast.message });
      }
    } catch {
      // best-effort
    }
  }

  // For testing: expose lifetimes
  lifetimeFor(kind: ToastKind): number {
    return this.lifetime(kind);
  }

  shouldAnimate(): boolean {
    return !prefersReducedMotion();
  }
}

/** Render toasts into a mount element. Respects reduced-motion. */
export function renderToasts(
  mount: HTMLElement,
  center: ToastCenter,
  onRetry: (integrationId: string) => void,
): () => void {
  function render(toasts: Toast[]): void {
    mount.innerHTML = "";
    const reduce = !center.shouldAnimate();
    for (const toast of toasts) {
      const card = document.createElement("div");
      card.className = `toast-card toast-${toast.kind}`;
      card.setAttribute("role", toast.kind === "error" ? "alert" : "status");
      card.setAttribute("aria-live", toast.kind === "error" ? "assertive" : "polite");
      card.dataset.toastId = toast.id;
      card.dataset.integrationId = toast.integrationId;

      const icon = document.createElement("span");
      icon.className = "toast-icon";
      icon.textContent = toast.kind === "error" ? "⚠" : "✓";
      icon.setAttribute("aria-hidden", "true");

      const body = document.createElement("div");
      body.className = "toast-body";
      const title = document.createElement("div");
      title.className = "toast-title";
      title.textContent = toast.title;
      const msg = document.createElement("div");
      msg.className = "toast-message";
      msg.textContent = toast.message;
      body.append(title, msg);

      if (toast.kind === "error") {
        const retry = document.createElement("button");
        retry.className = "btn btn-sm";
        retry.textContent = "Retry";
        retry.setAttribute("aria-label", `Retry connection for ${toast.title}`);
        retry.addEventListener("click", () => onRetry(toast.integrationId));
        body.appendChild(retry);
      }

      const dismiss = document.createElement("button");
      dismiss.className = "toast-dismiss";
      dismiss.textContent = "×";
      dismiss.setAttribute("aria-label", "Dismiss notification");
      dismiss.addEventListener("click", () => center.dismiss(toast.id));

      card.append(icon, body, dismiss);

      if (!reduce) {
        card.style.animation = "toast-in 180ms ease-out";
      }

      mount.appendChild(card);
    }
  }

  const unsub = center.subscribe(render);
  return unsub;
}
