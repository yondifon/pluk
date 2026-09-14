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
      readonly clipboardData?: { getData(format: string): string };
    }) {
      if (event.type === "paste" && fixturePasteSink) {
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

test("loads two pages of Instagram comments before the Load More button disappears", async () => {
  const commentRow = (text: string): FixtureNode =>
    node(text, {}, { "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })] });
  const c1 = commentRow("Comment one");
  const loadMoreButton = node(
    "",
    {},
    { 'svg[aria-label="Load more comments"]': [node("")] },
  );
  const listSelectors: Record<string, FixtureNode[]> = {
    li: [c1],
    ":scope > li": [c1],
    button: [loadMoreButton],
  };
  let clicks = 0;
  Object.defineProperty(loadMoreButton, "click", {
    value: () => {
      clicks += 1;
      if (clicks === 1) {
        const c2 = commentRow("Comment two");
        listSelectors.li = [c1, c2];
        listSelectors[":scope > li"] = [c1, c2];
      } else {
        const c3 = commentRow("Comment three");
        listSelectors.li = [...listSelectors.li, c3];
        listSelectors[":scope > li"] = [...listSelectors[":scope > li"], c3];
        delete listSelectors.button;
      }
    },
  });
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
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(clicks).toBe(2);
  expect(result).toMatchObject({ state: "ready", truncated: false });
  expect((result as unknown as { comments: unknown[] }).comments).toHaveLength(3);
});

test("expands a collapsed Instagram reply thread", async () => {
  const replyLi = node(
    "Reply text",
    {},
    { "time[datetime]": [node("", { datetime: "2024-01-01T00:05:00Z" })] },
  );
  const replyUlSelectors: Record<string, FixtureNode[]> = { ":scope > li": [] };
  const replyUl = node("", {}, replyUlSelectors);
  const c1 = node(
    "Comment one",
    {},
    {
      "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })],
      "ul._a9ym": [replyUl],
    },
  );
  const listSelectors: Record<string, FixtureNode[]> = {
    li: [c1],
    ":scope > li": [c1],
  };
  const toggleButton = node("");
  const toggleSpan = node("View replies (1)");
  Object.defineProperty(toggleSpan, "closest", {
    value: (selector: string) => (selector === "button" ? toggleButton : null),
  });
  Object.defineProperty(toggleButton, "click", {
    value: () => {
      Object.defineProperty(toggleSpan, "textContent", {
        configurable: true,
        value: "Hide replies",
      });
      replyUlSelectors[":scope > li"] = [replyLi];
      listSelectors.li = [c1, replyLi];
    },
  });
  listSelectors["span._a9yi"] = [toggleSpan];
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
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  const comments = (
    result as unknown as { comments: ReadonlyArray<{ replies: unknown[] }> }
  ).comments;
  expect(comments).toHaveLength(1);
  expect(comments[0]?.replies).toHaveLength(1);
  expect(result).toMatchObject({ state: "ready", truncated: false });
});

test("stops loading Instagram comments at the 300 cap and marks the result truncated", async () => {
  const makeBatch = (count: number, offset: number): FixtureNode[] =>
    Array.from({ length: count }, (_, index) =>
      node(`Comment ${offset + index}`, {}, {
        "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })],
      }),
    );
  const initial = makeBatch(100, 0);
  const listSelectors: Record<string, FixtureNode[]> = {
    li: initial,
    ":scope > li": initial,
    button: [
      node(
        "",
        {},
        { 'svg[aria-label="Load more comments"]': [node("")] },
      ),
    ],
  };
  let clicks = 0;
  Object.defineProperty(listSelectors.button[0], "click", {
    value: () => {
      clicks += 1;
      const next = makeBatch(100, listSelectors.li.length);
      listSelectors.li = [...listSelectors.li, ...next];
      listSelectors[":scope > li"] = listSelectors.li;
    },
  });
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
  const result = await runInstagramPage({
    action: "read_post",
    targetUrl: "https://www.instagram.com/p/ABC123/",
    postId: "ABC123",
  });
  restore();
  expect(clicks).toBe(2);
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

test("loads an Instagram grid across two scrolls before it settles", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const anchors: FixtureNode[] = [
    instagramGridAnchor("janedoe", "p", "First001", "First"),
  ];
  const pageSelectors: Record<string, FixtureNode[]> = {
    'header span[dir="auto"]': [displayNameSpan],
    'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
    "div._ac7v a[href]": anchors,
  };
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    pageSelectors,
  );
  let scrollCalls = 0;
  Object.defineProperty(globalThis.window, "scrollTo", {
    configurable: true,
    value: () => {
      scrollCalls += 1;
      if (scrollCalls === 1) {
        anchors.push(instagramGridAnchor("janedoe", "p", "Second002", "Second"));
        pageSelectors["div._ac7v a[href]"] = [...anchors];
      } else if (scrollCalls === 2) {
        anchors.push(instagramGridAnchor("janedoe", "reel", "Third003", "Third"));
        pageSelectors["div._ac7v a[href]"] = [...anchors];
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
  expect((result as unknown as { posts: unknown[] }).posts).toHaveLength(3);
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
      likes: null,
      views: null,
      commentCount: null,
    },
  ]);
});

test("caps an Instagram grid at 120 entries and marks it truncated", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const anchors = Array.from({ length: 130 }, (_, index) =>
    instagramGridAnchor("janedoe", "p", `Shortcode${index}`, `Caption ${index}`),
  );
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    {
      'header span[dir="auto"]': [displayNameSpan],
      'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
      "div._ac7v a[href]": anchors,
    },
  );
  const result = await runInstagramPage({
    action: "read_profile",
    targetUrl: "https://www.instagram.com/janedoe/",
  });
  restore();
  expect(result).toMatchObject({ state: "ready", truncated: true });
  expect((result as unknown as { posts: unknown[] }).posts).toHaveLength(120);
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
              __typename: "XIGPolarisCarouselMedia",
              media_dict: {
                code: "CxYz_1-2Ab",
                like_count: 4_200,
                comment_count: 31,
              },
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
  const commentRow = node(
    "Comment one",
    {},
    {
      "time[datetime]": [node("", { datetime: "2024-01-01T00:00:00Z" })],
      'a[href*="/c/"]': [node("", { href: "/p/ABC123/c/c1/" })],
    },
  );
  const commentList = node(
    "",
    {},
    { li: [commentRow], ":scope > li": [commentRow] },
  );
  const capturedPost = {
    data: {
      xdt_shortcode_media: {
        code: "ABC123",
        __typename: "XIGPolarisClipsMedia",
        like_count: 500,
        comment_count: 12,
        view_count: 9_999,
      },
      comments: [{ pk: "c1", text: "Comment one", comment_like_count: 7 }],
    },
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
    source: "mixed",
    post: { likes: 500, views: 9_999, commentCount: 12 },
  });
  expect(
    (result as unknown as { comments: ReadonlyArray<{ likes: number }> }).comments,
  ).toEqual([expect.objectContaining({ likes: 7 })]);
});

test("reports an Instagram grid run that stalls before the cap or deadline as truncated", async () => {
  const displayNameSpan = node("Jane Doe", { dir: "auto" });
  const bioSpan = instagramBioSpan(["Bio."]);
  const anchors: FixtureNode[] = [
    instagramGridAnchor("janedoe", "p", "First001", "First"),
  ];
  const pageSelectors: Record<string, FixtureNode[]> = {
    'header span[dir="auto"]': [displayNameSpan],
    'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]': [bioSpan],
    "div._ac7v a[href]": anchors,
  };
  const restore = installPage(
    "",
    "Profile",
    "https://www.instagram.com/janedoe/",
    pageSelectors,
  );
  Object.defineProperty(globalThis.window, "scrollTo", {
    configurable: true,
    value: () => {
      // Never grows: simulates a fetch that never resolves within the poll.
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
