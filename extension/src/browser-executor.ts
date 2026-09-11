import {
  type Action,
  type CommandEnvelope,
  canonicalizeTargetUrl,
  MAX_EXTRACT_BYTES,
  MAX_SCREENSHOT_BYTES,
  type Platform,
  type ResultData,
} from "./protocol";
import { getSiteDriver } from "./drivers";
import type {
  DriverPageResult,
  DriverScriptOptions,
  SiteDriver,
} from "./drivers/types";
import {
  type AutomationContext,
  readAutomationContext,
  writeAutomationContext,
} from "./state";

const NAVIGATION_TIMEOUT_MS = 20_000;
const CAPTURE_INTERVAL_MS = 500;
const MAX_RESULT_TEXT_LENGTH = 8_000;

// Only the explicit "capture" tool touches captureVisibleTab. Every other
// action, the submissions included, returns DOM text only, so a missing or
// failed screenshot grant can never block reading or posting.
const SCREENSHOT_ACTIONS = new Set<Action>(["capture"]);

// Both submit actions type the confirmed text into X's editor and publish it
// on a single click. Any uncertainty past that click (a Chrome failure, an
// unparsable result) must surface as "unknown", never as a clean failure
// that invites a retry. The window is never brought forward for it: posting
// happens behind whatever the owner is doing.
const SUBMIT_ACTIONS = new Set<Action>(["submit_reply", "submit_post"]);
const MAX_DEBUG_HTML_BYTES = 2 * 1024 * 1024;
const MAX_TRUSTED_CLICKS = 25;

function trustedClickRequest(
  result: DriverPageResult,
): { readonly x: number; readonly y: number } | null {
  const click = result.trustedClick;
  if (
    isRecord(click) &&
    typeof click.x === "number" &&
    typeof click.y === "number" &&
    Number.isFinite(click.x) &&
    Number.isFinite(click.y)
  ) {
    return { x: click.x, y: click.y };
  }
  return null;
}

function wantsDebug(payload: CommandEnvelope["payload"]): boolean {
  return (
    (payload.kind === "submission" || payload.kind === "post_submission") &&
    payload.debug === true
  );
}

interface TabState {
  readonly tabId: number;
  readonly windowId: number;
  readonly windowType: chrome.windows.WindowType | undefined;
  readonly windowState: chrome.windows.WindowState | undefined;
  readonly windowFocused: boolean;
  readonly active: boolean;
  readonly status: chrome.tabs.TabStatus | undefined;
  readonly url: string | undefined;
  readonly pendingUrl: string | undefined;
}

interface ArtifactSink {
  upload(
    jobId: string,
    kind: "screenshot" | "extract",
    contentType: string,
    body: Uint8Array,
  ): Promise<string>;
}

export type BrowserErrorCode =
  | "artifact_upload_failed"
  | "account_unverified"
  | "browser_unavailable"
  | "capture_discarded"
  | "document_not_ready"
  | "invalid_target"
  | "login_required"
  | "navigation_in_progress"
  | "navigation_timeout"
  | "page_changed"
  | "permission_denied"
  | "screenshot_too_large"
  | "site_markup_changed"
  | "submission_unknown"
  | "target_mismatch"
  | "target_not_found"
  | "unsupported_action";

export class BrowserExecutionError extends Error {
  readonly code: BrowserErrorCode;

  constructor(code: BrowserErrorCode, message: string) {
    super(message);
    this.name = "BrowserExecutionError";
    this.code = code;
  }
}

export class BrowserExecutor {
  private chain = Promise.resolve();
  private lastCaptureAt = 0;

  run(command: CommandEnvelope, sink: ArtifactSink): Promise<ResultData> {
    const next = this.chain.then(() => this.execute(command, sink));
    this.chain = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }

  private async execute(
    command: CommandEnvelope,
    sink: ArtifactSink,
  ): Promise<ResultData> {
    const driver = getSiteDriver(command.platform);
    if (!driver.capabilities.includes(command.action)) {
      throw new BrowserExecutionError(
        "unsupported_action",
        `The ${command.platform} driver does not support ${command.action}.`,
      );
    }
    const targetUrl = canonicalizeTargetUrl(
      driver.navigationTarget(command),
      command.platform,
    );
    if (!targetUrl.ok) {
      throw new BrowserExecutionError(
        "invalid_target",
        targetUrl.error.message,
      );
    }
    await requireSitePermission(targetUrl.value);

    const context = await this.ensureAutomationContext(targetUrl.value);
    const ready = await this.preparePage(
      context,
      targetUrl.value,
      command.action,
      command.platform,
    );
    if (ready.url === undefined) {
      throw new BrowserExecutionError(
        "browser_unavailable",
        "Chrome returned no final page URL.",
      );
    }
    if (ready.url !== targetUrl.value) {
      throw new BrowserExecutionError(
        "page_changed",
        "The page changed while it was being prepared. Try again.",
      );
    }
    const readPage = async () => {
      try {
        return await this.readDriverPage(
          context,
          driver,
          makeDriverScriptOptions(command, targetUrl.value),
          targetUrl.value,
          command.platform,
        );
      } catch (error) {
        if (wantsDebug(command.payload) && error instanceof BrowserExecutionError) {
          const notes = await this.attachDebug(context, command.jobId, sink);
          throw new BrowserExecutionError(
            error.code,
            `${error.message} Debug: ${notes.join("; ")}`.slice(0, 512),
          );
        }
        throw error;
      }
    };
    const page = SUBMIT_ACTIONS.has(command.action)
      ? await this.withWindowInFront(context, readPage)
      : await readPage();
    let extractArtifactId: string;
    let screenshotArtifactId: string | undefined;
    try {
      const extract = serializeExtract(page.data);
      extractArtifactId = await sink.upload(
        command.jobId,
        "extract",
        "application/json",
        extract,
      );
      if (SCREENSHOT_ACTIONS.has(command.action)) {
        const screenshot = await this.captureScreenshot(
          context,
          page.url,
          command.platform,
        );
        screenshotArtifactId = await sink.upload(
          command.jobId,
          "screenshot",
          "image/png",
          screenshot,
        );
      }
    } catch (error) {
      if (SUBMIT_ACTIONS.has(command.action)) {
        throw uncertainSubmissionError();
      }
      throw error;
    }

    return {
      ...page.data,
      url: page.url,
      title: page.title,
      extractArtifactId,
      ...(screenshotArtifactId === undefined ? {} : { screenshotArtifactId }),
    };
  }

  /** A real click, delivered through the browser rather than the page, for
   * controls that ignore or mistrust scripted events. The debugger stays
   * attached only for the press. */
  private async trustedClick(
    tabId: number,
    point: { readonly x: number; readonly y: number },
  ): Promise<void> {
    const target = { tabId };
    // `debugger` is a reserved word, so the namespace cannot be declared
    // alongside the others in chrome.d.ts and is typed through this lookup.
    const api = (chrome as unknown as { debugger: chrome.DebuggerApi }).debugger;
    try {
      await api.attach(target, "1.3");
    } catch (error) {
      throw mapChromeFailure(
        error,
        "Chrome would not let Pluk press the control on the page.",
      );
    }
    try {
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
        await api.sendCommand(target, "Input.dispatchMouseEvent", {
          type,
          x: point.x,
          y: point.y,
          button: "left",
          clickCount: 1,
        });
      }
    } catch (error) {
      throw mapChromeFailure(
        error,
        "Chrome did not deliver the press to the page.",
      );
    } finally {
      await api.detach(target).catch(() => undefined);
    }
  }

  /** What the page looked like when it refused: a screenshot and its HTML,
   * attached to the job for the owner to read. Best effort, and never in
   * the way of the failure itself. */
  private async attachDebug(
    context: AutomationContext,
    jobId: string,
    sink: ArtifactSink,
  ): Promise<string[]> {
    const reason = (error: unknown) =>
      error instanceof Error ? error.message : String(error);
    const screenshot = chrome.tabs
      .captureVisibleTab(context.windowId, { format: "png" })
      .then(decodePngDataUrl)
      .then((png) => sink.upload(jobId, "screenshot", "image/png", png))
      .then(() => "screenshot attached")
      .catch((error) => `screenshot failed (${reason(error)})`);
    const readPage = () =>
      chrome.scripting.executeScript({
        target: { tabId: context.tabId },
        func: () => ({
          href: window.location.href,
          html: document.documentElement.outerHTML,
          trace: sessionStorage.getItem("wande:trace") ?? "",
        }),
        args: [],
      });
    const html = readPage()
      .catch(() => delay(1_500).then(readPage))
      .then(async (results) => {
        const page = results[0]?.result;
        if (!page || typeof page.html !== "string") {
          throw new Error("page returned no markup");
        }
        const encoder = new TextEncoder();
        await sink.upload(
          jobId,
          "extract",
          "text/plain",
          encoder.encode(`${page.href}\n${page.trace}`),
        );
        return sink.upload(
          jobId,
          "extract",
          "text/html",
          encoder.encode(page.html).slice(0, MAX_DEBUG_HTML_BYTES),
        );
      })
      .then(() => "html attached")
      .catch((error) => `html failed (${reason(error)})`);
    return Promise.all([screenshot, html]);
  }

  /** X drives its composer from animation frames, which Chrome pauses while
   * the tab is covered. A submit gets the window in front for exactly as long
   * as it takes, then hands the front back. */
  private async withWindowInFront<T>(
    context: AutomationContext,
    work: () => Promise<T>,
  ): Promise<T> {
    try {
      await chrome.windows.update(context.windowId, { focused: true });
    } catch (error) {
      throw mapChromeFailure(
        error,
        "Chrome could not bring the dedicated window forward.",
      );
    }
    try {
      return await work();
    } finally {
      await chrome.windows
        .update(context.windowId, { focused: false })
        .catch(() => undefined);
    }
  }

  private async ensureAutomationContext(
    targetUrl: string,
  ): Promise<AutomationContext> {
    const stored = await readAutomationContext();
    if (stored && (await this.isUsableContext(stored))) {
      return stored;
    }

    const created = await this.createAutomationContext(targetUrl);
    await writeAutomationContext(created);
    return created;
  }

  private async isUsableContext(context: AutomationContext): Promise<boolean> {
    let window: chrome.windows.Window;
    try {
      window = await chrome.windows.get(context.windowId, { populate: true });
    } catch {
      return false;
    }
    const tabs = window.tabs ?? [];
    return (
      window.id === context.windowId &&
      window.type === "normal" &&
      !window.incognito &&
      tabs.length === 1 &&
      tabs[0]?.id === context.tabId
    );
  }

  private async createAutomationContext(
    targetUrl: string,
  ): Promise<AutomationContext> {
    let created: chrome.windows.Window | undefined;
    try {
      created = await chrome.windows.create({
        focused: false,
        height: 900,
        type: "normal",
        url: targetUrl,
        width: 1_200,
      });
    } catch (error) {
      throw mapChromeFailure(
        error,
        "The dedicated Chrome window could not be opened.",
      );
    }
    if (created?.id === undefined) {
      throw new BrowserExecutionError(
        "browser_unavailable",
        "Chrome did not return the dedicated automation window.",
      );
    }

    let window: chrome.windows.Window;
    try {
      window = await chrome.windows.get(created.id, { populate: true });
    } catch (error) {
      throw mapChromeFailure(
        error,
        "The dedicated Chrome window could not be inspected.",
      );
    }
    const tabs = window.tabs ?? [];
    const tab = tabs[0];
    if (
      window.type !== "normal" ||
      window.incognito ||
      tabs.length !== 1 ||
      tab?.id === undefined
    ) {
      throw new BrowserExecutionError(
        "browser_unavailable",
        "Chrome did not create an isolated automation tab.",
      );
    }
    return { windowId: created.id, tabId: tab.id };
  }

  private async preparePage(
    context: AutomationContext,
    targetUrl: string,
    action: Action,
    platform: Platform,
  ): Promise<TabState> {
    const current = await this.readTabState(context);
    if (current.pendingUrl !== undefined && current.pendingUrl !== targetUrl) {
      throw new BrowserExecutionError(
        "navigation_in_progress",
        "The automation tab is already navigating. Wait for it to finish, then try again.",
      );
    }

    const shouldObserveTransition =
      action === "refresh" ||
      current.url !== targetUrl ||
      current.pendingUrl !== undefined;
    let transitionObserved = !shouldObserveTransition;
    const onUpdated = (
      tabId: number,
      changeInfo: chrome.tabs.ChangeInfo,
    ): void => {
      if (
        tabId === context.tabId &&
        (changeInfo.status !== undefined ||
          changeInfo.pendingUrl !== undefined ||
          changeInfo.url !== undefined)
      ) {
        transitionObserved = true;
      }
    };
    if (shouldObserveTransition) {
      chrome.tabs.onUpdated.addListener(onUpdated);
    }
    try {
      try {
        if (action === "refresh" && current.url === targetUrl) {
          await chrome.tabs.reload(context.tabId);
        } else if (
          current.url !== targetUrl ||
          current.pendingUrl !== undefined
        ) {
          await chrome.tabs.update(context.tabId, {
            active: true,
            url: targetUrl,
          });
        } else {
          await chrome.tabs.update(context.tabId, { active: true });
        }
      } catch (error) {
        throw mapChromeFailure(
          error,
          "Chrome could not navigate the automation tab.",
        );
      }
      return await this.waitForReady(
        context,
        targetUrl,
        platform,
        current,
        () => transitionObserved,
      );
    } finally {
      if (shouldObserveTransition) {
        chrome.tabs.onUpdated.removeListener(onUpdated);
      }
    }
  }

  private async waitForReady(
    context: AutomationContext,
    targetUrl: string,
    platform: Platform,
    initial: TabState,
    hasObservedTransition: () => boolean,
  ): Promise<TabState> {
    const deadline = Date.now() + NAVIGATION_TIMEOUT_MS;
    let transitionObserved = hasObservedTransition();
    while (Date.now() < deadline) {
      const state = await this.readTabState(context);
      if (!transitionObserved && !sameNavigationState(initial, state)) {
        transitionObserved = true;
      }
      if (!transitionObserved) {
        await delay(100);
        continue;
      }
      rejectUnexpectedNavigation(state, targetUrl, platform);
      if (
        state.status === "complete" &&
        state.pendingUrl === undefined &&
        state.url !== undefined
      ) {
        return state;
      }
      await delay(100);
    }
    throw new BrowserExecutionError(
      "navigation_timeout",
      "The page did not finish loading before the job expired. Try again.",
    );
  }

  private async readDriverPage(
    context: AutomationContext,
    driver: SiteDriver,
    options: DriverScriptOptions,
    expectedUrl: string,
    platform: Platform,
  ): Promise<{
    readonly data: ResultData;
    readonly url: string;
    readonly title: string;
  }> {
    const deadline = Date.now() + NAVIGATION_TIMEOUT_MS;
    let trustedClicks = 0;
    while (Date.now() < deadline) {
      let results: readonly chrome.scripting.InjectionResult<DriverPageResult>[];
      try {
        results = await chrome.scripting.executeScript({
          target: { tabId: context.tabId },
          func: driver.pageScript,
          args: [options],
        });
      } catch (error) {
        if (SUBMIT_ACTIONS.has(options.action)) {
          throw uncertainSubmissionError(
            error instanceof Error ? error.message : String(error),
          );
        }
        throw mapChromeFailure(
          error,
          "Chrome could not read the selected site page.",
        );
      }
      const result = parseDriverPageResult(results[0]?.result);
      if (result === null) {
        if (SUBMIT_ACTIONS.has(options.action)) {
          throw uncertainSubmissionError(
            `the page script returned ${JSON.stringify(results[0]?.result).slice(0, 300)}`,
          );
        }
        throw new BrowserExecutionError(
          "site_markup_changed",
          `The ${platform} page returned an unsupported result. Its markup may have changed.`,
        );
      }
      if (result.state === "waiting") {
        const click = trustedClickRequest(result);
        if (click && SUBMIT_ACTIONS.has(options.action)) {
          trustedClicks += 1;
          if (trustedClicks > MAX_TRUSTED_CLICKS) {
            throw new BrowserExecutionError(
              "site_markup_changed",
              `The ${platform} page kept asking for more clicks than a thread can need. Nothing was submitted.`,
            );
          }
          await this.trustedClick(context.tabId, click);
          await delay(400);
          continue;
        }
        await delay(100);
        continue;
      }
      if (result.state !== "ready" && result.state !== "submission_succeeded") {
        throw driverFailure(result);
      }
      const submissionSucceeded = result.state === "submission_succeeded";
      try {
        const url = readResultString(result.url, 2_048);
        const title = readResultString(result.title, 512);
        if (url === null || title === null) {
          throw new BrowserExecutionError(
            "site_markup_changed",
            `The ${platform} page did not return a usable URL and title. Its markup may have changed.`,
          );
        }
        const finalUrl = assertFinalUrl(platform, expectedUrl, url);
        if (finalUrl !== expectedUrl) {
          throw new BrowserExecutionError(
            "page_changed",
            "The page changed while it was being read. Try again.",
          );
        }
        const {
          state: _state,
          message: _message,
          url: _url,
          title: _title,
          ...data
        } = result;
        if (typeof data.kind !== "string" || data.kind.length === 0) {
          throw new BrowserExecutionError(
            "site_markup_changed",
            `The ${platform} driver returned no supported result kind. Its markup may have changed.`,
          );
        }
        const boundedData =
          typeof data.text === "string"
            ? { ...data, text: data.text.slice(0, MAX_RESULT_TEXT_LENGTH) }
            : data;
        return {
          data: { kind: data.kind, ...boundedData },
          url: finalUrl,
          title,
        };
      } catch (error) {
        if (
          SUBMIT_ACTIONS.has(options.action) &&
          submissionSucceeded &&
          error instanceof BrowserExecutionError
        ) {
          throw uncertainSubmissionError();
        }
        throw error;
      }
    }
    throw new BrowserExecutionError(
      "site_markup_changed",
      `The ${platform} page did not expose the required controls before the job expired. Its markup may have changed.`,
    );
  }

  private async captureScreenshot(
    context: AutomationContext,
    expectedUrl: string,
    platform: Platform,
  ): Promise<Uint8Array> {
    const before = await this.readStableCaptureState(
      context,
      expectedUrl,
      platform,
    );
    const waitMs = Math.max(
      0,
      CAPTURE_INTERVAL_MS - (Date.now() - this.lastCaptureAt),
    );
    if (waitMs > 0) {
      await delay(waitMs);
    }
    this.lastCaptureAt = Date.now();

    let dataUrl: string;
    try {
      dataUrl = await chrome.tabs.captureVisibleTab(context.windowId, {
        format: "png",
      });
    } catch (error) {
      throw mapCaptureFailure(error);
    }
    const after = await this.readStableCaptureState(
      context,
      expectedUrl,
      platform,
    );
    if (!sameCaptureState(before, after)) {
      throw new BrowserExecutionError(
        "capture_discarded",
        "The screenshot was discarded because the automation tab changed during capture. Try again.",
      );
    }
    return decodePngDataUrl(dataUrl);
  }

  private async readStableCaptureState(
    context: AutomationContext,
    expectedUrl: string,
    platform: Platform,
  ): Promise<TabState> {
    const state = await this.readTabState(context);
    rejectUnexpectedNavigation(state, expectedUrl, platform);
    if (
      !state.active ||
      state.status !== "complete" ||
      state.pendingUrl !== undefined ||
      state.url !== expectedUrl
    ) {
      throw new BrowserExecutionError(
        "page_changed",
        "The page changed before the screenshot could be captured. Try again.",
      );
    }
    if (state.windowState === "minimized") {
      throw new BrowserExecutionError(
        "permission_denied",
        "Chrome cannot capture a minimized automation window. Restore it and try again.",
      );
    }
    return state;
  }

  private async readTabState(context: AutomationContext): Promise<TabState> {
    let window: chrome.windows.Window;
    let tab: chrome.tabs.Tab;
    let activeTabs: readonly chrome.tabs.Tab[];
    try {
      window = await chrome.windows.get(context.windowId, { populate: true });
      tab = await chrome.tabs.get(context.tabId);
      activeTabs = await chrome.tabs.query({
        active: true,
        windowId: context.windowId,
      });
    } catch (error) {
      throw mapChromeFailure(
        error,
        "The automation window is no longer available.",
      );
    }
    const windowTabs = window.tabs ?? [];
    if (
      window.id !== context.windowId ||
      window.type !== "normal" ||
      window.incognito ||
      windowTabs.length !== 1 ||
      windowTabs[0]?.id !== context.tabId ||
      tab.id !== context.tabId ||
      tab.windowId !== context.windowId
    ) {
      throw new BrowserExecutionError(
        "browser_unavailable",
        "The dedicated automation window was changed or closed. Try the job again.",
      );
    }
    return {
      tabId: context.tabId,
      windowId: context.windowId,
      windowType: window.type,
      windowState: window.state,
      windowFocused: window.focused,
      active: activeTabs[0]?.id === context.tabId && tab.active,
      status: tab.status,
      url: tab.url,
      pendingUrl: tab.pendingUrl,
    };
  }
}

function makeDriverScriptOptions(
  command: CommandEnvelope,
  targetUrl: string,
): DriverScriptOptions {
  if (command.payload.kind === "read_post") {
    return {
      action: command.action,
      targetUrl,
      postId: command.payload.postId,
    };
  }
  if (command.payload.kind === "submission") {
    return {
      action: command.action,
      targetUrl,
      postId: command.payload.postId,
      text: command.payload.text,
    };
  }
  if (command.payload.kind === "post_submission") {
    return {
      action: command.action,
      targetUrl,
      text: command.payload.text,
      parts: command.payload.parts,
    };
  }
  return { action: command.action, targetUrl };
}

function parseDriverPageResult(value: unknown): DriverPageResult | null {
  if (!isRecord(value) || typeof value.state !== "string") {
    return null;
  }
  const states = new Set([
    "ready",
    "waiting",
    "login_required",
    "unsupported",
    "target_not_found",
    "account_unverified",
    "target_mismatch",
    "submission_succeeded",
    "submission_unknown",
  ]);
  if (!states.has(value.state)) {
    return null;
  }
  if (value.message !== undefined && !isBoundedString(value.message, 512)) {
    return null;
  }
  return value as DriverPageResult;
}

function driverFailure(result: DriverPageResult): BrowserExecutionError {
  const message =
    readResultString(result.message, 512) ??
    "The site driver could not complete this browser job.";
  switch (result.state) {
    case "login_required":
      return new BrowserExecutionError("login_required", message);
    case "target_not_found":
      return new BrowserExecutionError("target_not_found", message);
    case "account_unverified":
      return new BrowserExecutionError("account_unverified", message);
    case "target_mismatch":
      return new BrowserExecutionError("target_mismatch", message);
    case "submission_unknown":
      return new BrowserExecutionError("submission_unknown", message);
    default:
      return new BrowserExecutionError("site_markup_changed", message);
  }
}

function uncertainSubmissionError(detail?: string): BrowserExecutionError {
  const reason = detail ? ` Chrome reported: ${detail.slice(0, 300)}` : "";
  return new BrowserExecutionError(
    "submission_unknown",
    `This may have gone through, but Pluk could not confirm it. Check before trying again; Pluk did not retry.${reason}`,
  );
}

function readResultString(value: unknown, maxLength: number): string | null {
  return isBoundedString(value, maxLength) ? value : null;
}

async function requireSitePermission(targetUrl: string): Promise<void> {
  const hostname = new URL(targetUrl).hostname;
  let granted = false;
  try {
    granted = await chrome.permissions.contains({
      origins: [`https://${hostname}/*`],
    });
  } catch (error) {
    throw mapChromeFailure(error, "Chrome could not check site access.");
  }
  if (!granted) {
    throw new BrowserExecutionError(
      "permission_denied",
      `Chrome has not granted access to ${hostname}. Open Wande and choose Grant site access, then try again.`,
    );
  }
}

function rejectUnexpectedNavigation(
  state: TabState,
  targetUrl: string,
  platform: Platform,
): void {
  if (
    state.pendingUrl !== undefined &&
    !isSameAllowedOrigin(targetUrl, state.pendingUrl, platform)
  ) {
    throw new BrowserExecutionError(
      "page_changed",
      "The page is redirecting outside the allowed site. No snapshot was returned.",
    );
  }
  if (
    state.pendingUrl === undefined &&
    state.url !== undefined &&
    !isSameAllowedOrigin(targetUrl, state.url, platform)
  ) {
    throw new BrowserExecutionError(
      "page_changed",
      "The page redirected outside the allowed site. No snapshot was returned.",
    );
  }
}

export function isSameAllowedOrigin(
  targetUrl: string,
  candidateUrl: string,
  platform: Platform,
): boolean {
  const target = canonicalizeTargetUrl(targetUrl, platform);
  const candidate = canonicalizeTargetUrl(candidateUrl, platform);
  if (!target.ok || !candidate.ok) {
    return false;
  }
  const targetParsed = new URL(target.value);
  const candidateParsed = new URL(candidate.value);
  return (
    targetParsed.protocol === candidateParsed.protocol &&
    targetParsed.hostname === candidateParsed.hostname &&
    targetParsed.port === candidateParsed.port
  );
}

function assertFinalUrl(
  platform: Platform,
  targetUrl: string,
  finalUrl: string,
): string {
  const parsed = canonicalizeTargetUrl(finalUrl, platform);
  if (!parsed.ok || !isSameAllowedOrigin(targetUrl, finalUrl, platform)) {
    throw new BrowserExecutionError(
      "page_changed",
      "The page redirected outside the allowed site. No snapshot was returned.",
    );
  }
  return parsed.value;
}

function serializeExtract(result: ResultData): Uint8Array {
  const encoder = new TextEncoder();
  const arrayKeys = ["posts", "threads", "trends", "messages"];
  const candidate: Record<string, unknown> = { ...result };
  for (const key of arrayKeys) {
    if (Array.isArray(candidate[key])) {
      candidate[key] = candidate[key].slice(0, 50);
    }
  }
  if (typeof candidate.text === "string") {
    candidate.text = candidate.text.slice(0, MAX_RESULT_TEXT_LENGTH);
  }
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const encoded = encoder.encode(JSON.stringify(candidate));
    if (encoded.byteLength <= MAX_EXTRACT_BYTES) {
      return encoded;
    }
    for (const key of arrayKeys) {
      if (Array.isArray(candidate[key])) {
        candidate[key] = candidate[key].slice(
          0,
          Math.max(0, Math.floor(candidate[key].length / 2)),
        );
      }
    }
    if (typeof candidate.text === "string") {
      candidate.text = candidate.text.slice(
        0,
        Math.floor(candidate.text.length / 2),
      );
    }
  }
  throw new BrowserExecutionError(
    "site_markup_changed",
    "The site result exceeded the local extract size limit. Reduce the visible result and try again.",
  );
}

export function decodePngDataUrl(dataUrl: string): Uint8Array {
  const prefix = "data:image/png;base64,";
  if (!dataUrl.startsWith(prefix)) {
    throw new BrowserExecutionError(
      "browser_unavailable",
      "Chrome returned an unsupported screenshot format.",
    );
  }
  const encoded = dataUrl.slice(prefix.length);
  if (encoded.length > Math.ceil((MAX_SCREENSHOT_BYTES * 4) / 3) + 4) {
    throw new BrowserExecutionError(
      "screenshot_too_large",
      "The screenshot exceeded the local size limit. Reduce the browser window size and try again.",
    );
  }
  let binary: string;
  try {
    binary = atob(encoded);
  } catch {
    throw new BrowserExecutionError(
      "browser_unavailable",
      "Chrome returned an invalid screenshot.",
    );
  }
  if (binary.length > MAX_SCREENSHOT_BYTES) {
    throw new BrowserExecutionError(
      "screenshot_too_large",
      "The screenshot exceeded the local size limit. Reduce the browser window size and try again.",
    );
  }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function mapCaptureFailure(error: unknown): BrowserExecutionError {
  const message = getErrorMessage(error);
  if (/permission|not allowed|access|capture/iu.test(message)) {
    return new BrowserExecutionError(
      "permission_denied",
      "Chrome blocked the screenshot. Open Wande and choose Grant site access, then try again.",
    );
  }
  return new BrowserExecutionError(
    "browser_unavailable",
    "Chrome could not capture the visible automation tab. Restore the window and try again.",
  );
}

function isBoundedString(value: unknown, maxLength: number): value is string {
  return (
    typeof value === "string" &&
    value.length <= maxLength &&
    ![...value].some((character) => {
      const code = character.charCodeAt(0);
      return code < 0x20 && code !== 0x09;
    })
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function mapChromeFailure(
  error: unknown,
  fallbackMessage: string,
): BrowserExecutionError {
  const message = getErrorMessage(error);
  if (/permission|not allowed|access denied|cannot access/iu.test(message)) {
    return new BrowserExecutionError(
      "permission_denied",
      "Chrome blocked access to the page. Open Wande and choose Grant site access, then try again.",
    );
  }
  return new BrowserExecutionError("browser_unavailable", fallbackMessage);
}

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function sameCaptureState(left: TabState, right: TabState): boolean {
  return (
    left.tabId === right.tabId &&
    left.windowId === right.windowId &&
    left.windowType === right.windowType &&
    left.windowState === right.windowState &&
    left.windowFocused === right.windowFocused &&
    left.active === right.active &&
    left.status === right.status &&
    left.url === right.url &&
    left.pendingUrl === right.pendingUrl
  );
}

function sameNavigationState(left: TabState, right: TabState): boolean {
  return (
    left.status === right.status &&
    left.url === right.url &&
    left.pendingUrl === right.pendingUrl
  );
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
