import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { applyOnly, type FieldMap } from "../onlyProjection.js";
import { slackFields } from "./fields.js";
import { slackConfig, slackRequest, resolveChannel, type SlackCfg } from "./client.js";

const AGENT_HINT = "Use this for Slack workspace access — list channels, read recent channel messages for context, and post messages back. list_channels to find a channel id, channel_history to read it; set default_channel to skip the arg.";

function onlySchema(presetNames: string[]) {
  const presetLine = presetNames.length ? ` Presets: ${presetNames.join(", ")}.` : "";
  return z.array(z.string()).optional().describe(`Trim the response to just these fields — omit for a lighter default, pass ["*"] for the full payload. Entries are dot paths or presets.${presetLine}`);
}

const CHANNEL_MAP: FieldMap = { fields: ["id", "name", "is_private", "topic", "purpose", "num_members"], default: ["id", "name", "topic"], presets: { details: ["purpose", "num_members", "is_private"] } };
const MESSAGE_MAP: FieldMap = { fields: ["type", "user", "text", "ts", "thread_ts", "reply_count", "reactions", "files"], default: ["user", "text", "ts"], presets: { thread: ["thread_ts", "reply_count"], attachments: ["files", "reactions"] } };
const POST_MAP: FieldMap = { fields: ["ok", "channel", "ts", "message"], default: ["ok", "channel", "ts"], presets: { message: ["message"] } };

// Slack's tools. Each declares its policy category, log line, and Web API method;
// gating, logging, and response shaping are handled by actionAdapter. v1 is bot-
// token only: search.messages needs a user token, so it is deferred.
export function slackTools(cfg: SlackCfg): ActionTool[] {
  return [
    {
      name: "list_channels",
      description: "List public channels in the workspace (id, name, topic).",
      category: "read",
       schema: { limit: z.number().int().min(1).max(1000).default(100).describe("Max channels to return"), only: onlySchema(["details"]) },
      detail: () => `list_channels`,
      run: async (a) => {
        const data = await slackRequest<{ channels: unknown[] }>(cfg, "conversations.list", {
          types: "public_channel",
          limit: a.limit as number,
        });
        return applyOnly(data.channels, a.only as string[] | undefined, CHANNEL_MAP);
      },
    },
    {
      name: "channel_history",
      description: "Read recent messages in a channel, newest first.",
      category: "read",
      schema: {
        channel: z.string().optional().describe("Channel id (e.g. C0123). Defaults to the integration's default_channel."),
        limit: z.number().int().min(1).max(100).default(20).describe("Max messages to return"),
        only: onlySchema(["thread", "attachments"]),
      },
      detail: (a) => `channel_history ${(a.channel as string) ?? cfg.defaultChannel ?? "?"} limit=${a.limit}`,
      run: async (a) => {
        const channel = resolveChannel(cfg, a.channel as string | undefined);
        const data = await slackRequest<{ messages: unknown[] }>(cfg, "conversations.history", {
          channel,
          limit: a.limit as number,
        });
        return applyOnly(data.messages, a.only as string[] | undefined, MESSAGE_MAP);
      },
    },

    // ── Write tool ───────────────────────────────────────────────────────────
    {
      name: "post_message",
      description: "Post a message to a channel.",
      category: "write",
      schema: {
        channel: z.string().optional().describe("Channel id. Defaults to the integration's default_channel."),
        text: z.string().describe("Message text (markdown)"),
        only: onlySchema(["message"]),
      },
      detail: (a) => `post_message ${(a.channel as string) ?? cfg.defaultChannel ?? "?"}`,
       run: async (a) => {
        const channel = resolveChannel(cfg, a.channel as string | undefined);
        const data = await slackRequest(cfg, "chat.postMessage", { channel, text: a.text as string });
        return applyOnly(data, a.only as string[] | undefined, POST_MAP);
      },
    },
  ];
}

export const slackAdapter = actionAdapter<SlackCfg>({
  id: "slack",
  label: "Slack",
  category: "chat",
  agentHint: AGENT_HINT,
  access:
    "Read channels and recent messages; post a message when write is permitted. Requires bot scopes channels:read, channels:history, chat:write. Every action is policy-checked and recorded in the activity log.",
  start: "list_channels",
  configFields: slackFields,
  client: (conn) => slackConfig(conn),
  async testConnection(conn: Integration): Promise<void> {
    const cfg = slackConfig(conn);
    // auth.test validates the bot token and returns team/user identity.
    await slackRequest(cfg, "auth.test", {});
  },
  tools: (_conn, cfg) => slackTools(cfg),
});
