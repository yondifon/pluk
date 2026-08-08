import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { sentryFields } from "./fields.js";
import { sentryConfig, sentryRequest, sentryRequestText, type SentryConfig } from "./client.js";

const LOG_FIELDS = ["timestamp", "severity", "message", "trace_id", "project"];

const TEXT_MIMES = new Set([
  "application/json",
  "application/ld+json",
  "application/xml",
  "application/xhtml+xml",
  "application/javascript",
  "application/x-javascript",
  "image/svg+xml",
]);

function isTextMime(mime: string): boolean {
  return mime.startsWith("text/") || TEXT_MIMES.has(mime);
}

export async function resolveIssueProject(cfg: SentryConfig, issueId: string): Promise<string | undefined> {
  const issue = await sentryRequest<Record<string, unknown>>(cfg, "GET", `/organizations/${cfg.org}/issues/${encodeURIComponent(issueId)}/`);
  const project = issue.project as Record<string, unknown> | undefined;
  return project && typeof project === "object" ? String(project.slug ?? "") : undefined;
}

export async function resolveLatestEventId(cfg: SentryConfig, issueId: string): Promise<string> {
  const event = await sentryRequest<Record<string, unknown>>(cfg, "GET", `/issues/${encodeURIComponent(issueId)}/events/latest/`);
  const eventId = event.eventID;
  if (!eventId) throw new Error(`No event found for issue ${issueId}`);
  return String(eventId);
}

/** Slice a text attachment at an offset/limit, appending a truncation marker when text remains. */
export function formatTextChunk(body: string, offset: number, limit: number): string {
  if (offset >= body.length) return `Offset ${offset} is past the end of ${body.length} characters.`;
  const chunk = body.slice(offset, offset + limit);
  if (offset + chunk.length >= body.length) return chunk;
  return `${chunk}\n\n[…truncated: showing characters ${offset + 1}–${offset + chunk.length} of ${body.length}. Read on with offset=${offset + chunk.length}.]`;
}

const NO_PROJECT = "No project given. Pass project or set project_slug in the integration config.";

const AGENT_HINT = "Use this for Sentry error monitoring and logs — list/read issues, pull latest issue events, inspect event attachments, and query structured logs. Start with list_issues + latest_event for issue debugging, list_event_attachments to see what an event captured, or query_logs for log search.";

// Sentry's tools. Each declares its policy category, log line, and REST call;
// gating, logging, and response shaping are handled by actionAdapter.
export function sentryTools(cfg: SentryConfig): ActionTool[] {
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
      name: "list_event_attachments",
      description: "List an event's attachments (id, name, content type, size, created time). Defaults to the issue's latest event unless event_id is given.",
      category: "read",
      schema: {
        id: z.string().describe("Issue id (numeric) or short id"),
        event_id: z.string().optional().describe("Event id (hex). Omit to use the issue's latest event."),
        project: z.string().optional().describe("Project slug. Defaults to the integration's project, else derived from the issue."),
      },
      detail: (a) => `list_event_attachments ${a.id}${a.event_id ? ` event=${a.event_id}` : ""}`,
      run: async (a) => {
        const issueId = String(a.id);
        const project = (a.project as string | undefined) ?? cfg.project ?? (await resolveIssueProject(cfg, issueId));
        if (!project) throw new Error(NO_PROJECT);
        const eventId = (a.event_id as string | undefined) ?? (await resolveLatestEventId(cfg, issueId));
        const attachments = await sentryRequest<Record<string, unknown>[]>(cfg, "GET", `/projects/${cfg.org}/${encodeURIComponent(project)}/events/${encodeURIComponent(eventId)}/attachments/`);
        return Array.isArray(attachments) ? attachments.map((att) => ({ ...att, project })) : attachments;
      },
    },
    {
      name: "read_event_attachment",
      description: "Fetch one attachment's contents by id. Text attachments come back as text, truncated at `limit` characters — pass `offset` to read further. Non-text attachments report their type and size only.",
      category: "read",
      schema: {
        project: z.string().optional().describe("Project slug. Defaults to the integration's project."),
        event_id: z.string().describe("Event id (hex) — returned by list_event_attachments."),
        attachment_id: z.string().describe("Attachment id — returned by list_event_attachments."),
        limit: z.number().int().min(1).max(100000).default(20000).describe("Max characters of text to return."),
        offset: z.number().int().min(0).default(0).describe("Character offset to start from; used to read past the first chunk."),
      },
      detail: (a) => `read_event_attachment ${a.attachment_id} event=${a.event_id}`,
      run: async (a) => {
        const project = (a.project as string | undefined) ?? cfg.project;
        if (!project) throw new Error(NO_PROJECT);
        const res = await sentryRequestText(cfg, "GET", `/projects/${cfg.org}/${encodeURIComponent(project)}/events/${encodeURIComponent(String(a.event_id))}/attachments/${encodeURIComponent(String(a.attachment_id))}/`, { download: 1 });
        const contentType = (res.contentType ?? "").split(";")[0]?.trim() ?? "";
        if (contentType && !isTextMime(contentType)) {
          const size = res.contentLength ?? String(res.text.length);
          return `Attachment #${a.attachment_id} is ${contentType} (${size} bytes) — not text, contents cannot be shown.`;
        }
        const offset = a.offset as number;
        const limit = a.limit as number;
        const header = `Attachment #${a.attachment_id} (${contentType || "unknown"}, ${res.text.length} characters)`;
        return `${header}\n\n${formatTextChunk(res.text, offset, limit)}`;
      },
    },
    {
      name: "list_events",
      description: "List recent error events for a project, optionally with full event bodies.",
      category: "read",
      defaultEnabled: false,
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
      defaultEnabled: false,
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
