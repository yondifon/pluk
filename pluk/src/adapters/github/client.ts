// Minimal GitHub REST client (api.github.com, v2022-11-28). Fine-grained PATs go
// in the Authorization header as `Bearer <token>`. Works against github.com and
// GitHub Enterprise Server via a configurable base URL.
// Reference: https://docs.github.com/en/rest

import type { Integration } from "../../store/integrations.js";

const TIMEOUT_MS = 20_000;

export interface GitHubConfig {
  baseUrl: string;
  token: string;
  defaultRepo?: string;
}

export function githubConfig(conn: Integration): GitHubConfig {
  const baseUrl = String(conn.config.base_url ?? "https://api.github.com").replace(/\/+$/, "");
  const token = String(conn.config.token ?? "");
  const defaultRepo = conn.config.default_repo ? String(conn.config.default_repo) : undefined;
  return { baseUrl, token, defaultRepo };
}

/** Resolve `owner/repo` from an explicit arg or the connection default. */
export function resolveRepo(cfg: GitHubConfig, arg?: string): { owner: string; repo: string } {
  const spec = (arg && arg.trim()) || cfg.defaultRepo;
  if (!spec) throw new Error("No repo given. Pass repo as owner/repo or set a default repo in the integration config.");
  const [owner, repo] = spec.split("/");
  if (!owner || !repo) throw new Error(`Invalid repo "${spec}". Use the form owner/repo.`);
  return { owner, repo };
}

export async function githubRequest<T>(
  cfg: GitHubConfig,
  method: string,
  path: string,
  query?: Record<string, string | number | undefined>,
  body?: unknown,
): Promise<T> {
  if (!cfg.token) throw new Error("GitHub token is missing. Set it in the integration config.");

  const url = new URL(`${cfg.baseUrl}${path}`);
  for (const [k, v] of Object.entries(query ?? {})) {
    if (v !== undefined && v !== "") url.searchParams.set(k, String(v));
  }

  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: {
        Authorization: `Bearer ${cfg.token}`,
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Content-Type": "application/json",
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (err) {
    if ((err as Error).name === "TimeoutError") throw new Error(`GitHub API timed out after ${TIMEOUT_MS / 1000}s`);
    throw new Error(`GitHub API request failed: ${(err as Error).message}`);
  }

  if (!res.ok) {
    let detail = "";
    try {
      const j = (await res.json()) as { message?: string };
      detail = j.message ? ` — ${j.message}` : "";
    } catch { /* non-JSON error body */ }
    if (res.status === 401) throw new Error(`GitHub: unauthorized (401) — check the token${detail}`);
    if (res.status === 403) throw new Error(`GitHub: forbidden (403) — token scope or rate limit${detail}`);
    if (res.status === 404) throw new Error(`GitHub: not found (404) — check the repo/owner/number${detail}`);
    throw new Error(`GitHub API ${res.status}${detail}`);
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}
