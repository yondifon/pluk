import { z } from "zod";
import { mkdir, rename, rm, stat, writeFile } from "fs/promises";
import { homedir } from "os";
import { join, resolve } from "path";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { applyOnly, onlySchema, onlyValue, type FieldMap } from "../onlyProjection.js";
import { sentryFields } from "./fields.js";
import { sentryConfig, sentryRequest, sentryRequestBytes, type SentryConfig } from "./client.js";

const LOG_FIELDS = ["timestamp", "severity", "message", "trace_id", "project"];

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

const NO_PROJECT = "No project given. Pass project or set project_slug in the integration config.";
export const SENTRY_ATTACHMENT_CACHE_DIR = resolve(process.env.PLUK_DATA_DIR ?? join(homedir(), ".pluk"), "sentry-attachments");

interface AttachmentReference {
  project: string;
  eventId: string;
  id: string;
  name?: string;
  size?: number;
}

function safePathPart(value: string): string {
  const clean = value.replace(/[^a-zA-Z0-9._-]/g, "_");
  return clean && clean !== "." && clean !== ".." ? clean : "_";
}

function attachmentName(ref: AttachmentReference): string {
  return safePathPart(ref.name ?? `attachment-${ref.id}`);
}

function attachmentSize(value: unknown): number | undefined {
  const size = Number(value);
  return Number.isSafeInteger(size) && size >= 0 ? size : undefined;
}

interface DownloadedAttachment {
  path: string;
  warning?: string;
}

// Sentry's listed size is metadata declared at ingest, never re-measured against
// the stored blob, so a body of a different length is normal rather than corrupt.
function sizeWarning(ref: AttachmentReference, actual: number): string | undefined {
  if (ref.size === undefined || ref.size === actual) return undefined;
  if (actual < ref.size) return `Saved ${actual} bytes, fewer than the ${ref.size} Sentry listed — the file may be incomplete.`;
  return `Saved ${actual} bytes; Sentry listed ${ref.size}.`;
}

export async function downloadAttachment(cfg: SentryConfig, ref: AttachmentReference): Promise<DownloadedAttachment> {
  const directory = join(SENTRY_ATTACHMENT_CACHE_DIR, safePathPart(ref.project), safePathPart(ref.eventId));
  const path = join(directory, `${safePathPart(ref.id)}-${attachmentName(ref)}`);

  await mkdir(directory, { recursive: true });
  try {
    const existing = await stat(path);
    if (existing.isFile() && (existing.size > 0 || ref.size === 0)) return { path, warning: sizeWarning(ref, existing.size) };
  } catch (error) {
    const code = error instanceof Error && "code" in error ? error.code : undefined;
    if (code !== "ENOENT") throw error;
  }

  const response = await sentryRequestBytes(
    cfg,
    "GET",
    `/projects/${cfg.org}/${encodeURIComponent(ref.project)}/events/${encodeURIComponent(ref.eventId)}/attachments/${encodeURIComponent(ref.id)}/`,
    { download: 1 },
  );
  if (response.bytes.length === 0 && ref.size !== 0) {
    throw new Error(`Attachment ${ref.id} downloaded empty — Sentry returned no bytes.`);
  }

  const temporaryPath = `${path}.${crypto.randomUUID()}.tmp`;
  try {
    await writeFile(temporaryPath, response.bytes);
    await rename(temporaryPath, path);
  } finally {
    await rm(temporaryPath, { force: true });
  }
  return { path, warning: sizeWarning(ref, response.bytes.length) };
}

function attachmentReference(att: Record<string, unknown>, project: string, eventId: string): AttachmentReference {
  const id = String(att.id ?? "");
  const name = String(att.name ?? att.filename ?? `attachment-${id}`);
  const size = attachmentSize(att.size);
  return { project, eventId, id, name, ...(size === undefined ? {} : { size }) };
}

const AGENT_HINT = "Use this for Sentry error monitoring and logs — list/read issues, pull latest issue events, download event attachments to file paths you can open, and query structured logs. Start with list_issues + latest_event for issue debugging, list_event_attachments to see what an event captured, or query_logs for log search.";

// ── `only` field selection ───────────────────────────────────────────────────
// Each tool's default output and its named shortcuts. `only` entries are
// validated against `fields` and expanded through `presets`; see onlyProjection.ts.

const LIST_PROJECTS_MAP: FieldMap = {
  fields: ["slug", "name", "platform", "team", "environments", "access", "features", "teams", "id", "isBookmarked", "isMember", "hasAccess", "dateCreated", "firstEvent", "firstTransactionEvent", "platforms", "latestRelease", "latestDeploys"],
  default: ["slug", "name", "platform", "team.slug", "environments"],
  presets: {
    deploys: ["latestRelease", "latestDeploys"],
    access: ["access", "hasAccess", "isMember"],
    capabilities: (item) => ({
      features: item.features,
      ...Object.fromEntries(Object.entries(item).filter(([k]) => k.startsWith("has"))),
    }),
  },
};

const LIST_ISSUES_MAP: FieldMap = {
  fields: ["shortId", "title", "culprit", "level", "status", "priority", "count", "userCount", "firstSeen", "lastSeen", "project", "stats", "lifetime", "metadata", "annotations", "permalink", "id", "shareId", "statusDetails", "substatus", "isPublic", "platform", "type", "numComments", "assignedTo", "isBookmarked", "isSubscribed", "subscriptionDetails", "hasSeen", "issueType", "issueCategory", "priorityLockedAt", "seerFixabilityScore", "seerAutofixLastTriggered", "isUnhandled", "filtered"],
  default: ["shortId", "title", "culprit", "level", "status", "priority", "count", "userCount", "firstSeen", "lastSeen", "project.slug"],
  presets: {
    stats: ["stats", "lifetime"],
    triage: ["assignedTo", "isBookmarked", "isSubscribed", "hasSeen", "numComments", "annotations"],
    links: ["permalink", "id"],
    meta: ["metadata", "issueType", "issueCategory", "substatus"],
  },
};

const GET_ISSUE_MAP: FieldMap = {
  fields: ["shortId", "title", "culprit", "level", "status", "substatus", "priority", "count", "userCount", "firstSeen", "lastSeen", "isUnhandled", "permalink", "project", "metadata", "stats", "activity", "tags", "seenBy", "participants", "pluginActions", "pluginIssues", "pluginContexts", "userReportCount", "firstRelease", "lastRelease", "id", "shareId", "statusDetails", "isPublic", "isBookmarked", "isSubscribed", "subscriptionDetails", "hasSeen", "issueType", "issueCategory", "priorityLockedAt", "seerFixabilityScore", "seerAutofixLastTriggered"],
  default: ["shortId", "title", "culprit", "level", "status", "substatus", "priority", "count", "userCount", "firstSeen", "lastSeen", "isUnhandled", "permalink", "project.slug", "metadata.type", "metadata.value"],
  presets: {
    stats: ["stats"],
    tags: ["tags"],
    activity: ["activity", "seenBy", "participants"],
    releases: ["firstRelease", "lastRelease"],
  },
};

const LIST_EVENT_ATTACHMENTS_MAP: FieldMap = {
  fields: ["id", "name", "mimetype", "dateCreated", "project", "event_id", "size", "path", "warning", "error"],
  default: ["name", "size", "mimetype", "path", "event_id", "warning", "error"],
};

// ── latest_event: entries.exception is an array of {type, data} envelopes, not
// a keyed object, so no dot path can reach into it — the frame presets below
// exist to make it addressable without dumping the whole envelope by default.

interface FrameOptions {
  all: boolean;
  context: boolean;
  vars: boolean;
  full: boolean;
}

const FRAME_KEYS = ["filename", "function", "lineNo", "module"] as const;

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

function findEntry(entries: unknown, type: string): Record<string, unknown> | undefined {
  if (!Array.isArray(entries)) return undefined;
  return entries.find((e) => isRecord(e) && e.type === type) as Record<string, unknown> | undefined;
}

function reduceFrame(frame: Record<string, unknown>, opts: FrameOptions): Record<string, unknown> {
  const base: Record<string, unknown> = {};
  for (const key of FRAME_KEYS) base[key] = frame[key];
  if (opts.context) base.context = frame.context;
  if (opts.vars) base.vars = frame.vars;
  return base;
}

function reduceException(entries: unknown, opts: FrameOptions): unknown[] {
  const data = findEntry(entries, "exception")?.data;
  const values = isRecord(data) && Array.isArray(data.values) ? (data.values as Record<string, unknown>[]) : [];
  return values.map((value) => {
    if (opts.full) return value;
    const stacktrace = value.stacktrace;
    const frames = isRecord(stacktrace) && Array.isArray(stacktrace.frames) ? (stacktrace.frames as Record<string, unknown>[]) : [];
    const kept = opts.all ? frames : frames.filter((f) => f.inApp === true);
    return { type: value.type, value: value.value, module: value.module, frames: kept.map((f) => reduceFrame(f, opts)) };
  });
}

function frameOptionsFrom(only: string[] | undefined): FrameOptions {
  const set = new Set(only ?? []);
  const full = set.has("frames.full");
  return { all: full || set.has("frames.all"), context: full || set.has("frames.context"), vars: full || set.has("frames.vars"), full };
}

const LATEST_EVENT_MAP: FieldMap = {
  fields: ["eventID", "dateCreated", "title", "culprit", "message", "tags", "contexts", "user", "packages", "_meta", "groupingConfig", "fingerprints", "breadcrumbs", "exception"],
  default: ["eventID", "dateCreated", "title", "culprit", "message", "tags", "contexts.runtime", "contexts.os", "contexts.trace", "exception"],
  presets: {
    breadcrumbs: ["breadcrumbs"],
    packages: ["packages"],
    request: ["contexts.response", "user"],
    grouping: ["groupingConfig", "fingerprints"],
    raw: ["_meta"],
    "frames.all": ["exception"],
    "frames.context": ["exception"],
    "frames.vars": ["exception"],
    "frames.full": ["exception"],
  },
};

// Sentry's tools. Each declares its policy category, log line, and REST call;
// gating, logging, and response shaping are handled by actionAdapter.
export function sentryTools(cfg: SentryConfig): ActionTool[] {
  return [
    {
      name: "list_projects",
      description: "List projects in the organization (slug, name, platform).",
      category: "read",
      schema: { only: onlySchema(["deploys", "access", "capabilities"]) },
      run: async (a) => {
        const projects = await sentryRequest(cfg, "GET", `/organizations/${cfg.org}/projects/`);
        return applyOnly(projects, onlyValue(a), LIST_PROJECTS_MAP);
      },
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
        only: onlySchema(["stats", "triage", "links", "meta"]),
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
        const capped = Array.isArray(issues) ? issues.slice(0, limit) : issues;
        return applyOnly(capped, onlyValue(a), LIST_ISSUES_MAP);
      },
    },
    {
      name: "get_issue",
      description: "Get a single issue by its id or short id (e.g. BACKEND-1A)",
      category: "read",
      schema: {
        id: z.string().describe("Issue id (numeric) or short id"),
        only: onlySchema(["stats", "tags", "activity", "releases"]),
      },
      detail: (a) => `get_issue ${a.id}`,
      run: async (a) => {
        const issue = await sentryRequest(cfg, "GET", `/organizations/${cfg.org}/issues/${encodeURIComponent(String(a.id))}/`);
        return applyOnly(issue, onlyValue(a), GET_ISSUE_MAP);
      },
    },
    {
      name: "latest_event",
      description: "Get the latest event for an issue, including the stacktrace and tags",
      category: "read",
      schema: {
        id: z.string().describe("Issue id (numeric) or short id"),
        only: onlySchema(["frames.all", "frames.context", "frames.vars", "frames.full", "breadcrumbs", "packages", "request", "grouping", "raw"]),
      },
      detail: (a) => `latest_event ${a.id}`,
      run: async (a) => {
        const raw = await sentryRequest<Record<string, unknown>>(cfg, "GET", `/issues/${encodeURIComponent(String(a.id))}/events/latest/`);
        const only = onlyValue(a);
        if (only?.includes("*")) return raw;
        const frameOpts = frameOptionsFrom(only);
        const derived: Record<string, unknown> = {
          eventID: raw.eventID,
          dateCreated: raw.dateCreated,
          title: raw.title,
          culprit: raw.culprit,
          message: raw.message,
          tags: raw.tags,
          contexts: raw.contexts,
          user: raw.user,
          packages: raw.packages,
          _meta: raw._meta,
          groupingConfig: raw.groupingConfig,
          fingerprints: raw.fingerprints,
          breadcrumbs: findEntry(raw.entries, "breadcrumbs")?.data,
          exception: reduceException(raw.entries, frameOpts),
        };
        return applyOnly(derived, only, LATEST_EVENT_MAP);
      },
    },
    {
      name: "list_event_attachments",
      description: "List an event's attachments and download each one to a local file path you can open. Defaults to the issue's latest event unless event_id is given.",
      category: "read",
      schema: {
        id: z.string().describe("Issue id (numeric) or short id"),
        event_id: z.string().optional().describe("Event id (hex). Omit to use the issue's latest event."),
        project: z.string().optional().describe("Project slug. Defaults to the integration's project, else derived from the issue."),
        only: onlySchema([]),
      },
      detail: (a) => `list_event_attachments ${a.id}${a.event_id ? ` event=${a.event_id}` : ""}`,
      run: async (a) => {
        const issueId = String(a.id);
        const project = (a.project as string | undefined) ?? cfg.project ?? (await resolveIssueProject(cfg, issueId));
        if (!project) throw new Error(NO_PROJECT);
        const eventId = (a.event_id as string | undefined) ?? (await resolveLatestEventId(cfg, issueId));
        const attachments = await sentryRequest<Record<string, unknown>[]>(cfg, "GET", `/projects/${cfg.org}/${encodeURIComponent(project)}/events/${encodeURIComponent(eventId)}/attachments/`);
        if (!Array.isArray(attachments)) return attachments;

        const results: Record<string, unknown>[] = [];
        for (const att of attachments) {
          const ref = attachmentReference(att, project, eventId);
          const metadata = {
            id: ref.id,
            name: ref.name,
            mimetype: att.mimetype ?? att.mime_type,
            dateCreated: att.dateCreated,
            project,
            event_id: eventId,
          };
          try {
            const { path, warning } = await downloadAttachment(cfg, ref);
            const file = await stat(path);
            results.push({ ...metadata, size: file.size, path, ...(warning ? { warning } : {}) });
          } catch (error) {
            results.push({ ...metadata, size: ref.size, path: null, error: error instanceof Error ? error.message : String(error) });
          }
        }
        return applyOnly(results, onlyValue(a), LIST_EVENT_ATTACHMENTS_MAP);
      },
    },
    {
      name: "read_event_attachment",
      description: "Download one attachment and return a local file path to open. Attachment content is never returned in the tool response.",
      category: "read",
      schema: {
        project: z.string().optional().describe("Project slug. Defaults to the integration's project."),
        event_id: z.string().describe("Event id (hex) — returned by list_event_attachments."),
        attachment_id: z.string().describe("Attachment id — returned by list_event_attachments."),
        name: z.string().optional().describe("File name returned by list_event_attachments."),
        size: z.number().int().min(0).optional().describe("Attachment size in bytes returned by list_event_attachments."),
      },
      detail: (a) => `read_event_attachment ${a.attachment_id} event=${a.event_id}`,
      run: async (a) => {
        const project = (a.project as string | undefined) ?? cfg.project;
        if (!project) throw new Error(NO_PROJECT);
        const ref: AttachmentReference = {
          project,
          eventId: String(a.event_id),
          id: String(a.attachment_id),
          name: typeof a.name === "string" ? a.name : undefined,
          size: attachmentSize(a.size),
        };
        const { path, warning } = await downloadAttachment(cfg, ref);
        const file = await stat(path);
        return { id: ref.id, name: ref.name ?? `attachment-${ref.id}`, size: file.size, path, ...(warning ? { warning } : {}) };
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
