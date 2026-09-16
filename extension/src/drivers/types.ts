import type { Action, CommandEnvelope, DebugRequest, Platform } from "../protocol";

export type DriverPageState =
  | "ready"
  | "waiting"
  | "login_required"
  | "unsupported"
  | "target_not_found"
  | "account_unverified"
  | "target_mismatch"
  | "submission_succeeded"
  | "submission_unknown";

export interface DriverScriptOptions {
  readonly action: Action;
  readonly targetUrl: string;
  readonly postId?: string;
  readonly text?: string;
  /** The posts of a thread, in order. Absent or one entry for a plain post. */
  readonly parts?: readonly string[];
  /** A reply's own images, approved with this submission, in order — the
   * exact bytes staging captured, already fetched and base64-encoded, ready
   * to paste. A reply is always one part, so there is nothing to associate
   * this list with beyond the reply itself. */
  readonly images?: readonly { readonly data: string; readonly contentType: string }[];
  /** One entry per part of a post, that part's own approved images — an
   * image-free part is a present, empty entry. Same length as `parts`, or a
   * single entry for a plain post's one implicit part. */
  readonly partImages?: readonly (readonly { readonly data: string; readonly contentType: string }[])[];
  /** Attach the page's capture buffer to a successful read too, filtered to
   * a URL glob when `debug` is a string rather than `true`. */
  readonly debug?: DebugRequest;
}

export interface DriverPageResult {
  readonly state: DriverPageState;
  readonly [key: string]: unknown;
}

export type DriverPageScript = (
  options: DriverScriptOptions,
) => DriverPageResult | Promise<DriverPageResult>;

export interface SiteDriver {
  readonly platform: Platform;
  readonly capabilities: readonly Action[];
  readonly navigationTarget: (command: CommandEnvelope) => string;
  readonly pageScript: DriverPageScript;
}
