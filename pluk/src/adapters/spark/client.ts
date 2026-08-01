import { existsSync } from "fs";
import { homedir } from "os";
import type { Integration } from "../../store/integrations.js";

/**
 * The Spark CLI is a thin IPC client for a running Spark Desktop — no
 * credentials, no network, no config of its own. Everything here shells out to
 * it with an argv array (no shell), so a filter or subject can never become a
 * command; values that land in a positional slot are checked anyway so they
 * can't be read as flags.
 *
 * Spark prints text tables, not JSON, so tool output is passed through verbatim
 * — that is what the CLI's own agent skill (`spark skill`) is written against.
 */

const DEFAULT_BIN = "/usr/local/bin/spark";
const DEFAULT_TIMEOUT_S = 30;
const DEFAULT_MAX_PAGE = 25;

export interface SparkCfg {
  bin: string;
  account: string;      // "" when unset
  folder: string;
  team: string;
  maxPageSize: number;
  timeoutMs: number;
}

const expandHome = (p: string): string => (p.startsWith("~") ? `${homedir()}${p.slice(1)}` : p);

const str = (v: unknown): string => String(v ?? "").trim();

function positive(v: unknown, fallback: number): number {
  const n = Number(v);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : fallback;
}

export function sparkConfig(conn: Integration): SparkCfg {
  const c = conn.config;
  return {
    bin: expandHome(str(c.spark_bin)) || DEFAULT_BIN,
    account: str(c.default_account),
    folder: str(c.default_folder),
    team: str(c.default_team),
    maxPageSize: positive(c.max_page_size, DEFAULT_MAX_PAGE),
    timeoutMs: positive(c.timeout_seconds, DEFAULT_TIMEOUT_S) * 1000,
  };
}

// ── Argument helpers ─────────────────────────────────────────────────────────

/** A value in a positional slot must not read as a flag — swift-argument-parser
 *  would consume it as one. Flag *values* are safe; only positions are checked. */
export function assertPositional(value: string, what: string): string {
  const v = value.trim();
  if (!v) throw new Error(`${what} is required.`);
  if (v.startsWith("-")) throw new Error(`Invalid ${what} "${value}" — it must not start with "-".`);
  return v;
}

/** Message ids are either Spark's numeric pk or one of its deep-link forms. */
const LINK_RE = /^(https:\/\/sparkmailapp\.com\/|readdle-spark:\/\/|readdlespark:\/\/)/;

export function assertMessageId(value: unknown, what = "message id"): string {
  const v = str(value);
  if (!v) throw new Error(`${what} is required.`);
  if (!/^\d+$/.test(v) && !LINK_RE.test(v)) {
    throw new Error(`Invalid ${what} "${v}" — pass a numeric id from list_emails or a Spark deep link.`);
  }
  return v;
}

// ── Account scope ────────────────────────────────────────────────────────────

/** Every mailbox identifier names its mailbox first: `account`, `account:Folder`,
 *  `"Team Name[:Folder]"` or a shared-inbox address. A bare name (`Inbox`,
 *  `Archive`) is Spark's *unified* folder, which spans every account. */
const mailboxOf = (id: string): string => {
  const colon = id.indexOf(":");
  return colon === -1 ? id : id.slice(0, colon);
};

const outOfScope = (account: string, what: string, value: string): Error =>
  new Error(
    `This integration is scoped to ${account}; ${what} "${value}" is another mailbox. Omit it to use ${account}, or clear the integration's Account setting to reach every mailbox.`,
  );

/**
 * Confine a folder, search scope or calendar to the configured account. A bare
 * name is *qualified* with it — left alone Spark would read the cross-account
 * unified folder — and anything naming another account, shared inbox or team is
 * refused rather than quietly redirected, so an agent that asks for the wrong
 * mailbox is told instead of handed someone else's mail. No account configured,
 * or no value: unchanged.
 */
export function scoped(cfg: SparkCfg, value: unknown, what = "folder"): string {
  const v = str(value);
  if (!v || !cfg.account) return v;
  if (mailboxOf(v).toLowerCase() === cfg.account.toLowerCase()) return v;
  if (!v.includes(":") && !v.includes("@")) return `${cfg.account}:${v}`;
  throw outOfScope(cfg.account, what, v);
}

/** The account-only form, for arguments that are a bare address (`folders`, a
 *  draft's from address): nothing to qualify, so it either matches the scope or
 *  is out of it. Empty falls back to the configured account. */
export function sameAccount(cfg: SparkCfg, value: unknown, what = "account"): string {
  const v = str(value);
  if (!cfg.account) return v;
  if (!v) return cfg.account;
  if (v.toLowerCase() !== cfg.account.toLowerCase()) throw outOfScope(cfg.account, what, v);
  return v;
}

/** Normalize an argument that may arrive as a single string or a list. */
export function list(value: unknown): string[] {
  const items = Array.isArray(value) ? value : value === undefined || value === null ? [] : [value];
  return items.map((v) => str(v)).filter(Boolean);
}

/** Append `--flag value` when the value is present. */
export function flag(args: string[], name: string, value: unknown): void {
  const v = str(value);
  if (v) args.push(name, v);
}

/** Append `--flag value` once per item. */
export function flagEach(args: string[], name: string, value: unknown): void {
  for (const item of list(value)) args.push(name, item);
}

/** Append a bare `--flag` when the value is true. */
export function toggle(args: string[], name: string, value: unknown): void {
  if (value === true) args.push(name);
}

/** Pagination shared by emails, search, meetings and templates. Page size is
 *  capped by the integration: Spark prints full bodies, so an unbounded page is
 *  a token bomb. */
export function paging(args: string[], cfg: SparkCfg, a: Record<string, unknown>): void {
  const page = Number(a.page);
  if (Number.isFinite(page) && page > 1) args.push("--page", String(Math.floor(page)));

  const asked = Number(a.page_size);
  const size = Number.isFinite(asked) && asked > 0 ? Math.floor(asked) : cfg.maxPageSize;
  args.push("--page-size", String(Math.min(size, cfg.maxPageSize)));
}

/** The mutually exclusive date range shared by `events` and `availability`. */
export function range(args: string[], a: Record<string, unknown>): void {
  const start = str(a.start);
  const end = str(a.end);
  if (start || end) {
    flag(args, "--start", start);
    flag(args, "--end", end);
    return;
  }
  const shortcut = str(a.range);
  if (shortcut) args.push(`--${shortcut}`);
}

// ── Process ──────────────────────────────────────────────────────────────────

export async function runSpark(cfg: SparkCfg, args: string[]): Promise<string> {
  if (!existsSync(cfg.bin)) {
    throw new Error(`Spark CLI not found: ${cfg.bin}. Install Spark Desktop, or set the CLI path on this integration.`);
  }

  const proc = Bun.spawn([cfg.bin, ...args], { stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    proc.kill();
  }, cfg.timeoutMs);

  try {
    const [stdout, stderr] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
    const code = await proc.exited;
    if (timedOut) throw new Error(`spark ${args[0]} timed out after ${cfg.timeoutMs / 1000}s.`);
    if (code !== 0) throw new Error((stderr || stdout).trim() || `spark ${args[0]} failed (exit ${code}).`);
    return stdout.trim() || "(no output)";
  } finally {
    clearTimeout(timer);
  }
}

/** Spark's own gate (read-only / triage / send, per account) and a stopped
 *  desktop are the two failures a user can actually fix — say how. */
export function humanizeSparkError(error: unknown): string {
  const msg = (error as Error)?.message ?? String(error);
  if (/Spark Desktop running|Connection refused|connect/i.test(msg)) {
    return `${msg}\n\nSpark Desktop must be running with its CLI server enabled (Settings → AI Agents).`;
  }
  if (/access level|read-only|triage|send access/i.test(msg)) {
    return `${msg}\n\nRaise the account's access level in Spark Desktop → Settings → AI Agents.`;
  }
  return msg;
}

export async function testSpark(conn: Integration): Promise<void> {
  const cfg = sparkConfig(conn);
  await runSpark(cfg, ["--version"]);
  // `accounts` is the cheapest call that proves the desktop IPC is actually up.
  await runSpark(cfg, ["accounts"]);
}
