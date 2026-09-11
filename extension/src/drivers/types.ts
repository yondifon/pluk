import type { Action, CommandEnvelope, Platform } from "../protocol";

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
