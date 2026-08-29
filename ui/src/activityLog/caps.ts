/** Caps exactly as ConnectionLogView.swift — do not raise. */

export const PREVIEW_LINES = 10;
export const PREVIEW_CHARS = 1200;
export const CONSOLE_PREVIEW_LINES = 40;
export const CONSOLE_PREVIEW_CHARS = 6000;

export interface CapResult {
  preview: string;
  truncated: boolean;
}

export function capText(text: string, maxLines: number, maxChars: number): CapResult {
  const lines = text.split("\n");
  const slice = lines.slice(0, maxLines).join("\n");
  const capped = slice.slice(0, maxChars);
  const truncated = lines.length > maxLines || text.length > capped.length;
  return { preview: capped, truncated };
}

export function capResponse(text: string): CapResult {
  return capText(text, PREVIEW_LINES, PREVIEW_CHARS);
}

export function capConsole(text: string): CapResult {
  return capText(text, CONSOLE_PREVIEW_LINES, CONSOLE_PREVIEW_CHARS);
}
