// The TypeScript mirror of `crates/pluk-browser/src/protocol.rs`.
//
// Both sides validate the same envelopes independently, so a drift between
// them is a real bug rather than a formality: the extension refuses a command
// the server would not have sent, and the server refuses a result the
// extension should not have produced. Some validators here have no caller in
// the extension — they mirror the server's half of the contract and exist so
// a change on one side is visibly missing on the other. The ignored
// `real_extension_transport_survives_slow_command_and_reconnect` test drives
// this file against the real server to catch that drift.

export const PROTOCOL_VERSION = 1 as const;

export const MAX_BODY_BYTES = 64 * 1024;
export const MAX_MESSAGE_BYTES = 64 * 1024;
export const MAX_SCREENSHOT_BYTES = 4 * 1024 * 1024;
export const MAX_EXTRACT_BYTES = 256 * 1024;
export const MAX_JOB_TTL_MS = 5 * 60 * 1000;
export const MIN_JOB_TTL_MS = 1_000;
export const DEFAULT_JOB_TTL_MS = 2 * 60 * 1000;
export const MAX_ENVELOPE_TTL_MS = MAX_JOB_TTL_MS;
export const MAX_TEXT_LENGTH = 4_000;
export const MAX_URL_LENGTH = 2_048;
export const MAX_ID_LENGTH = 256;
export const MAX_RESULT_TEXT_LENGTH = 8_000;
export const MAX_THREAD_PARTS = 25;
export const HEARTBEAT_INTERVAL_MS = 20_000;

export const PLATFORMS = ["x"] as const;
export type Platform = (typeof PLATFORMS)[number];

export const ACTIONS = [
  "inspect",
  "read_profile",
  "read_post",
  "read_feed",
  "read_trends",
  "refresh",
  "capture",
  "reply",
  "submit_reply",
  "post",
  "submit_post",
] as const;
export type Action = (typeof ACTIONS)[number];
export type PublicAction = Exclude<Action, "submit_reply" | "submit_post">;

export const DRIVER_CONTRACTS: Record<
  Platform,
  {
    readonly platform: Platform;
    readonly hostnames: readonly string[];
    readonly capabilities: readonly Action[];
  }
> = {
  x: {
    platform: "x",
    hostnames: ["x.com", "www.x.com", "twitter.com", "www.twitter.com"],
    capabilities: [
      "inspect",
      "read_profile",
      "read_post",
      "read_feed",
      "read_trends",
      "refresh",
      "capture",
      "submit_reply",
      "submit_post",
    ],
  },
};

// Actions with one fixed destination: the caller supplies no target URL and
// the site's default is used. An explicit targetUrl is still accepted and
// validated normally.
export const FIXED_FEED_TARGET = "https://x.com/home";
export const FIXED_TRENDS_TARGET = "https://x.com/explore";

// x.post has one fixed destination, and unlike read_feed/read_trends
// it never accepts a caller-supplied override: there is no other page a new
// post could be composed on.
export const FIXED_COMPOSE_TARGET = "https://x.com/compose/post";

const PROFILE_USERNAME_PATTERN = /^[A-Za-z0-9_]{1,50}$/u;

// Path segments that collide with X's own feature pages, so they can never be
// a real handle even though they match the username pattern. Shared with the
// site driver, which applies the same check against the live page's URL.
const RESERVED_PROFILE_HANDLES: ReadonlySet<string> = new Set([
  "home",
  "explore",
  "notifications",
  "messages",
  "settings",
  "search",
  "compose",
  "login",
  "i",
]);

export function isValidProfileUsername(value: string): boolean {
  return (
    PROFILE_USERNAME_PATTERN.test(value) &&
    !RESERVED_PROFILE_HANDLES.has(value.toLowerCase())
  );
}

export function normalizeUsername(value: string): string {
  return value.startsWith("@") ? value.slice(1) : value;
}

function canonicalProfileUrl(username: string): string {
  return `https://x.com/${username}`;
}

function extractProfileUsername(url: URL): string | null {
  const username = /^\/([A-Za-z0-9_]{1,50})\/?$/u.exec(url.pathname)?.[1] ?? null;
  return username !== null && isValidProfileUsername(username) ? username : null;
}

function isValidPostId(value: string): boolean {
  return isIdentifier(value) && /^\d{1,32}$/u.test(value);
}

function canonicalPostUrl(postId: string): string {
  return `https://x.com/status/${postId}`;
}

function extractPostId(url: URL): string | null {
  const postId = url.pathname.match(/\/status\/(\d+)/u)?.[1];
  return postId && isValidPostId(postId) ? postId : null;
}

export interface EmptyPayload {
  readonly kind: "empty";
}

export interface ReplyPayload {
  readonly kind: "reply";
  readonly postId: string;
  readonly text: string;
}

export interface ReadPostPayload {
  readonly kind: "read_post";
  readonly postId: string;
}

export interface SubmissionPayload {
  readonly kind: "submission";
  readonly draftId: string;
  readonly postId: string;
  readonly text: string;
}

export interface ComposePayload {
  readonly kind: "compose";
  readonly text: string;
  /** The posts of a thread, in order. One entry for a plain post. */
  readonly parts: readonly string[];
}

export interface PostSubmissionPayload {
  readonly kind: "post_submission";
  readonly draftId: string;
  readonly text: string;
  /** The posts to send, in order. More than one makes a thread. */
  readonly parts: readonly string[];
}

// `post` and `reply` requests never reach the extension as commands: Pluk
// holds them as drafts until the owner confirms, and only the submit
// actions are dispatched.
export type CommandPayload =
  | EmptyPayload
  | ReadPostPayload
  | SubmissionPayload
  | PostSubmissionPayload;

export interface CreateJobRequest {
  readonly platform: Platform;
  readonly action: PublicAction;
  readonly targetUrl: string;
  readonly payload:
    | EmptyPayload
    | ReplyPayload
    | ReadPostPayload
    | ComposePayload;
  readonly ttlMs: number;
}

export interface CommandEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "command";
  readonly jobId: string;
  readonly commandId: string;
  readonly platform: Platform;
  readonly action: Action;
  readonly targetUrl: string;
  readonly issuedAt: number;
  readonly expiresAt: number;
  readonly payload: CommandPayload;
}

export interface ProtocolError {
  readonly code: string;
  readonly message: string;
}

export interface ResultData {
  readonly kind: string;
  readonly [key: string]: unknown;
}

export interface ResultEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "result";
  readonly jobId: string;
  readonly commandId: string;
  readonly issuedAt: number;
  readonly expiresAt: number;
  readonly outcome: "succeeded" | "failed";
  readonly data?: ResultData;
  readonly error?: ProtocolError;
}

export interface ExtensionCapability {
  readonly platform: Platform;
  readonly capabilities: readonly Action[];
}

export interface ExtensionHelloEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "hello";
  readonly extensionVersion: string;
  readonly capabilities: readonly ExtensionCapability[];
  readonly issuedAt: number;
  readonly expiresAt: number;
}

export interface HeartbeatEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "heartbeat";
  readonly nonce: string;
  readonly issuedAt: number;
  readonly expiresAt: number;
}

export interface HeartbeatAckEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "heartbeat_ack";
  readonly nonce: string;
  readonly issuedAt: number;
  readonly expiresAt: number;
}

export type ExtensionMessage =
  | ExtensionHelloEnvelope
  | HeartbeatEnvelope
  | ResultEnvelope;

export type ServerMessage =
  | ReadyEnvelope
  | CommandEnvelope
  | HeartbeatEnvelope
  | HeartbeatAckEnvelope;

export interface ReadyEnvelope {
  readonly version: typeof PROTOCOL_VERSION;
  readonly type: "ready";
  readonly connectionId: string;
  readonly heartbeatIntervalMs: number;
  readonly contracts: readonly {
    readonly platform: Platform;
    readonly hostnames: readonly string[];
    readonly capabilities: readonly Action[];
  }[];
  readonly issuedAt: number;
  readonly expiresAt: number;
}

export interface ValidationFailure {
  readonly code: "invalid_schema" | "invalid_target" | "unsupported_action";
  readonly message: string;
}

export type ValidationResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: ValidationFailure };

const ACTION_SET = new Set<string>(ACTIONS);
const PLATFORM_SET = new Set<string>(PLATFORMS);
const ID_PATTERN = /^[A-Za-z0-9._:-]+$/;
const EXTENSION_ID_PATTERN = /^[a-p]{32}$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasOnlyKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = [],
): boolean {
  const allowed = new Set([...required, ...optional]);
  return (
    required.every((key) => key in value) &&
    Object.keys(value).every((key) => allowed.has(key))
  );
}

function isString(value: unknown, maxLength: number): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= maxLength &&
    !hasControlCharacter(value)
  );
}

function hasControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const code = character.charCodeAt(0);
    return (
      (code < 0x20 && code !== 0x09 && code !== 0x0a && code !== 0x0d) ||
      code === 0x7f
    );
  });
}

function isIdentifier(value: unknown): value is string {
  return isString(value, MAX_ID_LENGTH) && ID_PATTERN.test(value);
}

function isTimestamp(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function invalid(message: string): ValidationResult<never> {
  return { ok: false, error: { code: "invalid_schema", message } };
}

export function isSupportedPlatform(value: unknown): value is Platform {
  return typeof value === "string" && PLATFORM_SET.has(value);
}

export function isAction(value: unknown): value is Action {
  return typeof value === "string" && ACTION_SET.has(value);
}

export function isPublicAction(value: unknown): value is PublicAction {
  return isAction(value) && value !== "submit_reply" && value !== "submit_post";
}

export function canonicalizeTargetUrl(
  value: unknown,
  platform: Platform,
): ValidationResult<string> {
  if (!isString(value, MAX_URL_LENGTH)) {
    return {
      ok: false,
      error: {
        code: "invalid_target",
        message: "Target URL is missing or too long.",
      },
    };
  }

  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return {
      ok: false,
      error: {
        code: "invalid_target",
        message: "Target URL must be valid HTTPS.",
      },
    };
  }

  const contract = DRIVER_CONTRACTS[platform];
  if (
    url.protocol !== "https:" ||
    url.username !== "" ||
    url.password !== "" ||
    (url.port !== "" && url.port !== "443") ||
    !contract.hostnames.includes(url.hostname.toLowerCase()) ||
    url.hash !== ""
  ) {
    return {
      ok: false,
      error: {
        code: "invalid_target",
        message: "Target URL must use an exact supported HTTPS host.",
      },
    };
  }

  url.hostname = url.hostname.toLowerCase();
  if (url.port === "443") {
    url.port = "";
  }
  return { ok: true, value: url.toString() };
}

function parsePayload(
  value: unknown,
  action: PublicAction,
): ValidationResult<EmptyPayload | ReplyPayload | ComposePayload> {
  if (!isRecord(value)) {
    return invalid("Payload must be an object.");
  }

  if (action === "post") {
    if (hasOnlyKeys(value, ["thread"]) && isThread(value.thread)) {
      const parts = value.thread;
      return {
        ok: true,
        value: { kind: "compose", text: parts.join("\n\n"), parts },
      };
    }
    if (
      !hasOnlyKeys(value, ["text"]) ||
      !isString(value.text, MAX_TEXT_LENGTH)
    ) {
      return invalid("Post payload needs exact text, or a thread of posts.");
    }
    return {
      ok: true,
      value: { kind: "compose", text: value.text, parts: [value.text] },
    };
  }

  if (action === "reply") {
    if (
      !hasOnlyKeys(value, ["postId", "text"]) ||
      !isIdentifier(value.postId) ||
      !isString(value.text, MAX_TEXT_LENGTH)
    ) {
      return invalid("Reply payload needs a post ID and exact text.");
    }
    return {
      ok: true,
      value: { kind: "reply", postId: value.postId, text: value.text },
    };
  }

  if (Object.keys(value).length !== 0) {
    return invalid("This action does not accept a payload.");
  }
  return { ok: true, value: { kind: "empty" } };
}

function resolveFixedDestinationTarget(
  platform: Platform,
  action: "read_feed" | "read_trends",
  targetUrlValue: unknown,
): ValidationResult<string> {
  const fallback =
    action === "read_trends" ? FIXED_TRENDS_TARGET : FIXED_FEED_TARGET;
  return canonicalizeTargetUrl(targetUrlValue ?? fallback, platform);
}

function resolveComposeTarget(
  platform: Platform,
  targetUrlValue: unknown,
): ValidationResult<string> {
  if (targetUrlValue !== undefined) {
    return invalid("This action does not accept a target URL.");
  }
  return canonicalizeTargetUrl(FIXED_COMPOSE_TARGET, platform);
}

function resolveProfileTarget(
  platform: Platform,
  targetUrlValue: unknown,
  payloadValue: unknown,
): ValidationResult<{
  readonly targetUrl: string;
  readonly payload: EmptyPayload;
}> {
  const payload = isRecord(payloadValue) ? payloadValue : {};
  if ("username" in payload) {
    if (
      !hasOnlyKeys(payload, ["username"]) ||
      typeof payload.username !== "string"
    ) {
      return invalid("Profile payload needs a single username field.");
    }
    const username = normalizeUsername(payload.username);
    if (!isValidProfileUsername(username)) {
      return invalid("Enter a valid username for this site.");
    }
    return {
      ok: true,
      value: {
        targetUrl: canonicalProfileUrl(username),
        payload: { kind: "empty" },
      },
    };
  }
  if (Object.keys(payload).length !== 0) {
    return invalid("Profile payload needs a single username field.");
  }
  // Legacy compatibility: a full profile URL in place of a username.
  const targetUrl = canonicalizeTargetUrl(targetUrlValue, platform);
  if (!targetUrl.ok) {
    return targetUrl;
  }
  if (extractProfileUsername(new URL(targetUrl.value)) === null) {
    return invalid(
      "Target URL is not a recognized profile page for this site. Pass a username instead.",
    );
  }
  return {
    ok: true,
    value: { targetUrl: targetUrl.value, payload: { kind: "empty" } },
  };
}

function resolvePostTarget(
  platform: Platform,
  targetUrlValue: unknown,
  payloadValue: unknown,
): ValidationResult<{
  readonly targetUrl: string;
  readonly payload: ReadPostPayload;
}> {
  if (!isRecord(payloadValue)) {
    return invalid("Payload must be an object.");
  }
  if (
    !hasOnlyKeys(payloadValue, [], ["postId"]) ||
    (payloadValue.postId !== undefined &&
      !isString(payloadValue.postId, MAX_ID_LENGTH))
  ) {
    return invalid("Read-post payload accepts only an optional postId field.");
  }
  const payloadPostId =
    typeof payloadValue.postId === "string" ? payloadValue.postId : undefined;
  if (payloadPostId !== undefined && !isValidPostId(payloadPostId)) {
    return invalid("Enter a valid post ID for this site.");
  }
  if (targetUrlValue === undefined) {
    if (payloadPostId === undefined) {
      return invalid("Provide a post URL or a post ID.");
    }
    const validated = canonicalizeTargetUrl(
      canonicalPostUrl(payloadPostId),
      platform,
    );
    if (!validated.ok) {
      return validated;
    }
    return {
      ok: true,
      value: {
        targetUrl: validated.value,
        payload: { kind: "read_post", postId: payloadPostId },
      },
    };
  }
  const targetUrl = canonicalizeTargetUrl(targetUrlValue, platform);
  if (!targetUrl.ok) {
    return targetUrl;
  }
  const urlPostId = extractPostId(new URL(targetUrl.value));
  if (urlPostId === null) {
    return invalid(
      "Target URL is not a recognized post page for this site. Pass a post ID instead.",
    );
  }
  if (payloadPostId !== undefined && payloadPostId !== urlPostId) {
    return invalid("The post ID does not match the post visible at targetUrl.");
  }
  return {
    ok: true,
    value: {
      targetUrl: targetUrl.value,
      payload: { kind: "read_post", postId: urlPostId },
    },
  };
}

export function parseCreateJobRequest(
  value: unknown,
): ValidationResult<CreateJobRequest> {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(
      value,
      ["platform", "action", "payload"],
      ["targetUrl", "ttlMs"],
    )
  ) {
    return invalid("Job request has an unsupported shape.");
  }
  if (!isSupportedPlatform(value.platform) || !isPublicAction(value.action)) {
    return invalid("Job request has an unsupported platform or action.");
  }

  const contract = DRIVER_CONTRACTS[value.platform];
  if (!contract.capabilities.includes(value.action)) {
    return {
      ok: false,
      error: {
        code: "unsupported_action",
        message: "This driver does not support that action.",
      },
    };
  }

  let targetUrl: string;
  let payload: EmptyPayload | ReplyPayload | ReadPostPayload | ComposePayload;
  if (value.action === "read_profile") {
    const resolved = resolveProfileTarget(
      value.platform,
      value.targetUrl,
      value.payload,
    );
    if (!resolved.ok) {
      return resolved;
    }
    targetUrl = resolved.value.targetUrl;
    payload = resolved.value.payload;
  } else if (value.action === "post") {
    const resolvedTarget = resolveComposeTarget(
      value.platform,
      value.targetUrl,
    );
    if (!resolvedTarget.ok) {
      return resolvedTarget;
    }
    const parsedPayload = parsePayload(value.payload, value.action);
    if (!parsedPayload.ok) {
      return parsedPayload;
    }
    targetUrl = resolvedTarget.value;
    payload = parsedPayload.value;
  } else if (value.action === "read_post") {
    const resolved = resolvePostTarget(
      value.platform,
      value.targetUrl,
      value.payload,
    );
    if (!resolved.ok) {
      return resolved;
    }
    targetUrl = resolved.value.targetUrl;
    payload = resolved.value.payload;
  } else {
    const resolvedTarget =
      value.action === "read_feed" || value.action === "read_trends"
        ? resolveFixedDestinationTarget(
            value.platform,
            value.action,
            value.targetUrl,
          )
        : canonicalizeTargetUrl(value.targetUrl, value.platform);
    if (!resolvedTarget.ok) {
      return resolvedTarget;
    }
    const parsedPayload = parsePayload(value.payload, value.action);
    if (!parsedPayload.ok) {
      return parsedPayload;
    }
    targetUrl = resolvedTarget.value;
    payload = parsedPayload.value;
  }

  const ttlMs = value.ttlMs === undefined ? DEFAULT_JOB_TTL_MS : value.ttlMs;
  if (
    typeof ttlMs !== "number" ||
    !Number.isSafeInteger(ttlMs) ||
    ttlMs < MIN_JOB_TTL_MS ||
    ttlMs > MAX_JOB_TTL_MS
  ) {
    return invalid("Job expiry must be between one second and five minutes.");
  }

  return {
    ok: true,
    value: {
      platform: value.platform,
      action: value.action,
      targetUrl,
      payload,
      ttlMs,
    },
  };
}

export function parseCommandEnvelope(
  value: unknown,
): ValidationResult<CommandEnvelope> {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(value, [
      "version",
      "type",
      "jobId",
      "commandId",
      "platform",
      "action",
      "targetUrl",
      "issuedAt",
      "expiresAt",
      "payload",
    ]) ||
    value.version !== PROTOCOL_VERSION ||
    value.type !== "command" ||
    !isIdentifier(value.jobId) ||
    !isIdentifier(value.commandId) ||
    !isSupportedPlatform(value.platform) ||
    !isAction(value.action)
  ) {
    return invalid("Command envelope has an unsupported shape.");
  }
  const contract = DRIVER_CONTRACTS[value.platform];
  if (!contract.capabilities.includes(value.action)) {
    return {
      ok: false,
      error: {
        code: "unsupported_action",
        message: "Command action is not supported by this driver.",
      },
    };
  }
  const targetUrl = canonicalizeTargetUrl(value.targetUrl, value.platform);
  if (!targetUrl.ok) {
    return targetUrl;
  }
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Command envelope has an unsupported expiry.");
  }
  const payload = parseCommandPayload(value.payload, value.action);
  if (!payload.ok) {
    return payload;
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "command",
      jobId: value.jobId,
      commandId: value.commandId,
      platform: value.platform,
      action: value.action,
      targetUrl: targetUrl.value,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
      payload: payload.value,
    },
  };
}

function parseCommandPayload(
  value: unknown,
  action: Action,
): ValidationResult<CommandPayload> {
  if (!isRecord(value)) {
    return invalid("Command payload must be an object.");
  }
  if (action === "post" || action === "reply") {
    return invalid("Posts and replies are dispatched only as submissions.");
  }
  if (action === "submit_post") {
    if (
      !hasOnlyKeys(value, ["kind", "draftId", "text", "parts"]) ||
      value.kind !== "post_submission" ||
      !isIdentifier(value.draftId) ||
      !isString(value.text, MAX_TEXT_LENGTH) ||
      !isThread(value.parts)
    ) {
      return invalid("Post submission command payload is invalid.");
    }
    return {
      ok: true,
      value: {
        kind: "post_submission",
        draftId: value.draftId,
        text: value.text,
        parts: value.parts,
      },
    };
  }
  if (action === "read_post") {
    if (
      !hasOnlyKeys(value, ["kind", "postId"]) ||
      value.kind !== "read_post" ||
      !isIdentifier(value.postId)
    ) {
      return invalid("Read-post command payload is invalid.");
    }
    return {
      ok: true,
      value: { kind: "read_post", postId: value.postId },
    };
  }
  if (action === "submit_reply") {
    if (
      !hasOnlyKeys(value, ["kind", "draftId", "postId", "text"]) ||
      value.kind !== "submission" ||
      !isIdentifier(value.draftId) ||
      !isIdentifier(value.postId) ||
      !isString(value.text, MAX_TEXT_LENGTH)
    ) {
      return invalid("Submission command payload is invalid.");
    }
    return {
      ok: true,
      value: {
        kind: "submission",
        draftId: value.draftId,
        postId: value.postId,
        text: value.text,
      },
    };
  }
  if (!hasOnlyKeys(value, ["kind"]) || value.kind !== "empty") {
    return invalid("This command does not accept a payload.");
  }
  return { ok: true, value: { kind: "empty" } };
}

/** The posts of a thread on the wire: one to 25 bounded strings. */
function isThread(value: unknown): value is readonly string[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.length <= MAX_THREAD_PARTS &&
    value.every((part) => isString(part, MAX_TEXT_LENGTH))
  );
}

function isBoundedJson(value: unknown, depth = 0): boolean {
  if (depth > 6) {
    return false;
  }
  if (
    value === null ||
    typeof value === "boolean" ||
    typeof value === "number"
  ) {
    return typeof value !== "number" || Number.isFinite(value);
  }
  if (typeof value === "string") {
    return value.length <= MAX_EXTRACT_BYTES;
  }
  if (Array.isArray(value)) {
    return (
      value.length <= 100 &&
      value.every((item) => isBoundedJson(item, depth + 1))
    );
  }
  if (!isRecord(value) || Object.keys(value).length > 64) {
    return false;
  }
  return Object.entries(value).every(
    ([key, item]) => key.length <= 128 && isBoundedJson(item, depth + 1),
  );
}

function parseProtocolError(value: unknown): ValidationResult<ProtocolError> {
  if (
    !isRecord(value) ||
    !hasOnlyKeys(value, ["code", "message"]) ||
    !isString(value.code, 64) ||
    !isString(value.message, 512)
  ) {
    return invalid("Result error has an unsupported shape.");
  }
  return { ok: true, value: { code: value.code, message: value.message } };
}

function parseResultData(value: unknown): ValidationResult<ResultData> {
  if (!isRecord(value) || !isString(value.kind, 64) || !isBoundedJson(value)) {
    return invalid("Result data is missing or exceeds its bounds.");
  }
  if (value.kind === "scheduled_submission" || "scheduledAt" in value) {
    return invalid("Result data contains an unsupported scheduling field.");
  }
  return { ok: true, value: { kind: value.kind, ...value } };
}

function parseCapabilities(
  value: unknown,
): ValidationResult<readonly ExtensionCapability[]> {
  if (!Array.isArray(value) || value.length > PLATFORMS.length) {
    return invalid("Extension capabilities must be a bounded list.");
  }
  const seen = new Set<Platform>();
  const capabilities: ExtensionCapability[] = [];
  for (const item of value) {
    if (
      !isRecord(item) ||
      !hasOnlyKeys(item, ["platform", "capabilities"]) ||
      !isSupportedPlatform(item.platform) ||
      seen.has(item.platform)
    ) {
      return invalid("Extension capabilities contain an unsupported platform.");
    }
    if (
      !Array.isArray(item.capabilities) ||
      item.capabilities.length > ACTIONS.length ||
      !item.capabilities.every(isAction)
    ) {
      return invalid("Extension capabilities contain an unsupported action.");
    }
    const unique = [...new Set(item.capabilities)];
    const contract = DRIVER_CONTRACTS[item.platform];
    if (unique.some((action) => !contract.capabilities.includes(action))) {
      return invalid("Extension capabilities exceed the driver contract.");
    }
    seen.add(item.platform);
    capabilities.push({ platform: item.platform, capabilities: unique });
  }
  return { ok: true, value: capabilities };
}

export function parseExtensionMessage(
  value: unknown,
): ValidationResult<ExtensionMessage> {
  if (
    !isRecord(value) ||
    value.version !== PROTOCOL_VERSION ||
    !isString(value.type, 32)
  ) {
    return invalid("Message version or type is invalid.");
  }

  if (value.type === "hello") {
    return parseHelloMessage(value);
  }
  if (value.type === "heartbeat") {
    return parseHeartbeatMessage(value);
  }
  if (value.type !== "result") {
    return invalid("Message type is not supported.");
  }
  return parseResultMessage(value);
}

export function parseServerMessage(
  value: unknown,
): ValidationResult<ServerMessage> {
  if (
    !isRecord(value) ||
    value.version !== PROTOCOL_VERSION ||
    !isString(value.type, 32)
  ) {
    return invalid("Message version or type is invalid.");
  }

  if (value.type === "ready") {
    return parseReadyMessage(value);
  }
  if (value.type === "command") {
    return parseCommandEnvelope(value);
  }
  if (value.type === "heartbeat") {
    return parseHeartbeatMessage(value);
  }
  if (value.type === "heartbeat_ack") {
    return parseHeartbeatAckMessage(value);
  }
  return invalid("Message type is not supported.");
}

function parseHelloMessage(
  value: Record<string, unknown>,
): ValidationResult<ExtensionHelloEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "extensionVersion",
      "capabilities",
      "issuedAt",
      "expiresAt",
    ]) ||
    !isString(value.extensionVersion, 64)
  ) {
    return invalid("Hello message has an unsupported shape.");
  }
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Hello message has an unsupported shape.");
  }
  const capabilities = parseCapabilities(value.capabilities);
  if (!capabilities.ok) {
    return capabilities;
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "hello",
      extensionVersion: value.extensionVersion,
      capabilities: capabilities.value,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
    },
  };
}

function parseHeartbeatMessage(
  value: Record<string, unknown>,
): ValidationResult<HeartbeatEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "nonce",
      "issuedAt",
      "expiresAt",
    ]) ||
    !isIdentifier(value.nonce)
  ) {
    return invalid("Heartbeat message has an unsupported shape.");
  }
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Heartbeat message has an unsupported shape.");
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "heartbeat",
      nonce: value.nonce,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
    },
  };
}

function parseHeartbeatAckMessage(
  value: Record<string, unknown>,
): ValidationResult<HeartbeatAckEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "nonce",
      "issuedAt",
      "expiresAt",
    ]) ||
    value.type !== "heartbeat_ack" ||
    !isIdentifier(value.nonce)
  ) {
    return invalid("Heartbeat acknowledgement has an unsupported shape.");
  }
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Heartbeat acknowledgement has an unsupported shape.");
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "heartbeat_ack",
      nonce: value.nonce,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
    },
  };
}

function parseReadyMessage(
  value: Record<string, unknown>,
): ValidationResult<ReadyEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "connectionId",
      "heartbeatIntervalMs",
      "contracts",
      "issuedAt",
      "expiresAt",
    ]) ||
    value.type !== "ready" ||
    !isIdentifier(value.connectionId) ||
    value.heartbeatIntervalMs !== HEARTBEAT_INTERVAL_MS
  ) {
    return invalid("Ready message has an unsupported shape.");
  }
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Ready message has an unsupported expiry.");
  }
  const contracts = parseReadyContracts(value.contracts);
  if (!contracts.ok) {
    return contracts;
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "ready",
      connectionId: value.connectionId,
      heartbeatIntervalMs: HEARTBEAT_INTERVAL_MS,
      contracts: contracts.value,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
    },
  };
}

function parseReadyContracts(
  value: unknown,
): ValidationResult<ReadyEnvelope["contracts"]> {
  if (!Array.isArray(value) || value.length > PLATFORMS.length) {
    return invalid("Ready contracts must be a bounded list.");
  }
  const seen = new Set<Platform>();
  const contracts: Array<ReadyEnvelope["contracts"][number]> = [];
  for (const item of value) {
    if (
      !isRecord(item) ||
      !hasOnlyKeys(item, ["platform", "hostnames", "capabilities"]) ||
      !isSupportedPlatform(item.platform) ||
      seen.has(item.platform) ||
      !Array.isArray(item.hostnames) ||
      item.hostnames.length > 8 ||
      !Array.isArray(item.capabilities) ||
      item.capabilities.length > ACTIONS.length
    ) {
      return invalid("Ready contracts contain an unsupported platform.");
    }

    const hostnames: string[] = [];
    for (const hostname of item.hostnames) {
      if (!isString(hostname, 253)) {
        return invalid("Ready contracts contain an invalid hostname.");
      }
      hostnames.push(hostname);
    }

    const capabilities: Action[] = [];
    for (const action of item.capabilities) {
      if (!isAction(action)) {
        return invalid("Ready contracts contain an unsupported action.");
      }
      capabilities.push(action);
    }
    const uniqueCapabilities = [...new Set(capabilities)];
    const contract = DRIVER_CONTRACTS[item.platform];
    if (
      uniqueCapabilities.some(
        (action) => !contract.capabilities.includes(action),
      )
    ) {
      return invalid("Ready contracts exceed the driver contract.");
    }
    seen.add(item.platform);
    contracts.push({
      platform: item.platform,
      hostnames,
      capabilities: uniqueCapabilities,
    });
  }
  return { ok: true, value: contracts };
}

function parseResultMessage(
  value: Record<string, unknown>,
): ValidationResult<ResultEnvelope> {
  if (!isIdentifier(value.jobId) || !isIdentifier(value.commandId)) {
    return invalid("Result message has an unsupported shape.");
  }
  const identifiers = { jobId: value.jobId, commandId: value.commandId };
  const times = parseEnvelopeTimes(value);
  if (!times) {
    return invalid("Result message has an unsupported shape.");
  }
  if (value.outcome === "succeeded") {
    return parseSuccessfulResult(value, times, identifiers);
  }
  if (value.outcome === "failed") {
    return parseFailedResult(value, times, identifiers);
  }
  return invalid("Result message has an unsupported shape.");
}

function parseSuccessfulResult(
  value: Record<string, unknown>,
  times: { readonly issuedAt: number; readonly expiresAt: number },
  identifiers: { readonly jobId: string; readonly commandId: string },
): ValidationResult<ResultEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "jobId",
      "commandId",
      "issuedAt",
      "expiresAt",
      "outcome",
      "data",
    ])
  ) {
    return invalid("Successful result needs bounded data.");
  }
  const data = parseResultData(value.data);
  if (!data.ok) {
    return data;
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "result",
      jobId: identifiers.jobId,
      commandId: identifiers.commandId,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
      outcome: "succeeded",
      data: data.value,
    },
  };
}

function parseFailedResult(
  value: Record<string, unknown>,
  times: { readonly issuedAt: number; readonly expiresAt: number },
  identifiers: { readonly jobId: string; readonly commandId: string },
): ValidationResult<ResultEnvelope> {
  if (
    !hasOnlyKeys(value, [
      "version",
      "type",
      "jobId",
      "commandId",
      "issuedAt",
      "expiresAt",
      "outcome",
      "error",
    ])
  ) {
    return invalid("Failed result has an unsupported shape.");
  }
  const error = parseProtocolError(value.error);
  if (!error.ok) {
    return error;
  }
  return {
    ok: true,
    value: {
      version: PROTOCOL_VERSION,
      type: "result",
      jobId: identifiers.jobId,
      commandId: identifiers.commandId,
      issuedAt: times.issuedAt,
      expiresAt: times.expiresAt,
      outcome: "failed",
      error: error.value,
    },
  };
}

function parseEnvelopeTimes(
  value: Record<string, unknown>,
): { readonly issuedAt: number; readonly expiresAt: number } | null {
  if (!isTimestamp(value.issuedAt) || !isTimestamp(value.expiresAt)) {
    return null;
  }
  if (value.expiresAt <= value.issuedAt) {
    return null;
  }
  if (value.expiresAt - value.issuedAt > MAX_ENVELOPE_TTL_MS) {
    return null;
  }
  return { issuedAt: value.issuedAt, expiresAt: value.expiresAt };
}

export function isAllowedExtensionOrigin(origin: string): boolean {
  if (origin.startsWith("chrome-extension://")) {
    const extensionId = origin.slice("chrome-extension://".length);
    return EXTENSION_ID_PATTERN.test(extensionId);
  }
  return false;
}

export function makeReadyEnvelope(
  connectionId: string,
  now: number,
): ReadyEnvelope {
  return {
    version: PROTOCOL_VERSION,
    type: "ready",
    connectionId,
    heartbeatIntervalMs: HEARTBEAT_INTERVAL_MS,
    contracts: PLATFORMS.map((platform) => ({
      platform,
      hostnames: DRIVER_CONTRACTS[platform].hostnames,
      capabilities: DRIVER_CONTRACTS[platform].capabilities,
    })),
    issuedAt: now,
    expiresAt: now + HEARTBEAT_INTERVAL_MS,
  };
}

export function makeHeartbeatEnvelope(now: number): HeartbeatEnvelope {
  return {
    version: PROTOCOL_VERSION,
    type: "heartbeat",
    nonce: crypto.randomUUID(),
    issuedAt: now,
    expiresAt: now + HEARTBEAT_INTERVAL_MS,
  };
}

export function makeHeartbeatAckEnvelope(
  nonce: string,
  now: number,
): HeartbeatAckEnvelope {
  return {
    version: PROTOCOL_VERSION,
    type: "heartbeat_ack",
    nonce,
    issuedAt: now,
    expiresAt: now + HEARTBEAT_INTERVAL_MS,
  };
}

export function makeCommandEnvelope(job: {
  id: string;
  commandId: string;
  platform: Platform;
  action: Action;
  targetUrl: string;
  payload: CommandPayload;
  createdAt: number;
  expiresAt: number;
}): CommandEnvelope {
  return {
    version: PROTOCOL_VERSION,
    type: "command",
    jobId: job.id,
    commandId: job.commandId,
    platform: job.platform,
    action: job.action,
    targetUrl: job.targetUrl,
    issuedAt: job.createdAt,
    expiresAt: job.expiresAt,
    payload: job.payload,
  };
}
