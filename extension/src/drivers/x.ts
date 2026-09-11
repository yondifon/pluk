import type { DriverPageResult, DriverScriptOptions } from "./types";

export function runXPage(
  options: DriverScriptOptions,
): DriverPageResult | Promise<DriverPageResult> {
  const clean = (value: string | null | undefined): string =>
    (value ?? "").replace(/\s+/gu, " ").trim();
  const meta = (): { readonly url: string; readonly title: string } => ({
    url: window.location.href,
    title: document.title.slice(0, 512),
  });
  const failure = (
    state: DriverPageResult["state"],
    message: string,
  ): DriverPageResult => ({ state, message });
  const loginRequired = (): DriverPageResult =>
    failure(
      "login_required",
      "X needs an active sign-in. Sign in to the Chrome window Pluk opened, then try again.",
    );
  const bodyText = clean(document.body?.innerText ?? "");
  const path = window.location.pathname;
  if (
    /\/(?:login|i\/flow\/login)(?:\/|$)/iu.test(path) ||
    (/\b(?:sign in|log in|login)\b/iu.test(
      `${document.title} ${bodyText.slice(0, 4_000)}`,
    ) &&
      bodyText.length < 160)
  ) {
    return loginRequired();
  }

  const RESERVED_HANDLES = new Set([
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

  const accountIdentity = (): string | null => {
    const candidates = [
      document.querySelector('[data-testid="AppTabBar_Profile_Link"]'),
      document.querySelector('[data-testid="SideNav_AccountSwitcher_Button"]'),
      document.querySelector('a[href^="/"][aria-label*="Profile" i]'),
    ];
    for (const candidate of candidates) {
      if (!candidate) {
        continue;
      }
      const href = candidate.getAttribute("href") ?? "";
      const username = href.match(/^\/([A-Za-z0-9_]{1,50})(?:\/|$)/u)?.[1];
      if (username && !RESERVED_HANDLES.has(username.toLowerCase())) {
        return `@${username}`;
      }
      const label = clean(
        candidate.getAttribute("aria-label") ?? candidate.textContent,
      );
      const handle = label.match(/@[A-Za-z0-9_]{1,50}/u)?.[0];
      if (handle) {
        return handle;
      }
    }
    return null;
  };

  // The timestamp is the one permalink X wraps in an anchor; every other
  // `/status/` link on a post points somewhere else, `/analytics` included.
  const permalinkFromNode = (node: Element): string | null =>
    node.querySelector("time")?.closest("a")?.getAttribute("href") ?? null;

  const postIdFromNode = (node: Element): string | null => {
    const hrefs = [permalinkFromNode(node) ?? ""]
      .concat(
        Array.from(node.querySelectorAll('a[href*="/status/"]')).map(
          (link) => link.getAttribute("href") ?? "",
        ),
      )
      .concat(node.getAttribute("data-tweet-id") ?? "");
    for (const href of hrefs) {
      const match = href.match(/(?:\/status\/|^)(\d+)(?:[/?#]|$)/u);
      if (match?.[1]) {
        return match[1];
      }
    }
    return null;
  };

  const authorHandleFromNode = (node: Element): string | null => {
    const hrefs = [permalinkFromNode(node) ?? ""].concat(
      Array.from(node.querySelectorAll('a[href*="/status/"]')).map(
        (link) => link.getAttribute("href") ?? "",
      ),
    );
    for (const href of hrefs) {
      const handle = href.match(/^\/([A-Za-z0-9_]{1,50})\/status\//u)?.[1];
      if (handle && !RESERVED_HANDLES.has(handle.toLowerCase())) {
        return handle;
      }
    }
    return null;
  };

  const canonicalPostTarget = (
    authorHandle: string | null,
    postId: string,
  ): string =>
    authorHandle
      ? `https://x.com/${authorHandle}/status/${postId}`
      : `https://x.com/status/${postId}`;

  interface ReadPost {
    readonly postId: string;
    readonly targetUrl: string;
    readonly canonicalTarget: string;
    readonly author: string;
    readonly postedAt: string | null;
    readonly engagement: string | null;
    readonly text: string;
    readonly excerpt: string;
  }

  const readPosts = (): ReadPost[] => {
    const articles = Array.from(
      document.querySelectorAll('article[data-testid="tweet"]'),
    );
    const fallback =
      articles.length > 0
        ? articles
        : Array.from(document.querySelectorAll("article"));
    const seen = new Set<string>();
    const posts: ReadPost[] = [];
    for (const article of fallback) {
      const postId = postIdFromNode(article);
      if (!postId || seen.has(postId)) {
        continue;
      }
      // A shortened link ends in an ellipsis X draws itself; the href it
      // stands for is already in the text nodes around it.
      const text = clean(
        article.querySelector('[data-testid="tweetText"]')?.textContent ??
          article.textContent,
      )
        .replace(/\s*…\s*$/u, "")
        .slice(0, 1_000);
      const authorHandle = authorHandleFromNode(article);
      // The display name and the handle sit in separate nodes with no
      // separator between them, so they are read apart and joined.
      const displayName = clean(
        article.querySelector('[data-testid="User-Name"] span')?.textContent,
      );
      const author = (
        authorHandle ? `${displayName} @${authorHandle}` : displayName
      )
        .trim()
        .slice(0, 256);
      seen.add(postId);
      posts.push({
        postId,
        targetUrl: `https://x.com/status/${postId}`,
        canonicalTarget: canonicalPostTarget(authorHandle, postId),
        author,
        postedAt:
          article.querySelector("time")?.getAttribute("datetime") ?? null,
        // One aria-label carries replies, reposts, likes, bookmarks and views.
        engagement:
          clean(
            article
              .querySelector('[role="group"][aria-label]')
              ?.getAttribute("aria-label"),
          ) || null,
        text,
        excerpt: text.slice(0, 1_000),
      });
      if (posts.length === 25) {
        break;
      }
    }
    return posts;
  };

  const page = meta();
  const account = accountIdentity();
  const posts = readPosts();
  const targetUrlPostId = new URL(options.targetUrl).pathname.match(
    /\/status\/(\d+)(?:\/|$)/u,
  )?.[1];
  const requestedPostId = options.postId ?? targetUrlPostId;
  const target = requestedPostId
    ? posts.find((post) => post.postId === requestedPostId)
    : undefined;

  const baseData = (kind: string, text: string): DriverPageResult => ({
    state: "ready",
    kind,
    ...page,
    platform: "x",
    accountIdentity: account,
    text: text.slice(0, 8_000),
  });

  // Read at entry the signed-in account can still be null, because the
  // navigation that precedes a command lands before X has drawn the sidebar
  // it is read from. An absent account is not a changed one.
  const awaitAccount = async (): Promise<string | null> => {
    const deadline = Date.now() + 5_000;
    for (;;) {
      const found = account ?? accountIdentity();
      if (found || Date.now() >= deadline) {
        return found;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  };

  // The posts read at entry can be empty: a command that navigates first
  // arrives before X has drawn the timeline. Read again until the wanted
  // post appears.
  const awaitPost = async (postId: string): Promise<ReadPost | undefined> => {
    const deadline = Date.now() + 5_000;
    for (;;) {
      const found = readPosts().find((post) => post.postId === postId);
      if (found || Date.now() >= deadline) {
        return found;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  };

  const waitFor = async (
    read: () => Element | null,
    timeoutMs: number,
  ): Promise<Element | null> => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const found = read();
      if (found) {
        return found;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return null;
  };

  const composerScope = (): Element | null => {
    const anchors = Array.from(
      document.querySelectorAll('[data-testid="tweetTextarea_0"]'),
    );
    for (const anchor of anchors) {
      let candidate = anchor.parentElement;
      while (candidate) {
        const hasEditor =
          candidate.querySelector('[data-testid="tweetTextarea_0"]') !== null;
        const hasSubmitButton =
          candidate.querySelector('[data-testid="tweetButtonInline"]') !==
            null ||
          candidate.querySelector('[data-testid="tweetButton"]') !== null;
        if (hasEditor && hasSubmitButton) {
          return candidate;
        }
        candidate = candidate.parentElement;
      }
    }
    return null;
  };

  // Each line of the composer is its own block, and their text nodes carry no
  // separator between them, so the blocks are rejoined before comparing.
  const editorText = (editor: Element): string => {
    const blocks = editor.querySelectorAll('[data-block="true"]');
    if (blocks.length === 0) {
      return clean(editor.textContent);
    }
    return clean(
      Array.from(blocks)
        .map((block) => block.textContent ?? "")
        .join("\n"),
    );
  };

  const deepestTextNode = (node: Node): Text | null => {
    for (let index = node.childNodes.length - 1; index >= 0; index -= 1) {
      const child = node.childNodes[index];
      if (child?.nodeType === 3) {
        return child as Text;
      }
      if (child) {
        const text = deepestTextNode(child);
        if (text) {
          return text;
        }
      }
    }
    return null;
  };

  const placeCaretAtEnd = (editor: Element): void => {
    if (typeof document.createRange !== "function") {
      return;
    }
    if (typeof window.getSelection !== "function") {
      return;
    }
    const selection = window.getSelection();
    if (!selection) {
      return;
    }
    const range = document.createRange();
    const leaves = editor.querySelectorAll("span[data-offset-key]");
    const leaf = leaves[leaves.length - 1];
    const textNode = deepestTextNode(leaf ?? editor);
    if (textNode) {
      range.setStart(textNode, textNode.textContent?.length ?? 0);
      range.collapse(true);
    } else if (leaf) {
      range.setStart(leaf, 0);
      range.collapse(true);
    } else {
      range.selectNodeContents(editor);
      range.collapse(false);
    }
    selection.removeAllRanges();
    selection.addRange(range);
  };

  const emit = (element: Element, event: Event): void => {
    const target = element as unknown as {
      dispatchEvent?: (event: Event) => boolean;
    };
    target.dispatchEvent?.(event);
  };

  const dispatchPaste = (editor: Element, text: string): boolean => {
    if (
      typeof DataTransfer !== "function" ||
      typeof ClipboardEvent !== "function"
    ) {
      return false;
    }
    const clipboardData = new DataTransfer();
    clipboardData.setData("text/plain", text);
    emit(
      editor,
      new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData,
      }),
    );
    return true;
  };

  const settleEditor = async (
    editor: Element,
    expected: string,
    timeoutMs: number,
  ): Promise<boolean> => {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const current = editorText(editor);
      if (current === expected) {
        await new Promise((resolve) => setTimeout(resolve, 200));
        return editorText(editor) === expected;
      }
      // X rewrites the editor after the text lands — it decorates links and
      // reflows blocks — so intermediate states are expected. Only the
      // deadline ends the wait; the match above is still exact.
      if (Date.now() >= deadline) {
        return false;
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  };

  const dispatchSubmitShortcut = (editor: Element): void => {
    if (typeof KeyboardEvent !== "function") {
      return;
    }
    emit(
      editor,
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        code: "Enter",
        key: "Enter",
        metaKey: true,
      }),
    );
  };

  const insertComposerText = async (
    editor: Element,
    text: string,
  ): Promise<boolean> => {
    placeCaretAtEnd(editor);
    if (!dispatchPaste(editor, text)) {
      return false;
    }
    return settleEditor(editor, clean(text), 4_000);
  };

  const clearComposer = async (editor: Element): Promise<boolean> => {
    if (editorText(editor) === "") {
      return true;
    }
    if (
      typeof document.createRange !== "function" ||
      typeof window.getSelection !== "function" ||
      typeof document.execCommand !== "function"
    ) {
      return false;
    }
    const selection = window.getSelection();
    if (!selection) {
      return false;
    }
    const range = document.createRange();
    range.selectNodeContents(editor);
    selection.removeAllRanges();
    selection.addRange(range);
    document.execCommand("delete");
    const deadline = Date.now() + 600;
    for (;;) {
      if (editorText(editor) === "") {
        return true;
      }
      if (Date.now() >= deadline) {
        return false;
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  };

  const typeComposerText = async (
    editor: Element,
    text: string,
  ): Promise<boolean> => {
    (editor as unknown as { focus?: () => void }).focus?.();
    if (!(await clearComposer(editor))) {
      return false;
    }
    return insertComposerText(editor, text);
  };

  const isDisabled = (element: Element): boolean =>
    element.getAttribute("disabled") !== null ||
    element.getAttribute("aria-disabled") === "true";

  const composerSubmitButton = (scope: Element): Element | null => {
    for (const selector of [
      '[data-testid="tweetButtonInline"]',
      '[data-testid="tweetButton"]',
    ]) {
      const found = scope.querySelector(selector);
      if (found && !isDisabled(found)) {
        return found;
      }
    }
    return null;
  };

  const notificationSelector = '[role="alert"], [data-testid="toast"]';

  const notificationTexts = (): string[] =>
    Array.from(document.querySelectorAll(notificationSelector)).map((node) =>
      clean(node.textContent),
    );

  const waitForSubmissionEvidence = async (
    scope: Element,
    previousNotifications: readonly string[],
  ): Promise<Element | null> =>
    waitFor(() => {
      const notification = Array.from(
        document.querySelectorAll(notificationSelector),
      ).find((candidate) => {
        const text = clean(candidate.textContent);
        return (
          text.length > 0 &&
          !previousNotifications.includes(text) &&
          /(?:posted|sent|replied|published)/iu.test(text)
        );
      });
      if (!notification) {
        return null;
      }
      if (scope.isConnected === false) {
        return notification;
      }
      const editor = scope.querySelector('[data-testid="tweetTextarea_0"]');
      return !editor || editorText(editor) === "" ? notification : null;
    }, 15_000);

  const unsupportedComposer = (scope: Element): DriverPageResult | null => {
    const hasUnsupportedContent =
      scope.querySelector('[data-testid="pollOptions"]') !== null ||
      scope.querySelector('[data-testid="tweetTextarea_1"]') !== null;
    if (!hasUnsupportedContent) {
      return null;
    }
    return failure(
      "unsupported",
      "The visible X composer contains a poll or thread. Open a plain post or reply composer and try again; nothing was submitted.",
    );
  };

  // A submission is the one visit the page gets: confirm the signed-in
  // account and the target, type the confirmed text, and send it.
  const submit = async (): Promise<DriverPageResult> => {
    if (!(await awaitAccount())) {
      return failure(
        "account_unverified",
        "Pluk could not tell which X account is signed in. Open the account menu in that Chrome window and try again; nothing was submitted.",
      );
    }
    const target = requestedPostId ? await awaitPost(requestedPostId) : undefined;
    if (
      !requestedPostId ||
      !target ||
      targetUrlPostId !== target.postId ||
      window.location.href !== options.targetUrl
    ) {
      return failure(
        "target_mismatch",
        "The confirmed X post is not the visible target. Nothing was submitted.",
      );
    }
    const article = Array.from(
      document.querySelectorAll('article[data-testid="tweet"], article'),
    ).find((candidate) => postIdFromNode(candidate) === target.postId);
    const replyButton = article?.querySelector(
      '[data-testid="reply"], button[aria-label*="Reply" i], [role="button"][aria-label*="Reply" i]',
    );
    if (!article || !replyButton || !options.text) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post reply controls. Nothing was submitted.",
      );
    }
    (replyButton as HTMLElement).click();
    const scope = await waitFor(() => composerScope(), 3_000);
    if (!scope) {
      return failure(
        "unsupported",
        "X did not open the reply editor for the confirmed post. Nothing was submitted.",
      );
    }
    const unsupported = unsupportedComposer(scope);
    if (unsupported) {
      return unsupported;
    }
    const composer = scope.querySelector('[data-testid="tweetTextarea_0"]');
    if (!composer) {
      return failure(
        "unsupported",
        "X did not open the reply editor for the confirmed post. Nothing was submitted.",
      );
    }
    if (!(await typeComposerText(composer, options.text))) {
      return failure(
        "unsupported",
        "X did not accept the reply text. Check the visible composer and try again; nothing was submitted.",
      );
    }
    if (editorText(composer) !== clean(options.text)) {
      return failure(
        "unsupported",
        "X rejected the reply text in the confirmed editor. Nothing was submitted.",
      );
    }
    const submitButton = composerSubmitButton(scope);
    if (!submitButton) {
      return failure(
        "unsupported",
        "X did not expose an enabled submit control for the confirmed reply. Nothing was submitted.",
      );
    }
    const previousNotifications = notificationTexts();
    (submitButton as HTMLElement).click();
    const evidence = await waitForSubmissionEvidence(
      scope,
      previousNotifications,
    );
    return evidence
      ? {
          state: "submission_succeeded",
          kind: "submission",
          ...page,
          platform: "x",
        }
      : failure(
          "submission_unknown",
          "X accepted the click without visible outcome evidence. Check the post before trying again; Pluk did not retry.",
        );
  };

  const submitCompose = async (): Promise<DriverPageResult> => {
    if (!(await awaitAccount())) {
      return failure(
        "account_unverified",
        "Pluk could not tell which X account is signed in. Open the account menu in that Chrome window and try again; nothing was submitted.",
      );
    }
    if (window.location.href !== options.targetUrl) {
      return failure(
        "target_mismatch",
        "The confirmed X compose page is not the visible target. Nothing was submitted.",
      );
    }
    const scope = await waitFor(() => composerScope(), 3_000);
    if (!scope || !options.text) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post editor. Nothing was submitted.",
      );
    }
    const unsupported = unsupportedComposer(scope);
    if (unsupported) {
      return unsupported;
    }
    const composer = scope.querySelector('[data-testid="tweetTextarea_0"]');
    if (!composer) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post editor. Nothing was submitted.",
      );
    }
    if (
      editorText(composer) !== clean(options.text) &&
      !(await typeComposerText(composer, options.text))
    ) {
      return failure(
        "unsupported",
        "X did not accept the post text. Check the visible composer and try again; nothing was submitted.",
      );
    }
    if (editorText(composer) !== clean(options.text)) {
      return failure(
        "unsupported",
        "X rejected the post text in the confirmed editor. Nothing was submitted.",
      );
    }
    await new Promise((resolve) =>
      setTimeout(resolve, 600 + Math.random() * 800),
    );
    const submitButton = composerSubmitButton(scope);
    if (!submitButton) {
      return failure(
        "unsupported",
        "X did not expose an enabled submit control for the confirmed post. Nothing was submitted.",
      );
    }
    const previousNotifications = notificationTexts();
    dispatchSubmitShortcut(composer);
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    const nothingHappened =
      editorText(composer) === clean(options.text ?? "") &&
      notificationTexts().length === previousNotifications.length &&
      composerSubmitButton(scope) !== null;
    if (nothingHappened) {
      (submitButton as HTMLElement).click();
    }
    const evidence = await waitForSubmissionEvidence(
      scope,
      previousNotifications,
    );
    if (!evidence) {
      return failure(
        "submission_unknown",
        "X accepted the click without visible outcome evidence. Check the post before trying again; Pluk did not retry.",
      );
    }
    const href =
      evidence.querySelector('a[href*="/status/"]')?.getAttribute("href") ??
      evidence.getAttribute("href") ??
      "";
    const postedId = href.match(/\/status\/(\d+)/u)?.[1];
    if (!postedId) {
      return failure(
        "submission_unknown",
        "X may have posted this, but did not expose a link to confirm which one. Check X before trying again; Pluk did not retry.",
      );
    }
    return {
      state: "submission_succeeded",
      kind: "submission",
      ...page,
      platform: "x",
      postedId,
      postedUrl: `https://x.com${href}`,
    };
  };

  if (options.action === "read_trends") {
    if (!/\/explore(?:\/|$)/u.test(path)) {
      return failure(
        "unsupported",
        "Pluk could not confirm the X Explore page. No trend data was returned.",
      );
    }
    const trendNodes = Array.from(
      document.querySelectorAll(
        '[data-testid="trend"], [data-testid="trendItem"]',
      ),
    );
    const trends = trendNodes
      .map((node) => clean(node.textContent).slice(0, 512))
      .filter(Boolean)
      .slice(0, 50);
    if (trends.length === 0) {
      return failure(
        "waiting",
        "X Explore did not expose visible trend entries. The page markup may have changed.",
      );
    }
    return {
      ...baseData("x_trends", trends.join(" | ")),
      scope: "visible Explore content",
      personalized: true,
      trends,
    };
  }

  if (options.action === "submit_reply") {
    return submit();
  }
  if (options.action === "submit_post") {
    return submitCompose();
  }
  if (options.action === "read_profile") {
    const handle = path.match(/^\/([A-Za-z0-9_]{1,50})\/?$/u)?.[1];
    if (!handle || RESERVED_HANDLES.has(handle.toLowerCase())) {
      return failure(
        "unsupported",
        "This target is not an X profile page. Use read_post for a single post.",
      );
    }
    const nameNode = document.querySelector('[data-testid="UserName"]');
    const displayName = clean(nameNode?.textContent).slice(0, 256);
    const visibleHandle = displayName
      .match(/@([A-Za-z0-9_]{1,50})/u)?.[1]
      ?.toLowerCase();
    if (
      !displayName ||
      (visibleHandle && visibleHandle !== handle.toLowerCase())
    ) {
      return failure(
        "target_not_found",
        "The requested X profile was not visible. No profile data was returned.",
      );
    }
    const bio = clean(
      document.querySelector('[data-testid="UserDescription"]')?.textContent,
    ).slice(0, 1_000);
    return {
      ...baseData("x_profile", bio || displayName),
      canonicalTarget: `https://x.com/${handle}`,
      handle,
      displayName,
      bio,
    };
  }
  if (options.action === "inspect") {
    if (!target) {
      return failure(
        "target_not_found",
        "The requested X post was not visible. No post data was returned.",
      );
    }
    return { ...baseData("x_post", target.text), post: target };
  }
  if (options.action === "read_post") {
    if (!requestedPostId || !target || targetUrlPostId !== requestedPostId) {
      return failure(
        "target_not_found",
        "The requested X post was not visible, or the post ID did not match the target URL. No post data was returned.",
      );
    }
    return {
      ...baseData("x_post", target.text),
      canonicalTarget: target.canonicalTarget,
      post: target,
    };
  }
  if (
    options.action === "read_feed" ||
    options.action === "refresh" ||
    options.action === "capture"
  ) {
    if (posts.length === 0) {
      return failure(
        "waiting",
        "X did not expose visible posts. The page markup may have changed.",
      );
    }
    return {
      ...baseData("x_feed", posts.map((post) => post.text).join(" | ")),
      posts,
    };
  }
  return failure(
    "unsupported",
    "This X action is not supported by the site driver.",
  );
}
