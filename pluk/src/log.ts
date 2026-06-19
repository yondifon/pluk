import { appendFileSync, mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";

// Append-only debug log at ~/.pluk/pluk.log. Mirrored to stdout/stderr so the
// macOS app's captured server output still shows it. Errors are logged with
// full detail (message + driver error code + stack) — the terse messages
// surfaced to the UI (e.g. "SASL authentication failed") lose that here.

const LOG_DIR = join(homedir(), ".pluk");
export const LOG_PATH = join(LOG_DIR, "pluk.log");

mkdirSync(LOG_DIR, { recursive: true });

type Level = "info" | "warn" | "error";

function write(level: Level, msg: string, meta?: Record<string, unknown>): void {
  const metaStr = meta && Object.keys(meta).length ? " " + JSON.stringify(meta) : "";
  const line = `${new Date().toISOString()} [${level}] ${msg}${metaStr}`;
  try {
    appendFileSync(LOG_PATH, line + "\n");
  } catch {
    // logging must never break a request
  }
  (level === "error" ? console.error : console.log)(line);
}

export function logInfo(msg: string, meta?: Record<string, unknown>): void {
  write("info", msg, meta);
}

export function logError(msg: string, err: unknown, meta?: Record<string, unknown>): void {
  const e = err as { message?: string; code?: string; stack?: string };
  write("error", msg, { ...meta, error: e?.message, code: e?.code, stack: e?.stack });
}
