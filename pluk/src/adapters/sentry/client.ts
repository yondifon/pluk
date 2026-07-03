// Minimal Sentry REST client (api.sentry.io v0). Auth tokens go in the
// Authorization header as `Bearer <token>`. Works against sentry.io and
// self-hosted installs via a configurable base URL.
// Reference: https://docs.sentry.io/api/

import type { Integration } from "../../store/integrations.js";

const TIMEOUT_MS = 20_000;

export interface SentryConfig {
  baseUrl: string;
  token: string;
  org: string;
  project?: string;
}

export function sentryConfig(conn: Integration): SentryConfig {
  const baseUrl = String(conn.config.base_url ?? "https://sentry.io").replace(/\/+$/, "");
  const token = String(conn.config.auth_token ?? "");
  const org = String(conn.config.org_slug ?? "");
  const project = conn.config.project_slug ? String(conn.config.project_slug) : undefined;
  return { baseUrl, token, org, project };
}

export async function sentryRequest<T>(
  cfg: SentryConfig,
  method: string,
  path: string,
  query?: Record<string, string | number | boolean | (string | number | boolean)[] | undefined>,
  body?: unknown,
): Promise<T> {
  if (!cfg.token) throw new Error("Sentry auth token is missing. Set it in the integration config.");
  if (!cfg.org) throw new Error("Sentry organization slug is missing. Set it in the integration config.");

  const url = new URL(`${cfg.baseUrl}/api/0${path}`);
  for (const [k, v] of Object.entries(query ?? {})) {
    if (v === undefined || v === "") continue;
    const values = Array.isArray(v) ? v : [v];
    for (const value of values) url.searchParams.append(k, String(value));
  }

  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: {
        Authorization: `Bearer ${cfg.token}`,
        "Content-Type": "application/json",
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (err) {
    if ((err as Error).name === "TimeoutError") throw new Error(`Sentry API timed out after ${TIMEOUT_MS / 1000}s`);
    throw new Error(`Sentry API request failed: ${(err as Error).message}`);
  }

  if (!res.ok) {
    let detail = "";
    try {
      const j = (await res.json()) as { detail?: string };
      detail = j.detail ? ` — ${j.detail}` : "";
    } catch { /* non-JSON error body */ }
    if (res.status === 401) throw new Error(`Sentry: unauthorized (401) — check the auth token${detail}`);
    if (res.status === 404) throw new Error(`Sentry: not found (404) — check the org/project/issue id${detail}`);
    throw new Error(`Sentry API ${res.status}${detail}`);
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}
