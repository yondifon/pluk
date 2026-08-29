/**
 * Toasts — one stack in the bottom-right corner, newest nearest the corner.
 * A pending toast resolves into its own success or error, so an action reports
 * once rather than twice. Hovering or focusing the stack expands it and holds
 * every countdown.
 */

import { createIcon, type IconName } from "./icon";
import { createButton } from "./primitives";

export type ToastVariant = "success" | "error" | "info" | "pending";

export type ToastOptions = {
  description?: string;
  /** Long text the toast reveals on request and scrolls, never truncates away. */
  detail?: string;
  action?: { label: string; onClick: () => void };
};

type ToastRecord = ToastOptions & {
  id: string;
  variant: ToastVariant;
  title: string;
};

export type PendingToast = {
  success(title: string, options?: ToastOptions): void;
  error(title: string, options?: ToastOptions): void;
};

export const AUTO_DISMISS_MS = 4000;
export const VISIBLE_TOASTS = 3;
const MAX_TOASTS = 5;
const EXIT_MS = 220;

const ICONS: Record<ToastVariant, IconName> = {
  success: "check",
  error: "error",
  info: "info",
  pending: "spinner",
};

type Countdown = { remaining: number; startedAt: number; handle: ReturnType<typeof setTimeout> | null };

const records: ToastRecord[] = [];
const countdowns = new Map<string, Countdown>();
const listeners = new Set<() => void>();
let sequence = 0;
let held = false;

function notify(): void {
  for (const listener of listeners) listener();
}

function durationFor(variant: ToastVariant): number | null {
  return variant === "success" || variant === "info" ? AUTO_DISMISS_MS : null;
}

function startCountdown(id: string, countdown: Countdown): void {
  countdown.startedAt = Date.now();
  countdown.handle = setTimeout(() => dismiss(id), countdown.remaining);
}

function schedule(id: string, duration: number | null): void {
  clearCountdown(id);
  if (duration == null) return;
  const countdown: Countdown = { remaining: duration, startedAt: Date.now(), handle: null };
  countdowns.set(id, countdown);
  if (!held) startCountdown(id, countdown);
}

function clearCountdown(id: string): void {
  const countdown = countdowns.get(id);
  if (countdown?.handle) clearTimeout(countdown.handle);
  countdowns.delete(id);
}

function add(variant: ToastVariant, title: string, options?: ToastOptions): string {
  const id = `toast-${++sequence}`;
  records.push({ id, variant, title, ...options });
  while (records.length > MAX_TOASTS) clearCountdown(records.shift()!.id);
  schedule(id, durationFor(variant));
  notify();
  return id;
}

function resolve(id: string, variant: ToastVariant, title: string, options?: ToastOptions): void {
  const index = records.findIndex((record) => record.id === id);
  if (index === -1) return;
  records[index] = { id, variant, title, ...options };
  schedule(id, durationFor(variant));
  notify();
}

function dismiss(id: string): void {
  const index = records.findIndex((record) => record.id === id);
  if (index === -1) return;
  records.splice(index, 1);
  clearCountdown(id);
  notify();
}

function clear(): void {
  for (const record of records) clearCountdown(record.id);
  records.length = 0;
  notify();
}

function hold(): void {
  if (held) return;
  held = true;
  for (const countdown of countdowns.values()) {
    if (!countdown.handle) continue;
    clearTimeout(countdown.handle);
    countdown.handle = null;
    countdown.remaining = Math.max(0, countdown.remaining - (Date.now() - countdown.startedAt));
  }
}

function release(): void {
  if (!held) return;
  held = false;
  for (const [id, countdown] of countdowns) startCountdown(id, countdown);
}

export const toast = {
  success: (title: string, options?: ToastOptions): void => void add("success", title, options),
  error: (title: string, options?: ToastOptions): void => void add("error", title, options),
  info: (title: string, options?: ToastOptions): void => void add("info", title, options),
  pending(title: string, options?: ToastOptions): PendingToast {
    const id = add("pending", title, options);
    return {
      success: (nextTitle, nextOptions) => resolve(id, "success", nextTitle, nextOptions),
      error: (nextTitle, nextOptions) => resolve(id, "error", nextTitle, nextOptions),
    };
  },
  clear,
};

/** Renders the stack into its single mount point in the shell. */
export function mountToaster(container: HTMLElement): () => void {
  container.className = "toaster";
  container.setAttribute("role", "region");
  container.setAttribute("aria-label", "Notifications");
  container.setAttribute("aria-live", "polite");
  container.setAttribute("aria-atomic", "false");

  const elements = new Map<string, HTMLElement>();
  const overflow = document.createElement("div");
  overflow.className = "toast-overflow";
  overflow.setAttribute("aria-hidden", "true");
  let expanded = false;

  function fill(element: HTMLElement, record: ToastRecord): void {
    element.dataset.variant = record.variant;
    element.setAttribute("role", record.variant === "error" ? "alert" : "status");
    element.setAttribute("aria-live", record.variant === "error" ? "assertive" : "polite");

    const icon = document.createElement("span");
    icon.className = "toast-icon";
    icon.appendChild(createIcon(ICONS[record.variant]));

    const body = document.createElement("div");
    body.className = "toast-body";

    const title = document.createElement("p");
    title.className = "toast-title";
    title.textContent = record.title;
    body.appendChild(title);

    if (record.description) {
      const description = document.createElement("p");
      description.className = "toast-description";
      description.textContent = record.description;
      body.appendChild(description);
    }

    if (record.detail) {
      const detail = document.createElement("pre");
      detail.className = "toast-detail";
      detail.id = `${record.id}-detail`;
      detail.textContent = record.detail;
      detail.hidden = true;
      const toggle = createButton("Show details", { size: "sm" });
      toggle.classList.add("toast-detail-toggle");
      toggle.setAttribute("aria-expanded", "false");
      toggle.setAttribute("aria-controls", detail.id);
      toggle.addEventListener("click", () => {
        const opening = detail.hidden;
        detail.hidden = !opening;
        toggle.setAttribute("aria-expanded", String(opening));
        toggle.replaceChildren(document.createTextNode(opening ? "Hide details" : "Show details"));
        layout();
      });
      body.append(toggle, detail);
    }

    if (record.action) {
      const action = createButton(record.action.label, {
        variant: "secondary",
        size: "sm",
        onClick: record.action.onClick,
      });
      action.classList.add("toast-action");
      body.appendChild(action);
    }

    const close = createButton("", {
      icon: "close",
      ariaLabel: "Dismiss notification",
      onClick: () => dismiss(record.id),
    });
    close.classList.add("icon-button", "toast-close");

    element.replaceChildren(icon, body, close);
  }

  function layout(): void {
    const stack: HTMLElement[] = [];
    for (let i = records.length - 1; i >= 0; i--) {
      const element = elements.get(records[i].id);
      if (element) stack.push(element);
    }

    let stacked = 0;
    stack.forEach((element, index) => {
      const hidden = !expanded && index >= VISIBLE_TOASTS;
      element.toggleAttribute("data-hidden", hidden);
      element.style.setProperty(
        "--toast-y",
        expanded
          ? `calc(-1 * (${stacked}px + ${index} * var(--space-sm)))`
          : `calc(-1 * ${index} * var(--space-md))`,
      );
      element.style.setProperty(
        "--toast-scale",
        expanded ? "1" : String(1 - Math.min(index, VISIBLE_TOASTS) * 0.04),
      );
      stacked += element.offsetHeight;
    });

    const frontHeight = stack[0]?.offsetHeight ?? 0;
    const overflowing = !expanded && stack.length > VISIBLE_TOASTS;
    if (overflowing) {
      overflow.textContent = `+${stack.length - VISIBLE_TOASTS} more`;
      overflow.style.setProperty(
        "--toast-y",
        `calc(-1 * (${frontHeight}px + ${VISIBLE_TOASTS} * var(--space-md)))`,
      );
      container.appendChild(overflow);
    } else {
      overflow.remove();
    }

    container.toggleAttribute("data-empty", stack.length === 0);
    const overflowRoom = overflowing ? " + var(--space-lg)" : "";
    container.style.height = expanded
      ? `calc(${stacked}px + ${Math.max(stack.length - 1, 0)} * var(--space-sm))`
      : `calc(${frontHeight}px + ${Math.min(stack.length, VISIBLE_TOASTS)} * var(--space-md)${overflowRoom})`;
  }

  function setExpanded(next: boolean): void {
    if (expanded === next) return;
    expanded = next;
    if (next) hold();
    else release();
    layout();
  }

  function render(): void {
    for (const record of records) {
      const existing = elements.get(record.id);
      if (existing) {
        fill(existing, record);
        continue;
      }
      const element = document.createElement("div");
      element.className = "toast";
      element.dataset.toastId = record.id;
      element.dataset.enter = "true";
      fill(element, record);
      elements.set(record.id, element);
      container.appendChild(element);
      requestAnimationFrame(() => {
        delete element.dataset.enter;
      });
    }

    const live = new Set(records.map((record) => record.id));
    for (const [id, element] of elements) {
      if (live.has(id)) continue;
      elements.delete(id);
      element.dataset.exit = "true";
      setTimeout(() => element.remove(), EXIT_MS);
    }

    if (!records.length) setExpanded(false);
    layout();
  }

  container.addEventListener("mouseenter", () => setExpanded(true));
  container.addEventListener("mouseleave", () => setExpanded(false));
  container.addEventListener("focusin", () => setExpanded(true));
  container.addEventListener("focusout", (event) => {
    if (!container.contains(event.relatedTarget as Node | null)) setExpanded(false);
  });
  container.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const element = (event.target as HTMLElement).closest<HTMLElement>(".toast");
    if (!element?.dataset.toastId) return;
    event.preventDefault();
    dismiss(element.dataset.toastId);
  });

  listeners.add(render);
  render();
  return () => {
    listeners.delete(render);
  };
}
