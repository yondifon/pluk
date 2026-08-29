/**
 * Update notices.
 *
 * The host drives the whole update: it checks on launch, every six hours, and
 * whenever someone picks "Check for Updates…". The window only reacts — it
 * announces a new version once, offers to install it, and reports a failure
 * worth acting on. A build with no update endpoint stays silent.
 */

import { hasHost, invoke, listen } from "./host.ts";
import { toast, type PendingToast } from "./toast.ts";

const STATE_EVENT = "pluk://update-state";
const NO_UPDATE_EVENT = "pluk://update-none";

export type UpdateFailureKind = "unreachable" | "download" | "signature" | "other";

export type UpdateState =
  | { type: "disabled"; reason: string }
  | { type: "idle" }
  | { type: "checking" }
  | { type: "upToDate" }
  | { type: "available"; version: string; notes: string | null }
  | { type: "downloading"; progress: number }
  | { type: "ready"; version: string }
  | { type: "failed"; kind: UpdateFailureKind; message: string };

export type UpdateNotice =
  | { kind: "available"; version: string }
  | { kind: "failed"; message: string };

/**
 * What the window should say about a state, or null to stay quiet. An endpoint
 * we could not reach is the host's problem, not the person's — it never speaks.
 */
export function noticeFor(state: UpdateState): UpdateNotice | null {
  switch (state.type) {
    case "available":
      return { kind: "available", version: state.version };
    case "failed":
      return state.kind === "unreachable" ? null : { kind: "failed", message: state.message };
    default:
      return null;
  }
}

export async function mountUpdates(): Promise<void> {
  if (!hasHost()) return;

  let announced: string | null = null;
  let installing: PendingToast | null = null;

  async function install(version: string): Promise<void> {
    if (installing) return;
    installing = toast.pending(`Installing Pluk ${version}`, {
      description: "Pluk restarts when the update is in place.",
    });
    await invoke("install_update");
  }

  function apply(state: UpdateState): void {
    const notice = noticeFor(state);
    if (!notice) return;

    if (notice.kind === "failed") {
      const message = "Pluk could not finish the update. Try again later.";
      if (installing) {
        installing.error(message, { detail: notice.message });
        installing = null;
      } else {
        toast.error(message, { detail: notice.message });
      }
      return;
    }

    if (announced === notice.version) return;
    announced = notice.version;
    toast.info(`Pluk ${notice.version} is available`, {
      action: { label: "Install", onClick: () => void install(notice.version) },
    });
  }

  await listen<UpdateState>(STATE_EVENT, apply);
  await listen(NO_UPDATE_EVENT, () => toast.info("Pluk is up to date"));
  apply(await invoke<UpdateState>("get_update_state"));
}
