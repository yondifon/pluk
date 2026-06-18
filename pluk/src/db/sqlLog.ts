import { AsyncLocalStorage } from "node:async_hooks";
import { logExecutedStatement } from "../store/queryLog.js";

// Carries the originating tool/operation through async driver calls so the
// driver layer can tag every executed statement without threading a parameter
// through every method. Only statements run inside a context are logged.

interface SqlLogContext {
  connId: string;
  connName: string;
  source: string;
}

const als = new AsyncLocalStorage<SqlLogContext>();

export function runWithSqlLog<T>(ctx: SqlLogContext, fn: () => Promise<T>): Promise<T> {
  return als.run(ctx, fn);
}

/** Record one statement sent to the DB. No-ops outside a logging context. */
export function recordExecutedSql(sql: string, rowCount: number | null, error?: string): void {
  const ctx = als.getStore();
  if (!ctx) return;
  logExecutedStatement(ctx.connId, ctx.connName, sql, ctx.source, rowCount, error);
}
