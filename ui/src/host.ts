/**
 * Bridge to the Tauri host. Every backend call goes through here: the window
 * talks to the Rust commands, never to the loopback HTTP server, so the
 * packaged app and the dev server behave the same.
 */

type TauriGlobal = {
  core?: { invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
  event?: {
    listen?: (event: string, fn: (e: { payload: unknown }) => void) => Promise<() => void>;
  };
};

function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

/** False in a plain browser tab, where no host is attached. */
export function hasHost(): boolean {
  return typeof tauri()?.core?.invoke === "function";
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const call = tauri()?.core?.invoke;
  if (!call) throw new Error(`No Pluk host attached — cannot run ${cmd}`);
  return (await call(cmd, args)) as T;
}

/** Subscribe to a host event. Resolves to an unlisten function. */
export async function listen<T>(event: string, fn: (payload: T) => void): Promise<() => void> {
  const subscribe = tauri()?.event?.listen;
  if (!subscribe) return () => {};
  return subscribe(event, (e) => fn(e.payload as T));
}
