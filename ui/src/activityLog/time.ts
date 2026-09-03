/**
 * Timestamp handling — port of LogTime in ConnectionLogView.swift
 * Stored as UTC "yyyy-MM-dd HH:mm:ss" (SQLite datetime('now')).
 * Parsed explicitly with POSIX locale, then shown in local time.
 */

const UTC_RE = /^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2})$/;

export function parseUtcToMillis(raw: string): number | null {
  const m = UTC_RE.exec(raw);
  if (!m) return null;
  const [, ys, mos, ds, hs, mins, ss] = m;
  const y = Number(ys), mo = Number(mos), d = Number(ds), h = Number(hs), mi = Number(mins), s = Number(ss);
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59 || s > 59) return null;
  return Date.UTC(y, mo - 1, d, h, mi, s);
}

export function parseUtcToDate(raw: string): Date | null {
  const ms = parseUtcToMillis(raw);
  return ms === null ? null : new Date(ms);
}

/** Relative label like Swift: just now / 12s ago / 3m ago / 2h ago / 1d ago */
export function relativeTime(raw: string): string {
  const ms = parseUtcToMillis(raw);
  if (ms === null) return raw;
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 10) return "just now";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

/** Full local time string yyyy-MM-dd HH:mm:ss */
export function localTimeString(raw: string): string {
  const d = parseUtcToDate(raw);
  if (!d) return raw;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function formatUnixSeconds(secs: number): string {
  const d = new Date(secs * 1000);
  const pad = (n: number) => String(n).padStart(2,"0");
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth()+1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}
