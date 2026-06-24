// Minimal Slack Web API client. Bot tokens (`xoxb-…`) go in the Authorization
// header as `Bearer <token>`. Slack returns HTTP 200 even on logical failures,
// with `{ ok:false, error }` in the body — we throw on that so the gated runner
// records it as an error.
// Reference: https://docs.slack.dev/apis/web-api/

import type { Integration } from "../../store/integrations.js";

const TIMEOUT_MS = 20_000;
const BASE_URL = "https://slack.com/api";

export interface SlackCfg {
  token: string;
  defaultChannel?: string;
}

export function slackConfig(conn: Integration): SlackCfg {
  const token = String(conn.config.bot_token ?? "");
  if (!token) throw new Error("Slack bot token is missing. Set it in the integration config.");
  const defaultChannel = conn.config.default_channel ? String(conn.config.default_channel) : undefined;
  return { token, defaultChannel };
}

export async function slackRequest<T>(
  cfg: SlackCfg,
  method: string,
  params: Record<string, string | number | undefined>,
): Promise<T> {
  const body = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== "") body.set(k, String(v));
  }

  let res: Response;
  try {
    res = await fetch(`${BASE_URL}/${method}`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${cfg.token}`,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (err) {
    if ((err as Error).name === "TimeoutError") throw new Error(`Slack API timed out after ${TIMEOUT_MS / 1000}s`);
    throw new Error(`Slack API request failed: ${(err as Error).message}`);
  }

  if (!res.ok) throw new Error(`Slack API ${method}: HTTP ${res.status}`);

  const json = (await res.json()) as { ok: boolean; error?: string } & Record<string, unknown>;
  if (!json.ok) throw new Error(`Slack API ${method}: ${json.error ?? "unknown error"}`);
  return json as T;
}

/** Resolve a channel from an explicit arg or the connection default. */
export function resolveChannel(cfg: SlackCfg, arg?: string): string {
  const channel = (arg && arg.trim()) || cfg.defaultChannel;
  if (!channel) throw new Error("No channel given. Pass a channel id/name or set a default channel in the integration config.");
  return channel;
}
