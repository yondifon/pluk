import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { sparkFields } from "./fields.js";
import {
  assertMessageId,
  assertPositional,
  flag,
  flagEach,
  humanizeSparkError,
  list,
  paging,
  range,
  runSpark,
  sameAccount,
  scoped,
  sparkConfig,
  testSpark,
  toggle,
  type SparkCfg,
} from "./client.js";

const AGENT_HINT =
  "Use this for the user's mail, calendar, contacts and meetings in Spark. accounts first to see accounts, calendars and each one's access level; list_emails to browse a folder, search_emails to answer questions (it returns bodies), read_thread for the whole conversation. When this integration names an account every folder, scope and calendar is confined to it — a bare folder name means that account's folder, and another account, shared inbox or team is refused, not silently redirected. Spark itself gates writes per account (read-only / triage / send) on top of this integration's tools.";

const ACCESS =
  "Reads mail, calendar, contacts, meetings and teams from the Spark Desktop running on this machine; drafts, comments, email and contact actions, and calendar writes only when those tools are enabled. Sending a draft and deleting an event are separate tools, off by default. Every call is policy-checked and recorded in the activity log — including the message bodies Spark returns.";

/** Spark's email verbs, minus `send` / `unschedule` — those emit mail (or cancel
 *  a send) and need `send` access, so they live in their own gated tools. */
const EMAIL_ACTIONS = [
  "pin", "unpin", "mute", "unmute", "snooze", "unsnooze", "changeReminder", "clearReminder",
  "setAside", "archive", "moveToInbox", "moveToTrash", "moveToFolder", "attachLabel", "detachLabel",
  "markAsDone", "markAsUndone", "markAsSeen", "markAsUnseen", "markAsSpam",
  "markThreadAsPriority", "unmarkThreadAsPriority", "unsubscribe",
  "changeCategoryPersonal", "changeCategoryNotification", "changeCategoryNewsletters",
  "shareInTeam", "assign", "delegationComplete", "delegationReopen",
] as const;

const CONTACT_ACTIONS = [
  "changeCategoryPersonal", "changeCategoryNotification", "changeCategoryNewsletters",
  "groupEmailsFromContact", "groupEmailsFromContactAndShowInInbox", "ungroupEmailsFromContact",
  "markContactAsImportant", "unmarkContactAsImportant", "markContactAsPrimary", "unmarkContactAsPrimary",
  "acceptContact", "blockContact", "acceptDomain", "blockDomain",
  "enableAutosummaryForContact", "disableAutosummaryForContact",
] as const;

const RANGE = z.enum(["today", "tomorrow", "week"]).optional().describe("Range shortcut; ignored when start/end are given");
const START = z.string().optional().describe("Start date: yyyy-MM-dd, dd/MM/yyyy or yyyy-MM-ddTHH:mm");
const END = z.string().optional().describe("End date, same formats as start");
const PAGE = z.number().int().min(1).optional().describe("Page number, 1-based");
const PAGE_SIZE = z.number().int().min(1).optional().describe("Rows per page; capped by the integration");
const FILTER = z
  .string()
  .optional()
  .describe(
    "Gmail-style filter, combinable: from: to: cc: subject: before:yyyy/MM/dd after: newer_than:7d older_than:30d has:attachment is:unread is:starred is:pinned category:priority assigned_to:me filename:",
  );

const ids = (a: Record<string, unknown>, what = "message id"): string[] => {
  const out = list(a.message_ids).map((v) => assertMessageId(v, what));
  if (!out.length) throw new Error(`At least one ${what} is required.`);
  return out;
};

function readTools(cfg: () => SparkCfg): ActionTool[] {
  const spark = (args: string[]) => runSpark(cfg(), args);

  return [
    {
      name: "accounts",
      description: "List accounts with their calendars, teams, shared inboxes and each one's Spark access level. Run this first.",
      category: "read",
      run: () => spark(["accounts"]),
    },
    {
      name: "folders",
      description: "List folders and labels with message counts. Returns the qualified identifiers other tools take.",
      category: "read",
      schema: { accounts: z.array(z.string()).optional().describe("Account or shared-inbox addresses; the integration's account, or all of them when it names none, when omitted") },
      detail: (a) => `folders ${list(a.accounts).join(" ") || "all"}`,
      run: (a) => {
        const c = cfg();
        const asked = list(a.accounts).map((v) => sameAccount(c, assertPositional(v, "account")));
        return spark(["folders", ...(asked.length ? asked : list(c.account))]);
      },
    },
    {
      name: "list_emails",
      description:
        "List emails in a folder — id, account, sender, date, subject, flags. Browsing only: use search_emails to find mail across every folder.",
      category: "read",
      schema: {
        folders: z.array(z.string()).optional().describe('Folder ids from `folders`, e.g. "you@co.com:Archive". The integration\'s inbox — or the cross-account Unified Inbox when it names no account — when omitted'),
        filter: FILTER,
        order: z.enum(["ascending", "descending"]).optional(),
        new_senders: z.boolean().optional().describe("Only mail from senders GateKeeper is holding back"),
        page: PAGE,
        page_size: PAGE_SIZE,
      },
      detail: (a) => `list_emails ${list(a.folders).join(" ") || "inbox"}${a.filter ? ` [${a.filter}]` : ""}`,
      run: (a) => {
        const c = cfg();
        const asked = list(a.folders).length ? list(a.folders) : list(c.folder);
        // Nothing named anywhere: the scoped account's own inbox, since the
        // bare default (Unified Inbox) would span every account.
        const folders = asked.length ? asked : list(c.account);
        const args = ["emails", ...folders.map((v) => scoped(c, assertPositional(v, "folder")))];
        flag(args, "--filter", a.filter);
        flag(args, "--order", a.order);
        toggle(args, "--new-senders", a.new_senders);
        paging(args, c, a);
        return spark(args);
      },
    },
    {
      name: "search_emails",
      description:
        "Search mail across every folder. With `about` it does keyword + semantic matching and returns bodies — use it to answer questions. Without it, filters across all folders.",
      category: "read",
      schema: {
        about: z.string().optional().describe("Topic to search for; omit to list by filter instead"),
        filter: FILTER,
        in: z.string().optional().describe('Scope: account, "Team Name", shared inbox or a qualified folder. The integration\'s account — or every folder when it names none — when omitted'),
        order: z.enum(["ascending", "descending"]).optional().describe("List mode only"),
        page: PAGE,
        page_size: PAGE_SIZE,
      },
      detail: (a) => `search_emails ${(a.about as string) ?? ""}${a.filter ? ` [${a.filter}]` : ""}`.trim(),
      run: (a) => {
        const c = cfg();
        const args = ["search"];
        flag(args, "--filter", a.filter);
        // Scoping the search also opts it into that account's Trash and Spam,
        // which an unscoped `search` leaves out.
        flag(args, "--in", scoped(c, a.in, "scope") || c.account);
        flag(args, "--order", a.order);
        paging(args, c, a);
        const about = String(a.about ?? "").trim();
        if (about) args.push(assertPositional(about, "search topic"));
        return spark(args);
      },
    },
    {
      name: "read_thread",
      description: "Read a full thread — headers, plain-text bodies, attachment table and the thread's custom labels.",
      category: "read",
      schema: {
        message_id: z.string().describe("Message id from list_emails / search_emails, or a Spark deep link"),
        download_attachments: z.boolean().optional().describe("Fetch attachments that aren't cached locally yet"),
      },
      detail: (a) => `read_thread ${a.message_id}`,
      run: (a) => {
        const args = ["thread"];
        toggle(args, "--download-attachments", a.download_attachments);
        args.push(assertMessageId(a.message_id));
        return spark(args);
      },
    },
    {
      name: "read_attachment",
      description: "Show one attachment's metadata — name, size, MIME type and local path — downloading it first if needed. Ids come from read_thread.",
      category: "read",
      schema: { id: z.number().int().describe("Attachment id (pk) from the thread's Attachments table") },
      detail: (a) => `read_attachment ${a.id}`,
      // Metadata only: `--stream` writes raw bytes, which an MCP text response can't carry.
      run: (a) => spark(["attachment", String(a.id)]),
    },
    {
      name: "list_events",
      description: "List calendar events for a time range. Defaults to the rest of today.",
      category: "read",
      schema: {
        range: RANGE,
        start: START,
        end: END,
        in: z.string().optional().describe('Account or calendar, e.g. "you@co.com:Work". The integration\'s account — or every calendar when it names none — when omitted'),
      },
      detail: (a) => `list_events ${(a.range as string) ?? `${a.start ?? ""}..${a.end ?? ""}`}`,
      run: (a) => {
        const c = cfg();
        const args = ["events"];
        range(args, a);
        flag(args, "--in", scoped(c, a.in, "calendar") || c.account);
        return spark(args);
      },
    },
    {
      name: "availability",
      description: "Find free time slots — your own, or the mutual windows for a set of attendees.",
      category: "read",
      schema: {
        attendees: z.array(z.string()).optional().describe("Attendee addresses; your own calendar when omitted"),
        range: RANGE,
        start: START,
        end: END,
      },
      detail: (a) => `availability ${list(a.attendees).join(",") || "self"}`,
      run: (a) => {
        const args = ["availability"];
        range(args, a);
        flag(args, "--attendees", list(a.attendees).join(","));
        return spark(args);
      },
    },
    {
      name: "find_contacts",
      description: "Search contacts by name, part of a name, or any part of an email address including the domain.",
      category: "read",
      schema: { query: z.string().describe("Name or email fragment") },
      detail: (a) => `find_contacts ${a.query}`,
      run: (a) => spark(["contacts", assertPositional(String(a.query ?? ""), "query")]),
    },
    {
      name: "team_info",
      description: "Show a team's members, shared inboxes and assignments. Omit the name to list the available teams.",
      category: "read",
      schema: { name: z.string().optional().describe("Team name or a partial match") },
      detail: (a) => `team_info ${(a.name as string) ?? ""}`.trim(),
      run: (a) => {
        const name = String(a.name ?? "").trim();
        return spark(name ? ["team", assertPositional(name, "team name")] : ["team"]);
      },
    },
    {
      name: "list_meetings",
      description: "List the meeting transcripts Spark recorded, newest first.",
      category: "read",
      schema: {
        filter: z.string().optional().describe("subject:<text>, before:/after:yyyy/MM/dd, newer_than:30d, older_than:30d"),
        page: PAGE,
        page_size: PAGE_SIZE,
      },
      detail: (a) => `list_meetings${a.filter ? ` [${a.filter}]` : ""}`,
      run: (a) => {
        const args = ["meetings"];
        flag(args, "--filter", a.filter);
        paging(args, cfg(), a);
        return spark(args);
      },
    },
    {
      name: "read_meeting",
      description: "Read a meeting's summary, and optionally its full transcript and the user's notes.",
      category: "read",
      schema: {
        message_id: z.string().describe("Meeting id from list_meetings, or a Spark deep link"),
        transcript: z.boolean().optional().describe("Include the full transcript"),
        notes: z.boolean().optional().describe("Include the user's notes"),
      },
      detail: (a) => `read_meeting ${a.message_id}`,
      run: (a) => {
        const args = ["meeting"];
        toggle(args, "--transcript", a.transcript);
        toggle(args, "--notes", a.notes);
        args.push(assertMessageId(a.message_id, "meeting id"));
        return spark(args);
      },
    },
    {
      name: "list_templates",
      description: "List the user's saved message templates, personal and team.",
      category: "read",
      schema: {
        personal: z.boolean().optional().describe("Only templates not tied to a team"),
        team: z.string().optional().describe("Only this team's templates"),
        page: PAGE,
        page_size: PAGE_SIZE,
      },
      detail: () => "list_templates",
      run: (a) => {
        const args = ["templates"];
        toggle(args, "--personal", a.personal);
        flag(args, "--team", a.team);
        paging(args, cfg(), a);
        return spark(args);
      },
    },
    {
      name: "read_template",
      description: "Show one template's recipients, subject, body and placeholders. Run it before drafting from a template — manual placeholders are required.",
      category: "read",
      schema: { ref: z.string().describe("Template id or name") },
      detail: (a) => `read_template ${a.ref}`,
      run: (a) => spark(["template", assertPositional(String(a.ref ?? ""), "template id or name")]),
    },
  ];
}

function writeTools(cfg: () => SparkCfg): ActionTool[] {
  const spark = (args: string[]) => runSpark(cfg(), args);

  return [
    {
      name: "draft",
      description:
        "Create or edit an email draft (body in markdown). Never sends. Replying to an existing conversation? Pass reply_to with the thread's last message id, or the draft starts a new thread.",
      category: "write",
      schema: {
        to: z.array(z.string()).optional().describe("Recipient addresses"),
        cc: z.array(z.string()).optional(),
        bcc: z.array(z.string()).optional(),
        subject: z.string().optional(),
        body: z.string().optional().describe("Body in markdown; required for a new draft unless a template supplies one"),
        account: z.string().optional().describe("From address; the integration's account when omitted, and refused when it names a different one"),
        edit: z.string().optional().describe("Message id of an existing draft to update"),
        reply_to: z.string().optional().describe("Message id to reply to — required to stay in an existing thread"),
        forward: z.string().optional().describe("Message id to forward"),
        attach: z.array(z.string()).optional().describe("Absolute paths Spark can read; max 25 MB each"),
        template: z.string().optional().describe("Template id or name to apply"),
        placeholder: z.array(z.string()).optional().describe('Fill a manual template placeholder: "name=value"'),
        no_signature: z.boolean().optional().describe("Drop the account's default signature"),
      },
      detail: (a) => `draft ${a.edit ? `edit ${a.edit}` : a.reply_to ? `reply ${a.reply_to}` : a.forward ? `forward ${a.forward}` : list(a.to).join(",")}`,
      run: (a) => {
        const args = ["draft"];
        flagEach(args, "--to", a.to);
        flagEach(args, "--cc", a.cc);
        flagEach(args, "--bcc", a.bcc);
        flag(args, "--subject", a.subject);
        flag(args, "--body", a.body);
        flag(args, "--account", sameAccount(cfg(), a.account, "from address"));
        if (a.edit !== undefined) flag(args, "--edit", assertMessageId(a.edit, "draft id"));
        if (a.reply_to !== undefined) flag(args, "--reply-to", assertMessageId(a.reply_to));
        if (a.forward !== undefined) flag(args, "--forward", assertMessageId(a.forward));
        flagEach(args, "--attach", a.attach);
        flag(args, "--template", a.template);
        flagEach(args, "--placeholder", a.placeholder);
        toggle(args, "--no-signature", a.no_signature);
        return spark(args);
      },
    },
    {
      name: "comment",
      description: "Post a team comment on a thread, sharing the thread with the team first when it isn't shared yet.",
      category: "write",
      schema: {
        message_id: z.string().optional().describe("A message in the thread to comment on"),
        body: z.string().describe("Comment text"),
        team: z.string().optional().describe("Team name; the integration's default team when omitted"),
        user: z.array(z.string()).optional().describe("Teammates to share with when the thread isn't shared yet"),
        edit: z.string().optional().describe("Message id of an existing comment to edit instead"),
      },
      detail: (a) => `comment ${a.edit ? `edit ${a.edit}` : a.message_id}`,
      run: (a) => {
        const args = ["comment"];
        if (a.edit === undefined) args.push(assertMessageId(a.message_id));
        flag(args, "--body", a.body);
        flag(args, "--team", String(a.team ?? "").trim() || cfg().team);
        flagEach(args, "--user", a.user);
        if (a.edit !== undefined) flag(args, "--edit", assertMessageId(a.edit, "comment id"));
        return spark(args);
      },
    },
    {
      name: "email_action",
      description:
        "Act on one or more emails: archive, pin, snooze, move, label, categorize, share, assign, mark read/unread and so on. Sending drafts is a separate tool.",
      category: "write",
      schema: {
        action: z.enum(EMAIL_ACTIONS).describe("The verb to apply"),
        message_ids: z.array(z.string()).describe("Message ids to act on"),
        date: z.string().optional().describe("Required by snooze and changeReminder; the due date for assign"),
        folder: z.string().optional().describe("Qualified folder for moveToFolder, attachLabel and detachLabel"),
        team: z.string().optional().describe("Team for team actions; the integration's default when omitted"),
        user: z.array(z.string()).optional().describe("Teammates for shareInTeam"),
        assignee: z.string().optional().describe("Teammate address for assign"),
        comment: z.string().optional().describe("Comment attached to an assign"),
      },
      detail: (a) => `${a.action} ${ids(a).join(" ")}`,
      run: (a) => {
        const c = cfg();
        const args = ["action", String(a.action), ...ids(a)];
        flag(args, "--date", a.date);
        flag(args, "--folder", scoped(c, a.folder));
        flag(args, "--team", String(a.team ?? "").trim() || c.team);
        flagEach(args, "--user", a.user);
        flag(args, "--assignee", a.assignee);
        flag(args, "--comment", a.comment);
        return spark(args);
      },
    },
    {
      name: "contact_action",
      description: "Act on contacts: block or accept a sender or their domain, recategorize their mail, toggle priority, notifications or auto-summary.",
      category: "write",
      schema: {
        action: z.enum(CONTACT_ACTIONS).describe("The verb to apply"),
        emails: z.array(z.string()).describe("Contact addresses to act on"),
      },
      detail: (a) => `${a.action} ${list(a.emails).join(" ")}`,
      run: (a) => {
        const emails = list(a.emails).map((v) => assertPositional(v, "contact address"));
        if (!emails.length) throw new Error("At least one contact address is required.");
        return spark(["contact-action", String(a.action), ...emails]);
      },
    },
    {
      name: "event_write",
      description:
        "Create or update a calendar event, or RSVP to an invitation. Adding or removing attendees mails invitations and cancellations, so Spark requires send access on the account.",
      category: "write",
      schema: {
        mode: z.enum(["create", "update", "rsvp"]),
        event_id: z.string().optional().describe("Required for update and rsvp: a calendar event id, or the invitation email's message id"),
        status: z.enum(["accept", "decline", "maybe"]).optional().describe("Required for rsvp"),
        title: z.string().optional(),
        start: START,
        end: END,
        all_day: z.boolean().optional(),
        description: z.string().optional(),
        location: z.string().optional(),
        calendar: z.string().optional().describe('Target calendar for create: "you@co.com" or "you@co.com:Work". The integration\'s account when omitted'),
        video_conference: z.enum(["auto", "meet", "zoom", "teams"]).optional(),
        alerts: z.string().optional().describe("Comma-separated offsets in seconds (300s,600s) or absolute dates"),
        add: z.array(z.string()).optional().describe("Attendees to invite — they receive an invitation"),
        remove: z.array(z.string()).optional().describe("Attendees to remove on update — they receive a cancellation"),
      },
      detail: (a) => `event ${a.mode} ${(a.event_id as string) ?? (a.title as string) ?? ""}`.trim(),
      run: (a) => {
        const c = cfg();
        const mode = String(a.mode);
        const args = ["event"];
        flag(args, "--title", a.title);
        flag(args, "--start", a.start);
        flag(args, "--end", a.end);
        toggle(args, "--all-day", a.all_day);
        flag(args, "--description", a.description);
        flag(args, "--location", a.location);
        flag(args, "--calendar", scoped(c, a.calendar, "calendar") || c.account);
        flag(args, "--video-conference", a.video_conference);
        flag(args, "--alerts", a.alerts);
        flagEach(args, "--add", a.add);
        flagEach(args, "--remove", a.remove);

        args.push(mode);
        if (mode !== "create") args.push(assertPositional(String(a.event_id ?? ""), "event id"));
        if (mode === "rsvp") args.push(assertPositional(String(a.status ?? ""), "rsvp status"));
        return spark(args);
      },
    },
    {
      name: "delete_event",
      description: "Delete a calendar event. An event with attendees also mails them a cancellation.",
      category: "delete",
      schema: { event_id: z.string().describe("Calendar event id from list_events") },
      detail: (a) => `delete_event ${a.event_id}`,
      run: (a) => spark(["event", "delete", assertPositional(String(a.event_id ?? ""), "event id")]),
    },

    // ── Mail-emitting tools ──────────────────────────────────────────────────
    // Off by default and gated as admin: these are the only tools that put mail
    // on the wire, and Spark itself requires `send` access on the account.
    {
      name: "send_draft",
      description: "Send an existing draft now, or schedule it with a future date. The only tool here that emits mail.",
      category: "admin",
      schema: {
        message_ids: z.array(z.string()).describe("Draft ids to send"),
        date: z.string().optional().describe("Future date to schedule for (Send Later); sends now when omitted"),
      },
      detail: (a) => `send_draft ${ids(a, "draft id").join(" ")}${a.date ? ` at ${a.date}` : ""}`,
      run: (a) => {
        const args = ["action", "send", ...ids(a, "draft id")];
        flag(args, "--date", a.date);
        return spark(args);
      },
    },
    {
      name: "unschedule_draft",
      description: "Cancel a scheduled send and return the message to drafts. Spark mints a new draft id, so re-list Drafts afterwards.",
      category: "admin",
      schema: { message_ids: z.array(z.string()).describe("Scheduled message ids") },
      detail: (a) => `unschedule_draft ${ids(a, "message id").join(" ")}`,
      run: (a) => spark(["action", "unschedule", ...ids(a, "message id")]),
    },
  ];
}

export const sparkAdapter = actionAdapter<() => SparkCfg>({
  id: "spark",
  label: "Spark Mail",
  category: "email",
  agentHint: AGENT_HINT,
  access: ACCESS,
  start: "accounts",
  configFields: sparkFields,
  // Read per call, not at register time: the CLI path and defaults are local
  // state that can change under a long-lived session.
  client: (conn) => () => sparkConfig(conn),
  testConnection: (conn: Integration) => testSpark(conn),
  tools: (_conn, cfg) => [...readTools(cfg), ...writeTools(cfg)],
  humanizeError: humanizeSparkError,
});
