/**
 * Zoom bridge — the host owns the scale.
 * R15 defines the bridge; this module reads from it and applies scale to
 * typography only (never as a CSS transform on the page).
 *
 * Host contract (Tauri):
 *  - `invoke("get_zoom_scale")` returns { scale, steps, defaultIndex } when available
 *  - event "zoom-changed" with payload scale when host changes zoom via menu
 * Fallback: localStorage + keyboard shortcuts, same steps as AppZoom.swift
 */

import { ZoomSteps, DefaultZoomIndex } from "./tokens";

const STORAGE_KEY = "PlukUIZoomStep";

export type ZoomState = {
  scale: number;
  steps: readonly number[];
  defaultIndex: number;
  canZoomIn: boolean;
  canZoomOut: boolean;
  isDefault: boolean;
  label: string;
};

function readStoredIndex(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw != null) {
      const n = parseInt(raw, 10);
      if (!Number.isNaN(n) && n >= 0 && n < ZoomSteps.length) return n;
    }
  } catch {
    // ignore
  }
  return DefaultZoomIndex;
}

function writeStoredIndex(i: number): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(i));
  } catch {
    // ignore
  }
}

export class Zoom {
  private index = readStoredIndex();
  private listeners = new Set<(s: ZoomState) => void>();

  get state(): ZoomState {
    const scale = ZoomSteps[this.index];
    return {
      scale,
      steps: ZoomSteps,
      defaultIndex: DefaultZoomIndex,
      canZoomIn: this.index < ZoomSteps.length - 1,
      canZoomOut: this.index > 0,
      isDefault: this.index === DefaultZoomIndex,
      label: `${Math.round(scale * 100)}%`,
    };
  }

  get scale(): number {
    return ZoomSteps[this.index];
  }

  subscribe(fn: (s: ZoomState) => void): () => void {
    this.listeners.add(fn);
    fn(this.state);
    return () => this.listeners.delete(fn);
  }

  private notify(): void {
    const s = this.state;
    for (const fn of this.listeners) fn(s);
    // Apply to typography only
    document.documentElement.style.setProperty("--zoom-scale", String(s.scale));
  }

  private setIndex(next: number): void {
    const clamped = Math.max(0, Math.min(ZoomSteps.length - 1, next));
    if (clamped === this.index) return;
    this.index = clamped;
    writeStoredIndex(clamped);
    this.notify();
  }

  zoomIn(): void {
    this.setIndex(this.index + 1);
  }
  zoomOut(): void {
    this.setIndex(this.index - 1);
  }
  reset(): void {
    this.setIndex(DefaultZoomIndex);
  }

  /** Apply initial scale to CSS var. */
  apply(): void {
    document.documentElement.style.setProperty("--zoom-scale", String(this.scale));
  }

  /** Try to sync from Tauri host. Silently falls back to localStorage. */
  async syncFromHost(): Promise<void> {
    const tauri = (window as unknown as { __TAURI__?: { core?: { invoke?: (cmd: string) => Promise<unknown> }; event?: { listen?: (ev: string, fn: (e: { payload: unknown }) => void) => Promise<unknown> } } }).__TAURI__;
    if (!tauri?.core?.invoke) {
      this.apply();
      return;
    }
    try {
      const res = (await tauri.core.invoke("get_zoom_scale")) as { scale?: number; index?: number };
      if (typeof res?.index === "number" && res.index >= 0 && res.index < ZoomSteps.length) {
        this.index = res.index;
        this.apply();
      } else if (typeof res?.scale === "number") {
        const idx = ZoomSteps.indexOf(res.scale as (typeof ZoomSteps)[number]);
        if (idx !== -1) this.index = idx;
        this.apply();
      }
    } catch {
      this.apply();
    }
    // Listen for host-driven changes
    try {
      const listen = tauri.event?.listen;
      if (listen) {
        await listen("zoom-changed", (e) => {
          const p = e.payload as { scale?: number; index?: number };
          if (typeof p?.index === "number") this.setIndex(p.index);
          else if (typeof p?.scale === "number") {
            const idx = ZoomSteps.indexOf(p.scale as (typeof ZoomSteps)[number]);
            if (idx !== -1) this.setIndex(idx);
          }
        });
      }
    } catch {
      // ignore
    }
  }

  bindKeyboard(): void {
    window.addEventListener("keydown", (e) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod) return;
      if ((e.key === "+" || e.key === "=") && !e.shiftKey) {
        // Cmd + = (plus) is captured below with shift; keep for Intl keyboards
      }
      if ((e.key === "+" || e.key === "=") && mod) {
        e.preventDefault();
        this.zoomIn();
      } else if (e.key === "-" && mod) {
        e.preventDefault();
        this.zoomOut();
      } else if (e.key === "0" && mod) {
        e.preventDefault();
        this.reset();
      }
    });
    // Also Cmd+Shift+= for plus on US layout
    window.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "=") {
        e.preventDefault();
        this.zoomIn();
      }
    });
  }
}

export const zoom = new Zoom();
