/**
 * Adding MCP servers from a config copied out of another client.
 *
 * The host reads the text into one draft per server. The user reviews them
 * here: renames any whose name is taken, flips secret guesses, skips the ones
 * they do not want. The host then adds the rest through the usual create path.
 */

export interface DraftRow {
  name: string;
  value: string;
  secret: boolean;
}

/** One server as the host read it. Field names match the host's. */
export interface ServerDraft {
  name: string;
  connection: "remote" | "local";
  url?: string | null;
  headers: DraftRow[];
  command?: string | null;
  args: string[];
  env: DraftRow[];
  cwd?: string | null;
  disabledTools: string[];
  notImported: string[];
  viaMcpRemote: boolean;
  turnedOff: boolean;
  sse: boolean;
}

export interface ServerProblem {
  name: string;
  message: string;
}

export interface ParsedImport {
  servers: ServerDraft[];
  problems: ServerProblem[];
}

/** Text the host could not read, and where, counted from 1. */
export interface ImportError {
  message: string;
  line?: number;
  column?: number;
}

/** How saving one server went. */
export interface ImportedServer {
  name: string;
  integration?: { id: string };
  error?: string;
}

/** A draft under review, with the user's choice to leave it out. */
export interface ReviewItem {
  draft: ServerDraft;
  skip: boolean;
  /** Why the last save did not add it. */
  error?: string;
}

export function reviewItems(parsed: ParsedImport): ReviewItem[] {
  // A server the config had turned off starts skipped, so nothing off comes back on unasked.
  return parsed.servers.map((draft) => ({ draft, skip: draft.turnedOff }));
}

function key(name: string): string {
  return name.trim().toLowerCase();
}

/** Why an item's name cannot be saved, or null when it can. */
export function nameProblem(items: ReviewItem[], index: number, taken: string[]): string | null {
  const item = items[index];
  if (item.skip) return null;
  const name = key(item.draft.name);
  if (name === "") return "Add a name for this server.";
  if (taken.some((t) => key(t) === name)) return "This name is already in Pluk. Choose another.";
  const twin = items.some((other, i) => i !== index && !other.skip && key(other.draft.name) === name);
  return twin ? "Another server in this list has this name. Choose another." : null;
}

export function chosen(items: ReviewItem[]): ReviewItem[] {
  return items.filter((item) => !item.skip);
}

export function canSave(items: ReviewItem[], taken: string[]): boolean {
  return chosen(items).length > 0 && items.every((_, i) => nameProblem(items, i, taken) === null);
}

export function saveLabel(items: ReviewItem[]): string {
  const count = chosen(items).length;
  return count === 1 ? "Add 1 server" : `Add ${count} servers`;
}

/** The parse error, led by where it is when the host knows. */
export function describeImportError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const { message, line, column } = error as ImportError;
    if (line != null && column != null) return `Line ${line}, column ${column}: ${message}`;
    if (line != null) return `Line ${line}: ${message}`;
    return message;
  }
  return String(error);
}

/**
 * What is left to review after a save: the items that were not added, each
 * with its reason. Saved and skipped ones drop out.
 */
export function afterSave(items: ReviewItem[], outcomes: ImportedServer[]): ReviewItem[] {
  const failed = new Map(outcomes.filter((o) => o.error).map((o) => [key(o.name), o.error]));
  return chosen(items)
    .filter((item) => failed.has(key(item.draft.name)))
    .map((item) => ({ ...item, error: failed.get(key(item.draft.name)) }));
}

/** The drafts a save added, matched to its outcomes by name. */
export function addedDrafts(drafts: ServerDraft[], outcomes: ImportedServer[]): ServerDraft[] {
  const added = new Set(outcomes.filter((o) => o.integration).map((o) => key(o.name)));
  return drafts.filter((draft) => added.has(key(draft.name)));
}

export function savedIds(outcomes: ImportedServer[]): string[] {
  return outcomes.flatMap((o) => (o.integration ? [o.integration.id] : []));
}

/** The line shown once some servers were added and some were not. */
export function partialMessage(added: number, left: number): string {
  const servers = (n: number) => (n === 1 ? "1 server" : `${n} servers`);
  return left === 1
    ? `Added ${servers(added)}. 1 server wasn't added. Fix the problem below, or skip it.`
    : `Added ${servers(added)}. ${left} servers weren't added. Fix the problems below, or skip them.`;
}

/** The toast after every chosen server was added. */
export function doneMessage(drafts: ServerDraft[]): { title: string; description?: string } {
  const title = drafts.length === 1 ? `Added ${drafts[0].name}` : `Added ${drafts.length} servers`;
  const local = drafts.some((d) => d.connection === "local");
  return local
    ? { title, description: "Local servers start only after you approve their command. Open each one to review it." }
    : { title };
}
