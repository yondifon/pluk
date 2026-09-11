import { expect, test } from "bun:test";
import { runXPage } from "./drivers/x";

interface FixtureNode {
  readonly textContent: string;
  readonly attributes: Readonly<Record<string, string>>;
  readonly selectors: Readonly<Record<string, readonly FixtureNode[]>>;
  querySelector(selector: string): FixtureNode | null;
  querySelectorAll(selector: string): readonly FixtureNode[];
  getAttribute(name: string): string | null;
  dispatchEvent(event: {
    readonly type?: string;
    readonly key?: string;
    readonly metaKey?: boolean;
    readonly ctrlKey?: boolean;
    readonly clipboardData?: { getData(format: string): string };
  }): boolean;
}

let fixturePasteSink: ((text: string) => boolean) | null = null;
let fixtureSubmitSink: (() => void) | null = null;

class HarnessKeyboardEvent {
  readonly type: string;
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;

  constructor(
    type: string,
    init: {
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly ctrlKey?: boolean;
    },
  ) {
    this.type = type;
    this.key = init.key ?? "";
    this.metaKey = init.metaKey ?? false;
    this.ctrlKey = init.ctrlKey ?? false;
  }
}

class HarnessDataTransfer {
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
    dispatchEvent(event: {
      readonly type?: string;
      readonly key?: string;
      readonly metaKey?: boolean;
      readonly ctrlKey?: boolean;
      readonly clipboardData?: { getData(format: string): string };
    }) {
      if (event.type === "paste" && fixturePasteSink) {
        fixturePasteSink(event.clipboardData?.getData("text/plain") ?? "");
      }
      if (
        event.type === "keydown" &&
        event.key === "Enter" &&
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

test("submits an exact X reply exactly once after explicit confirmation", async () => {
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
  expect(replyClicks).toBe(1);
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
