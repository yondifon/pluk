import type { DriverPageResult, DriverScriptOptions } from "./types";

export function runInstagramPage(
  options: DriverScriptOptions,
): DriverPageResult | Promise<DriverPageResult> {
  const MAX_COMMENTS = 300;
  const COMMENT_LOAD_TIMEOUT_MS = 45_000;
  // A page that does not grow is not a page that is finished: Instagram drops
  // a scroll while a fetch is already in flight, so a miss is retried before
  // the run gives up on the thread.
  const COMMENT_SCROLL_ATTEMPTS = 4;
  const COMMENT_FETCH_TRIGGER_TIMEOUT_MS = 2_000;
  const MAX_GRID_ENTRIES = 600;
  const GRID_LOAD_TIMEOUT_MS = 90_000;
  const GRID_GROWTH_TIMEOUT_MS = 5_000;
  const CAPTURE_ELEMENT_ID = "pluk-instagram-captures";
  const CLIP_TYPENAMES = new Set([
    "XIGPolarisClipsMedia",
    "XIGPolarisVideoMedia",
    "XIGPolarisReelMedia",
  ]);
  const LIKE_KEYS = [
    "like_count",
    "comment_like_count",
    "edge_media_preview_like_count",
  ] as const;
  const VIEW_KEYS = ["view_count", "play_count", "video_view_count"] as const;
  const COMMENT_COUNT_KEYS = [
    "comment_count",
    "edge_media_to_comment_count",
  ] as const;

  interface CaptureEntry {
    readonly url: string;
    readonly method: string;
    readonly receivedAt: number;
    readonly body: unknown;
  }

  // The capture layer (a separate MAIN-world content script) publishes
  // Instagram's own fetch/XHR JSON responses into this hidden element so the
  // driver can read exact counts instead of scraping the rendered markup.
  const captureRawText = (): string | null =>
    document.getElementById(CAPTURE_ELEMENT_ID)?.textContent ?? null;

  const parseCaptureText = (raw: string | null): readonly CaptureEntry[] => {
    if (!raw) {
      return [];
    }
    try {
      const parsed: unknown = JSON.parse(raw);
      return Array.isArray(parsed) ? (parsed as CaptureEntry[]) : [];
    } catch {
      return [];
    }
  };

  const readCaptures = (): readonly CaptureEntry[] =>
    parseCaptureText(captureRawText());

  // Depth-first walk over every plain object nested in a captured JSON body.
  // Instagram's response shapes are not stable enough to key a fixed path
  // against, so fields are located by name wherever they appear instead.
  const walkObjects = (
    value: unknown,
    visit: (candidate: Record<string, unknown>) => void,
    depth = 0,
  ): void => {
    if (depth > 24 || value === null || typeof value !== "object") {
      return;
    }
    if (Array.isArray(value)) {
      for (const item of value) {
        walkObjects(item, visit, depth + 1);
      }
      return;
    }
    const record = value as Record<string, unknown>;
    visit(record);
    for (const key of Object.keys(record)) {
      walkObjects(record[key], visit, depth + 1);
    }
  };

  const firstNumber = (
    record: Record<string, unknown>,
    keys: readonly string[],
  ): number | null => {
    for (const key of keys) {
      const value = record[key];
      if (typeof value === "number") {
        return value;
      }
    }
    return null;
  };

  // Instagram never links out directly: an anchor's href and a bio link's
  // `lynx_url` are both `l.instagram.com` redirects carrying the real
  // destination in a `u=` query param.
  const decodeRedirectUrl = (href: string): string | null => {
    try {
      const target = new URL(href, "https://www.instagram.com/").searchParams.get(
        "u",
      );
      return target ? decodeURIComponent(target) : null;
    } catch {
      return null;
    }
  };

  interface CapturedEngagement {
    readonly likes: number | null;
    readonly views: number | null;
    readonly commentCount: number | null;
  }

  const engagementFromObject = (
    record: Record<string, unknown>,
    includeViews: boolean,
  ): CapturedEngagement => ({
    likes: firstNumber(record, LIKE_KEYS),
    views: includeViews ? firstNumber(record, VIEW_KEYS) : null,
    commentCount: firstNumber(record, COMMENT_COUNT_KEYS),
  });

  const NO_ENGAGEMENT: CapturedEngagement = {
    likes: null,
    views: null,
    commentCount: null,
  };

  interface CapturedLink {
    readonly label: string;
    readonly url: string;
  }

  interface CapturedProfile {
    readonly followers: number | null;
    readonly following: number | null;
    readonly posts: number | null;
    readonly verified: boolean | null;
    readonly private: boolean | null;
    readonly links: readonly CapturedLink[];
    readonly address: string | null;
  }

  // Matched on `username` alongside `follower_count`, since either field
  // alone could belong to an unrelated captured object (a suggested
  // account, a comment author, and so on). The object's own path is not
  // trusted: it has moved once already between Meta's own query shapes.
  const findCapturedProfile = (
    captures: readonly CaptureEntry[],
    handle: string,
  ): CapturedProfile | null => {
    let found: Record<string, unknown> | null = null;
    for (const entry of captures) {
      walkObjects(entry.body, (candidate) => {
        if (found) {
          return;
        }
        if (
          typeof candidate.username === "string" &&
          candidate.username.toLowerCase() === handle.toLowerCase() &&
          typeof candidate.follower_count === "number"
        ) {
          found = candidate;
        }
      });
      if (found) {
        break;
      }
    }
    if (!found) {
      return null;
    }
    const record: Record<string, unknown> = found;
    const bioLinks = Array.isArray(record.bio_links) ? record.bio_links : [];
    const links: CapturedLink[] = [];
    for (const item of bioLinks) {
      if (item === null || typeof item !== "object") {
        continue;
      }
      const link = item as Record<string, unknown>;
      const url =
        typeof link.lynx_url === "string" ? decodeRedirectUrl(link.lynx_url) : null;
      if (!url) {
        continue;
      }
      const label = typeof link.title === "string" && link.title ? link.title : url;
      links.push({ label, url });
    }
    const addressParts = [record.address_street, record.city_name, record.zip]
      .filter((part): part is string => typeof part === "string" && part !== "");
    return {
      followers: firstNumber(record, ["follower_count"]),
      following: firstNumber(record, ["following_count"]),
      posts: firstNumber(record, ["media_count"]),
      verified:
        typeof record.is_verified === "boolean" ? record.is_verified : null,
      private: typeof record.is_private === "boolean" ? record.is_private : null,
      links,
      address: addressParts.length ? addressParts.join(", ") : null,
    };
  };

  interface CapturedGridEntry extends CapturedEngagement {
    readonly shortcode: string;
    readonly caption: string;
    readonly kind: "reel" | "post";
    readonly pinned: boolean;
    readonly postedAt: string | null;
  }

  const isVideoLikeMedia = (
    mediaDict: Record<string, unknown>,
    typename: string,
  ): boolean =>
    CLIP_TYPENAMES.has(typename) ||
    mediaDict.product_type === "clips" ||
    mediaDict.media_type === 2;

  // Instagram nests a media's caption as either a plain string or a
  // `{ text }` object depending on the endpoint that returned it.
  const captionFromMediaDict = (mediaDict: Record<string, unknown>): string => {
    const caption = mediaDict.caption;
    if (typeof caption === "string") {
      return clean(caption);
    }
    if (caption !== null && typeof caption === "object") {
      const text = (caption as Record<string, unknown>).text;
      if (typeof text === "string") {
        return clean(text);
      }
    }
    return "";
  };

  const buildCapturedGridEntry = (
    mediaDict: Record<string, unknown>,
    typename: string,
  ): CapturedGridEntry => {
    const isVideoLike = isVideoLikeMedia(mediaDict, typename);
    const takenAt = firstNumber(mediaDict, ["taken_at"]);
    return {
      shortcode: mediaDict.code as string,
      caption: captionFromMediaDict(mediaDict),
      kind: isVideoLike ? "reel" : "post",
      pinned:
        Array.isArray(mediaDict.timeline_pinned_user_ids) &&
        mediaDict.timeline_pinned_user_ids.length > 0,
      postedAt: takenAt !== null ? new Date(takenAt * 1_000).toISOString() : null,
      ...engagementFromObject(mediaDict, isVideoLike),
    };
  };

  // A timeline edge carries its media on the node itself; the logged-out
  // profile query wraps the same fields in a `media_dict` instead. `taken_at`
  // separates a real media object from the id-only references that share the
  // `code` field elsewhere in the same response.
  const gridMediaFrom = (
    candidate: Record<string, unknown>,
  ): Record<string, unknown> | null => {
    const nested =
      candidate.media_dict !== null && typeof candidate.media_dict === "object"
        ? (candidate.media_dict as Record<string, unknown>)
        : null;
    const media = nested ?? candidate;
    return typeof media.code === "string" &&
      typeof media.taken_at === "number"
      ? media
      : null;
  };

  // Drains whatever timeline pages are currently buffered into a
  // shortcode-keyed accumulator that outlives any single capture snapshot:
  // the capture element keeps only the last 40 responses, so a long scroll
  // evicts early pages that must already have been merged in by then.
  const drainCapturedGridEntries = (
    captures: readonly CaptureEntry[],
    accumulator: Map<string, CapturedGridEntry>,
  ): void => {
    for (const entry of captures) {
      walkObjects(entry.body, (candidate) => {
        const mediaDict = gridMediaFrom(candidate);
        if (!mediaDict || accumulator.has(mediaDict.code as string)) {
          return;
        }
        const typename =
          typeof mediaDict.__typename === "string" ? mediaDict.__typename : "";
        accumulator.set(
          mediaDict.code as string,
          buildCapturedGridEntry(mediaDict, typename),
        );
      });
    }
  };

  interface CapturedPost extends CapturedEngagement {
    readonly author: string | null;
    readonly caption: string;
    readonly postedAt: string | null;
  }

  // A media object's poster sits either directly on a `username` field or
  // nested under `owner`/`user`, depending on which endpoint returned it.
  const usernameFromRecord = (record: Record<string, unknown>): string | null => {
    if (typeof record.username === "string" && record.username) {
      return record.username;
    }
    for (const key of ["owner", "user"] as const) {
      const nested = record[key];
      if (nested !== null && typeof nested === "object") {
        const username = (nested as Record<string, unknown>).username;
        if (typeof username === "string" && username) {
          return username;
        }
      }
    }
    return null;
  };

  // Matches the post's own media object by shortcode, then reads whatever
  // of its author, timestamp and engagement that object happens to carry.
  const findCapturedPost = (
    captures: readonly CaptureEntry[],
    shortcode: string,
  ): CapturedPost | null => {
    let found: Record<string, unknown> | null = null;
    let foundTypename = "";
    for (const entry of captures) {
      walkObjects(entry.body, (candidate) => {
        if (found) {
          return;
        }
        const code =
          typeof candidate.code === "string"
            ? candidate.code
            : typeof candidate.shortcode === "string"
              ? candidate.shortcode
              : null;
        if (code !== shortcode) {
          return;
        }
        const hasSignal =
          firstNumber(candidate, LIKE_KEYS) !== null ||
          firstNumber(candidate, COMMENT_COUNT_KEYS) !== null ||
          firstNumber(candidate, ["taken_at"]) !== null ||
          usernameFromRecord(candidate) !== null;
        if (!hasSignal) {
          return;
        }
        found = candidate;
        foundTypename =
          typeof candidate.__typename === "string" ? candidate.__typename : "";
      });
      if (found) {
        break;
      }
    }
    if (!found) {
      return null;
    }
    const record: Record<string, unknown> = found;
    const takenAt = firstNumber(record, ["taken_at"]);
    return {
      author: usernameFromRecord(record),
      caption: captionFromMediaDict(record),
      postedAt: takenAt !== null ? new Date(takenAt * 1_000).toISOString() : null,
      ...engagementFromObject(record, CLIP_TYPENAMES.has(foundTypename)),
    };
  };

  // Instagram embeds a large JSON state blob directly in a post page's own
  // HTML on first load; when nothing was fetched for the capture layer to
  // intercept, this is checked before falling back to the page's rendered
  // markup. The capture element is excluded so it is never read as if it
  // were the page's own embedded state.
  const embeddedJsonEntries = (): readonly CaptureEntry[] =>
    Array.from(document.querySelectorAll('script[type="application/json"]'))
      .filter((script) => script.getAttribute("id") !== CAPTURE_ELEMENT_ID)
      .map((script): CaptureEntry | null => {
        try {
          return {
            url: "",
            method: "GET",
            receivedAt: 0,
            body: JSON.parse(script.textContent ?? ""),
          };
        } catch {
          return null;
        }
      })
      .filter((entry): entry is CaptureEntry => entry !== null);

  // The post URL's own username segment (`/<handle>/p|reel/<shortcode>/`)
  // is the cheapest reliable author source: it costs no parsing of
  // Instagram's own unstable text, and is absent only from the bare
  // `/p/<shortcode>/` canonical form.
  const authorFromPath = (value: string): string | null =>
    value.match(/^\/([A-Za-z0-9_.]{1,30})\/(?:p|reel)\//u)?.[1] ?? null;

  // Instagram never abbreviates a comment count the way it abbreviates
  // likes and views, so a plain digit run next to "comment(s)" in the
  // engagement string is exact and safe to parse as an integer.
  const exactCommentCountFromEngagement = (
    engagement: string | null,
  ): number | null => {
    if (!engagement) {
      return null;
    }
    const match = engagement.match(/([\d,]+)\s*comments?\b/iu);
    const digits = match?.[1]?.replace(/,/gu, "");
    return digits ? Number.parseInt(digits, 10) : null;
  };

  // Instagram returns every comment, top level or reply, as the same
  // `XDTCommentDict` shape; only a reply carries `parent_comment_id`.
  const isCommentDict = (
    candidate: Record<string, unknown>,
  ): boolean =>
    typeof candidate.text === "string" &&
    typeof candidate.created_at === "number" &&
    (typeof candidate.pk === "string" || typeof candidate.pk === "number") &&
    candidate.user !== null &&
    typeof candidate.user === "object";

  const findCapturedComments = (
    captures: readonly CaptureEntry[],
  ): readonly Record<string, unknown>[] => {
    const byId = new Map<string, Record<string, unknown>>();
    for (const entry of captures) {
      walkObjects(entry.body, (candidate) => {
        if (!isCommentDict(candidate)) {
          return;
        }
        const id = String(candidate.pk);
        if (!byId.has(id)) {
          byId.set(id, candidate);
        }
      });
    }
    return Array.from(byId.values());
  };

  // "p" and "reel" are Instagram's own post routes, so they can never be a
  // real profile handle even though the pattern would otherwise accept them.
  const RESERVED_PROFILE_PATHS = new Set(["p", "reel"]);
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
    readonly likes: number | null;
    readonly views: number | null;
    readonly commentCount: number | null;
  }

  type PostSource = "captured" | "embedded" | "scraped";

  const readMainPost = (
    shortcode: string,
  ): { readonly post: ReadPost; readonly source: PostSource } | null => {
    const capturedPost = findCapturedPost(readCaptures(), shortcode);
    const embeddedPost = capturedPost
      ? null
      : findCapturedPost(embeddedJsonEntries(), shortcode);
    const jsonPost = capturedPost ?? embeddedPost;
    const engagement = engagementFromMeta();
    const author =
      jsonPost?.author ?? authorFromPath(path) ?? authorFromMeta() ?? "";
    const postedAt = jsonPost?.postedAt ?? mainPostTime();
    if (!author && !postedAt) {
      return null;
    }
    const likes = jsonPost?.likes ?? null;
    const commentCount =
      jsonPost?.commentCount ?? exactCommentCountFromEngagement(engagement);
    return {
      post: {
        postId: shortcode,
        targetUrl: options.targetUrl,
        canonicalTarget: canonicalPostTarget(shortcode),
        author,
        postedAt,
        // Instagram's Open Graph description lags the live counts badly, so
        // it is reported only when the page's own JSON gave no counts and it
        // is therefore the source of whatever is reported here.
        engagement: jsonPost === null ? engagement : null,
        text: jsonPost?.caption || captionFromMeta(),
        likes,
        views: jsonPost?.views ?? null,
        commentCount,
      },
      source: capturedPost ? "captured" : embeddedPost ? "embedded" : "scraped",
    };
  };

  // Instagram's SPA can land the navigation before it has drawn the post, so
  // an absent post is read again until it appears or the deadline passes.
  const awaitMainPost = async (
    shortcode: string,
  ): Promise<{ readonly post: ReadPost; readonly source: PostSource } | null> => {
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
    readonly likes: number | null;
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
  // row's own text for the count pattern Instagram renders next to it. The
  // matched text is always a plain digit run, never an abbreviation, so it
  // converts to an exact integer rather than reporting the rendered string.
  const likesFromNode = (node: Element): number | null => {
    for (const el of Array.from(node.querySelectorAll("span, button"))) {
      const text = clean(el.textContent);
      if (/^\d[\d,.]*\s*likes?$/iu.test(text)) {
        const digits = text.replace(/\D/gu, "");
        return digits ? Number.parseInt(digits, 10) : null;
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
    replies: isReply ? [] : repliesFor(node).map((reply) => readComment(reply, true)),
  });

  const commentFromCaptured = (
    record: Record<string, unknown>,
    replies: readonly ReadComment[],
  ): ReadComment => {
    const createdAt = firstNumber(record, ["created_at"]);
    const user = record.user as Record<string, unknown>;
    return {
      commentId: String(record.pk),
      author: typeof user.username === "string" ? user.username : "",
      text: trimOnly(record.text as string).slice(0, 2_000),
      postedAt:
        createdAt !== null ? new Date(createdAt * 1_000).toISOString() : null,
      likes: firstNumber(record, LIKE_KEYS),
      replies,
    };
  };

  // A post page renders its first comments server side and fetches the rest,
  // so both the embedded state and the captured pages have to be read for a
  // reply to find the parent it belongs under.
  const readCapturedComments = (): ReadComment[] => {
    const records = findCapturedComments([
      ...readCaptures(),
      ...embeddedJsonEntries(),
    ]);
    if (records.length === 0) {
      return [];
    }
    const repliesByParent = new Map<string, ReadComment[]>();
    for (const record of records) {
      const parent = record.parent_comment_id;
      if (typeof parent !== "string" || !parent) {
        continue;
      }
      const bucket = repliesByParent.get(parent) ?? [];
      bucket.push(commentFromCaptured(record, []));
      repliesByParent.set(parent, bucket);
    }
    return records
      .filter((record) => typeof record.parent_comment_id !== "string")
      .map((record) =>
        commentFromCaptured(record, repliesByParent.get(String(record.pk)) ?? []),
      );
  };

  const readComments = (): ReadComment[] => {
    const captured = readCapturedComments();
    if (captured.length > 0) {
      return captured;
    }
    const root = commentListRoot();
    if (!root) {
      return [];
    }
    return Array.from(root.querySelectorAll(":scope > li"))
      .filter(isCommentRow)
      .map((node) => readComment(node, false));
  };

  // Counts what has actually been fetched, not what is rendered: the panel
  // virtualises its rows, so a DOM count stops growing long before the
  // comments do. It has to read the same two sources the reader does, or a
  // load that is working reads as stalled.
  const countCommentRows = (): number => {
    const captured = findCapturedComments([
      ...readCaptures(),
      ...embeddedJsonEntries(),
    ]).length;
    if (captured > 0) {
      return captured;
    }
    const root = commentListRoot();
    return root ? root.querySelectorAll("li").length : 0;
  };

  // The comment panel is its own scroll region, and scrolling it is what
  // makes Instagram fetch the next page. It is the only element on a post
  // page that both overflows and scrolls.
  const commentScroller = (): Element | null =>
    Array.from(document.querySelectorAll("div")).find(
      (node) =>
        /auto|scroll/u.test(getComputedStyle(node).overflowY) &&
        node.scrollHeight > node.clientHeight + 40,
    ) ?? null;

  const collapsedReplyButton = (): Element | null => {
    const root = commentListRoot() ?? document;
    for (const node of Array.from(root.querySelectorAll("span, button"))) {
      if (/^view (all \d+ )?repl(y|ies)/iu.test(clean(node.textContent))) {
        return node.closest("button") ?? node;
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

  // Instagram does not fetch a post's comments until the comment panel is
  // scrolled into view, exactly as it withholds the next timeline page
  // until the grid is scrolled. Without this, a freshly loaded post can
  // show neither a comment row nor a "Load more" button to click.
  const ensureCommentsFetched = async (): Promise<void> => {
    if (countCommentRows() > 0) {
      return;
    }
    const root = commentListRoot() as unknown as {
      readonly scrollIntoView?: () => void;
    } | null;
    if (typeof root?.scrollIntoView === "function") {
      root.scrollIntoView();
    }
    if (typeof window.scrollTo === "function") {
      window.scrollTo(0, document.body?.scrollHeight ?? 0);
    }
    await pollUntil(() => countCommentRows() > 0, COMMENT_FETCH_TRIGGER_TIMEOUT_MS);
  };

  // The post's own comment count is what decides truncation: a scroll that
  // stops growing is not proof the thread is exhausted, and on a post with
  // only a handful of comments it is not proof of anything at all.
  const loadAllComments = async (
    commentCount: number | null,
  ): Promise<{ readonly truncated: boolean }> => {
    await ensureCommentsFetched();
    const deadline = Date.now() + COMMENT_LOAD_TIMEOUT_MS;
    const capped = (): boolean =>
      Date.now() >= deadline ||
      countCommentRows() >= MAX_COMMENTS ||
      (commentCount !== null && countCommentRows() >= commentCount);

    let misses = 0;
    while (!capped() && misses < COMMENT_SCROLL_ATTEMPTS) {
      const scroller = commentScroller();
      if (!scroller) {
        break;
      }
      const before = countCommentRows();
      // Instagram fetches on the scroll event, not on the position, so a
      // panel already sitting at the bottom has to be moved off it first.
      scroller.scrollTop = Math.max(0, scroller.scrollHeight - scroller.clientHeight * 2);
      scroller.scrollTop = scroller.scrollHeight;
      const grew = await pollUntil(() => countCommentRows() > before, 5_000);
      misses = grew ? 0 : misses + 1;
    }

    const expanded = new Set<Element>();
    while (!capped()) {
      const button = collapsedReplyButton();
      if (!button || expanded.has(button)) {
        break;
      }
      expanded.add(button);
      const before = countCommentRows();
      clickElement(button);
      await pollUntil(() => countCommentRows() > before, 5_000);
    }

    return {
      truncated:
        commentCount !== null ? countCommentRows() < commentCount : capped(),
    };
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
  const MULTI_LINK_BUTTON_TEXT = /^.+\s+and\s+\d+\s+more$/iu;
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
    readonly postedAt: string | null;
    readonly likes: number | null;
    readonly views: number | null;
    readonly commentCount: number | null;
  }

  const GRID_ENTRY_HREF_PATTERN =
    /^\/[A-Za-z0-9_.]{1,30}\/(p|reel)\/([A-Za-z0-9_-]+)\/?$/u;

  // The DOM grid carries none of a captured page's engagement or timing
  // data; this is the fallback used only when scrolling never produced a
  // single captured timeline response to accumulate from instead.
  const readGridEntryFromDom = (anchor: Element): GridEntry | null => {
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
      postedAt: null,
      ...NO_ENGAGEMENT,
    };
  };

  const readGridEntriesFromDom = (): GridEntry[] =>
    Array.from(document.querySelectorAll("div._ac7v a[href]"))
      .map(readGridEntryFromDom)
      .filter((entry): entry is GridEntry => entry !== null);

  // Instagram's own permalink shape is `/<username>/<p|reel>/<shortcode>/`;
  // a captured timeline entry carries the shortcode but not that permalink,
  // so it is rebuilt from the profile handle this read is already scoped to.
  const gridEntryFromCaptured = (
    handle: string,
    captured: CapturedGridEntry,
  ): GridEntry => ({
    shortcode: captured.shortcode,
    canonicalTarget: `https://www.instagram.com/${handle}/${captured.kind}/${captured.shortcode}/`,
    caption: captured.caption,
    kind: captured.kind,
    pinned: captured.pinned,
    postedAt: captured.postedAt,
    likes: captured.likes,
    views: captured.views,
    commentCount: captured.commentCount,
  });

  const resolveGridTruncation = (
    postCount: number,
    mediaCount: number | null,
    capped: boolean,
    stalled: boolean,
  ): boolean => (mediaCount !== null ? postCount < mediaCount : capped || stalled);

  // Scrolling makes Instagram fetch the next timeline page; growth is
  // decided by a new page landing in the capture element, never by counting
  // DOM nodes, since the grid only renders what it has already fetched. A
  // scroll that stops growing before the cap or the deadline is not proof
  // the account is exhausted (a fetch can still be in flight), so that
  // "stalled" exit is reported truncated too, unlike a genuine end of scroll.
  //
  // The capture element keeps only its last 40 responses, so a scroll run
  // long enough to pass that many pages would otherwise lose the earliest
  // ones; every newly landed page is drained into `accumulator` as it
  // arrives rather than read once at the end.
  const loadGridPosts = async (
    handle: string,
    mediaCount: number | null,
  ): Promise<{ readonly posts: GridEntry[]; readonly truncated: boolean }> => {
    const accumulator = new Map<string, CapturedGridEntry>();
    let lastRaw = captureRawText();
    drainCapturedGridEntries(parseCaptureText(lastRaw), accumulator);

    const deadline = Date.now() + GRID_LOAD_TIMEOUT_MS;
    const capped = (): boolean =>
      Date.now() >= deadline || accumulator.size >= MAX_GRID_ENTRIES;

    let stalled = false;
    while (!capped()) {
      if (typeof window.scrollTo !== "function") {
        break;
      }
      window.scrollTo(0, document.body?.scrollHeight ?? 0);
      const sizeBefore = accumulator.size;
      const grew = await pollUntil(() => {
        const raw = captureRawText();
        if (raw !== lastRaw) {
          lastRaw = raw;
          drainCapturedGridEntries(parseCaptureText(raw), accumulator);
        }
        return accumulator.size > sizeBefore;
      }, GRID_GROWTH_TIMEOUT_MS);
      if (!grew) {
        stalled = true;
        break;
      }
    }

    const posts =
      accumulator.size > 0
        ? Array.from(accumulator.values())
            .slice(0, MAX_GRID_ENTRIES)
            .map((captured) => gridEntryFromCaptured(handle, captured))
        : readGridEntriesFromDom().slice(0, MAX_GRID_ENTRIES);
    return {
      posts,
      truncated: resolveGridTruncation(posts.length, mediaCount, capped(), stalled),
    };
  };

  interface ExternalLink {
    readonly label: string;
    readonly url: string | null;
  }

  const parseExternalLinkAnchor = (anchor: Element): ExternalLink | null => {
    const label = clean(anchor.textContent);
    if (!label) {
      return null;
    }
    const url = decodeRedirectUrl(anchor.getAttribute("href") ?? "") ?? label;
    return { label, url };
  };

  // A single link renders as an anchor with the real destination in its
  // `u=` redirect param. More than one link collapses into a plain button
  // reading "example.com and 1 more" with no href for any of them, so that
  // case is surfaced as one descriptive entry rather than an invented list.
  const readExternalLinksFromDom = (): ExternalLink[] => {
    const anchor = document.querySelector(EXTERNAL_LINK_SELECTOR);
    if (anchor) {
      const parsed = parseExternalLinkAnchor(anchor);
      return parsed ? [parsed] : [];
    }
    const multiButton = Array.from(document.querySelectorAll("button")).find(
      (button) => MULTI_LINK_BUTTON_TEXT.test(clean(button.textContent)),
    );
    if (!multiButton) {
      return [];
    }
    const label = clean(multiButton.textContent);
    return label ? [{ label, url: null }] : [];
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
    const counts = {
      posts: readCount(autoSpans, " posts"),
      followers: readCount(autoSpans, " followers"),
      following: readCount(autoSpans, " following"),
    };
    const captures = readCaptures();
    const capturedProfile = findCapturedProfile(captures, handle);
    const links = capturedProfile?.links.length
      ? capturedProfile.links
      : readExternalLinksFromDom();
    const externalLink = links[0]?.label ?? links[0]?.url ?? null;
    const { posts, truncated } = await loadGridPosts(
      handle,
      capturedProfile?.posts ?? null,
    );
    const usedCapturedEngagement = posts.some(
      (entry) => entry.likes !== null || entry.views !== null || entry.commentCount !== null,
    );
    return {
      ...baseData("instagram_profile", bio || displayName),
      canonicalTarget: `https://www.instagram.com/${handle}/`,
      handle,
      displayName,
      category,
      bio,
      externalLink,
      links,
      followers: capturedProfile?.followers ?? null,
      following: capturedProfile?.following ?? null,
      postsCount: capturedProfile?.posts ?? null,
      verified: capturedProfile?.verified ?? null,
      private: capturedProfile?.private ?? null,
      address: capturedProfile?.address ?? null,
      counts,
      posts,
      source: capturedProfile || usedCapturedEngagement ? "mixed" : "scraped",
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

  // Reports whichever of the three sources actually won: a scraped post
  // whose comment likes were captured still reports "mixed" rather than
  // claiming the post's own fields came from JSON they did not come from.
  const resolvePostSource = (
    postSource: PostSource,
    usedCommentCapture: boolean,
  ): string =>
    postSource !== "scraped"
      ? postSource
      : usedCommentCapture
        ? "mixed"
        : "scraped";

  // `*` and `?` are the only wildcards a debug glob carries, so the rest of
  // the string is matched literally rather than pulling in a glob library.
  const matchesGlob = (value: string, glob: string): boolean => {
    const pattern = glob.replace(/[.*+?^${}()|[\]\\]/gu, (char) =>
      char === "*" ? ".*" : char === "?" ? "." : `\\${char}`,
    );
    return new RegExp(`^${pattern}$`, "u").test(value);
  };

  // `debug: true` attaches every captured response; a glob narrows that to
  // the ones whose URL matches it. A glob that matched nothing still says
  // so, alongside every URL that was actually recorded, rather than
  // attaching an empty blob with no explanation.
  const debugFields = (): { readonly debugCaptures?: string } => {
    const request = options.debug;
    if (request === undefined) {
      return {};
    }
    const captures = readCaptures();
    const matched =
      request === true
        ? captures
        : captures.filter((entry) => matchesGlob(entry.url, request));
    return {
      debugCaptures: JSON.stringify({
        requested: request,
        matched,
        ...(matched.length === 0
          ? {
              note: "No captured response matched this request.",
              recordedUrls: captures.map((entry) => entry.url),
            }
          : {}),
      }),
    };
  };

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
    const { truncated } = await loadAllComments(target.post.commentCount);
    const comments = readComments().slice(0, MAX_COMMENTS);
    const usedCommentCapture = comments.some(
      (comment) =>
        comment.likes !== null ||
        comment.replies.some((reply) => reply.likes !== null),
    );
    return {
      ...baseData("instagram_post", target.post.text),
      canonicalTarget: target.post.canonicalTarget,
      post: target.post,
      comments,
      source: resolvePostSource(target.source, usedCommentCapture),
      truncated,
      ...debugFields(),
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
      ...baseData("instagram_post", target.post.text),
      canonicalTarget: target.post.canonicalTarget,
      post: target.post,
      source: resolvePostSource(target.source, false),
      ...debugFields(),
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
