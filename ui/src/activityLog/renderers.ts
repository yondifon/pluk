import type { LogEntry } from "./types";
import { escapeHtml } from "./highlight";

export type EntryType = "database" | "command" | "forward" | "http" | "policy" | "error" | "generic";
export type EntryRenderer = (entry: LogEntry) => string;

function parseResult(raw: string): { fields: string[]; rows: Record<string, unknown>[] } | null {
  try {
    const result = JSON.parse(raw) as { fields?: string[]; rows?: Record<string, unknown>[] };
    return { fields: result.fields ?? [], rows: result.rows ?? [] };
  } catch { return null; }
}

function resultTable(entry: LogEntry): string {
  if (!entry.resultJson) return "";
  const result = parseResult(entry.resultJson);
  if (!result || result.fields.length === 0) return "";
  const head = result.fields.map(field => `<span class="al-th">${escapeHtml(field)}</span>`).join("");
  const rows = result.rows.map(row => `<div class="al-tr">${result.fields.map(field => `<span class="al-td">${escapeHtml(row[field] == null ? "NULL" : String(row[field]))}</span>`).join("")}</div>`).join("");
  return `<div class="al-table"><div class="al-table-head">${head}</div>${rows}</div>`;
}

export function responseTextForCopy(entry: LogEntry): string {
  if (!entry.resultJson) return entry.responseText ?? entry.reason ?? "";
  const result = parseResult(entry.resultJson);
  if (!result || result.fields.length === 0) return entry.responseText ?? entry.reason ?? entry.resultJson;
  return [result.fields, ...result.rows.map(row => result.fields.map(field => row[field] == null ? "NULL" : String(row[field])))].map(row => row.join("\t")).join("\n");
}

function copyButton(kind: "request" | "response"): string {
  return `<button class="al-copy-block" data-copy-block="${kind}" aria-label="Copy ${kind}"></button>`;
}

function detailBlock(kind: "request" | "response", content: string): string {
  return `<div class="al-detail-section">${copyButton(kind)}${content}</div>`;
}

const databaseRenderer: EntryRenderer = entry => `<section class="al-detail">${detailBlock("request", `<pre class="al-detail-text" data-sql="${entry.id}">${escapeHtml(entry.sql)}</pre>`)}${detailBlock("response", resultTable(entry) || `<pre class="al-detail-text">${escapeHtml(entry.reason ?? "No response")}</pre>`)}</section>`;

const commandRenderer: EntryRenderer = entry => {
  const result = entry.resultJson ? parseResult(entry.resultJson) : null;
  const exitCode = result?.rows[0]?.exit_code ?? result?.rows[0]?.exitCode;
  const output = entry.responseText ?? entry.reason ?? "No response";
  const exit = exitCode === undefined ? "" : `\nExit code: ${String(exitCode)}`;
  return `<section class="al-detail">${detailBlock("request", `<pre class="al-detail-text" data-cmd="${entry.id}">${escapeHtml(entry.sql)}</pre>`)}${detailBlock("response", `<pre class="al-detail-text" data-console="${entry.id}">${escapeHtml(output + exit)}</pre>`)}</section>`;
};

const httpRenderer: EntryRenderer = entry => `<section class="al-detail">${detailBlock("request", `<pre class="al-detail-text">${escapeHtml(entry.sql)}</pre>`)}${detailBlock("response", `<pre class="al-detail-text">${escapeHtml(entry.responseText ?? entry.reason ?? "No response")}</pre>`)}</section>`;
const forwardRenderer: EntryRenderer = entry => httpRenderer(entry);
const policyRenderer: EntryRenderer = entry => httpRenderer({ ...entry, responseText: entry.reason ?? "This request is not allowed" });
const errorRenderer: EntryRenderer = entry => httpRenderer({ ...entry, responseText: entry.reason ?? entry.responseText ?? "Unknown error" });
const genericRenderer: EntryRenderer = entry => httpRenderer(entry);

export const ENTRY_RENDERERS: Record<EntryType, EntryRenderer> = { database: databaseRenderer, command: commandRenderer, forward: forwardRenderer, http: httpRenderer, policy: policyRenderer, error: errorRenderer, generic: genericRenderer };

export function entryType(entry: LogEntry, connectionType?: string): EntryType {
  if (entry.verdict === "blocked") return "policy";
  if (entry.verdict === "error" || entry.verdict === "cancelled") return "error";
  if (entry.categories === "command" || ["ssh", "github-cli", "spark", "herd"].includes(connectionType ?? "")) return "command";
  if (entry.categories === "forward") return "forward";
  if (entry.categories?.includes("database") || ["query", "export_query", "run_saved_query"].includes(entry.source ?? "")) return "database";
  if (entry.source) return "http";
  return "generic";
}
