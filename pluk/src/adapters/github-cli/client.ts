// The gh-CLI bridge. Every operation shells out to the installed `gh` with an
// argv array (no shell), so a title, branch or body can never become a command.
// gh brings its own credentials (keychain / ~/.config/gh); nothing here stores
// or passes a token.

import { existsSync } from "fs";
import { homedir } from "os";
import type { Subprocess } from "bun";
import type { Integration } from "../../store/integrations.js";

const DEFAULT_TIMEOUT_MS = 30_000;

const expandHome = (p: string): string => (p.startsWith("~") ? `${homedir()}${p.slice(1)}` : p);

export interface GhConfig {
  bin: string;
  defaultRepo?: string;
  defaultCwd: string;
  timeoutMs: number;
}

export function ghConfig(conn: Integration): GhConfig {
  const c = conn.config;
  const bin = expandHome(String(c.gh_bin ?? "").trim() || "gh");
  const defaultRepo = String(c.default_repo ?? "").trim() || undefined;
  const defaultCwd = String(c.default_cwd ?? "").trim() || process.cwd();
  const timeoutMs = Number(c.timeout_seconds ?? 30) * 1000;
  return { bin, defaultRepo, defaultCwd, timeoutMs: Number.isFinite(timeoutMs) && timeoutMs > 0 ? Math.floor(timeoutMs) : DEFAULT_TIMEOUT_MS };
}

/** The working directory for a call: the caller's `cwd`, else the integration default. */
export function ghCwd(cfg: GhConfig, arg?: string): string {
  return String(arg ?? "").trim() || cfg.defaultCwd;
}

/** The `--repo` override, when the call or the integration names one. Empty means
 *  gh infers the repository from the working directory. */
export function repoFlag(cfg: GhConfig, arg?: string): string[] {
  const spec = String(arg ?? "").trim() || cfg.defaultRepo || "";
  return spec ? ["--repo", spec] : [];
}

export function ghCommand(cfg: GhConfig, args: string[]): string {
  const quote = (value: string): string => /^[A-Za-z0-9_./:@%+=,-]+$/.test(value) ? value : `'${value.replace(/'/g, "'\\''")}'`;
  return [cfg.bin, ...args].map(quote).join(" ");
}

/** A positional value must not read as a flag. */
export function positional(value: unknown, what: string): string {
  const v = String(value ?? "").trim();
  if (!v) throw new Error(`${what} is required.`);
  if (v.startsWith("-")) throw new Error(`Invalid ${what} "${value}" — it must not start with "-".`);
  return v;
}

/** Resolve `owner/repo` for the api-backed tools, which need an explicit repo. */
export function resolveRepo(cfg: GhConfig, arg?: string): { owner: string; repo: string } {
  const spec = (arg && String(arg).trim()) || cfg.defaultRepo;
  if (!spec) throw new Error("No repo given. Pass repo as owner/repo or set a default repo in the integration config.");
  const [owner, repo] = spec.split("/");
  if (!owner || !repo) throw new Error(`Invalid repo "${spec}". Use the form owner/repo.`);
  return { owner, repo };
}

export interface GhRunResult {
  code: number;
  stdout: string;
  stderr: string;
}

export type GhRunner = (bin: string, args: string[], cwd: string, timeoutMs: number) => Promise<GhRunResult>;

let testRunner: GhRunner | undefined;

/** Test seam: swap the process runner for a fake that records calls. */
export function setGhRunner(runner: GhRunner | undefined): void {
  testRunner = runner;
}

async function spawnGh(bin: string, args: string[], cwd: string, timeoutMs: number): Promise<GhRunResult> {
  if (bin.includes("/") && !existsSync(bin)) {
    throw new Error(`gh executable not found: ${bin}. Install GitHub CLI or set gh_bin on this integration.`);
  }
  let proc: Subprocess<"ignore", "pipe", "pipe">;
  try {
    proc = Bun.spawn([bin, ...args], { cwd, stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  } catch (err) {
    if (/ENOENT/.test((err as Error).message)) {
      throw new Error(`gh executable not found ("${bin}"). Install GitHub CLI and make sure it is on PATH, or set gh_bin on this integration.`);
    }
    throw new Error(`Could not start gh: ${(err as Error).message}`);
  }
  let timedOut = false;
  const timer = setTimeout(() => { timedOut = true; proc.kill(); }, timeoutMs);
  try {
    const [stdout, stderr] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
    const code = await proc.exited;
    if (timedOut) throw new Error(`gh ${args.join(" ")} timed out after ${Math.round(timeoutMs / 1000)}s.`);
    return { code, stdout, stderr };
  } finally {
    clearTimeout(timer);
  }
}

/** Run one gh command. A non-zero exit is returned, not thrown — the caller
 *  decides how to surface it. Throws only when gh cannot start or times out. */
export async function runGh(cfg: GhConfig, args: string[], cwdArg?: string): Promise<GhRunResult> {
  const cwd = ghCwd(cfg, cwdArg);
  return testRunner ? testRunner(cfg.bin, args, cwd, cfg.timeoutMs) : spawnGh(cfg.bin, args, cwd, cfg.timeoutMs);
}

function ghError(op: string, res: GhRunResult): Error {
  return new Error(`gh ${op} failed (exit ${res.code}): ${(res.stderr || res.stdout).trim() || "no output"}`);
}

/** Run a gh command that answers JSON on stdout; throws on non-zero exit. */
export async function ghJson(cfg: GhConfig, args: string[], cwdArg?: string): Promise<unknown> {
  const res = await runGh(cfg, args, cwdArg);
  if (res.code !== 0) throw ghError(args.join(" "), res);
  const text = res.stdout.trim();
  if (!text) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** Run a gh command and return its text output; throws on non-zero exit. */
export async function ghText(cfg: GhConfig, args: string[], cwdArg?: string): Promise<string> {
  const res = await runGh(cfg, args, cwdArg);
  if (res.code !== 0) throw ghError(args.join(" "), res);
  return res.stdout.trim();
}

/** Map gh's own failures to what a user can actually fix. */
export function humanizeGhError(error: unknown): string {
  const msg = (error as Error)?.message ?? String(error);
  if (/executable not found/.test(msg)) {
    return `${msg}\n\nInstall GitHub CLI (https://cli.github.com) and sign in with \`gh auth login\`.`;
  }
  if (/not authenticated|auth login|not logged|please log in|auth:/i.test(msg)) {
    return `${msg}\n\nRun \`gh auth login\` in a terminal, then test again.`;
  }
  return msg;
}

export async function testGh(conn: Integration): Promise<void> {
  const cfg = ghConfig(conn);
  const res = await runGh(cfg, ["auth", "status"]);
  if (res.code !== 0) {
    const msg = (res.stderr || res.stdout).trim() || `exit ${res.code}`;
    throw new Error(/not logged|auth:|please log in/i.test(msg) ? `gh is not authenticated: ${msg}. Run \`gh auth login\`.` : `gh auth status failed: ${msg}`);
  }
}
