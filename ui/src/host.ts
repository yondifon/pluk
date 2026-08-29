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

/**
 * Native folder chooser. Resolves to the chosen path, or null when the user
 * cancels. Throws when the chooser cannot open — a caller must tell the user
 * that, not mistake it for a cancellation.
 */
export async function pickDirectory(title: string): Promise<string | null> {
  const picked = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: { directory: true, multiple: false, title },
  });
  if (Array.isArray(picked)) return picked[0] ?? null;
  return picked;
}

/**
 * Register an MCP server in one AI client's config file.
 *
 * Argument names must stay camelCase: Tauri converts a command's Rust
 * parameters to camelCase, so `project_dir` arrives as a missing argument and
 * the host rejects the call.
 */
export async function injectMcpConfig(args: {
  client: string;
  scope: string;
  projectDir: string | null;
  key: string;
  url: string;
}): Promise<{ status: "added" | "skipped"; path: string }> {
  return invoke("inject_mcp_config", {
    client: args.client,
    scope: args.scope,
    projectDir: args.projectDir,
    key: args.key,
    url: args.url,
  });
}

/** Subscribe to a host event. Resolves to an unlisten function. */
export async function listen<T>(event: string, fn: (payload: T) => void): Promise<() => void> {
  const subscribe = tauri()?.event?.listen;
  if (!subscribe) return () => {};
  return subscribe(event, (e) => fn(e.payload as T));
}
