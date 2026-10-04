/**
 * The host reports window show and hide because WKWebView does not report a
 * hidden NSWindow through `document.visibilityState`.
 */

import { hasHost, invoke, listen } from "./host";

export interface Visibility {
  isVisible(): boolean;
  subscribe(fn: (visible: boolean) => void): () => void;
}

export class WindowVisibility implements Visibility {
  // Shown until the host says otherwise: the window may be shown at launch.
  private visible = true;
  private listeners = new Set<(visible: boolean) => void>();

  isVisible(): boolean {
    return this.visible;
  }

  setVisible(next: boolean): void {
    if (next === this.visible) return;
    this.visible = next;
    for (const fn of this.listeners) fn(next);
  }

  subscribe(fn: (visible: boolean) => void): () => void {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  }
}

export const windowVisibility = new WindowVisibility();

// No host means a plain browser tab, which is always visible: every poll runs.
if (typeof window !== "undefined" && hasHost()) {
  let heardFromHost = false;
  void listen("pluk://window-shown", () => {
    heardFromHost = true;
    windowVisibility.setVisible(true);
  });
  void listen("pluk://window-hidden", () => {
    heardFromHost = true;
    windowVisibility.setVisible(false);
  });
  // A launch at login starts with the window hidden, and no event says so.
  void invoke<boolean>("plugin:window|is_visible", { label: "main" })
    .then((visible) => {
      if (!heardFromHost) windowVisibility.setVisible(visible);
    })
    .catch(() => {});
}

export function isWindowVisible(): boolean {
  return windowVisibility.isVisible();
}

/** Runs `fn` on an interval while the window is visible, and once when it returns. */
export function pollWhileVisible(
  fn: () => void,
  ms: number,
  visibility: Visibility = windowVisibility,
): () => void {
  let timer: ReturnType<typeof setInterval> | null = null;
  let stopped = false;

  const start = (): void => {
    if (stopped || timer !== null || !visibility.isVisible()) return;
    timer = setInterval(fn, ms);
  };

  const unsubscribe = visibility.subscribe((visible) => {
    if (stopped) return;
    if (visible) {
      fn();
      start();
    } else {
      if (timer !== null) clearInterval(timer);
      timer = null;
    }
  });

  start();

  return () => {
    stopped = true;
    unsubscribe();
    if (timer !== null) clearInterval(timer);
    timer = null;
  };
}
