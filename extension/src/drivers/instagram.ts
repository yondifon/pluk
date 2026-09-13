import type { DriverPageResult, DriverScriptOptions } from "./types";

const MAX_COMMENTS = 300;
const COMMENT_LOAD_TIMEOUT_MS = 45_000;
const MAX_GRID_ENTRIES = 120;
const GRID_LOAD_TIMEOUT_MS = 30_000;

// "p" and "reel" are Instagram's own post routes, so they can never be a
// real profile handle even though the pattern would otherwise accept them.
const RESERVED_PROFILE_PATHS = new Set(["p", "reel"]);

export function runInstagramPage(
  options: DriverScriptOptions,
): DriverPageResult | Promise<DriverPageResult> {
  const clean = (value: string | null | undefined): string =>
    (value ?? "").replace(/\s+/gu, " ").trim();
  // Trims only the ends: some captured text (a profile's display name) keeps
  // meaningful internal spacing that `clean` would otherwise collapse away.
  const trimOnly = (value: string | null | undefined): string =>
    (value ?? "").trim();
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
      "Instagram needs an active sign-in. Sign in to the Chrome window Pluk opened, then try again.",
    );

  const bodyText = clean(document.body?.innerText ?? "");
  const path = window.location.pathname;
  if (
    /^\/accounts\/login(?:\/|$)/iu.test(path) ||
    (/\b(?:log in|sign in)\b/iu.test(
      `${document.title} ${bodyText.slice(0, 4_000)}`,
    ) &&
      bodyText.length < 160)
  ) {
    return loginRequired();
  }

  const shortcodeFromPath = (value: string): string | null =>
    value.match(
      /^\/(?:[A-Za-z0-9_.]{1,30}\/)?(?:p|reel)\/([A-Za-z0-9_-]+)/u,
    )?.[1] ?? null;

  const canonicalPostTarget = (shortcode: string): string =>
    `https://www.instagram.com/p/${shortcode}/`;

  const metaContent = (property: string): string | null =>
    document
      .querySelector(`meta[property="${property}"]`)
      ?.getAttribute("content") ?? null;

  // Instagram's obfuscated classes give no stable hook for the post's own
  // author, caption or engagement, but its Open Graph tags are a standard,
  // semantic surface it keeps populated on every post page.
  const authorFromMeta = (): string | null =>
    metaContent("og:title")?.match(/^([A-Za-z0-9_.]{1,30})\s+on\s+Instagram/iu)
      ?.[1] ?? null;

  const engagementFromMeta = (): string | null => {
    const description = metaContent("og:description") ?? "";
    return clean(description.split(" - ")[0] ?? "") || null;
  };

  const captionFromMeta = (): string => {
    const description = metaContent("og:description") ?? "";
    const quoted = description.match(/:\s*"([\s\S]*)"\s*$/u)?.[1];
    return clean(quoted ?? description);
  };

  const isCommentRow = (node: Element): boolean =>
    node.querySelector("time[datetime]") !== null;

  // The comments ul carries no stable class of its own, so it is picked out
  // as whichever ul on the page holds the most direct comment-row children.
  const commentListRoot = (): Element | null => {
    let best: Element | null = null;
    let bestCount = 0;
    for (const list of Array.from(document.querySelectorAll("ul"))) {
      const rows = Array.from(list.querySelectorAll(":scope > li")).filter(
        isCommentRow,
      );
      if (rows.length > bestCount) {
        best = list;
        bestCount = rows.length;
      }
    }
    return best;
  };

  const mainPostTime = (): string | null => {
    const root = commentListRoot();
    const times = Array.from(document.querySelectorAll("time[datetime]"));
    const outside = times.find((time) => !root || !root.contains(time));
    return (outside ?? times[0])?.getAttribute("datetime") ?? null;
  };

  interface ReadPost {
    readonly postId: string;
    readonly targetUrl: string;
    readonly canonicalTarget: string;
    readonly author: string;
    readonly postedAt: string | null;
    readonly engagement: string | null;
    readonly text: string;
  }

  const readMainPost = (shortcode: string): ReadPost | null => {
    const author = authorFromMeta();
    const postedAt = mainPostTime();
    if (!author && !postedAt) {
      return null;
    }
    return {
      postId: shortcode,
      targetUrl: options.targetUrl,
      canonicalTarget: canonicalPostTarget(shortcode),
      author: author ?? "",
      postedAt,
      engagement: engagementFromMeta(),
      text: captionFromMeta(),
    };
  };

  // Instagram's SPA can land the navigation before it has drawn the post, so
  // an absent post is read again until it appears or the deadline passes.
  const awaitMainPost = async (shortcode: string): Promise<ReadPost | null> => {
    const deadline = Date.now() + 5_000;
    for (;;) {
      const found = readMainPost(shortcode);
      if (found || Date.now() >= deadline) {
        return found;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  };

  interface ReadComment {
    readonly commentId: string;
    readonly author: string;
    readonly text: string;
    readonly postedAt: string | null;
    readonly likes: string | null;
    readonly replies: readonly ReadComment[];
  }

  const commentIdFromNode = (node: Element): string =>
    node.querySelector('a[href*="/c/"]')?.getAttribute("href")?.match(/\/c\/([^/?#]+)/u)
      ?.[1] ?? "";

  const authorFromNode = (node: Element): string => {
    const altUsername = node
      .querySelector("img[alt]")
      ?.getAttribute("alt")
      ?.match(/^([A-Za-z0-9_.]{1,30})/u)?.[1];
    if (altUsername) {
      return altUsername;
    }
    for (const link of Array.from(node.querySelectorAll("a[href]"))) {
      const handle = (link.getAttribute("href") ?? "").match(
        /^\/([A-Za-z0-9_.]{1,30})\/?$/u,
      )?.[1];
      if (handle && !RESERVED_PROFILE_PATHS.has(handle.toLowerCase())) {
        return handle;
      }
    }
    return "";
  };

  // No selector for a comment's like count was captured live; this scans a
  // row's own text for the count pattern Instagram renders next to it.
  const likesFromNode = (node: Element): string | null => {
    for (const el of Array.from(node.querySelectorAll("span, button"))) {
      const text = clean(el.textContent);
      if (/^\d[\d,.]*\s*likes?$/iu.test(text)) {
        return text;
      }
    }
    return null;
  };

  // `_a9ym`, `_ap3a` and `_a9yi` are obfuscated Instagram classes captured
  // from a live post; they are the least-bad hook available until Instagram
  // exposes something semantic for a comment's body and its reply thread.
  const repliesFor = (node: Element): Element[] => {
    const list = node.querySelector("ul._a9ym") ?? node.querySelector("ul");
    if (!list) {
      return [];
    }
    return Array.from(list.querySelectorAll(":scope > li")).filter(
      isCommentRow,
    );
  };

  const readComment = (node: Element, isReply: boolean): ReadComment => ({
    commentId: commentIdFromNode(node),
    author: authorFromNode(node),
    text: clean(node.querySelector("span._ap3a")?.textContent).slice(0, 2_000),
    postedAt:
      node.querySelector("time[datetime]")?.getAttribute("datetime") ?? null,
    likes: likesFromNode(node),
    replies: isReply
      ? []
      : repliesFor(node).map((reply) => readComment(reply, true)),
  });

  const readComments = (): ReadComment[] => {
    const root = commentListRoot();
    if (!root) {
      return [];
    }
    return Array.from(root.querySelectorAll(":scope > li"))
      .filter(isCommentRow)
      .map((node) => readComment(node, false));
  };

  const countCommentRows = (): number => {
    const root = commentListRoot();
    return root ? root.querySelectorAll("li").length : 0;
  };

  const loadMoreCommentsButton = (): Element | null => {
    const scope = commentListRoot() ?? document;
    return (
      Array.from(scope.querySelectorAll("button")).find(
        (button) =>
          button.querySelector('svg[aria-label="Load more comments"]') !==
          null,
      ) ?? null
    );
  };

  const collapsedReplyButton = (): Element | null => {
    const root = commentListRoot();
    if (!root) {
      return null;
    }
    for (const span of Array.from(root.querySelectorAll("span._a9yi"))) {
      if (/^view replies/iu.test(clean(span.textContent))) {
        return span.closest("button") ?? span;
      }
    }
    return null;
  };

  const pollUntil = async (
    predicate: () => boolean,
    timeoutMs: number,
  ): Promise<boolean> => {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      if (predicate()) {
        return true;
      }
      if (Date.now() >= deadline) {
        return false;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  };

  const clickElement = (element: Element): void =>
    (element as HTMLElement).click();

  const loadAllComments = async (): Promise<{ readonly truncated: boolean }> => {
    const deadline = Date.now() + COMMENT_LOAD_TIMEOUT_MS;
    const capped = (): boolean =>
      Date.now() >= deadline || countCommentRows() >= MAX_COMMENTS;

    while (!capped()) {
      const button = loadMoreCommentsButton();
      if (!button) {
        break;
      }
      const before = countCommentRows();
      clickElement(button);
      const grew = await pollUntil(
        () => countCommentRows() > before || !loadMoreCommentsButton(),
        5_000,
      );
      if (!grew) {
        break;
      }
    }

    while (!capped()) {
      const button = collapsedReplyButton();
      if (!button) {
        break;
      }
      const before = countCommentRows();
      clickElement(button);
      const grew = await pollUntil(() => countCommentRows() > before, 5_000);
      if (!grew) {
        break;
      }
    }

    return { truncated: capped() };
  };

  const page = meta();

  const baseData = (kind: string, text: string): DriverPageResult => ({
    state: "ready",
    kind,
    ...page,
    platform: "instagram",
    text: text.slice(0, 8_000),
  });

  const shortcode = shortcodeFromPath(path);

  // A `<br>` inside the bio leaves no trace in `textContent`, so the bio's
  // own subtree is walked and each `<br>` is turned back into `\n`.
  const bioTextFrom = (element: Element | null): string => {
    if (!element) {
      return "";
    }
    const parts: string[] = [];
    const visit = (node: Node): void => {
      if ((node as Element).tagName === "BR") {
        parts.push("\n");
        return;
      }
      if (node.nodeType === 3) {
        parts.push(node.textContent ?? "");
        return;
      }
      for (const child of Array.from(node.childNodes ?? [])) {
        visit(child);
      }
    };
    for (const child of Array.from(element.childNodes ?? [])) {
      visit(child);
    }
    return trimOnly(parts.join(""));
  };

  const AUTO_SPAN_SELECTOR = 'header span[dir="auto"]';
  const BIO_SELECTOR = 'span._ap3a._aaco._aacu._aacx._aad7._aade[dir="auto"]';
  const CATEGORY_SELECTOR = "div._ap3a._aaco._aacu._aacy";
  const EXTERNAL_LINK_SELECTOR = 'a[href^="https://l.instagram.com/?u="]';
  const COUNT_SUFFIXES = [" posts", " followers", " following"] as const;

  const isCountText = (text: string): boolean =>
    COUNT_SUFFIXES.some((suffix) => text.endsWith(suffix));

  interface CountValue {
    readonly exact: string | null;
    readonly label: string;
  }

  // The exact figure lives in the inner span's `title`; its own text is the
  // abbreviated one Instagram draws ("60.5K"). Both are worth keeping.
  const readCount = (spans: readonly Element[], suffix: string): CountValue => {
    const span = spans.find((candidate) =>
      trimOnly(candidate.textContent).endsWith(suffix),
    );
    if (!span) {
      return { exact: null, label: "" };
    }
    const inner = span.querySelector("span[title]");
    return {
      exact: inner?.getAttribute("title") ?? null,
      label: inner
        ? trimOnly(inner.textContent)
        : trimOnly(span.textContent).slice(0, -suffix.length),
    };
  };

  interface GridEntry {
    readonly shortcode: string;
    readonly canonicalTarget: string;
    readonly caption: string;
    readonly kind: "reel" | "post";
    readonly pinned: boolean;
  }

  const GRID_ENTRY_HREF_PATTERN =
    /^\/[A-Za-z0-9_.]{1,30}\/(p|reel)\/([A-Za-z0-9_-]+)\/?$/u;

  const readGridEntry = (anchor: Element): GridEntry | null => {
    const href = anchor.getAttribute("href") ?? "";
    const match = href.match(GRID_ENTRY_HREF_PATTERN);
    if (!match) {
      return null;
    }
    const [, kindSegment, entryShortcode] = match;
    const caption = trimOnly(
      anchor.querySelector("div._aagu > div._aagv img")?.getAttribute("alt"),
    );
    const isClip = anchor.querySelector('svg[aria-label="Clip"]') !== null;
    const pinned =
      anchor.querySelector('svg[aria-label="Pinned post icon"]') !== null;
    return {
      shortcode: entryShortcode ?? "",
      canonicalTarget: `https://www.instagram.com${href.endsWith("/") ? href : `${href}/`}`,
      caption,
      kind: isClip || kindSegment === "reel" ? "reel" : "post",
      pinned,
    };
  };

  const readGridEntries = (): GridEntry[] =>
    Array.from(document.querySelectorAll("div._ac7v a[href]"))
      .map(readGridEntry)
      .filter((entry): entry is GridEntry => entry !== null);

  const countGridEntries = (): number => readGridEntries().length;

  // Scrolled to the bottom repeatedly until a scroll adds nothing new, the
  // same growth-polling shape `loadAllComments` uses for its own load-more
  // button, bounded here by entry count and wall time instead of a cap click.
  const loadGridEntries = async (): Promise<{ readonly truncated: boolean }> => {
    const deadline = Date.now() + GRID_LOAD_TIMEOUT_MS;
    const capped = (): boolean =>
      Date.now() >= deadline || countGridEntries() >= MAX_GRID_ENTRIES;

    while (!capped()) {
      if (typeof window.scrollTo !== "function") {
        break;
      }
      const before = countGridEntries();
      window.scrollTo(0, document.body?.scrollHeight ?? 0);
      const grew = await pollUntil(() => countGridEntries() > before, 5_000);
      if (!grew) {
        break;
      }
    }

    return { truncated: capped() };
  };

  const readProfile = async (handle: string): Promise<DriverPageResult> => {
    const autoSpans = Array.from(document.querySelectorAll(AUTO_SPAN_SELECTOR));
    const bioNode = document.querySelector(BIO_SELECTOR);
    // The display name is whichever header span[dir="auto"] is neither the
    // bio nor one of the three count spans.
    const displayNameSpan = autoSpans.find((span) => {
      const text = trimOnly(span.textContent);
      return span !== bioNode && text !== "" && !isCountText(text);
    });
    const displayName = trimOnly(displayNameSpan?.textContent).slice(0, 256);
    if (!displayName) {
      return failure("waiting", "Instagram has not shown the profile yet.");
    }
    const category =
      clean(document.querySelector(CATEGORY_SELECTOR)?.textContent) || null;
    const bio = bioTextFrom(bioNode).slice(0, 1_000);
    const externalLink =
      clean(document.querySelector(EXTERNAL_LINK_SELECTOR)?.textContent) ||
      null;
    const counts = {
      posts: readCount(autoSpans, " posts"),
      followers: readCount(autoSpans, " followers"),
      following: readCount(autoSpans, " following"),
    };
    const { truncated } = await loadGridEntries();
    const posts = readGridEntries().slice(0, MAX_GRID_ENTRIES);
    return {
      ...baseData("instagram_profile", bio || displayName),
      canonicalTarget: `https://www.instagram.com/${handle}/`,
      handle,
      displayName,
      category,
      bio,
      externalLink,
      counts,
      posts,
      truncated,
    };
  };

  if (options.action === "read_profile") {
    const handle = path.match(/^\/([A-Za-z0-9_.]{1,30})\/?$/u)?.[1];
    if (!handle || RESERVED_PROFILE_PATHS.has(handle.toLowerCase())) {
      return failure(
        "unsupported",
        "This target is not an Instagram profile page. Use read_post for a single post.",
      );
    }
    return readProfile(handle);
  }

  const readPost = async (): Promise<DriverPageResult> => {
    const requestedPostId = options.postId ?? shortcode;
    if (!requestedPostId || shortcode !== requestedPostId) {
      return failure(
        "target_not_found",
        "The post ID did not match the target URL. No post data was returned.",
      );
    }
    const target = await awaitMainPost(requestedPostId);
    if (!target) {
      return failure(
        "waiting",
        "The requested Instagram post was not visible. No post data was returned.",
      );
    }
    const { truncated } = await loadAllComments();
    return {
      ...baseData("instagram_post", target.text),
      canonicalTarget: target.canonicalTarget,
      post: target,
      comments: readComments(),
      truncated,
    };
  };

  const readCurrentPost = async (): Promise<DriverPageResult> => {
    if (!shortcode) {
      return failure(
        "target_not_found",
        "This page is not a recognized Instagram post. No post data was returned.",
      );
    }
    const target = await awaitMainPost(shortcode);
    if (!target) {
      return failure("waiting", "Instagram has not shown the post yet.");
    }
    return {
      ...baseData("instagram_post", target.text),
      canonicalTarget: target.canonicalTarget,
      post: target,
    };
  };

  if (options.action === "read_post") {
    return readPost();
  }

  if (
    options.action === "inspect" ||
    options.action === "capture" ||
    options.action === "refresh"
  ) {
    return readCurrentPost();
  }

  return failure(
    "unsupported",
    "This Instagram action is not supported by the site driver.",
  );
}
