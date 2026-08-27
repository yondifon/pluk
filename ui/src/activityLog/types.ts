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

export const timeRangeLabels: Record<TimeRange, string> = {
  hour: "Last hour",
  today: "Today",
  "7d": "Last 7 days",
  "30d": "Last 30 days",
  all: "All time",
};

export type VerdictFilter = "all" | "allowed" | "blocked" | "error";

export function isCommandAdapter(type?: string | null): boolean {
  return type === "ssh" || type === "github-cli" || type === "spark" || type === "herd";
}
