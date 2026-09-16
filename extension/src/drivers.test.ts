import { expect, test } from "bun:test";
import { runInstagramPage } from "./drivers/instagram";
import { runXPage } from "./drivers/x";

interface FixtureNode {
  readonly textContent: string;
  readonly attributes: Readonly<Record<string, string>>;
  readonly selectors: Readonly<Record<string, readonly FixtureNode[]>>;
  querySelector(selector: string): FixtureNode | null;
  querySelectorAll(selector: string): readonly FixtureNode[];
  getAttribute(name: string): string | null;
  setAttribute(name: string, value: string): void;
  hasAttribute(name: string): boolean;
  closest(selector: string): FixtureNode | null;
  contains(other: FixtureNode): boolean;
  getBoundingClientRect(): {
    readonly left: number;
    readonly top: number;
    readonly width: number;
    readonly height: number;
  };
  dispatchEvent(event: {
    readonly type?: string;
    readonly key?: string;
    readonly metaKey?: boolean;
    readonly ctrlKey?: boolean;
    readonly shiftKey?: boolean;
    readonly clipboardData?: { getData(format: string): string };
  }): boolean;
}

let fixturePasteSink: ((text: string) => boolean) | null = null;
let fixtureSubmitSink: (() => void) | null = null;
let fixtureImagePasteSink: ((editor: FixtureNode, files: readonly File[]) => void) | null = null;

class HarnessKeyboardEvent {
  readonly type: string;
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly shiftKey: boolean;

  constructor(
    type: string,
    init: {
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly ctrlKey?: boolean;
      readonly shiftKey?: boolean;
    },
  ) {
    this.type = type;
    this.key = init.key ?? "";
    this.metaKey = init.metaKey ?? false;
    this.ctrlKey = init.ctrlKey ?? false;
    this.shiftKey = init.shiftKey ?? false;
  }
}

class HarnessDataTransfer {
  private text = "";
  readonly files: File[] = [];
  readonly items = {
    add: (file: File): void => {
      this.files.push(file);
    },
  };

  setData(format: string, value: string): void {
    if (format === "text/plain") {
      this.text = value;
    }
  }

  getData(format: string): string {
    return format === "text/plain" ? this.text : "";
  }
}

class HarnessClipboardEvent {
  readonly type: string;
  readonly clipboardData: HarnessDataTransfer;

  constructor(
    type: string,
    init: { readonly clipboardData: HarnessDataTransfer },
  ) {
    this.type = type;
    this.clipboardData = init.clipboardData;
  }
}

function node(
  textContent: string,
  attributes: Readonly<Record<string, string>> = {},
  selectors: Readonly<Record<string, readonly FixtureNode[]>> = {},
): FixtureNode {
  return {
    textContent,
    attributes,
    selectors,
    querySelector(selector) {
      return this.selectors[selector]?.[0] ?? null;
    },
    querySelectorAll(selector) {
      return this.selectors[selector] ?? [];
    },
    getAttribute(name) {
      return this.attributes[name] ?? null;
    },
    setAttribute(name, value) {
      (this.attributes as Record<string, string>)[name] = value;
    },
    hasAttribute(name) {
      return name in this.attributes;
    },
    closest() {
      return null;
    },
    getBoundingClientRect() {
      return { left: 10, top: 20, width: 30, height: 40 };
    },
    contains(other) {
      return (
        other === this ||
        Object.values(this.selectors).some((list) =>
          list.some((child) => child.contains(other)),
        )
      );
    },
    dispatchEvent(event: {
      readonly type?: string;
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly ctrlKey?: boolean;
      readonly shiftKey?: boolean;
      readonly clipboardData?: { getData(format: string): string; files?: readonly File[] };
    }) {
      if (event.type === "paste" && event.clipboardData?.files?.length && fixtureImagePasteSink) {
        fixtureImagePasteSink(this, event.clipboardData.files);
      } else if (event.type === "paste" && fixturePasteSink) {
        fixturePasteSink(event.clipboardData?.getData("text/plain") ?? "");
      }
      if (
        event.type === "keydown" &&
        event.key === "Enter" &&
        event.shiftKey !== true &&
        (event.metaKey === true || event.ctrlKey === true)
      ) {
        fixtureSubmitSink?.();
      }
      return true;
    },
  };
}

function makeComposerScope(
  editor: FixtureNode,
  submitButton?: FixtureNode,
  selectors: Readonly<Record<string, readonly FixtureNode[]>> = {},
): {
  readonly scope: FixtureNode;
} {
  fixtureSubmitSink = submitButton
    ? () => (submitButton as unknown as { click?: () => void }).click?.()
    : null;
  const scope = node(
    "",
    {},
    {
      '[data-testid="tweetTextarea_0"]': [editor],
      ...(submitButton
        ? {
            '[data-testid="tweetButtonInline"]': [submitButton],
            '[data-testid="tweetButton"]': [submitButton],
            button: [submitButton],
          }
        : {}),
      ...selectors,
    },
  );
  Object.defineProperty(editor, "parentElement", {
    configurable: true,
    value: scope,
  });
  return { scope };
}

function installPage(
  bodyText: string,
  title: string,
  url: string,
  selectors: Readonly<Record<string, readonly FixtureNode[]>>,
  execCommand?: (
    commandId: string,
    showUi: boolean | undefined,
    value: string | undefined,
  ) => boolean,
  elementsById: Readonly<Record<string, string>> = {},
): () => void {
  const globals = globalThis as unknown as Record<string, unknown>;
  const previousDocument = globals.document;
  const previousWindow = globals.window;
  const documentValue = {
    body: { innerText: bodyText },
    title,
    execCommand,
    querySelector(selector: string) {
      return selectors[selector]?.[0] ?? null;
    },
    querySelectorAll(selector: string) {
      return selectors[selector] ?? [];
    },
    getElementById(id: string) {
      return id in elementsById ? { textContent: elementsById[id] } : null;
    },
  };
  const parsed = new URL(url);
  const previousDataTransfer = globals.DataTransfer;
  const previousClipboardEvent = globals.ClipboardEvent;
  const previousKeyboardEvent = globals.KeyboardEvent;
  Object.defineProperty(globals, "KeyboardEvent", {
    configurable: true,
    value: HarnessKeyboardEvent,
  });
  if (execCommand) {
    fixturePasteSink = (text) => execCommand("insertText", undefined, text);
    Object.defineProperty(globals, "DataTransfer", {
      configurable: true,
      value: HarnessDataTransfer,
    });
    Object.defineProperty(globals, "ClipboardEvent", {
      configurable: true,
      value: HarnessClipboardEvent,
    });
  }
  Object.defineProperty(globals, "document", {
    configurable: true,
    value: documentValue,
  });
  Object.defineProperty(globals, "window", {
    configurable: true,
    value: {
      location: {
        href: url,
        origin: parsed.origin,
        pathname: parsed.pathname,
      },
    },
  });
  return () => {
    fixturePasteSink = null;
    fixtureSubmitSink = null;
    fixtureImagePasteSink = null;
    if (previousKeyboardEvent === undefined) {
      delete globals.KeyboardEvent;
    } else {
      Object.defineProperty(globals, "KeyboardEvent", {
        configurable: true,
        value: previousKeyboardEvent,
      });
    }
    if (previousDataTransfer === undefined) {
      delete globals.DataTransfer;
    } else {
      Object.defineProperty(globals, "DataTransfer", {
        configurable: true,
        value: previousDataTransfer,
      });
    }
    if (previousClipboardEvent === undefined) {
      delete globals.ClipboardEvent;
    } else {
      Object.defineProperty(globals, "ClipboardEvent", {
        configurable: true,
        value: previousClipboardEvent,
      });
    }
    if (previousDocument === undefined) {
      delete globals.document;
    } else {
      Object.defineProperty(globals, "document", {
        configurable: true,
        value: previousDocument,
      });
    }
    if (previousWindow === undefined) {
      delete globals.window;
    } else {
      Object.defineProperty(globals, "window", {
        configurable: true,
        value: previousWindow,
      });
    }
  };
}

test("extracts the visible post from an X status page", () => {
  const xText = node("A visible post", {}, {});
  const xAuthor = node("@author");
  const xStatus = node("", { href: "/author/status/42" });
  const xArticle = node(
    "A visible post",
    {},
    {
      'a[href*="/status/"]': [xStatus],
      '[data-testid="tweetText"]': [xText],
      '[data-testid="User-Name"]': [xAuthor],
    },
  );
  const restoreX = installPage(
    "A visible post",
    "X",
    "https://x.com/status/42",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      'article[data-testid="tweet"]': [xArticle],
    },
  );
  const x = runXPage({
    action: "inspect",
    targetUrl: "https://x.com/status/42",
    postId: "42",
  });
  restoreX();
  expect(x).toMatchObject({
    state: "ready",
    kind: "x_post",
    post: { postId: "42", text: "A visible post" },
  });
});

test("rejects login pages and never submits a reply to a post that is not the target", async () => {
  const restoreLogin = installPage(
    "Sign in to X",
    "Log in to X",
    "https://x.com/i/flow/login",
    {},
  );
  const login = runXPage({
    action: "inspect",
    targetUrl: "https://x.com/status/42",
    postId: "42",
  });
  restoreLogin();
  expect(login).toMatchObject({ state: "login_required" });

  let clicks = 0;
  const replyButton = node("", {});
  Object.defineProperty(replyButton, "click", {
    value: () => {
      clicks += 1;
    },
  });
  const article = node(
    "Visible post",
    {},
    {
      'a[href*="/status/"]': [node("", { href: "/owner/status/42" })],
      '[data-testid="tweetText"]': [node("Visible post")],
      '[data-testid="User-Name"]': [node("@author")],
      '[data-testid="reply"], button[aria-label*="Reply" i], [role="button"][aria-label*="Reply" i]':
        [replyButton],
    },
  );
  const restoreMismatch = installPage(
    "Visible post",
    "X",
    "https://x.com/status/42",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      'article[data-testid="tweet"]': [article],
    },
  );
  const targetMismatch = await runXPage({
    action: "submit_reply",
    targetUrl: "https://x.com/status/42",
    postId: "999",
    text: "Do not send",
  });
  restoreMismatch();
  expect(targetMismatch).toMatchObject({ state: "target_mismatch" });
  expect(clicks).toBe(0);
}, 10_000);

test("reads a typed X profile and refuses a read-post ID that does not match the target", () => {
  const restoreProfile = installPage(
    "Jane Doe profile",
    "Jane Doe (@janedoe) / X",
    "https://x.com/janedoe",
    {
      '[data-testid="UserName"]': [node("Jane Doe@janedoe")],
      '[data-testid="UserDescription"]': [node("Building things.")],
    },
  );
  const profile = runXPage({
    action: "read_profile",
    targetUrl: "https://x.com/janedoe",
  });
  restoreProfile();
  expect(profile).toMatchObject({
    state: "ready",
    kind: "x_profile",
    canonicalTarget: "https://x.com/janedoe",
    handle: "janedoe",
    displayName: "Jane Doe@janedoe",
    bio: "Building things.",
  });

  const article = node(
    "A visible post",
    {},
    {
      'a[href*="/status/"]': [node("", { href: "/janedoe/status/42" })],
      '[data-testid="tweetText"]': [node("A visible post")],
      '[data-testid="User-Name"]': [node("Jane Doe@janedoe")],
    },
  );
  const restorePost = installPage(
    "A visible post",
    "X",
    "https://x.com/janedoe/status/42",
    { 'article[data-testid="tweet"]': [article] },
  );
  const post = runXPage({
    action: "read_post",
    targetUrl: "https://x.com/janedoe/status/42",
    postId: "42",
  });
  const mismatched = runXPage({
    action: "read_post",
    targetUrl: "https://x.com/janedoe/status/42",
    postId: "999",
  });
  restorePost();
  expect(post).toMatchObject({
    state: "ready",
    kind: "x_post",
    canonicalTarget: "https://x.com/janedoe/status/42",
    post: {
      postId: "42",
      canonicalTarget: "https://x.com/janedoe/status/42",
    },
  });
  expect(mismatched).toMatchObject({ state: "target_not_found" });
});

test("submits an exact X reply exactly once into the editor already under the post", async () => {
  let replyClicks = 0;
  let submitClicks = 0;
  let insertedText = "";
  const replyButton = node("");
  Object.defineProperty(replyButton, "click", {
    value: () => {
      replyClicks += 1;
    },
  });
  const submitButton = node("");
  const composer = node("");
  const toast = node("An earlier X notification.");
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: "",
      });
      Object.defineProperty(toast, "textContent", {
        configurable: true,
        value: "Your reply was sent.",
      });
    },
  });
  Object.defineProperty(composer, "focus", { value: () => {} });
  makeComposerScope(composer, submitButton);
  const article = node(
    "Visible post",
    {},
    {
      'a[href*="/status/"]': [node("", { href: "/owner/status/42" })],
      '[data-testid="tweetText"]': [node("Visible post")],
      '[data-testid="User-Name"]': [node("@owner")],
      '[data-testid="reply"], button[aria-label*="Reply" i], [role="button"][aria-label*="Reply" i]':
        [replyButton],
    },
  );
  const restore = installPage(
    "Visible post",
    "X",
    "https://x.com/status/42",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      'article[data-testid="tweet"]': [article],
      'article[data-testid="tweet"], article': [article],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      insertedText += value ?? "";
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: insertedText,
      });
      return true;
    },
  );
  const result = await runXPage({
    action: "submit_reply",
    targetUrl: "https://x.com/status/42",
    postId: "42",
    text: "Thanks for sharing this.",
  });
  restore();
  expect(result).toMatchObject({
    state: "submission_succeeded",
    kind: "submission",
  });
  expect(insertedText).toBe("Thanks for sharing this.");
  expect(replyClicks).toBe(0);
  expect(submitClicks).toBe(1);
});

test("waits for X to enable the reply button instead of giving up while it is still disabled", async () => {
  let submitClicks = 0;
  let insertedText = "";
  let disabledReads = 0;
  const submitButton = node("");
  const composer = node("");
  const toast = node("An earlier X notification.");
  Object.defineProperty(submitButton, "getAttribute", {
    value: (name: string) => {
      if (name !== "aria-disabled") {
        return null;
      }
      disabledReads += 1;
      return disabledReads <= 3 ? "true" : null;
    },
  });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: "",
      });
      Object.defineProperty(toast, "textContent", {
        configurable: true,
        value: "Your reply was sent.",
      });
    },
  });
  Object.defineProperty(composer, "focus", { value: () => {} });
  makeComposerScope(composer, submitButton);
  const article = node(
    "Visible post",
    {},
    {
      'a[href*="/status/"]': [node("", { href: "/owner/status/42" })],
      '[data-testid="tweetText"]': [node("Visible post")],
      '[data-testid="User-Name"]': [node("@owner")],
    },
  );
  const restore = installPage(
    "Visible post",
    "X",
    "https://x.com/status/42",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      'article[data-testid="tweet"]': [article],
      'article[data-testid="tweet"], article': [article],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      insertedText += value ?? "";
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: insertedText,
      });
      return true;
    },
  );
  const result = await runXPage({
    action: "submit_reply",
    targetUrl: "https://x.com/status/42",
    postId: "42",
    text: "Thanks for sharing this.",
  });
  restore();
  expect(result).toMatchObject({
    state: "submission_succeeded",
    kind: "submission",
  });
  expect(disabledReads).toBeGreaterThan(3);
  expect(submitClicks).toBe(1);
});

test("refuses X poll and thread composers without submitting", async () => {
  const unsupportedParts: ReadonlyArray<
    Readonly<Record<string, readonly FixtureNode[]>>
  > = [
    { '[data-testid="pollOptions"]': [node("")] },
    { '[data-testid="tweetTextarea_1"]': [node("")] },
  ];
  for (const parts of unsupportedParts) {
    let submitClicks = 0;
    const submitButton = node("");
    Object.defineProperty(submitButton, "click", {
      value: () => {
        submitClicks += 1;
      },
    });
    const composer = node("");
    Object.defineProperty(composer, "focus", { value: () => {} });
    makeComposerScope(composer, submitButton, parts);
    const restore = installPage("Compose", "X", "https://x.com/compose/post", {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
    });
    const result = await runXPage({
      action: "submit_post",
      targetUrl: "https://x.com/compose/post",
      text: "Do not send",
      });
    restore();
    expect(result).toMatchObject({
      state: "unsupported",
      message:
        "The visible X composer contains a poll or thread. Open a plain post or reply composer and try again; nothing was submitted.",
    });
    expect(submitClicks).toBe(0);
  }
});

test("does not pair controls across multiple X composers", async () => {
  let activeSubmitClicks = 0;
  const firstEditor = node("");
  const firstSubmitButton = node("", {
    disabled: "",
  });
  makeComposerScope(firstEditor, firstSubmitButton, {
    '[data-testid="pollOptions"]': [node("")],
  });
  const secondEditor = node("");
  const secondSubmitButton = node("");
  Object.defineProperty(secondSubmitButton, "click", {
    value: () => {
      activeSubmitClicks += 1;
    },
  });
  makeComposerScope(secondEditor, secondSubmitButton);
  const restore = installPage("Compose", "X", "https://x.com/compose/post", {
    '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
    '[data-testid="tweetTextarea_0"]': [firstEditor, secondEditor],
    '[data-testid="tweetButtonInline"]:not([disabled]), [data-testid="tweetButton"]:not([disabled])':
      [secondSubmitButton],
  });
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Do not send",
  });
  restore();
  expect(result).toMatchObject({ state: "unsupported" });
  expect(activeSubmitClicks).toBe(0);
});

test("never submits a new X post without a signed-in account or off the compose page", async () => {
  let clicks = 0;
  const submitButton = node("");
  Object.defineProperty(submitButton, "click", {
    value: () => {
      clicks += 1;
    },
  });
  const composer = node("");
  Object.defineProperty(composer, "focus", { value: () => {} });
  const composerParts = {
    '[data-testid="tweetTextarea_0"]': [composer],
    '[data-testid="tweetButtonInline"]:not([disabled]), [data-testid="tweetButton"]:not([disabled])':
      [submitButton],
  };
  const restoreNoAccount = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    composerParts,
  );
  const noAccount = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Do not send",
  });
  restoreNoAccount();
  const restore = installPage("Compose", "X", "https://x.com/compose/post", {
    '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
    ...composerParts,
  });
  const targetMismatch = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post?draft=stale",
    text: "Do not send",
  });
  restore();
  expect(noAccount).toMatchObject({ state: "account_unverified" });
  expect(targetMismatch).toMatchObject({ state: "target_mismatch" });
  expect(clicks).toBe(0);
}, 10_000);

test("reports when the confirmed X post editor does not accept input", async () => {
  let submitClicks = 0;
  const submitButton = node("");
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
    },
  });
  const composer = node("");
  Object.defineProperty(composer, "focus", { value: () => {} });
  Object.defineProperty(composer, "textContent", {
    configurable: true,
    get: () => "",
    set: () => {},
  });
  makeComposerScope(composer, submitButton);
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
    },
    () => true,
  );
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Do not send",
  });
  restore();
  expect(result).toMatchObject({
    state: "unsupported",
    message:
      "X did not accept the post text. Check the visible composer and try again; nothing was submitted.",
  });
  expect(submitClicks).toBe(0);
});

test("pastes the exact text in one event when X ignores execCommand input", async () => {
  let submitClicks = 0;
  const pastedText: string[] = [];
  const submitButton = node("Post");
  const composer = node("");
  const toastLink = node("View", { href: "/owner/status/999" });
  const toast = node(
    "An earlier X notification.",
    {},
    {
      'a[href*="/status/"]': [toastLink],
    },
  );
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: "",
      });
      Object.defineProperty(toast, "textContent", {
        configurable: true,
        value: "Your post was posted.",
      });
    },
  });
  Object.defineProperty(composer, "focus", { value: () => {} });
  Object.defineProperty(composer, "dispatchEvent", {
    value: (event: {
      readonly type?: string;
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly ctrlKey?: boolean;
      readonly shiftKey?: boolean;
      readonly clipboardData?: { getData(format: string): string };
    }) => {
      if (event.type === "paste") {
        const chunk = event.clipboardData?.getData("text/plain") ?? "";
        pastedText.push(chunk);
        Object.defineProperty(composer, "textContent", {
          configurable: true,
          value: `${(composer as { textContent?: string }).textContent ?? ""}${chunk}`,
        });
      }
      if (
        event.type === "keydown" &&
        event.key === "Enter" &&
        event.shiftKey !== true &&
        (event.metaKey === true || event.ctrlKey === true)
      ) {
        fixtureSubmitSink?.();
      }
      return true;
    },
  });
  makeComposerScope(composer, submitButton);
  class FixtureDataTransfer {
    private text = "";

    setData(format: string, value: string): void {
      if (format === "text/plain") {
        this.text = value;
      }
    }

    getData(format: string): string {
      return format === "text/plain" ? this.text : "";
    }
  }
  class FixtureClipboardEvent {
    readonly type: string;
    readonly clipboardData: FixtureDataTransfer;

    constructor(
      type: string,
      init: { readonly clipboardData: FixtureDataTransfer },
    ) {
      this.type = type;
      this.clipboardData = init.clipboardData;
    }
  }
  const globals = globalThis as unknown as Record<string, unknown>;
  const previousDataTransfer = Object.getOwnPropertyDescriptor(
    globalThis,
    "DataTransfer",
  );
  const previousClipboardEvent = Object.getOwnPropertyDescriptor(
    globalThis,
    "ClipboardEvent",
  );
  Object.defineProperty(globals, "DataTransfer", {
    configurable: true,
    value: FixtureDataTransfer,
  });
  Object.defineProperty(globals, "ClipboardEvent", {
    configurable: true,
    value: FixtureClipboardEvent,
  });
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    () => true,
  );
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
  });
  restore();
  if (previousDataTransfer) {
    Object.defineProperty(globalThis, "DataTransfer", previousDataTransfer);
  } else {
    delete globals.DataTransfer;
  }
  if (previousClipboardEvent) {
    Object.defineProperty(globalThis, "ClipboardEvent", previousClipboardEvent);
  } else {
    delete globals.ClipboardEvent;
  }
  expect(result).toMatchObject({
    state: "submission_succeeded",
    kind: "submission",
  });
  expect(pastedText).toEqual(["Hello from Wande"]);
  expect(submitClicks).toBe(1);
});

test("places the caret at the end of an empty X DraftJS block", async () => {
  let submitClicks = 0;
  let insertedText = "";
  let caretNode: FixtureNode | null = null;
  let caretOffset: number | null = null;
  let collapsed: boolean | null = null;
  let rangeCount = 0;
  const submitButton = node("Post");
  const lineBreak = node("");
  Object.defineProperty(lineBreak, "nodeType", { value: 1 });
  Object.defineProperty(lineBreak, "childNodes", { value: [] });
  const leaf = node("", { "data-offset-key": "a-0-0" });
  Object.defineProperty(leaf, "nodeType", { value: 1 });
  Object.defineProperty(leaf, "childNodes", { value: [lineBreak] });
  const composer = node(
    "",
    {},
    {
      "span[data-offset-key]": [leaf],
    },
  );
  Object.defineProperty(composer, "nodeType", { value: 1 });
  Object.defineProperty(composer, "childNodes", { value: [leaf] });
  Object.defineProperty(composer, "focus", { value: () => {} });
  const toastLink = node("View", { href: "/owner/status/999" });
  const toast = node(
    "An earlier X notification.",
    {},
    {
      'a[href*="/status/"]': [toastLink],
    },
  );
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: "",
      });
      Object.defineProperty(toast, "textContent", {
        configurable: true,
        value: "Your post was posted.",
      });
    },
  });
  makeComposerScope(composer, submitButton);
  const range = {
    setStart: (target: FixtureNode, offset: number) => {
      caretNode = target;
      caretOffset = offset;
    },
    collapse: (toStart: boolean) => {
      collapsed = toStart;
    },
    selectNodeContents: () => {},
  };
  const selection = {
    get rangeCount() {
      return rangeCount;
    },
    removeAllRanges: () => {
      rangeCount = 0;
    },
    addRange: () => {
      rangeCount = 1;
    },
  };
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      if (caretNode !== leaf || collapsed !== true || rangeCount !== 1) {
        return false;
      }
      insertedText += value ?? "";
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: insertedText,
      });
      return true;
    },
  );
  const documentValue = globalThis.document as unknown as Record<
    string,
    unknown
  >;
  const windowValue = globalThis.window as unknown as Record<string, unknown>;
  Object.defineProperty(documentValue, "createRange", {
    configurable: true,
    value: () => range,
  });
  Object.defineProperty(windowValue, "getSelection", {
    configurable: true,
    value: () => selection,
  });
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
  });
  restore();
  expect(result).toMatchObject({ state: "submission_succeeded" });
  expect(caretNode === leaf).toBe(true);
  expect(caretOffset === 0).toBe(true);
  expect(collapsed === true).toBe(true);
  expect(submitClicks).toBe(1);
});

test("submits an X post immediately", async () => {
  let submitClicks = 0;
  let insertedText = "";
  const submitButton = node("Post");
  const composer = node("");
  const toastLink = node("View", { href: "/owner/status/999" });
  const toast = node(
    "An earlier X notification.",
    {},
    {
      'a[href*="/status/"]': [toastLink],
    },
  );
  Object.defineProperty(composer, "focus", { value: () => {} });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: "",
      });
      Object.defineProperty(toast, "textContent", {
        configurable: true,
        value: "Your post was posted.",
      });
    },
  });
  makeComposerScope(composer, submitButton);
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      insertedText += value ?? "";
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: insertedText,
      });
      return true;
    },
  );
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
  });
  restore();
  expect(result).toMatchObject({
    state: "submission_succeeded",
    kind: "submission",
    postedId: "999",
    postedUrl: "https://x.com/owner/status/999",
  });
  expect(insertedText).toBe("Hello from Wande");
  expect(submitClicks).toBe(1);
});

test("a thread asks for a real press of the plus, then fills the new editor and posts all", async () => {
  let submitClicks = 0;
  let addClicks = 0;
  const typed: string[] = [];
  let active: FixtureNode | null = null;
  const first = node("");
  const second = node("");
  const submitButton = node("Post all");
  const addButton = node("");
  const toastLink = node("View", { href: "/owner/status/777" });
  const toast = node("An earlier X notification.", {}, { 'a[href*="/status/"]': [toastLink] });
  for (const editor of [first, second]) {
    Object.defineProperty(editor, "focus", {
      value: () => {
        active = editor;
      },
    });
  }
  makeComposerScope(first, submitButton, {
    '[data-testid="addButton"]': [addButton],
  });
  const page: Record<string, readonly FixtureNode[]> = {
    '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
    '[data-testid="tweetTextarea_0"]': [first],
    '[data-testid="addButton"]': [addButton],
    '[role="alert"], [data-testid="toast"]': [toast],
  };
  // The new editor appears in its own block with the toolbar, not inside
  // the first post's box.
  const secondScope = node("", {}, {
    '[data-testid="tweetTextarea_1"]': [second],
    '[data-testid="tweetButtonInline"]': [submitButton],
    '[data-testid="tweetButton"]': [submitButton],
  });
  Object.defineProperty(second, "parentElement", { configurable: true, value: secondScope });
  Object.defineProperty(addButton, "click", {
    value: () => {
      addClicks += 1;
    },
  });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      for (const editor of [first, second]) {
        Object.defineProperty(editor, "textContent", { configurable: true, value: "" });
      }
      Object.defineProperty(toast, "textContent", { configurable: true, value: "Your post was posted." });
    },
  });
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    page,
    (_commandId, _showUi, value) => {
      typed.push(value ?? "");
      if (active) {
        Object.defineProperty(active, "textContent", { configurable: true, value: value ?? "" });
      }
      return true;
    },
  );
  const options = {
    action: "submit_post" as const,
    targetUrl: "https://x.com/compose/post",
    text: "One.\n\nTwo.",
    parts: ["One.", "Two."],
  };
  const paused = await runXPage(options);
  expect(paused).toMatchObject({ state: "waiting", trustedClick: { x: 25, y: 40 } });
  expect(typed).toEqual(["One."]);
  page['[data-testid="tweetTextarea_1"]'] = [second];
  const result = await runXPage(options);
  restore();
  expect(result).toMatchObject({ state: "submission_succeeded", postedId: "777" });
  expect(typed).toEqual(["One.", "Two."]);
  expect(addClicks).toBe(0);
  expect(submitClicks).toBe(1);
});

test("pastes approved images once, then waits rather than pasting again until the preview renders", async () => {
  let submitClicks = 0;
  let insertedText = "";
  let pastedFiles: readonly File[] = [];
  fixtureImagePasteSink = (_editor, files) => {
    pastedFiles = files;
  };
  const submitButton = node("Post");
  const composer = node("");
  const toastLink = node("View", { href: "/owner/status/999" });
  const toast = node("An earlier X notification.", {}, { 'a[href*="/status/"]': [toastLink] });
  Object.defineProperty(composer, "focus", { value: () => {} });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      Object.defineProperty(composer, "textContent", { configurable: true, value: "" });
      Object.defineProperty(toast, "textContent", { configurable: true, value: "Your post was posted." });
    },
  });
  const { scope } = makeComposerScope(composer, submitButton);
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      insertedText += value ?? "";
      Object.defineProperty(composer, "textContent", { configurable: true, value: insertedText });
      return true;
    },
  );
  const options = {
    action: "submit_post" as const,
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
    partImages: [[{ data: "aGVsbG8=", contentType: "image/png" }]],
  };

  const first = await runXPage(options);
  expect(first).toMatchObject({ state: "waiting" });
  expect(pastedFiles).toHaveLength(1);
  expect(pastedFiles[0]?.type).toBe("image/png");
  expect(submitClicks).toBe(0);

  // The paste already happened, but X has not rendered a preview yet:
  // pasting again would risk attaching the same image twice, so the driver
  // only waits.
  pastedFiles = [];
  const second = await runXPage(options);
  expect(second).toMatchObject({ state: "waiting" });
  expect(pastedFiles).toHaveLength(0);
  expect(submitClicks).toBe(0);

  // The preview has rendered: the submission proceeds and sends once.
  const preview = node("");
  (scope.selectors as Record<string, readonly FixtureNode[]>)['[data-testid="attachments"] img'] = [preview];
  const result = await runXPage(options);
  restore();
  expect(result).toMatchObject({ state: "submission_succeeded" });
  expect(submitClicks).toBe(1);
});

test("refuses to submit when the composer already carries media Pluk did not approve", async () => {
  let submitClicks = 0;
  const submitButton = node("Post");
  const composer = node("");
  Object.defineProperty(composer, "focus", { value: () => {} });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
    },
  });
  const existingMedia = node("");
  makeComposerScope(composer, submitButton, {
    '[data-testid="attachments"] img': [existingMedia],
  });
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
    },
    (_commandId, _showUi, value) => {
      Object.defineProperty(composer, "textContent", { configurable: true, value: value ?? "" });
      return true;
    },
  );
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
  });
  restore();
  expect(result).toMatchObject({
    state: "unsupported",
    message:
      "The composer already has media Pluk did not attach. Remove it and try again; nothing was submitted.",
  });
  expect(submitClicks).toBe(0);
});

test("a thread pastes each part's own distinct images, never twice, with an image-free part in between", async () => {
  let submitClicks = 0;
  const typed: string[] = [];
  let active: FixtureNode | null = null;
  const pastesByEditor = new Map<FixtureNode, readonly File[]>();
  fixtureImagePasteSink = (editor, files) => {
    pastesByEditor.set(editor, files);
  };
  const first = node("");
  const second = node("");
  const third = node("");
  const submitButton = node("Post all");
  const toastLink = node("View", { href: "/owner/status/777" });
  const toast = node("An earlier X notification.", {}, { 'a[href*="/status/"]': [toastLink] });
  for (const editor of [first, second, third]) {
    Object.defineProperty(editor, "focus", { value: () => (active = editor) });
  }
  const { scope: firstScope } = makeComposerScope(first, submitButton);
  // All three post boxes are already open and reachable: this test is about
  // each part's own images, not re-proving the "+" choreography already
  // covered by the plain-thread test above.
  const secondScope = node("", {}, {
    '[data-testid="tweetTextarea_1"]': [second],
    '[data-testid="tweetButtonInline"]': [submitButton],
    '[data-testid="tweetButton"]': [submitButton],
  });
  Object.defineProperty(second, "parentElement", { configurable: true, value: secondScope });
  const thirdScope = node("", {}, {
    '[data-testid="tweetTextarea_2"]': [third],
    '[data-testid="tweetButtonInline"]': [submitButton],
    '[data-testid="tweetButton"]': [submitButton],
  });
  Object.defineProperty(third, "parentElement", { configurable: true, value: thirdScope });
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
      for (const editor of [first, second, third]) {
        Object.defineProperty(editor, "textContent", { configurable: true, value: "" });
      }
      Object.defineProperty(toast, "textContent", { configurable: true, value: "Your post was posted." });
    },
  });
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [first],
      '[data-testid="tweetTextarea_1"]': [second],
      '[data-testid="tweetTextarea_2"]': [third],
      '[role="alert"], [data-testid="toast"]': [toast],
    },
    (_commandId, _showUi, value) => {
      typed.push(value ?? "");
      if (active) {
        Object.defineProperty(active, "textContent", { configurable: true, value: value ?? "" });
      }
      return true;
    },
  );
  const options = {
    action: "submit_post" as const,
    targetUrl: "https://x.com/compose/post",
    text: "One.\n\nTwo.\n\nThree.",
    parts: ["One.", "Two.", "Three."],
    partImages: [
      [{ data: "YQ==", contentType: "image/png" }],
      [],
      [
        { data: "YjE=", contentType: "image/png" },
        { data: "YjI=", contentType: "image/png" },
      ],
    ],
  };

  // The first post's own image is pasted first.
  const pausedFirst = await runXPage(options);
  expect(pausedFirst).toMatchObject({ state: "waiting" });
  expect(pastesByEditor.get(first)).toHaveLength(1);
  expect(pastesByEditor.has(second)).toBe(false);
  expect(pastesByEditor.has(third)).toBe(false);
  expect(typed).toEqual(["One."]);

  // Its preview has rendered: the thread types the image-free middle post,
  // then reaches the third and pastes its own, different images — never the
  // first post's images a second time.
  (firstScope.selectors as Record<string, readonly FixtureNode[]>)['[data-testid="attachments"] img'] = [node("")];
  const pausedThird = await runXPage(options);
  expect(pausedThird).toMatchObject({ state: "waiting" });
  expect(pastesByEditor.get(third)).toHaveLength(2);
  expect(pastesByEditor.has(second)).toBe(false);
  expect(typed).toEqual(["One.", "Two.", "Three."]);

  // Still no preview on the third post: asking again must not paste its
  // files a second time.
  pastesByEditor.delete(third);
  const stillWaitingOnThird = await runXPage(options);
  expect(stillWaitingOnThird).toMatchObject({ state: "waiting" });
  expect(pastesByEditor.has(third)).toBe(false);

  (thirdScope.selectors as Record<string, readonly FixtureNode[]>)['[data-testid="attachments"] img'] = [
    node(""),
    node(""),
  ];
  const result = await runXPage(options);
  restore();
  expect(result).toMatchObject({ state: "submission_succeeded", postedId: "777" });
  expect(typed).toEqual(["One.", "Two.", "Three."]);
  expect(submitClicks).toBe(1);
});

test("exposes an uncertain outcome instead of claiming success when no post identity is visible", async () => {
  let submitClicks = 0;
  let submitShortcuts = 0;
  const submitButton = node("Post");
  const composer = node("");
  Object.defineProperty(composer, "focus", { value: () => {} });
  const staleToast = node("Your post was posted.");
  Object.defineProperty(submitButton, "click", {
    value: () => {
      submitClicks += 1;
    },
  });
  makeComposerScope(composer, submitButton);
  Object.defineProperty(composer, "dispatchEvent", {
    value: (event: {
      readonly type?: string;
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly clipboardData?: { getData(format: string): string };
    }) => {
      if (event.type === "paste") {
        const pasted = event.clipboardData?.getData("text/plain") ?? "";
        Object.defineProperty(composer, "textContent", {
          configurable: true,
          value: `${(composer as { textContent?: string }).textContent ?? ""}${pasted}`,
        });
      }
      if (event.type === "keydown" && event.key === "Enter" && event.metaKey) {
        submitShortcuts += 1;
      }
      return true;
    },
  });
  const restore = installPage(
    "Compose",
    "X",
    "https://x.com/compose/post",
    {
      '[data-testid="AppTabBar_Profile_Link"]': [node("", { href: "/owner" })],
      '[data-testid="tweetTextarea_0"]': [composer],
      '[role="alert"], [data-testid="toast"]': [staleToast],
    },
    (_commandId, _showUi, value) => {
      Object.defineProperty(composer, "textContent", {
        configurable: true,
        value: `${(composer as { textContent?: string }).textContent ?? ""}${value ?? ""}`,
      });
      return true;
    },
  );
  const result = await runXPage({
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    text: "Hello from Wande",
  });
  restore();
  expect(result).toMatchObject({ state: "submission_unknown" });
  expect(submitShortcuts).toBe(1);
  expect(submitClicks).toBe(1);
}, 20_000);

const instagramOgTitle = (username: string): FixtureNode =>
  node("", { content: `${username} on Instagram` });

test("reads an Instagram post with no comments", async () => {
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    kind: "instagram_post",
    comments: [],
    truncated: false,
  });
});

// Instagram returns every comment, top level or reply, in the same shape;
// only a reply carries `parent_comment_id`.
function instagramCommentNode(
  pk: string,
  text: string,
  username: string,
  extra: Record<string, unknown> = {},
): unknown {
  return {
    __typename: "XDTCommentDict",
    pk,
    text,
    created_at: 1_700_000_000,
    comment_like_count: 7,
    child_comment_count: 0,
    parent_comment_id: null,
    user: { username, is_verified: false },
    ...extra,
  };
}

function instagramCommentsBody(nodes: readonly unknown[]): unknown {
  return {
    data: {
      xdt_api__v1__media__media_id__comments__connection: {
        edges: nodes.map((node_) => ({ node: node_ })),
        page_info: { end_cursor: null, has_next_page: false },
      },
    },
  };
}

test("reads Instagram comments and their replies from captured responses", async () => {
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    { 'meta[property="og:title"]': [instagramOgTitle("janedoe")] },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([
        instagramCommentsBody([
          instagramCommentNode("111", "Comment one", "alice", {
            child_comment_count: 1,
          }),
          instagramCommentNode("222", "Comment two", "bob"),
        ]),
        {
          data: {
            xdt_api__v1__media__media_id__comments__parent_comment_id__child_comments__connection:
              {
                edges: [
                  {
                    node: instagramCommentNode("333", "A reply", "carol", {
                      parent_comment_id: "111",
                    }),
                  },
                ],
                page_info: { end_cursor: null, has_next_page: false },
              },
          },
        },
      ]),
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: false });
  const comments = (
    result as unknown as {
      comments: ReadonlyArray<{
        commentId: string;
        author: string;
        text: string;
        likes: number | null;
        postedAt: string | null;
        replies: ReadonlyArray<{ author: string; text: string }>;
      }>;
    }
  ).comments;
  expect(comments).toHaveLength(2);
  expect(comments[0]).toMatchObject({
    commentId: "111",
    author: "alice",
    text: "Comment one",
    likes: 7,
    postedAt: "2023-11-14T22:13:20.000Z",
  });
  expect(comments[0]?.replies).toEqual([
    expect.objectContaining({ author: "carol", text: "A reply" }),
  ]);
  expect(comments[1]?.replies).toEqual([]);
});

test("stops loading Instagram comments at the 300 cap and marks the result truncated", async () => {
  const nodes = Array.from({ length: 400 }, (_, index) =>
    instagramCommentNode(String(index), `Comment ${index}`, "alice"),
  );
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    { 'meta[property="og:title"]': [instagramOgTitle("janedoe")] },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([
        instagramCommentsBody(nodes),
      ]),
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: true });
  expect((result as unknown as { comments: unknown[] }).comments).toHaveLength(300);
});

function instagramCountSpan(
  abbreviated: string,
  exact: string,
  suffix: string,
): FixtureNode {
  const inner = node(abbreviated, { title: exact });
  return node(`${abbreviated}${suffix}`, { dir: "auto" }, { "span[title]": [inner] });
}

function instagramBioSpan(lines: readonly string[]): FixtureNode {
  const children: FixtureNode[] = [];
  lines.forEach((line, index) => {
    if (index > 0) {
      const br = node("");
      Object.defineProperty(br, "tagName", { value: "BR" });
      children.push(br);
    }
    const text = node(line);
    Object.defineProperty(text, "nodeType", { value: 3 });
    children.push(text);
  });
  const bio = node(lines.join(""), { dir: "auto" });
  Object.defineProperty(bio, "childNodes", { value: children });
  return bio;
}

function instagramGridAnchor(
  username: string,
  kind: "p" | "reel",
  shortcode: string,
  caption: string,
  options: { readonly clip?: boolean; readonly pinned?: boolean } = {},
): FixtureNode {
  const img = node("", { alt: caption });
  const svgSelectors: Record<string, FixtureNode[]> = {};
  if (options.clip) {
    svgSelectors['svg[aria-label="Clip"]'] = [node("")];
  }
  if (options.pinned) {
    svgSelectors['svg[aria-label="Pinned post icon"]'] = [node("")];
  }
  return node(
    "",
    { href: `/${username}/${kind}/${shortcode}/` },
    {
      "div._aagu > div._aagv img": [img],
      ...svgSelectors,
    },
  );
}

test("reads a full Instagram profile header with posts, counts, bio line breaks, and an external link", async () => {
  const displayNameSpan = node("Michael  Rapheal 🙂⭐️", { dir: "auto" });
  const categoryDiv = node("Public figure");
  const bioSpan = instagramBioSpan(["Coffee first.", "Then the world."]);
  const externalLinkAnchor = node("example.com/profile", {
    href: "https://l.instagram.com/?u=https%3A%2F%2Fexample.com%2Fprofile",
  });
  const postsSpan = instagramCountSpan("60.5K", "60,599", " posts");
  const followersSpan = instagramCountSpan("1.2M", "1,204,552", " followers");
  const followingSpan = instagramCountSpan("180", "180", " following");
  const gridAnchor = instagramGridAnchor(
    "onenigaofficial1",
    "p",
    "CxYz_1-2Ab",
    "Sunset walk\n#golden #hour",
  );
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/onenigaofficial1/",
    {
      'header span[dir="auto"]': [
        bioSpan,
        displayNameSpan,
        postsSpan,
        followersSpan,
        followingSpan,
      ],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ap3a._aaco._aacu._aacy": [categoryDiv],
      'a[href^="https://l.instagram.com/?u="]': [externalLinkAnchor],
      "div._ac7v a[href]": [gridAnchor],
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/onenigaofficial1/",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    kind: "instagram_profile",
    canonicalTarget: "https://www.instagram.com/onenigaofficial1/",
    handle: "onenigaofficial1",
    displayName: "Michael  Rapheal 🙂⭐️",
    category: "Public figure",
    bio: "Coffee first.\nThen the world.",
    externalLink: "example.com/profile",
    counts: {
      posts: { exact: "60,599", label: "60.5K" },
      followers: { exact: "1,204,552", label: "1.2M" },
      following: { exact: "180", label: "180" },
    },
    truncated: false,
  });
  expect((result as unknown as { posts: unknown[] }).posts).toEqual([
    {
      shortcode: "CxYz_1-2Ab",
      canonicalTarget:
        "https://www.instagram.com/onenigaofficial1/p/CxYz_1-2Ab/",
      caption: "Sunset walk\n#golden #hour",
      kind: "post",
      pinned: false,
      postedAt: null,
      likes: null,
      views: null,
      commentCount: null,
    },
  ]);
});

test("reads an Instagram profile with no category and no external link", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Just one line."]);
  const postsSpan = instagramCountSpan("12", "12", " posts");
  const followersSpan = instagramCountSpan("340", "340", " followers");
  const followingSpan = instagramCountSpan("50", "50", " following");
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [
        displayNameSpan,
        postsSpan,
        followersSpan,
        followingSpan,
      ],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [],
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    kind: "instagram_profile",
    handle: "janedoe",
    displayName: "Jane Doe",
    category: null,
    bio: "Just one line.",
    externalLink: null,
    truncated: false,
  });
  expect((result as unknown as { posts: unknown[] }).posts).toEqual([]);
});

function instagramTimelineCaptureBody(shortcode: string): unknown {
  return {
    data: {
      edges: [
        {
          node: {
            __typename: "XDTMediaDict",
            code: shortcode,
            taken_at: 1_700_000_000,
          },
        },
      ],
    },
  };
}

test("accumulates an Instagram grid across two scrolls from newly landed timeline captures", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const capturesState: Record<string, string> = {
    "pluk-instagram-captures": instagramCaptureElement([]),
  };
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [],
    },
    undefined,
    capturesState,
  );
  let scrollCalls = 0;
  Object.defineProperty(globalThis.window, "scrollTo", {
    configurable: true,
    value: () => {
      scrollCalls += 1;
      if (scrollCalls === 1) {
        capturesState["pluk-instagram-captures"] = instagramCaptureElement([
          instagramTimelineCaptureBody("First001"),
        ]);
      } else if (scrollCalls === 2) {
        capturesState["pluk-instagram-captures"] = instagramCaptureElement([
          instagramTimelineCaptureBody("First001"),
          instagramTimelineCaptureBody("Second002"),
        ]);
        Object.defineProperty(globalThis.window, "scrollTo", {
          configurable: true,
          value: undefined,
        });
      }
    },
  });
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(scrollCalls).toBe(2);
  expect(result).toMatchObject({ state: "ready", truncated: false });
  expect(
    (result as unknown as { posts: ReadonlyArray<{ shortcode: string }> }).posts.map(
      (post) => post.shortcode,
    ),
  ).toEqual(["First001", "Second002"]);
});

test("marks a pinned Instagram reel grid entry", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const pinnedReel = instagramGridAnchor(
    "janedoe",
    "reel",
    "Pin001",
    "Pinned reel",
    { clip: true, pinned: true },
  );
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [pinnedReel],
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect((result as unknown as { posts: unknown[] }).posts).toEqual([
    {
      shortcode: "Pin001",
      canonicalTarget: "https://www.instagram.com/janedoe/reel/Pin001/",
      caption: "Pinned reel",
      kind: "reel",
      pinned: true,
      postedAt: null,
      likes: null,
      views: null,
      commentCount: null,
    },
  ]);
});

test("caps an accumulated Instagram grid at 600 entries and marks it truncated", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const edges = Array.from({ length: 601 }, (_, index) => ({
    node: {
      __typename: "XIGPolarisCarouselMedia",
      code: `Shortcode${index}`,
      taken_at: 1_700_000_000,
    },
  }));
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [],
    },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([{ data: { edges } }]),
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: true });
  expect((result as unknown as { posts: unknown[] }).posts).toHaveLength(600);
});

function instagramCaptureElement(bodies: readonly unknown[]): string {
  return JSON.stringify(
    bodies.map((body) => ({
      url: "https://www.instagram.com/graphql/query",
      method: "POST",
      receivedAt: Date.now(),
      body,
    })),
  );
}

test("reads exact counts, verified state, and multiple links from a captured Instagram profile response", async () => {
  const displayNameSpan = node("On E. Niga", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const gridAnchor = instagramGridAnchor(
    "onenigaofficial1",
    "p",
    "CxYz_1-2Ab",
    "Sunset walk",
  );
  const capturedProfileBody = {
    data: {
      user: {
        username: "onenigaofficial1",
        follower_count: 148_449_900,
        following_count: 115,
        media_count: 27_942,
        is_verified: true,
        is_private: false,
        bio_links: [
          {
            title: "Shop",
            link_type: "external",
            is_pinned: true,
            link_id: "1",
            lynx_url:
              "https://l.instagram.com/?u=https%3A%2F%2Fexample.com%2Fshop&e=AT0",
          },
          {
            title: "Site",
            link_type: "external",
            is_pinned: false,
            link_id: "2",
            lynx_url: "https://l.instagram.com/?u=https%3A%2F%2Fexample.com&e=AT1",
          },
        ],
      },
    },
  };
  const capturedGridBody = {
    data: {
      xdt_api__v1__feed__user_timeline_graphql_connection: {
        edges: [
          {
            node: {
              __typename: "XDTMediaDict",
              code: "CxYz_1-2Ab",
              taken_at: 1_700_000_000,
              like_count: 4_200,
              comment_count: 31,
            },
          },
        ],
      },
    },
  };
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/onenigaofficial1/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [gridAnchor],
    },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([
        capturedProfileBody,
        capturedGridBody,
      ]),
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/onenigaofficial1/",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    source: "mixed",
    followers: 148_449_900,
    following: 115,
    postsCount: 27_942,
    verified: true,
    private: false,
    links: [
      { label: "Shop", url: "https://example.com/shop" },
      { label: "Site", url: "https://example.com" },
    ],
  });
  expect((result as unknown as { posts: [{ likes: number; commentCount: number }] }).posts).toEqual([
    expect.objectContaining({ likes: 4_200, commentCount: 31 }),
  ]);
});

test("falls back to scraping when the Instagram capture element is absent", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const externalLinkAnchor = node("example.com/profile", {
    href: "https://l.instagram.com/?u=https%3A%2F%2Fexample.com%2Fprofile",
  });
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      'a[href^="https://l.instagram.com/?u="]': [externalLinkAnchor],
      "div._ac7v a[href]": [],
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    source: "scraped",
    followers: null,
    following: null,
    postsCount: null,
    verified: null,
    private: null,
    externalLink: "example.com/profile",
    links: [{ label: "example.com/profile", url: "https://example.com/profile" }],
  });
});

test("reads a captured Instagram business address without treating it as the category", async () => {
  const displayNameSpan = node("Man City", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const capturedProfileBody = {
    data: {
      user: {
        username: "mancity",
        follower_count: 57_008_870,
        following_count: 807,
        media_count: 44_371,
        is_verified: true,
        is_private: false,
        category: "",
        address_street: "Etihad Stadium",
        city_name: "Manchester, United Kingdom",
        zip: "M11 3FF",
      },
    },
  };
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/mancity/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [],
    },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([capturedProfileBody]),
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/mancity/",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    category: null,
    address: "Etihad Stadium, Manchester, United Kingdom, M11 3FF",
  });
});

test("returns a multi-link profile's whole list instead of the single-link shape", async () => {
  const displayNameSpan = node("Man City", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const multiLinkButton = node("bio.mancity.com and 1 more");
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/mancity/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      button: [multiLinkButton],
      "div._ac7v a[href]": [],
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/mancity/",
  });
  restore();
  expect(
    (result as unknown as { links: readonly unknown[] }).links,
  ).toEqual([{ label: "bio.mancity.com and 1 more", url: null }]);
});

test("reads captured likes, views, and comment count for an Instagram post and its comments", async () => {
  const capturedPost = {
    data: {
      xdt_shortcode_media: {
        code: "ABC123",
        __typename: "XIGPolarisClipsMedia",
        owner: { username: "mancity" },
        like_count: 500,
        comment_count: 12,
        view_count: 9_999,
      },
      xdt_api__v1__media__media_id__comments__connection: {
        edges: [
          {
            node: instagramCommentNode("c1", "Comment one", "alice", {
              comment_like_count: 7,
            }),
          },
        ],
      },
    },
  };
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
    },
    undefined,
    { "pluk-instagram-captures": instagramCaptureElement([capturedPost]) },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    source: "captured",
    post: { author: "mancity", likes: 500, views: 9_999, commentCount: 12 },
  });
  expect(
    (result as unknown as { comments: ReadonlyArray<{ likes: number }> }).comments,
  ).toEqual([expect.objectContaining({ likes: 7, author: "alice" })]);
});

test("reads an Instagram post from JSON embedded in the page's own HTML when nothing was captured", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const embeddedScript = node(
    JSON.stringify({
      require: [
        {
          data: {
            xdt_shortcode_media: {
              code: "ABC123",
              __typename: "XIGPolarisVideoMedia",
              owner: { username: "mancity" },
              like_count: 300,
              comment_count: 8,
              view_count: 4_444,
            },
          },
        },
      ],
    }),
  );
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("someoneelse")],
      'script[type="application/json"]': [embeddedScript],
      ul: [commentList],
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    source: "embedded",
    post: { author: "mancity", likes: 300, views: 4_444, commentCount: 8 },
  });
});

test("falls back to og:description for an exact comment count without turning an abbreviated like count into an integer", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      'meta[property="og:description"]': [
        node("", {
          content:
            '136K likes, 1,638 comments - janedoe on September 13, 2026: "Great day at the match"',
        }),
      ],
      ul: [commentList],
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    source: "scraped",
    post: {
      commentCount: 1_638,
      likes: null,
      engagement: "136K likes, 1,638 comments",
      text: "Great day at the match",
    },
  });
});

test("reads the posting account's handle straight from the post URL when nothing else names an author", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/mancity/reel/DdPfl0kOUKy/",
    { ul: [commentList] },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/mancity/reel/DdPfl0kOUKy/",
    postId: "DdPfl0kOUKy",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", post: { author: "mancity" } });
});

test("triggers Instagram to fetch comments by scrolling the panel into view when none have rendered yet", async () => {
  const commentRow = (text: string): FixtureNode =>
    node(text, {}, { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] });
  const listSelectors: Record<string, FixtureNode[]> = { li: [], ":scope > li": [] };
  const commentList = node("", {}, listSelectors);
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      ul: [commentList],
    },
  );
  Object.defineProperty(globalThis.window, "scrollTo", {
    configurable: true,
    value: () => {
      const c1 = commentRow("Comment one");
      listSelectors.li = [c1];
      listSelectors[":scope > li"] = [c1];
    },
  });
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect((result as unknown as { comments: unknown[] }).comments).toHaveLength(1);
});

test("explains a debug read_post result when nothing at all was captured", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      ul: [commentList],
    },
    undefined,
    { "pluk-instagram-captures": instagramCaptureElement([]) },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
    debug: true,
  });
  restore();
  expect(result).toMatchObject({
    state: "ready",
    debugCaptures: JSON.stringify({
      requested: true,
      matched: [],
      note: "No captured response matched this request.",
      recordedUrls: [],
    }),
  });
});

test("attaches every captured response to a debug read_post result when debug is true", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const graphqlEntry = {
    url: "https://www.instagram.com/graphql/query",
    method: "POST",
    receivedAt: 1,
    body: { data: {} },
  };
  const likersEntry = {
    url: "https://www.instagram.com/api/v1/media/likers/",
    method: "GET",
    receivedAt: 2,
    body: {},
  };
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      ul: [commentList],
    },
    undefined,
    { "pluk-instagram-captures": JSON.stringify([graphqlEntry, likersEntry]) },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
    debug: true,
  });
  restore();
  const parsed = JSON.parse(
    (result as unknown as { debugCaptures: string }).debugCaptures,
  );
  expect(parsed).toEqual({ requested: true, matched: [graphqlEntry, likersEntry] });
});

test("attaches only the captured responses matching a debug glob", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const graphqlEntry = {
    url: "https://www.instagram.com/graphql/query",
    method: "POST",
    receivedAt: 1,
    body: { data: {} },
  };
  const likersEntry = {
    url: "https://www.instagram.com/api/v1/media/likers/",
    method: "GET",
    receivedAt: 2,
    body: {},
  };
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      ul: [commentList],
    },
    undefined,
    { "pluk-instagram-captures": JSON.stringify([graphqlEntry, likersEntry]) },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
    debug: "*/graphql*",
  });
  restore();
  const parsed = JSON.parse(
    (result as unknown as { debugCaptures: string }).debugCaptures,
  );
  expect(parsed).toEqual({ requested: "*/graphql*", matched: [graphqlEntry] });
});

test("explains a debug glob that matched nothing instead of attaching an empty blob", async () => {
  const commentRow = node(
    "Comment one",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const graphqlEntry = {
    url: "https://www.instagram.com/graphql/query",
    method: "POST",
    receivedAt: 1,
    body: { data: {} },
  };
  const likersEntry = {
    url: "https://www.instagram.com/api/v1/media/likers/",
    method: "GET",
    receivedAt: 2,
    body: {},
  };
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    {
      'meta[property="og:title"]': [instagramOgTitle("janedoe")],
      ul: [commentList],
    },
    undefined,
    { "pluk-instagram-captures": JSON.stringify([graphqlEntry, likersEntry]) },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
    debug: "*/reels_media*",
  });
  restore();
  const parsed = JSON.parse(
    (result as unknown as { debugCaptures: string }).debugCaptures,
  );
  expect(parsed).toEqual({
    requested: "*/reels_media*",
    matched: [],
    note: "No captured response matched this request.",
    recordedUrls: [graphqlEntry.url, likersEntry.url],
  });
});

test("reports an Instagram grid run that stalls before the cap or deadline as truncated", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": [],
    },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([
        instagramTimelineCaptureBody("First001"),
      ]),
    },
  );
  Object.defineProperty(globalThis.window, "scrollTo", {
    configurable: true,
    value: () => {
      // No new response ever lands: simulates a fetch that never resolves.
    },
  });
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: true });
  expect((result as unknown as { posts: unknown[] }).posts).toHaveLength(1);
}, 8_000);

// Chrome injects a page script by its source text, so anything it reads from
// module scope is gone by the time it runs. Evaluating each script in
// isolation is the only check that catches that.
test.each([
  ["x", runXPage],
  ["instagram", runInstagramPage],
])("the %s page script is self-contained", async (_platform, script) => {
  const isolated = new Function(`return (${script.toString()})`)() as (
    options: unknown,
  ) => unknown;
  const restore = installPage("", "", "https://example.com/someone/", {});
  try {
    await isolated({
      action: "read_profile",
      targetUrl: "https://example.com/someone/",
      username: "someone",
    });
  } catch (error) {
    expect((error as Error).message).not.toMatch(/is not defined/u);
  } finally {
    restore();
  }
});

test("marks an Instagram comment read truncated when the post has more comments than were loaded", async () => {
  const restore = installPage(
    "",
    "Post",
    "https://www.instagram.com/p/ABC123/",
    { 'meta[property="og:title"]': [instagramOgTitle("mancity")] },
    undefined,
    {
      "pluk-instagram-captures": instagramCaptureElement([
        {
          data: {
            xdt_shortcode_media: {
              code: "ABC123",
              owner: { username: "mancity" },
              like_count: 10,
              comment_count: 1_290,
            },
          },
        },
        instagramCommentsBody([
          instagramCommentNode("1", "Only one", "alice"),
        ]),
      ]),
    },
  );
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: true });
  expect((result as unknown as { comments: unknown[] }).comments).toHaveLength(1);
});
