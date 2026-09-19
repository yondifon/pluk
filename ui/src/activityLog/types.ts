export type Verdict = "pending" | "allowed" | "blocked" | "cancelled" | "error";

export interface LogEntry {
  id: number;
  connectionId: string;
  connectionName: string;
  sql: string;
  verdict: string;
  reason: string | null;
  categories: string | null;
  source: string | null;
  resultJson: string | null;
  rowCount: number | null;
  responseText: string | null;
  groupId: string | null;
  groupName: string | null;
  database?: string | null;
  createdAt: string;
}

export interface LogCursor {
  createdAt: string;
  id: number;
}

export interface LogPage {
  entries: LogEntry[];
  nextCursor: LogCursor | null;
  hasMore: boolean;
}

export type TimeRange = "hour" | "today" | "7d" | "30d" | "all";

export type VerdictFilter = "all" | "allowed" | "blocked" | "error";

export const verdictFilters: VerdictFilter[] = ["all", "allowed", "blocked", "error"];

/** One word per verdict, shared by the row status and the filter chips. */
export const verdictLabels: Record<string, string> = {
  allowed: "Successful",
  blocked: "Blocked",
  cancelled: "Cancelled",
  error: "Failed",
  pending: "Running",
};

export function verdictLabel(verdict: string): string {
  return verdictLabels[verdict] ?? verdict;
}

export function verdictFilterLabel(filter: VerdictFilter): string {
  return filter === "all" ? "All" : verdictLabels[filter];
}

export function verdictFilterCounts(entries: LogEntry[]): Record<VerdictFilter, number> {
  const counts: Record<VerdictFilter, number> = { all: entries.length, allowed: 0, blocked: 0, error: 0 };
  for (const entry of entries) {
    if (entry.verdict === "allowed" || entry.verdict === "blocked" || entry.verdict === "error") {
      counts[entry.verdict] += 1;
    }
  }
  return counts;
}

export function isCommandAdapter(type?: string | null): boolean {
  return type === "ssh" || type === "github-cli" || type === "spark" || type === "herd";
}
