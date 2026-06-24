import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { sentryFields } from "./fields.js";
import { sentryConfig, sentryRequest, type SentryConfig } from "./client.js";

const AGENT_HINT = "Use this for Sentry error monitoring — list and read issues across projects, pull the latest event's stack trace and tags to debug, and resolve or ignore issues when write is permitted. Start with list_issues, then use latest_event for stack traces.";

// Sentry's tools. Each declares its policy category, log line, and REST call;
// gating, logging, and response shaping are handled by actionAdapter.
function sentryTools(cfg: SentryConfig): ActionTool[] {
  return [
    {
      name: "list_projects",
      description: "List projects in the organization (slug, name, platform).",
      category: "read",
      run: () => sentryRequest(cfg, "GET", `/organizations/${cfg.org}/projects/`),
    },
    {
      name: "list_issues",
      description: "List issues, newest first. Scoped to the default project if set, else all projects.",
      category: "read",
      schema: {
        query: z.string().optional().describe('Sentry search query, e.g. "is:unresolved level:error"'),
        project: z.string().optional().describe("Project slug. Defaults to the integration's project if set."),
        period: z.string().default("14d").describe("Stats period, e.g. 24h, 14d, 90d"),
        limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
      },
      detail: (a) => `list_issues project=${(a.project as string) ?? cfg.project ?? "*"} query="${(a.query as string) ?? ""}" period=${a.period} limit=${a.limit}`,
      run: async (a) => {
        const proj = (a.project as string | undefined) ?? cfg.project;
        const query = a.query as string | undefined;
        const period = a.period as string;
        const limit = a.limit as number;
        const issues = proj
          ? await sentryRequest<unknown[]>(cfg, "GET", `/projects/${cfg.org}/${proj}/issues/`, { query, statsPeriod: period })
          : await sentryRequest<unknown[]>(cfg, "GET", `/organizations/${cfg.org}/issues/`, { query, statsPeriod: period, project: "-1" });
        return Array.isArray(issues) ? issues.slice(0, limit) : issues;
      },
    },
    {
      name: "get_issue",
      description: "Get a single issue by its id or short id (e.g. BACKEND-1A)",
      category: "read",
      schema: { id: z.string().describe("Issue id (numeric) or short id") },
      detail: (a) => `get_issue ${a.id}`,
      run: (a) => sentryRequest(cfg, "GET", `/organizations/${cfg.org}/issues/${encodeURIComponent(String(a.id))}/`),
    },
    {
      name: "latest_event",
      description: "Get the latest event for an issue, including the stacktrace and tags",
      category: "read",
      schema: { id: z.string().describe("Issue id (numeric) or short id") },
      detail: (a) => `latest_event ${a.id}`,
      run: (a) => sentryRequest(cfg, "GET", `/issues/${encodeURIComponent(String(a.id))}/events/latest/`),
    },
    {
      name: "update_issue",
      description: "Resolve, ignore, or reopen an issue (write).",
      category: "write",
      schema: {
        id: z.string().describe("Issue id (numeric) or short id"),
        status: z.enum(["resolved", "ignored", "unresolved"]).describe("New status"),
      },
      detail: (a) => `update_issue ${a.id} -> ${a.status}`,
      run: (a) =>
        sentryRequest(cfg, "PUT", `/organizations/${cfg.org}/issues/${encodeURIComponent(String(a.id))}/`, undefined, { status: a.status }),
    },
  ];
}

export const sentryAdapter = actionAdapter<SentryConfig>({
  id: "sentry",
  label: "Sentry",
  category: "observability",
  agentHint: AGENT_HINT,
  access:
    "Read projects, issues, and event stack traces; resolve or ignore issues when write is permitted. Every action is policy-checked and recorded in the activity log.",
  configFields: sentryFields,
  client: (conn) => sentryConfig(conn),
  async testConnection(conn: Integration): Promise<void> {
    const cfg = sentryConfig(conn);
    // Cheapest authenticated call that validates token + org slug.
    await sentryRequest(cfg, "GET", `/organizations/${cfg.org}/`);
  },
  tools: (_conn, cfg) => sentryTools(cfg),
});
