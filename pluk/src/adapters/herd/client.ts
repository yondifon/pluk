import { existsSync, readdirSync, readFileSync, realpathSync } from "fs";
import { mkdir, readFile, symlink, writeFile } from "fs/promises";
import { homedir } from "os";
import { basename, dirname, isAbsolute, join, sep } from "path";
import type { Integration } from "../../store/integrations.js";

/**
 * Local git-worktree + Laravel Herd plumbing. A feature site is a worktree of the
 * app's tracked files (`<root>/<feature>`), with the untracked-but-needed paths
 * (vendor, node_modules, build output) symlinked back to the app so it boots
 * without a reinstall, and a Herd link at `<feature>.<site>.<tld>`.
 *
 * Everything here shells out to `git` and `herd` with argv arrays — no shell, so
 * a feature name can never become a command. Names are validated anyway: they
 * land in a hostname and a path.
 */

const DEFAULT_HERD_BIN = `${homedir()}/Library/Application Support/Herd/bin/herd`;

/** Herd keeps a Valet-shaped config here: the TLD, the parked paths, and a `Sites`
 *  folder of `herd link` symlinks. Reading it is how an integration finds the app
 *  without being told a path. */
const HERD_CONFIG_ROOT = `${homedir()}/Library/Application Support/Herd/config/valet`;

export interface HerdConfig {
  appPath: string;
  site: string;        // base site name, e.g. "app" -> feature.app.test
  tld: string;
  secure: boolean;
  worktreeRoot: string;
  linkPaths: string[];
  envFile: string;     // "" disables the env copy
  herdBin: string;
}

const expandHome = (p: string): string => (p.startsWith("~") ? `${homedir()}${p.slice(1)}` : p);

/** Feature names become a DNS label and a directory name — keep them boring. */
const FEATURE_RE = /^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/;

export function assertFeature(feature: string): string {
  const name = feature.trim().toLowerCase();
  if (!FEATURE_RE.test(name)) {
    throw new Error(`Invalid feature name "${feature}" — use lowercase letters, digits and hyphens (max 40).`);
  }
  return name;
}

/** Split the comma/newline separated `link_paths` field, rejecting escapes. */
export function parseLinkPaths(raw: unknown): string[] {
  return String(raw ?? "")
    .split(/[,\n]/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      if (isAbsolute(p) || p.split("/").includes("..")) throw new Error(`Invalid linked path "${p}" — must be relative to the app.`);
      return p;
    });
}

// ── Discovery ────────────────────────────────────────────────────────────────

/** An app Herd already serves: the site name it answers on and the folder behind it. */
export interface HerdSite {
  site: string;
  path: string;
}

interface ValetConfig {
  tld?: string;
  paths?: string[];
}

function readValet(root: string): ValetConfig {
  try {
    const parsed: unknown = JSON.parse(readFileSync(join(root, "config.json"), "utf8"));
    return parsed && typeof parsed === "object" ? (parsed as ValetConfig) : {};
  } catch {
    return {};
  }
}

/** Folder names in `dir`, symlinks included — a linked site is a symlink. Sorted,
 *  so the discovered order doesn't depend on the filesystem. */
function folders(dir: string): string[] {
  try {
    return readdirSync(dir, { withFileTypes: true })
      .filter((e) => !e.name.startsWith(".") && (e.isDirectory() || e.isSymbolicLink()))
      .map((e) => e.name)
      .sort();
  } catch {
    return [];
  }
}

const realpath = (p: string): string | null => {
  try {
    return realpathSync(p);
  } catch {
    return null;
  }
};

/** Every app Herd serves: the `herd link`ed sites first, then the folders inside
 *  each parked path. A linked name wins — it's the name Herd actually answers on. */
export function discoverSites(root: string = HERD_CONFIG_ROOT): HerdSite[] {
  const sitesDir = join(root, "Sites");
  const found = new Map<string, string>();

  for (const name of folders(sitesDir)) {
    const target = realpath(join(sitesDir, name));
    if (target) found.set(name, target);
  }
  for (const parked of readValet(root).paths ?? []) {
    const dir = String(parked).replace(/\/+$/, "");
    if (!dir || realpath(dir) === realpath(sitesDir)) continue;
    for (const name of folders(dir)) {
      const target = realpath(join(dir, name));
      if (target && !found.has(name)) found.set(name, target);
    }
  }
  return [...found].map(([site, path]) => ({ site, path }));
}

const isRepo = (p: string): boolean => existsSync(join(p, ".git"));

const nameList = (sites: HerdSite[]): string => {
  const names = sites.map((s) => s.site).sort();
  return `${names.slice(0, 12).join(", ")}${names.length > 12 ? `, … (${names.length} total)` : ""}`;
};

/** Find the app when the config names no path: by site name when one is given,
 *  otherwise only when Herd serves a single repository — anything else is a guess. */
export function resolveApp(site: string, root: string = HERD_CONFIG_ROOT): HerdSite {
  const sites = discoverSites(root);
  if (!sites.length) throw new Error("Herd serves no sites — set the app path.");

  if (site) {
    const wanted = site.toLowerCase();
    const hit = sites.find((s) => s.site.toLowerCase() === wanted);
    if (!hit) throw new Error(`Herd serves no site called "${site}". Sites: ${nameList(sites)}.`);
    return hit;
  }

  const repos = sites.filter((s) => isRepo(s.path));
  if (repos.length === 1) return repos[0]!;
  if (!repos.length) throw new Error("None of Herd's sites is a git repository — set the app path.");
  throw new Error(`Herd serves ${repos.length} apps — set the base site to one of them, or set the app path. Sites: ${nameList(repos)}.`);
}

/** `root` is the Herd config folder; tests point it at a fixture. */
export function herdConfig(conn: Integration, root: string = HERD_CONFIG_ROOT): HerdConfig {
  const c = conn.config;
  const explicit = expandHome(String(c.app_path ?? "").trim()).replace(/\/+$/, "");
  const chosen = String(c.site ?? "").trim();
  const app = explicit ? { site: chosen || basename(explicit), path: explicit } : resolveApp(chosen, root);
  const appPath = app.path;

  const wtRoot = String(c.worktree_root ?? "").trim();
  return {
    appPath,
    site: app.site,
    tld: String(c.tld ?? "").trim() || readValet(root).tld || "test",
    secure: c.secure !== false,
    worktreeRoot: wtRoot ? expandHome(wtRoot).replace(/\/+$/, "") : `${dirname(appPath)}/${basename(appPath)}-worktrees`,
    linkPaths: parseLinkPaths(c.link_paths ?? "vendor, node_modules, public/build"),
    envFile: String(c.env_file ?? ".env").trim(),
    herdBin: expandHome(String(c.herd_bin ?? "").trim()) || DEFAULT_HERD_BIN,
  };
}

export const siteName = (cfg: HerdConfig, feature: string): string => `${feature}.${cfg.site}`;
export const siteUrl = (cfg: HerdConfig, feature: string): string =>
  `${cfg.secure ? "https" : "http"}://${siteName(cfg, feature)}.${cfg.tld}`;
export const worktreePath = (cfg: HerdConfig, feature: string): string => join(cfg.worktreeRoot, feature);

// ── Process helpers ──────────────────────────────────────────────────────────

async function run(cmd: string[], cwd?: string): Promise<string> {
  const proc = Bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  const code = await proc.exited;
  if (code !== 0) throw new Error(`${basename(cmd[0]!)} ${cmd[1] ?? ""} failed: ${(stderr || stdout).trim() || `exit ${code}`}`);
  return stdout;
}

const git = (cfg: HerdConfig, args: string[]): Promise<string> => run(["git", "-C", cfg.appPath, ...args]);
const herd = (cfg: HerdConfig, args: string[], cwd?: string): Promise<string> => run([cfg.herdBin, ...args], cwd);

/** Best-effort teardown step: a missing link or an already-gone worktree must not
 *  abort the rest of the cleanup, but the agent should still hear about it. */
async function attempt(label: string, fn: () => Promise<unknown>, notes: string[]): Promise<void> {
  try {
    await fn();
  } catch (e) {
    notes.push(`${label}: ${(e as Error).message}`);
  }
}

async function branchExists(cfg: HerdConfig, branch: string): Promise<boolean> {
  try {
    await git(cfg, ["rev-parse", "--verify", "--quiet", `refs/heads/${branch}`]);
    return true;
  } catch {
    return false;
  }
}

// ── Env ──────────────────────────────────────────────────────────────────────

/** Point the copy at its own domain. The env file is copied rather than linked:
 *  rewriting APP_URL in place would repoint the app everyone else is using. */
export function setAppUrl(env: string, url: string): string {
  if (/^APP_URL=.*$/m.test(env)) return env.replace(/^APP_URL=.*$/m, () => `APP_URL=${url}`);
  return `${env}${env === "" || env.endsWith("\n") ? "" : "\n"}APP_URL=${url}\n`;
}

// ── Operations ───────────────────────────────────────────────────────────────

export interface CreateResult {
  site: string;
  url: string;
  path: string;
  branch: string;
  linked: string[];
  skipped: string[];
}

export async function createSite(
  cfg: HerdConfig,
  feature: string,
  opts: { branch?: string; base?: string } = {},
): Promise<CreateResult> {
  const target = worktreePath(cfg, feature);
  if (existsSync(target)) throw new Error(`${target} already exists — destroy_site ${feature} first.`);

  const branch = opts.branch?.trim() || feature;
  await mkdir(cfg.worktreeRoot, { recursive: true });
  if (await branchExists(cfg, branch)) await git(cfg, ["worktree", "add", target, branch]);
  else await git(cfg, ["worktree", "add", "-b", branch, target, opts.base?.trim() || "HEAD"]);

  const linked: string[] = [];
  const skipped: string[] = [];
  for (const rel of cfg.linkPaths) {
    const src = join(cfg.appPath, rel);
    const dest = join(target, rel);
    if (!existsSync(src)) { skipped.push(`${rel} (not in the app)`); continue; }
    if (existsSync(dest)) { skipped.push(`${rel} (already in the worktree)`); continue; }
    await mkdir(dirname(dest), { recursive: true });
    await symlink(src, dest);
    linked.push(rel);
  }

  const url = siteUrl(cfg, feature);
  if (cfg.envFile) {
    const src = join(cfg.appPath, cfg.envFile);
    if (existsSync(src)) {
      await writeFile(join(target, cfg.envFile), setAppUrl(await readFile(src, "utf8"), url));
      linked.push(`${cfg.envFile} (copied, APP_URL=${url})`);
    } else {
      skipped.push(`${cfg.envFile} (not in the app)`);
    }
  }

  await herd(cfg, ["link", siteName(cfg, feature), ...(cfg.secure ? ["--secure"] : [])], target);
  return { site: siteName(cfg, feature), url, path: target, branch, linked, skipped };
}

export interface DestroyResult {
  site: string;
  path: string;
  removed: boolean;
  notes: string[];
}

/** Unlink the site and drop the worktree. The branch is left alone — the work on
 *  it outlives the test site. */
export async function destroySite(cfg: HerdConfig, feature: string, force = false): Promise<DestroyResult> {
  const target = worktreePath(cfg, feature);
  const site = siteName(cfg, feature);
  const notes: string[] = [];

  if (cfg.secure) await attempt("herd unsecure", () => herd(cfg, ["unsecure", site]), notes);
  await attempt("herd unlink", () => herd(cfg, ["unlink", site]), notes);

  let removed = false;
  if (existsSync(target)) {
    await attempt("git worktree remove", async () => {
      await git(cfg, ["worktree", "remove", ...(force ? ["--force"] : []), target]);
      removed = true;
    }, notes);
  } else {
    notes.push(`${target} was already gone`);
  }
  await attempt("git worktree prune", () => git(cfg, ["worktree", "prune"]), notes);

  return { site, path: target, removed, notes };
}

export interface SiteInfo {
  feature: string;
  site: string;
  url: string;
  path: string;
  branch: string;
}

/** Parse `git worktree list --porcelain` into path/branch pairs. */
export function parseWorktrees(text: string): { path: string; branch: string }[] {
  const out: { path: string; branch: string }[] = [];
  for (const block of text.trim().split(/\n\s*\n/)) {
    let path = "";
    let branch = "detached";
    for (const line of block.split("\n")) {
      if (line.startsWith("worktree ")) path = line.slice(9).trim();
      else if (line.startsWith("branch ")) branch = line.slice(7).trim().replace(/^refs\/heads\//, "");
    }
    if (path) out.push({ path, branch });
  }
  return out;
}

export async function listSites(cfg: HerdConfig): Promise<SiteInfo[]> {
  const prefix = `${cfg.worktreeRoot}${sep}`;
  return parseWorktrees(await git(cfg, ["worktree", "list", "--porcelain"]))
    .filter((w) => w.path.startsWith(prefix))
    .map((w) => {
      const feature = basename(w.path);
      return { feature, site: siteName(cfg, feature), url: siteUrl(cfg, feature), path: w.path, branch: w.branch };
    });
}

export async function testHerd(conn: Integration): Promise<void> {
  const cfg = herdConfig(conn);
  if (!existsSync(cfg.appPath)) throw new Error(`App path not found: ${cfg.appPath}`);
  if (!existsSync(cfg.herdBin)) throw new Error(`Herd CLI not found: ${cfg.herdBin}`);
  await git(cfg, ["rev-parse", "--git-dir"]);
  await herd(cfg, ["--version"]);
}
