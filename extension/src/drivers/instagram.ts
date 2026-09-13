import type { DriverPageResult, DriverScriptOptions } from "./types";

const MAX_COMMENTS = 300;
const COMMENT_LOAD_TIMEOUT_MS = 45_000;

// "p" and "reel" are Instagram's own post routes, so they can never be a
// real profile handle even though the pattern would otherwise accept them.
const RESERVED_PROFILE_PATHS = new Set(["p", "reel"]);

export function runInstagramPage(
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
    value.match(/^\/(?:p|reel)\/([A-Za-z0-9_-]+)/u)?.[1] ?? null;

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

  if (options.action === "read_profile") {
    const handle = path.match(/^\/([A-Za-z0-9_.]{1,30})\/?$/u)?.[1];
    if (!handle || RESERVED_PROFILE_PATHS.has(handle.toLowerCase())) {
      return failure(
        "unsupported",
        "This target is not an Instagram profile page. Use read_post for a single post.",
      );
    }
    const displayName = clean(
      metaContent("og:title")?.match(/^(.*?)\s*\(@/u)?.[1],
    ).slice(0, 256);
    if (!displayName) {
      return failure("waiting", "Instagram has not shown the profile yet.");
    }
    const bio = clean(
      document.querySelector('header span[dir="auto"]')?.textContent,
    ).slice(0, 1_000);
    return {
      ...baseData("instagram_profile", bio || displayName),
      canonicalTarget: `https://www.instagram.com/${handle}/`,
      handle,
      displayName,
      bio,
    };
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
