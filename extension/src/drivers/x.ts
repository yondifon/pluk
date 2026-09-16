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
      : `https://x.com/i/status/${postId}`;

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
        targetUrl: `https://x.com/i/status/${postId}`,
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

  // The tightest box holding both an editor and its Post button. In a
  // thread X moves the toolbar to the newest post, so the anchor is the
  // editor the button should sit with.
  // The compose route opens a modal over the home timeline, and the timeline
  // has a composer of its own with the same test ids, so the modal wins and
  // every composer lookup stays inside whichever region holds the editor.
  const inModal = (element: Element): boolean =>
    element.closest('[aria-modal="true"]') !== null;
  const composerRoot = (scope: Element): ParentNode =>
    scope.closest('[aria-modal="true"]') ?? document;

  const composerScope = (
    editorSelector = '[data-testid="tweetTextarea_0"]',
  ): Element | null => {
    const anchors = Array.from(document.querySelectorAll(editorSelector)).sort(
      (left, right) => Number(inModal(right)) - Number(inModal(left)),
    );
    for (const anchor of anchors) {
      let candidate = anchor.parentElement;
      while (candidate) {
        const hasEditor = candidate.querySelector(editorSelector) !== null;
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

  // A step log that outlives a page reload, read back by debug capture.
  const trace = (step: string): void => {
    try {
      const soFar = sessionStorage.getItem("wande:trace") ?? "";
      sessionStorage.setItem("wande:trace", `${soFar}${Date.now()} ${step}\n`);
    } catch {
      // Storage refused; the trace is only a diagnostic.
    }
  };

  // The plus that belongs to this editor: the nearest ancestor holding both
  // is the editor's own composer. Anything higher up is another composer's
  // control, or the timeline's own plus.
  const addButtonFor = (editor: Element): Element | null => {
    const candidates = Array.from(
      document.querySelectorAll('[data-testid="addButton"]'),
    ).filter((candidate) => !isDisabled(candidate));
    let container: Element | null = editor.parentElement;
    while (container) {
      const own = candidates.filter((candidate) => container?.contains(candidate));
      if (own.length > 0) {
        trace(`plus candidates: ${candidates.length}, own: ${own.length}`);
        return own[own.length - 1] ?? null;
      }
      if (container.getAttribute("aria-modal") === "true") {
        return null;
      }
      container = container.parentElement;
    }
    return null;
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
      // X rewrites the editor after the text lands: it decorates links and
      // reflows blocks, so intermediate states are expected. Only the
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

  // A quote's composer shows the post it quotes among the attachments, as one
  // control holding that post's author and words. Its avatar and media belong
  // to that post, not to anything attached here.
  const inQuotedPost = (media: Element): boolean => {
    const control = media.closest('button, [role="link"]');
    return (
      control !== null &&
      control.querySelector('[data-testid="User-Name"], [data-testid="tweetText"]') !== null
    );
  };

  // How many pieces of media the composer currently shows attached, however
  // they got there. Counted, never trusted by content: this is what tells
  // apart "still uploading", "fully attached", and media Pluk never asked
  // for.
  const attachedMediaCount = (scope: Element): number =>
    [
      ...Array.from(scope.querySelectorAll('[data-testid="attachments"] img')),
      ...Array.from(scope.querySelectorAll('[data-testid="attachments"] video')),
    ].filter((media) => !inQuotedPost(media)).length;

  const base64ToBytes = (base64: string): Uint8Array<ArrayBuffer> => {
    const binary = atob(base64);
    const bytes = new Uint8Array(new ArrayBuffer(binary.length));
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return bytes;
  };

  const extensionFor = (contentType: string): string =>
    contentType === "image/png" ? "png" : "jpg";

  /**
   * Reconciles this part's composer against what its own submission was
   * approved with, and pastes the rest straight into that part's own editor
   * — the same synthetic, untrusted `paste` a script already uses for text,
   * now carrying files instead. Nothing is written to the system clipboard
   * and no clipboard permission is needed: the `DataTransfer` here is built
   * from scratch and only ever handed to this one dispatched event.
   *
   * Any media the composer shows beyond what was approved is refused
   * outright, images or not: it did not come from this submission's own
   * staged snapshot, so nothing here can tell what it actually is. Once every
   * approved image has a matching preview, `null` lets the caller continue.
   */
  const guardComposerMedia = (
    editor: Element,
    scope: Element,
    images: readonly { readonly data: string; readonly contentType: string }[],
  ): DriverPageResult | null => {
    const existing = attachedMediaCount(scope);
    if (existing > images.length) {
      return failure(
        "unsupported",
        "The composer already has media Pluk did not attach. Remove it and try again; nothing was submitted.",
      );
    }
    if (images.length === 0 || existing === images.length) {
      return null;
    }
    // A real upload can take far longer to render its preview than one poll:
    // once this editor carries Pluk's own marker, the paste already
    // happened, and every later pass here just keeps waiting on it rather
    // than pasting the files a second time.
    if (existing > 0 || editor.hasAttribute("data-pluk-pasted")) {
      return { state: "waiting", message: "Waiting for the image previews to finish loading." };
    }
    if (
      typeof DataTransfer !== "function" ||
      typeof ClipboardEvent !== "function" ||
      typeof File !== "function"
    ) {
      return failure(
        "unsupported",
        "This version of Chrome does not support pasting images into X. Nothing was submitted.",
      );
    }
    const clipboardData = new DataTransfer();
    images.forEach((image, index) => {
      const blob = new Blob([base64ToBytes(image.data)], { type: image.contentType });
      const file = new File([blob], `pluk-${index}.${extensionFor(image.contentType)}`, {
        type: image.contentType,
      });
      clipboardData.items.add(file);
    });
    editor.setAttribute("data-pluk-pasted", "true");
    trace(`pasting ${images.length} image(s)`);
    emit(
      editor,
      new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData,
      }),
    );
    return { state: "waiting", message: "Waiting for the image previews to finish loading." };
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

  // A poll, or a thread already sitting in the composer when a single post
  // was asked for, is content Pluk did not write and will not send.
  const unsupportedComposer = (
    scope: Element,
    partCount = 1,
  ): DriverPageResult | null => {
    const hasUnsupportedContent =
      scope.querySelector('[data-testid="pollOptions"]') !== null ||
      (partCount === 1 &&
        scope.querySelector('[data-testid="tweetTextarea_1"]') !== null);
    if (!hasUnsupportedContent) {
      return null;
    }
    return failure(
      "unsupported",
      "The visible X composer contains a poll or thread. Open a plain post or reply composer and try again; nothing was submitted.",
    );
  };

  // Pluk's own marks on X's controls, so a pass can tell what an earlier one
  // already asked the browser to press. X redraws an article when it flips to
  // reposted and the mark goes with it; losing one only costs another press
  // of the repost button, which opens a menu rather than reposting.
  const MENU_ASKED = "data-pluk-repost-menu";
  const CONFIRM_ASKED = "data-pluk-repost-confirm";
  const QUOTE_ASKED = "data-pluk-quote";

  const articleFor = (postId: string): Element | null =>
    Array.from(
      document.querySelectorAll('article[data-testid="tweet"], article'),
    ).find((candidate) => postIdFromNode(candidate) === postId) ?? null;

  // Whichever choice X's open repost menu is offering: Repost on a post that
  // is not reposted, Undo repost on one that is.
  const openMenuChoice = (): Element | null =>
    document.querySelector(
      '[role="menu"] [role="menuitem"][data-testid="retweetConfirm"]',
    ) ??
    document.querySelector(
      '[role="menu"] [role="menuitem"][data-testid="unretweetConfirm"]',
    );

  /**
   * `choice` in the post's own repost menu, opening that menu first when it
   * is not already showing. A menu asked for once and still not shown is
   * given up on rather than asked for again.
   */
  const repostMenuItem = async (
    repostButton: Element | null,
    choice: () => Element | null,
    nothingDone: string,
  ): Promise<Element | DriverPageResult> => {
    const menuItem =
      choice() ??
      (repostButton?.hasAttribute(MENU_ASKED) ? await waitFor(choice, 3_000) : null);
    if (menuItem) {
      return menuItem;
    }
    if (!repostButton) {
      return failure(
        "unsupported",
        `X did not expose the repost control on this post. ${nothingDone}`,
      );
    }
    if (repostButton.hasAttribute(MENU_ASKED)) {
      return failure("unsupported", `X did not open the repost menu. ${nothingDone}`);
    }
    repostButton.setAttribute(MENU_ASKED, "true");
    trace(`asking for a real press of the repost control for ${requestedPostId}`);
    return pressRequest(repostButton);
  };

  const pressRequest = (control: Element): DriverPageResult => {
    const rect = control.getBoundingClientRect();
    return {
      state: "waiting",
      trustedClick: {
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2,
      },
    };
  };

  /**
   * Shares a post as it stands, through X's own repost menu.
   *
   * X honours only a real press on both controls, so each one is asked for
   * and the script runs again once the browser has delivered it. Only the
   * post's own button turning into the reposted state counts as done; a post
   * that already carries it is left exactly as it is.
   */
  const submitRepost = async (): Promise<DriverPageResult> => {
    if (!(await awaitAccount())) {
      return failure(
        "account_unverified",
        "Pluk could not tell which X account is signed in. Open the account menu in that Chrome window and try again; nothing was reposted.",
      );
    }
    if (!requestedPostId || targetUrlPostId !== requestedPostId) {
      return failure(
        "target_mismatch",
        "The confirmed X post is not the visible target. Nothing was reposted.",
      );
    }
    const article = await waitFor(() => articleFor(requestedPostId), 5_000);
    if (!article) {
      return failure(
        "target_not_found",
        "The X post to repost was not visible. Nothing was reposted.",
      );
    }
    const done = (already: boolean): DriverPageResult => ({
      state: "submission_succeeded",
      kind: "repost",
      ...page,
      platform: "x",
      postedId: requestedPostId,
      alreadyReposted: already,
    });
    // The article's own button is the only proof: a menu that closed says
    // nothing about what it did.
    const askedToRepost = article.hasAttribute(CONFIRM_ASKED);
    if (article.querySelector('[data-testid="unretweet"]')) {
      return done(!askedToRepost);
    }
    if (askedToRepost) {
      const reposted = await waitFor(
        () => articleFor(requestedPostId)?.querySelector('[data-testid="unretweet"]') ?? null,
        10_000,
      );
      return reposted
        ? done(false)
        : failure(
            "submission_unknown",
            "X took the repost without showing it on the post. Check X before trying again; Pluk did not retry.",
          );
    }
    const menuItem = await repostMenuItem(
      article.querySelector('[data-testid="retweet"]'),
      openMenuChoice,
      "Nothing was reposted.",
    );
    if ("state" in menuItem) {
      return menuItem;
    }
    // An already reposted post offers Undo repost in place of Repost.
    // Pressing that would take the post down, so it is read as done.
    if (menuItem.getAttribute("data-testid") === "unretweetConfirm") {
      return done(true);
    }
    article.setAttribute(CONFIRM_ASKED, "true");
    trace(`asking for a real press of Repost for ${requestedPostId}`);
    return pressRequest(menuItem);
  };

  // The Quote choice in X's open repost menu.
  const quoteMenuChoice = (): Element | null =>
    document.querySelector('[role="menu"] a[role="menuitem"][href="/compose/post"]');

  // The composer X opens for a quote: the new-post modal, carrying the
  // quoted post among its attachments.
  const quoteComposer = (): Element | null =>
    Array.from(document.querySelectorAll('[role="dialog"][aria-modal="true"]')).find(
      (dialog) =>
        dialog.querySelector('[data-testid="tweetTextarea_0"]') !== null &&
        dialog.querySelector('[data-testid="attachments"] [data-testid="User-Name"]') !== null,
    ) ?? null;

  /**
   * Quotes a post with the confirmed text, images, and thread.
   *
   * The Quote choice sits in the same repost menu, and X honours only a real
   * press on both, so each is asked for and the script runs again. Once X's
   * quote composer is open it is filled and sent exactly as a new post.
   */
  const submitQuote = async (): Promise<DriverPageResult> => {
    if (!(await awaitAccount())) {
      return failure(
        "account_unverified",
        "Pluk could not tell which X account is signed in. Open the account menu in that Chrome window and try again; nothing was posted.",
      );
    }
    if (!requestedPostId || targetUrlPostId !== requestedPostId) {
      return failure(
        "target_mismatch",
        "The confirmed X post is not the visible target. Nothing was posted.",
      );
    }
    if (!options.text) {
      return failure(
        "unsupported",
        "The quote has no text to post. Nothing was posted.",
      );
    }
    const article = await waitFor(() => articleFor(requestedPostId), 5_000);
    if (!article) {
      return failure(
        "target_not_found",
        "The X post to quote was not visible. Nothing was posted.",
      );
    }
    if (article.hasAttribute(QUOTE_ASKED)) {
      const dialog = await waitFor(quoteComposer, 3_000);
      const scope = composerScope();
      if (!dialog || !scope || !dialog.contains(scope)) {
        return failure(
          "unsupported",
          "X did not open the quote composer. Nothing was posted.",
        );
      }
      // X draws the quote composer at its own compose address, over the
      // quoted post's page, and that post is still what this job is on.
      const landedOn = path.startsWith("/compose/")
        ? { ...page, url: options.targetUrl }
        : page;
      return publishParts(scope, options.text, landedOn);
    }
    const menuItem = await repostMenuItem(
      article.querySelector('[data-testid="retweet"], [data-testid="unretweet"]'),
      quoteMenuChoice,
      "Nothing was posted.",
    );
    if ("state" in menuItem) {
      return menuItem;
    }
    article.setAttribute(QUOTE_ASKED, "true");
    trace(`asking for a real press of Quote for ${requestedPostId}`);
    return pressRequest(menuItem);
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
    // The page is the post's own, reached through X's redirect from the
    // id-only URL, so it is matched on the post id rather than the address.
    if (!requestedPostId || !target || targetUrlPostId !== target.postId) {
      return failure(
        "target_mismatch",
        "The confirmed X post is not the visible target. Nothing was submitted.",
      );
    }
    if (!options.text) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post reply controls. Nothing was submitted.",
      );
    }
    trace(`reply to ${target.postId} at ${window.location.href}`);
    // A post's page already carries a reply editor under the post. Only
    // when it is missing is the reply control pressed, and X honours only a
    // real press there, so that goes through the browser and the script
    // runs again to find the editor open.
    const scope = composerScope();
    if (!scope) {
      const article = Array.from(
        document.querySelectorAll('article[data-testid="tweet"], article'),
      ).find((candidate) => postIdFromNode(candidate) === target.postId);
      const replyButton = article?.querySelector(
        '[data-testid="reply"], button[aria-label*="Reply" i], [role="button"][aria-label*="Reply" i]',
      );
      if (!replyButton) {
        return failure(
          "unsupported",
          "X did not expose the confirmed post reply controls. Nothing was submitted.",
        );
      }
      const rect = replyButton.getBoundingClientRect();
      trace("asking for a real press of the reply control");
      return {
        state: "waiting",
        trustedClick: {
          x: rect.left + rect.width / 2,
          y: rect.top + rect.height / 2,
        },
      };
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
    if (
      editorText(composer) !== clean(options.text) &&
      !(await typeComposerText(composer, options.text))
    ) {
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
    trace("typed reply");
    const mediaGuard = guardComposerMedia(composer, scope, options.images ?? []);
    if (mediaGuard) {
      return mediaGuard;
    }
    await new Promise((resolve) =>
      setTimeout(resolve, 600 + Math.random() * 800),
    );
    // X enables the reply button only once its draft state has taken the
    // pasted text, which can land after the pause above.
    const submitButton = await waitFor(() => composerSubmitButton(scope), 5_000);
    if (!submitButton) {
      return failure(
        "unsupported",
        "X did not expose an enabled submit control for the confirmed reply. Nothing was submitted.",
      );
    }
    const previousNotifications = notificationTexts();
    trace("submit shortcut");
    dispatchSubmitShortcut(composer);
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    const nothingHappened =
      editorText(composer) === clean(options.text) &&
      notificationTexts().length === previousNotifications.length &&
      composerSubmitButton(scope) !== null;
    if (nothingHappened) {
      trace("submit click");
      (submitButton as HTMLElement).click();
    }
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
    const firstScope = await waitFor(() => composerScope(), 3_000);
    if (!firstScope || !options.text) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post editor. Nothing was submitted.",
      );
    }
    return publishParts(firstScope, options.text, page);
  };

  // Types every confirmed part into the composer open at `firstScope`, with
  // each part's own images, and sends them as one post or thread. `landedOn`
  // is the page the job reports it finished on.
  const publishParts = async (
    firstScope: Element,
    text: string,
    landedOn: { readonly url: string; readonly title: string },
  ): Promise<DriverPageResult> => {
    const parts = options.parts?.length ? options.parts : [text];
    const unsupported = unsupportedComposer(firstScope, parts.length);
    if (unsupported) {
      return unsupported;
    }
    try {
      sessionStorage.removeItem("wande:trace");
    } catch {
      // Storage refused; the trace is only a diagnostic.
    }
    trace(`start ${parts.length} part(s) at ${window.location.href}`);
    // Each part gets its own editor: the first is already open, every next
    // one is added with X's plus button, which lands in a sibling block
    // outside the first post's box, so it is waited for on the whole page.
    let composer: Element | null = null;
    let scope: Element = firstScope;
    for (const [index, part] of parts.entries()) {
      if (index > 0) {
        // X honours only a real press of its plus, so the page script hands
        // the button's position back and is run again once the extension
        // has clicked it through the browser. Every earlier part is found
        // already typed on that second pass.
        const typedEditor: Element = composer ?? scope;
        const selector = `[data-testid="tweetTextarea_${index}"]`;
        composer = composerRoot(typedEditor).querySelector(selector);
        if (!composer) {
          const addButton = await waitFor(
            () => addButtonFor(typedEditor),
            3_000,
          );
          if (!addButton) {
            return failure(
              "unsupported",
              `X did not offer to add post ${index + 1} of the thread. Nothing was submitted.`,
            );
          }
          const rect = addButton.getBoundingClientRect();
          trace(`asking for a real click on the plus for post ${index + 1}`);
          return {
            state: "waiting",
            trustedClick: {
              x: rect.left + rect.width / 2,
              y: rect.top + rect.height / 2,
            },
          };
        }
        scope = (await waitFor(() => composerScope(selector), 3_000)) ?? scope;
      } else {
        composer = await waitFor(
          () => scope.querySelector('[data-testid="tweetTextarea_0"]'),
          5_000,
        );
        if (!composer) {
          return failure(
            "unsupported",
            "X did not expose the confirmed post editor. Nothing was submitted.",
          );
        }
      }
      if (
        editorText(composer) !== clean(part) &&
        !(await typeComposerText(composer, part))
      ) {
        return failure(
          "unsupported",
          "X did not accept the post text. Check the visible composer and try again; nothing was submitted.",
        );
      }
      if (editorText(composer) !== clean(part)) {
        return failure(
          "unsupported",
          "X rejected the post text in the confirmed editor. Nothing was submitted.",
        );
      }
      trace(`typed part ${index + 1}`);
      // Every part gets its own images, verified attached in that part's own
      // composer before the thread ever moves on to add the next one.
      const mediaGuard = guardComposerMedia(composer, scope, options.partImages?.[index] ?? []);
      if (mediaGuard) {
        return mediaGuard;
      }
    }
    if (!composer) {
      return failure(
        "unsupported",
        "X did not expose the confirmed post editor. Nothing was submitted.",
      );
    }
    const lastPart = parts[parts.length - 1] ?? "";
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
    trace("submit shortcut");
    dispatchSubmitShortcut(composer);
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    const nothingHappened =
      editorText(composer) === clean(lastPart) &&
      notificationTexts().length === previousNotifications.length &&
      composerSubmitButton(scope) !== null;
    if (nothingHappened) {
      trace("submit click");
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
      ...landedOn,
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
  if (options.action === "submit_repost") {
    return submitRepost();
  }
  if (options.action === "submit_quote") {
    return submitQuote();
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
    // The header's spans run together with no separator: the name, the
    // handle, then whatever badge X adds. Each is read on its own.
    const headerTexts = Array.from(nameNode?.querySelectorAll("span") ?? [])
      .map((span) => clean(span.textContent))
      .filter(Boolean);
    const displayName = (
      headerTexts.find((text) => !text.startsWith("@")) ??
      clean(nameNode?.textContent)
    ).slice(0, 256);
    // X draws the profile header a beat after the page loads, so an empty
    // header is not yet a missing profile.
    if (!displayName) {
      return failure("waiting", "X has not shown the profile yet.");
    }
    const visibleHandle = headerTexts
      .find((text) => /^@[A-Za-z0-9_]{1,50}$/u.test(text))
      ?.slice(1)
      .toLowerCase();
    if (visibleHandle && visibleHandle !== handle.toLowerCase()) {
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
        posts.length === 0 ? "waiting" : "target_not_found",
        "The requested X post was not visible. No post data was returned.",
      );
    }
    return { ...baseData("x_post", target.text), post: target };
  }
  if (options.action === "read_post") {
    if (!requestedPostId || targetUrlPostId !== requestedPostId) {
      return failure(
        "target_not_found",
        "The post ID did not match the target URL. No post data was returned.",
      );
    }
    if (!target) {
      return failure(
        posts.length === 0 ? "waiting" : "target_not_found",
        "The requested X post was not visible. No post data was returned.",
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
