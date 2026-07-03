import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { sentryFields } from "./fields.js";
import { sentryConfig, sentryRequest, type SentryConfig } from "./client.js";

const LOG_FIELDS = ["timestamp", "severity", "message", "trace_id", "project"];

const AGENT_HINT = "Use this for Sentry error monitoring and logs — list/read issues, pull latest issue events, query structured logs, and inspect project error events. Start with list_issues + latest_event for issue debugging, or query_logs for log search.";

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
      name: "list_events",
      description: "List recent error events for a project, optionally with full event bodies.",
      category: "read",
      schema: {
        project: z.string().optional().describe("Project slug. Defaults to the integration's project if set."),
        period: z.string().default("24h").describe("Stats period, e.g. 15m, 24h, 7d"),
        full: z.boolean().default(false).describe("Include full event bodies, including stacktraces."),
        limit: z.number().int().min(1).max(100).default(25).describe("Max events to return"),
      },
      detail: (a) => `list_events project=${(a.project as string) ?? cfg.project ?? "*"} period=${a.period} full=${a.full} limit=${a.limit}`,
      run: async (a) => {
        const proj = (a.project as string | undefined) ?? cfg.project;
        if (!proj) throw new Error("No project given. Pass project or set project_slug in the integration config.");
        const events = await sentryRequest<unknown[]>(cfg, "GET", `/projects/${cfg.org}/${proj}/events/`, {
          statsPeriod: a.period as string,
          full: a.full as boolean,
        });
        return Array.isArray(events) ? events.slice(0, a.limit as number) : events;
      },
    },
    {
      name: "query_logs",
      description: "Query Sentry structured logs using Explore's logs dataset.",
      category: "read",
      schema: {
        query: z.string().optional().describe('Sentry log search query, e.g. "severity:error payment.failed"'),
        project: z.string().optional().describe("Project slug or id. Defaults to the integration's project if set; omit for all projects."),
        period: z.string().default("24h").describe("Stats period, e.g. 15m, 24h, 7d"),
        fields: z.array(z.string()).default(LOG_FIELDS).describe("Explore fields to return. Defaults to timestamp, severity, message, trace_id, project."),
        sort: z.string().default("-timestamp").describe("Sort field, e.g. -timestamp"),
        limit: z.number().int().min(1).max(100).default(25).describe("Max log rows to return"),
      },
      detail: (a) => `query_logs project=${(a.project as string) ?? cfg.project ?? "*"} query="${(a.query as string) ?? ""}" period=${a.period} limit=${a.limit}`,
      run: (a) => sentryRequest(cfg, "GET", `/organizations/${cfg.org}/events/`, {
        dataset: "logs",
        field: (a.fields as string[]).slice(0, 20),
        query: a.query as string | undefined,
        project: (a.project as string | undefined) ?? cfg.project ?? "-1",
        statsPeriod: a.period as string,
        sort: a.sort as string,
        per_page: a.limit as number,
      }),
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
    "Read projects, issues, event stack traces, project error events, and structured logs; resolve or ignore issues when write is permitted. Every action is policy-checked and recorded in the activity log.",
  configFields: sentryFields,
  client: (conn) => sentryConfig(conn),
  async testConnection(conn: Integration): Promise<void> {
    const cfg = sentryConfig(conn);
    // Cheapest authenticated call that validates token + org slug.
    await sentryRequest(cfg, "GET", `/organizations/${cfg.org}/`);
  },
  tools: (_conn, cfg) => sentryTools(cfg),
});
