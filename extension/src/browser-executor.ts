import {
  type Action,
  type CommandEnvelope,
  type ImageAttachment,
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

// How long a clicked link gets to land on its page before the tab is sent
// there directly instead.
const LINK_NAVIGATION_WINDOW_MS = 5_000;
const CAPTURE_INTERVAL_MS = 500;
const MAX_RESULT_TEXT_LENGTH = 8_000;

// Only the explicit "capture" tool touches captureVisibleTab. Every other
// action, the submissions included, returns DOM text only, so a missing or
// failed screenshot grant can never block reading or posting.
const SCREENSHOT_ACTIONS = new Set<Action>(["capture"]);

// The submit actions drive X's own controls and publish on a single click.
// Any uncertainty past that click (a Chrome failure, an unparsable result)
// must surface as "unknown", never as a clean failure that invites a retry.
// The window is never brought forward for it: posting happens behind
// whatever the owner is doing.
const SUBMIT_ACTIONS = new Set<Action>([
  "submit_reply",
  "submit_repost",
  "submit_quote",
  "submit_post",
]);
const MAX_DEBUG_HTML_BYTES = 2 * 1024 * 1024;
const MAX_TRUSTED_CLICKS = 25;
const IN_PLACE_ACTIONS = new Set<Action>(["read_post", "inspect"]);

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
  return payload.debug !== undefined;
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

interface HostBridge {
  upload(
    jobId: string,
    kind: "screenshot" | "extract",
    contentType: string,
    body: Uint8Array,
  ): Promise<string>;
  /** One approved image's exact bytes, fetched by id over the same
   * authenticated connection everything else here uses — never a local
   * path, which a browser tab cannot read anyway. */
  downloadImage(
    draftId: string,
    imageId: string,
  ): Promise<{ readonly data: Uint8Array; readonly contentType: string }>;
}

interface DebuggerSession {
  readonly target: chrome.DebuggerTarget;
  readonly api: chrome.DebuggerApi;
}

export type BrowserErrorCode =
  | "artifact_upload_failed"
  | "account_unverified"
  | "browser_unavailable"
  | "capture_discarded"
  | "document_not_ready"
  | "image_download_failed"
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

  run(command: CommandEnvelope, sink: HostBridge): Promise<ResultData> {
    const next = this.chain.then(() => this.execute(command, sink));
    this.chain = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }

  private async execute(
    command: CommandEnvelope,
    sink: HostBridge,
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
    const options = await makeDriverScriptOptions(command, targetUrl.value, sink);
    const page =
      (await this.readInPlace(context, driver, options, command.platform)) ??
      (await this.readAfterNavigation(
        context,
        driver,
        command,
        options,
        targetUrl.value,
        sink,
      ));
    const { debugCaptures, ...pageData } = page.data;
    let extractArtifactId: string;
    let screenshotArtifactId: string | undefined;
    let debugCapturesArtifactId: string | undefined;
    try {
      const extract = serializeExtract(pageData);
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
      if (wantsDebug(command.payload) && typeof debugCaptures === "string") {
        debugCapturesArtifactId = await sink.upload(
          command.jobId,
          "extract",
          "application/json",
          new TextEncoder().encode(debugCaptures),
        );
      }
    } catch (error) {
      if (SUBMIT_ACTIONS.has(command.action)) {
        throw uncertainSubmissionError();
      }
      throw error;
    }

    return {
      ...pageData,
      url: page.url,
      title: page.title,
      extractArtifactId,
      ...(screenshotArtifactId === undefined ? {} : { screenshotArtifactId }),
      ...(debugCapturesArtifactId === undefined
        ? {}
        : { debugCapturesArtifactId }),
    };
  }

  /** A post already on screen is read where it is, so the tab is not sent
   * off to load a page it is already looking at. Anything short of the
   * post being there falls through to navigation. */
  private async readInPlace(
    context: AutomationContext,
    driver: SiteDriver,
    options: DriverScriptOptions,
    platform: Platform,
  ): Promise<PageRead | null> {
    if (!IN_PLACE_ACTIONS.has(options.action)) {
      return null;
    }
    const state = await this.readTabState(context);
    if (
      state.url === undefined ||
      state.pendingUrl !== undefined ||
      state.status !== "complete" ||
      !isSameAllowedOrigin(options.targetUrl, state.url, platform)
    ) {
      return null;
    }
    let results: readonly chrome.scripting.InjectionResult<DriverPageResult>[];
    try {
      results = await chrome.scripting.executeScript({
        target: { tabId: context.tabId },
        func: driver.pageScript,
        args: [options],
      });
    } catch {
      return null;
    }
    const result = parseDriverPageResult(results[0]?.result);
    if (result?.state !== "ready") {
      return null;
    }
    try {
      return pageFromResult(platform, state.url, result);
    } catch {
      return null;
    }
  }

  private async readAfterNavigation(
    context: AutomationContext,
    driver: SiteDriver,
    command: CommandEnvelope,
    options: DriverScriptOptions,
    targetUrl: string,
    sink: HostBridge,
  ): Promise<PageRead> {
    const ready = await this.preparePage(
      context,
      targetUrl,
      command.action,
      command.platform,
      command.expiresAt,
    );
    if (ready.url === undefined) {
      throw new BrowserExecutionError(
        "browser_unavailable",
        "Chrome returned no final page URL.",
      );
    }
    if (!sameDestination(command.platform, targetUrl, ready.url)) {
      throw new BrowserExecutionError(
        "page_changed",
        "The page changed while it was being prepared. Try again.",
      );
    }
    const readPage = async (session?: DebuggerSession) => {
      try {
        return await this.readDriverPage(
          context,
          driver,
          options,
          targetUrl,
          command.platform,
          command.expiresAt,
          session,
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
    return SUBMIT_ACTIONS.has(command.action)
      ? this.withEmulatedFocus(context, readPage)
      : readPage();
  }

  /** A real click, delivered through the browser rather than the page, for
   * controls that ignore or mistrust scripted events. Runs on the debugger
   * session withEmulatedFocus already holds for the submit. */
  private async trustedClick(
    session: DebuggerSession,
    point: { readonly x: number; readonly y: number },
  ): Promise<void> {
    try {
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
        await session.api.sendCommand(session.target, "Input.dispatchMouseEvent", {
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
    }
  }

  /** What the page looked like when it refused: a screenshot and its HTML,
   * attached to the job for the owner to read. Best effort, and never in
   * the way of the failure itself. */
  private async attachDebug(
    context: AutomationContext,
    jobId: string,
    sink: HostBridge,
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

  /** X drives its composer from animation frames, which Chrome throttles for
   * a tab that is covered or backgrounded. The CDP debugger emulates a
   * focused, active page on the automation tab for the submit, so the
   * composer keeps running without the window ever coming forward. */
  private async withEmulatedFocus<T>(
    context: AutomationContext,
    work: (session: DebuggerSession) => Promise<T>,
  ): Promise<T> {
    const target: chrome.DebuggerTarget = { tabId: context.tabId };
    // `debugger` is a reserved word, so the namespace cannot be declared
    // alongside the others in chrome.d.ts and is typed through this lookup.
    const api = (chrome as unknown as { debugger: chrome.DebuggerApi }).debugger;
    try {
      await api.attach(target, "1.3");
    } catch (error) {
      throw mapChromeFailure(
        error,
        "Chrome would not let Pluk emulate focus on the automation tab.",
      );
    }
    try {
      await api.sendCommand(target, "Emulation.setFocusEmulationEnabled", {
        enabled: true,
      });
      await api.sendCommand(target, "Page.setWebLifecycleState", {
        state: "active",
      });
      return await work({ target, api });
    } finally {
      await api
        .sendCommand(target, "Emulation.setFocusEmulationEnabled", {
          enabled: false,
        })
        .catch(() => undefined);
      await api.detach(target).catch(() => undefined);
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
    deadline: number,
  ): Promise<TabState> {
    const current = await this.readTabState(context);
    if (current.pendingUrl !== undefined && current.pendingUrl !== targetUrl) {
      throw new BrowserExecutionError(
        "navigation_in_progress",
        "The automation tab is already navigating. Wait for it to finish, then try again.",
      );
    }

    const alreadyThere =
      current.url !== undefined &&
      sameDestination(platform, targetUrl, current.url);
    if (
      action !== "refresh" &&
      !alreadyThere &&
      current.pendingUrl === undefined &&
      current.status === "complete" &&
      current.url !== undefined &&
      isSameAllowedOrigin(targetUrl, current.url, platform)
    ) {
      const followed = await this.followLink(
        context,
        targetUrl,
        platform,
        deadline,
      );
      if (followed !== null) {
        return followed;
      }
    }
    const shouldObserveTransition =
      action === "refresh" || !alreadyThere || current.pendingUrl !== undefined;
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
        if (action === "refresh" && alreadyThere) {
          await chrome.tabs.reload(context.tabId);
        } else if (!alreadyThere || current.pendingUrl !== undefined) {
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
        deadline,
      );
    } finally {
      if (shouldObserveTransition) {
        chrome.tabs.onUpdated.removeListener(onUpdated);
      }
    }
  }

  /** Move within the site the way a person would: click a visible link that
   * already points at the target, and confirm the tab landed there. `null`
   * when no such link is on the page or the click did not land in time, so
   * the caller navigates directly instead. */
  private async followLink(
    context: AutomationContext,
    targetUrl: string,
    platform: Platform,
    deadline: number,
  ): Promise<TabState | null> {
    let results: readonly chrome.scripting.InjectionResult<boolean>[];
    try {
      results = await chrome.scripting.executeScript({
        target: { tabId: context.tabId },
        func: clickLinkTo,
        args: [targetUrl],
      });
    } catch {
      return null;
    }
    if (results[0]?.result !== true) {
      return null;
    }
    const landBy = Math.min(deadline, Date.now() + LINK_NAVIGATION_WINDOW_MS);
    while (Date.now() < landBy) {
      const state = await this.readTabState(context);
      rejectUnexpectedNavigation(state, targetUrl, platform);
      if (
        state.status === "complete" &&
        state.pendingUrl === undefined &&
        state.url !== undefined &&
        sameDestination(platform, targetUrl, state.url)
      ) {
        return state;
      }
      await delay(100);
    }
    return null;
  }

  private async waitForReady(
    context: AutomationContext,
    targetUrl: string,
    platform: Platform,
    initial: TabState,
    hasObservedTransition: () => boolean,
    deadline: number,
  ): Promise<TabState> {
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
    deadline: number,
    session?: DebuggerSession,
  ): Promise<PageRead> {
    let trustedClicks = 0;
    let lastWait: string | null = null;
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
        lastWait = readResultString(result.message, 512);
        const click = trustedClickRequest(result);
        if (click && SUBMIT_ACTIONS.has(options.action)) {
          trustedClicks += 1;
          if (trustedClicks > MAX_TRUSTED_CLICKS) {
            throw new BrowserExecutionError(
              "site_markup_changed",
              `The ${platform} page kept asking for more clicks than this could need. Nothing was submitted.`,
            );
          }
          // SUBMIT_ACTIONS is the only source of trusted clicks, and
          // withEmulatedFocus always supplies a session for that path.
          await this.trustedClick(session!, click);
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
        return pageFromResult(platform, expectedUrl, result);
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
    // A submit that runs out of time may already have gone out; saying it
    // failed would invite the same post a second time.
    if (SUBMIT_ACTIONS.has(options.action)) {
      throw uncertainSubmissionError(lastWait ?? undefined);
    }
    throw new BrowserExecutionError(
      "site_markup_changed",
      lastWait
        ? `Waited on the ${platform} page until the job expired. Last seen: ${lastWait}`
        : `The ${platform} page did not expose the required controls before the job expired. Its markup may have changed.`,
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

/** Every image this command's payload names, fetched over the host bridge's
 * own authenticated connection and base64-encoded, ready for the driver to
 * paste — a browser tab cannot read a host file path, and none crosses the
 * bounded WebSocket command channel to begin with. Fetched once, up front,
 * so a slow or failing download surfaces before any page or composer is
 * touched, rather than mid-thread. */
async function makeDriverScriptOptions(
  command: CommandEnvelope,
  targetUrl: string,
  sink: HostBridge,
): Promise<DriverScriptOptions> {
  const debug = command.payload.debug;
  if (command.payload.kind === "read_post") {
    return {
      action: command.action,
      targetUrl,
      postId: command.payload.postId,
      debug,
    };
  }
  if (command.payload.kind === "submission") {
    const { draftId, postId, text, images: requested } = command.payload;
    const images = requested ? await fetchImages(sink, draftId, requested) : undefined;
    return {
      action: command.action,
      targetUrl,
      draftId,
      postId,
      text,
      images,
      debug,
    };
  }
  if (command.payload.kind === "repost_submission") {
    return {
      action: command.action,
      targetUrl,
      draftId: command.payload.draftId,
      postId: command.payload.postId,
      debug,
    };
  }
  if (
    command.payload.kind === "post_submission" ||
    command.payload.kind === "quote_submission"
  ) {
    const { draftId, text, parts, partImages: requested } = command.payload;
    const partImages = requested
      ? await Promise.all(requested.map((images) => fetchImages(sink, draftId, images)))
      : undefined;
    return {
      action: command.action,
      targetUrl,
      draftId,
      ...(command.payload.kind === "quote_submission"
        ? { postId: command.payload.postId }
        : {}),
      text,
      parts,
      partImages,
      debug,
    };
  }
  return { action: command.action, targetUrl, debug };
}

async function fetchImages(
  sink: HostBridge,
  draftId: string,
  images: readonly ImageAttachment[],
): Promise<readonly { readonly data: string; readonly contentType: string }[]> {
  return Promise.all(
    images.map(async (image) => {
      const fetched = await sink.downloadImage(draftId, image.imageId);
      return { data: bytesToBase64(fetched.data), contentType: fetched.contentType };
    }),
  );
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
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

interface PageRead {
  readonly data: ResultData;
  readonly url: string;
  readonly title: string;
}

/** The page a ready driver result describes, checked against where the
 * tab was meant to be. */
function pageFromResult(
  platform: Platform,
  expectedUrl: string,
  result: DriverPageResult,
): PageRead {
  const url = readResultString(result.url, 2_048);
  const title = readResultString(result.title, 512);
  if (url === null || title === null) {
    throw new BrowserExecutionError(
      "site_markup_changed",
      `The ${platform} page did not return a usable URL and title. Its markup may have changed.`,
    );
  }
  const finalUrl = assertFinalUrl(platform, expectedUrl, url);
  if (!sameDestination(platform, expectedUrl, finalUrl)) {
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
  return { data: { kind: data.kind, ...boundedData }, url: finalUrl, title };
}

/** Runs in the page. Clicks the first visible same-origin link whose
 * destination is the target — the exact path, or for a post the same status
 * id under any handle — and reports whether it clicked. Links that open a new
 * tab, download, or sit inside editable content are never touched. */
function clickLinkTo(target: string): boolean {
  const wanted = new URL(target);
  const postId = (path: string) =>
    /^(?:\/[A-Za-z0-9_]{1,50})?\/status\/(\d+)\/?$/u.exec(path)?.[1] ?? null;
  const trimmed = (path: string) => path.replace(/\/+$/u, "") || "/";
  const wantedPost = postId(wanted.pathname);
  for (const anchor of Array.from(document.querySelectorAll("a[href]"))) {
    if (!(anchor instanceof HTMLAnchorElement)) {
      continue;
    }
    let href: URL;
    try {
      href = new URL(anchor.href, window.location.href);
    } catch {
      continue;
    }
    const matches =
      href.origin === window.location.origin &&
      href.hash === "" &&
      href.search === wanted.search &&
      (wantedPost !== null
        ? postId(href.pathname) === wantedPost
        : trimmed(href.pathname) === trimmed(wanted.pathname));
    if (
      !matches ||
      anchor.target === "_blank" ||
      anchor.hasAttribute("download") ||
      anchor.closest("[contenteditable='true']") !== null
    ) {
      continue;
    }
    const box = anchor.getBoundingClientRect();
    const style = window.getComputedStyle(anchor);
    if (
      box.width === 0 ||
      box.height === 0 ||
      style.visibility === "hidden" ||
      style.display === "none"
    ) {
      continue;
    }
    anchor.click();
    return true;
  }
  return false;
}

/** The same page, allowing for X moving a post from /i/status/<id> to the
 * author's own URL for that post. */
function sameDestination(
  platform: Platform,
  expected: string,
  actual: string,
): boolean {
  if (expected === actual) {
    return true;
  }
  if (platform !== "x") {
    return false;
  }
  const postId = (value: string) =>
    new URL(value).pathname.match(/\/status\/(\d+)(?:\/|$)/u)?.[1] ?? null;
  const expectedId = postId(expected);
  return expectedId !== null && expectedId === postId(actual);
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
