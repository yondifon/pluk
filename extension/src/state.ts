import { type Action, isAction } from "./protocol";

const DEFAULT_PORT = 4242;
export const DEFAULT_SERVER_URL = `http://127.0.0.1:${DEFAULT_PORT}`;
export const ALL_URLS_ORIGIN = "<all_urls>";
// How many ports above the default discovery probes before giving up.
const DISCOVERY_PORT_SPAN = 8;
const PROBE_TIMEOUT_MS = 1_000;
export const SITE_ORIGINS = [
  "https://x.com/*",
  "https://www.x.com/*",
  "https://twitter.com/*",
  "https://www.twitter.com/*",
] as const;
const MIN_TOKEN_LENGTH = 16;
const MAX_TOKEN_LENGTH = 256;
const MAX_STATUS_MESSAGE_LENGTH = 512;
const MAX_LEDGER_ENTRIES = 128;
const MAX_LEDGER_AGE_MS = 24 * 60 * 60 * 1000;

const SETTINGS_KEY = "connectionSettings";
const STATUS_KEY = "connectionStatus";
const AUTOMATION_KEY = "automationContext";
const LEDGER_KEY = "commandLedger";

export interface ConnectionSettings {
  readonly serverUrl: string;
  readonly token: string;
  readonly enabled: boolean;
}

export type ConnectionState =
  | "not_configured"
  | "disabled"
  | "connecting"
  | "connected"
  | "error";

export interface ConnectionStatus {
  readonly state: ConnectionState;
  readonly message: string;
  readonly updatedAt: number;
}

export interface AutomationContext {
  readonly windowId: number;
  readonly tabId: number;
}

export interface CommandLedgerEntry {
  readonly commandId: string;
  readonly action: Action;
  readonly state: "inflight" | "completed";
  readonly updatedAt: number;
}

export const DEFAULT_SETTINGS: ConnectionSettings = {
  serverUrl: DEFAULT_SERVER_URL,
  token: "",
  enabled: false,
};

export const DEFAULT_STATUS: ConnectionStatus = {
  state: "not_configured",
  message: "Paste your Pluk ID to connect.",
  updatedAt: 0,
};

// The addresses Pluk could be listening on, the last known good one first.
export function candidateServerUrls(preferred: string): readonly string[] {
  const candidates = [preferred];
  for (let offset = 0; offset <= DISCOVERY_PORT_SPAN; offset += 1) {
    const candidate = `http://127.0.0.1:${DEFAULT_PORT + offset}`;
    if (!candidates.includes(candidate)) {
      candidates.push(candidate);
    }
  }
  return candidates;
}

// Find the running Pluk on this machine, or `null` when none answers.
export async function discoverServerUrl(
  preferred: string,
): Promise<string | null> {
  const candidates = candidateServerUrls(preferred);
  const answered = await Promise.all(candidates.map(isPlukAddress));
  const index = answered.indexOf(true);
  return index === -1 ? null : (candidates[index] ?? null);
}

async function isPlukAddress(serverUrl: string): Promise<boolean> {
  let response: Response;
  try {
    response = await fetch(`${serverUrl}/wande/healthz`, {
      credentials: "omit",
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
  } catch {
    return false;
  }
  if (!response.ok) {
    return false;
  }
  try {
    const body: unknown = await response.json();
    return isRecord(body) && body.status === "ok";
  } catch {
    return false;
  }
}

export function parseServerUrl(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > 256) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (
    url.protocol !== "http:" ||
    url.hostname !== "127.0.0.1" ||
    url.username !== "" ||
    url.password !== "" ||
    url.port === "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    return null;
  }
  const port = Number(url.port);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    return null;
  }
  return `http://127.0.0.1:${port}`;
}

export function makeWebSocketUrl(serverUrl: string, token: string): string {
  return `${serverUrl.replace(/^http:/u, "ws:")}/wande/extension/ws?token=${encodeURIComponent(token)}`;
}

export function parseConnectionSettings(
  value: unknown,
): ConnectionSettings | null {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(value, ["serverUrl", "token", "enabled"])
  ) {
    return null;
  }
  const serverUrl = parseServerUrl(value.serverUrl);
  if (
    serverUrl === null ||
    !isSingleLineString(value.token, MAX_TOKEN_LENGTH) ||
    (value.token !== "" && !isValidPlukId(value.token)) ||
    typeof value.enabled !== "boolean" ||
    (value.enabled && value.token.length === 0)
  ) {
    return null;
  }
  return { serverUrl, token: value.token, enabled: value.enabled };
}

export function isValidPlukId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length >= MIN_TOKEN_LENGTH &&
    isSingleLineString(value, MAX_TOKEN_LENGTH) &&
    value.trim() === value &&
    ![...value].some((character) => character.trim() === "")
  );
}

export function parseConnectionStatus(value: unknown): ConnectionStatus | null {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(value, ["state", "message", "updatedAt"]) ||
    !isConnectionState(value.state) ||
    !isSingleLineString(value.message, MAX_STATUS_MESSAGE_LENGTH) ||
    !isTimestamp(value.updatedAt)
  ) {
    return null;
  }
  return {
    state: value.state,
    message: value.message,
    updatedAt: value.updatedAt,
  };
}

export function parseAutomationContext(
  value: unknown,
): AutomationContext | null {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(value, ["windowId", "tabId"]) ||
    !isPositiveInteger(value.windowId) ||
    !isPositiveInteger(value.tabId)
  ) {
    return null;
  }
  return { windowId: value.windowId, tabId: value.tabId };
}

export function parseCommandLedger(
  value: unknown,
  now = Date.now(),
): readonly CommandLedgerEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const entries: CommandLedgerEntry[] = [];
  for (const item of value) {
    if (
      !isRecord(item) ||
      !hasOnlyKeys(item, ["commandId", "action", "state", "updatedAt"]) ||
      !isIdentifier(item.commandId) ||
      !isAction(item.action) ||
      (item.state !== "inflight" && item.state !== "completed") ||
      !isTimestamp(item.updatedAt) ||
      now - item.updatedAt > MAX_LEDGER_AGE_MS
    ) {
      continue;
    }
    entries.push({
      commandId: item.commandId,
      action: item.action,
      state: item.state,
      updatedAt: item.updatedAt,
    });
  }
  return entries
    .sort((left, right) => left.updatedAt - right.updatedAt)
    .slice(-MAX_LEDGER_ENTRIES);
}

export async function readSettings(): Promise<ConnectionSettings> {
  const stored = await chrome.storage.local.get([SETTINGS_KEY]);
  return parseConnectionSettings(stored[SETTINGS_KEY]) ?? DEFAULT_SETTINGS;
}

export async function writeSettings(
  settings: ConnectionSettings,
): Promise<void> {
  await chrome.storage.local.set({ [SETTINGS_KEY]: settings });
}

export async function readStatus(): Promise<ConnectionStatus> {
  const stored = await chrome.storage.local.get([STATUS_KEY]);
  return parseConnectionStatus(stored[STATUS_KEY]) ?? DEFAULT_STATUS;
}

export async function writeStatus(status: ConnectionStatus): Promise<void> {
  await chrome.storage.local.set({ [STATUS_KEY]: status });
}

export async function readAutomationContext(): Promise<AutomationContext | null> {
  const stored = await chrome.storage.local.get([AUTOMATION_KEY]);
  return parseAutomationContext(stored[AUTOMATION_KEY]);
}

export async function writeAutomationContext(
  context: AutomationContext,
): Promise<void> {
  await chrome.storage.local.set({ [AUTOMATION_KEY]: context });
}

export async function readCommandLedger(): Promise<
  readonly CommandLedgerEntry[]
> {
  const stored = await chrome.storage.local.get([LEDGER_KEY]);
  return parseCommandLedger(stored[LEDGER_KEY]);
}

export async function findCommandLedgerEntry(
  commandId: string,
): Promise<CommandLedgerEntry | null> {
  const entry = (await readCommandLedger()).find(
    (candidate) => candidate.commandId === commandId,
  );
  return entry ?? null;
}

export async function recordCommand(
  commandId: string,
  action: Action,
  state: CommandLedgerEntry["state"],
  now = Date.now(),
): Promise<void> {
  const entries = (await readCommandLedger()).filter(
    (entry) => entry.commandId !== commandId,
  );
  entries.push({ commandId, action, state, updatedAt: now });
  entries.sort((left, right) => left.updatedAt - right.updatedAt);
  await chrome.storage.local.set({
    [LEDGER_KEY]: entries.slice(-MAX_LEDGER_ENTRIES),
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasOnlyKeys(
  value: Record<string, unknown>,
  keys: readonly string[],
): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function isSingleLineString(
  value: unknown,
  maxLength: number,
): value is string {
  return (
    typeof value === "string" &&
    value.length <= maxLength &&
    !value.includes("\n") &&
    !value.includes("\r") &&
    ![...value].some((character) => {
      const code = character.charCodeAt(0);
      return code < 0x20 || code === 0x7f;
    })
  );
}

function isIdentifier(value: unknown): value is string {
  return isSingleLineString(value, 256) && /^[A-Za-z0-9._:-]+$/u.test(value);
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function isTimestamp(value: unknown): value is number {
  return isPositiveInteger(value);
}

function isConnectionState(value: unknown): value is ConnectionState {
  return (
    value === "not_configured" ||
    value === "disabled" ||
    value === "connecting" ||
    value === "connected" ||
    value === "error"
  );
}
