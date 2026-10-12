import type { DriverPageResult, DriverScriptOptions } from "./types";

// Injected into the page on its own, so everything it needs lives inside this
// function. Class names are generated; controls are found by component key,
// role, aria attribute and visible text only.
export function runLinkedInPage(
  options: DriverScriptOptions,
): DriverPageResult | Promise<DriverPageResult> {
  const clean = (value: string | null | undefined): string =>
    (value ?? "").replace(/\s+/gu, " ").trim();
  const textOf = (element: Element): string =>
    clean((element as HTMLElement).innerText ?? element.textContent);
  const nameKey = (value: string): string => clean(value).toLowerCase();
  const wait = (milliseconds: number): Promise<void> =>
    new Promise((resolve) => setTimeout(resolve, milliseconds));
  // Result messages are capped at 512 characters on the wire.
  const failure = (
    state: DriverPageResult["state"],
    message: string,
  ): DriverPageResult => ({ state, message: message.slice(0, 500) });

  const loginRequired = (): DriverPageResult =>
    failure(
      "login_required",
      "LinkedIn needs an active sign-in. Sign in to the Chrome window Pluk opened, then try again.",
    );
  const bodyText = clean(document.body?.innerText ?? "");
  const isSignInPage = (): boolean =>
    /^\/(?:login|checkpoint|authwall|uas\/login)(?:\/|$)/iu.test(
      window.location.pathname,
    ) ||
    document.querySelector('input[type="password"]') !== null ||
    (/\b(?:sign in|log in|login)\b/iu.test(
      `${document.title} ${bodyText.slice(0, 4_000)}`,
    ) &&
      bodyText.length < 160);
  if (isSignInPage()) {
    return loginRequired();
  }

  // A submission ends on the feed, not the composer it was asked for, so the
  // result names the address the job was given.
  const landed = {
    url: options.targetUrl,
    title: document.title.slice(0, 512),
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

  const waitFor = async <T>(
    read: () => T | null | undefined | false,
    timeoutMs: number,
  ): Promise<T | null> => {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const found = read();
      if (found) {
        return found;
      }
      if (Date.now() >= deadline) {
        return null;
      }
      await wait(100);
    }
  };

  const isDisabled = (element: Element): boolean =>
    element.getAttribute("disabled") !== null ||
    element.getAttribute("aria-disabled") === "true";

  const composerEditable = (): Element | null => {
    const holder = document.querySelector('[componentkey="ShareBox_textEditor"]');
    if (!holder) {
      return null;
    }
    return holder.getAttribute("contenteditable") === "true"
      ? holder
      : holder.querySelector('div[contenteditable="true"][role="textbox"]');
  };

  const postButtonIn = (box: Element): Element | null =>
    Array.from(box.querySelectorAll("button")).find(
      (button) => textOf(button) === "Post",
    ) ?? null;

  const composerBoxFor = (editor: Element): Element | null => {
    let candidate = editor.parentElement;
    while (candidate) {
      if (postButtonIn(candidate)) {
        return candidate;
      }
      candidate = candidate.parentElement;
    }
    return null;
  };

  // The visibility and comment controls beside the switcher are the same kind
  // of element, so they are told apart by their text.
  const NEIGHBOUR_CONTROL = /^(?:post to|comments:)\s/iu;
  const identitySwitcher = (editor: Element): Element | null => {
    let scope = editor.parentElement;
    while (scope) {
      const found = Array.from(
        scope.querySelectorAll('div[role="button"][aria-expanded]'),
      ).find((candidate) => !NEIGHBOUR_CONTROL.test(textOf(candidate)));
      if (found) {
        return found;
      }
      scope = scope.parentElement;
    }
    return null;
  };

  const identityRadios = (): Element[] => {
    const heading = Array.from(document.querySelectorAll("p")).find(
      (paragraph) => textOf(paragraph) === "Post as",
    );
    let scope: Element | null = heading?.parentElement ?? null;
    while (scope) {
      const own = Array.from(scope.querySelectorAll('[role="radio"]'));
      if (own.length > 0) {
        return own;
      }
      scope = scope.parentElement;
    }
    return Array.from(document.querySelectorAll('[role="radio"]'));
  };

  const identityListClosed = (): Promise<boolean> =>
    waitFor(() => identityRadios().length === 0, 1_500).then(Boolean);

  // Closes the list by pressing the switcher again. Escape is never used in a
  // post, because on the composer it closes the whole composer.
  const closeIdentityListWithSwitcher = async (
    switcher: Element,
  ): Promise<void> => {
    if (identityRadios().length > 0) {
      (switcher as HTMLElement).click();
      await identityListClosed();
    }
  };

  // Escape closes the list but also the composer, so this is only for a read
  // that is done with the page.
  const closeIdentityListWithEscape = async (
    switcher: Element,
  ): Promise<void> => {
    if (identityRadios().length === 0) {
      return;
    }
    if (typeof KeyboardEvent === "function") {
      const target = document.activeElement ?? document.body;
      target?.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          key: "Escape",
          code: "Escape",
        }),
      );
    }
    if (
      !(await identityListClosed()) &&
      switcher.getAttribute("aria-expanded") === "true"
    ) {
      await closeIdentityListWithSwitcher(switcher);
    }
  };

  // Pages the account administers, from the left rail's `/company/<id>/admin/`
  // links. Sibling links such as `.../admin/analytics/` are not pages.
  const ADMIN_PATH = /^\/company\/(\d+)\/admin\/?$/u;
  const adminLinks = (): { readonly id: string; readonly name: string }[] =>
    Array.from(document.querySelectorAll('a[href*="/company/"]')).flatMap(
      (link) => {
        const href = link.getAttribute("href") ?? "";
        const id = ADMIN_PATH.exec(new URL(href, window.location.href).pathname)?.[1];
        const name = textOf(link);
        return id && name ? [{ id, name }] : [];
      },
    );

  interface Identity {
    readonly element: Element;
    readonly name: string;
    readonly selected: boolean;
  }

  const openIdentityList = async (
    switcher: Element,
  ): Promise<Identity[] | null> => {
    const read = (): Identity[] =>
      identityRadios().map((element) => ({
        element,
        name: textOf(element),
        selected: element.getAttribute("aria-checked") === "true",
      }));
    if (read().length === 0) {
      (switcher as HTMLElement).click();
    }
    const opened = await waitFor(() => read().length > 0, 5_000);
    return opened ? read() : null;
  };

  const openComposer = async (): Promise<
    | { readonly editor: Element; readonly box: Element; readonly switcher: Element }
    | DriverPageResult
  > => {
    const editor = await waitFor(composerEditable, 25_000);
    const box = editor ? composerBoxFor(editor) : null;
    if (!editor || !box) {
      return isSignInPage()
        ? loginRequired()
        : failure(
            "unsupported",
            "LinkedIn did not show its post composer. Nothing was submitted.",
          );
    }
    const switcher = await waitFor(() => identitySwitcher(editor), 5_000);
    if (!switcher) {
      return failure(
        "unsupported",
        "LinkedIn did not show who the post goes out as. Nothing was submitted.",
      );
    }
    return { editor, box, switcher };
  };

  const readPages = async (): Promise<DriverPageResult> => {
    const composer = await openComposer();
    if ("state" in composer) {
      return composer;
    }
    const identities = await openIdentityList(composer.switcher);
    if (!identities) {
      return failure(
        "unsupported",
        "LinkedIn did not list the pages you can post as.",
      );
    }
    const links = adminLinks();
    const pages = identities.map((identity, index) => {
      const ids = new Set(
        links
          .filter((link) => nameKey(link.name) === nameKey(identity.name))
          .map((link) => link.id),
      );
      const [id] = ids;
      return {
        name: identity.name,
        personal: index === 0,
        selected: identity.selected,
        ...(index > 0 && ids.size === 1 ? { id } : {}),
      };
    });
    await closeIdentityListWithEscape(composer.switcher);
    return {
      state: "ready",
      kind: "pages",
      ...landed,
      platform: "linkedin",
      pages,
    };
  };

  const clearEditor = async (editor: Element): Promise<boolean> => {
    if (textOf(editor) === "") {
      return true;
    }
    if (typeof document.execCommand !== "function") {
      return false;
    }
    document.execCommand("selectAll");
    document.execCommand("delete");
    return (await waitFor(() => textOf(editor) === "", 1_000)) !== null;
  };

  const typeIntoEditor = async (
    editor: Element,
    text: string,
    expected: string,
  ): Promise<boolean> => {
    (editor as HTMLElement).focus();
    if (!(await clearEditor(editor))) {
      return false;
    }
    if (typeof document.execCommand !== "function") {
      return false;
    }
    document.execCommand("insertText", false, text);
    const reached = await waitFor(() => textOf(editor) === expected, 4_000);
    if (!reached) {
      return false;
    }
    // The editor rewrites itself after the text lands; it has to stay put.
    await wait(200);
    return textOf(editor) === expected;
  };

  const TOAST_LINK = 'a[href*="/feed/update/urn:li:activity:"]';
  const toastLinks = (): string[] =>
    Array.from(document.querySelectorAll('div[role="alert"]'))
      .flatMap((alert) => Array.from(alert.querySelectorAll(TOAST_LINK)))
      .map((link) => link.getAttribute("href") ?? "");

  // Pluk's own record that Post was pressed for this draft, so a later pass
  // reads the outcome instead of typing and pressing a second time.
  const pressedKey = `pluk:linkedin:${options.draftId ?? ""}`;
  interface Pressed {
    readonly earlierLinks: readonly string[];
    readonly linkPreview: boolean | null;
  }
  const readPressed = (): Pressed | null => {
    if (options.draftId === undefined) {
      return null;
    }
    try {
      const stored = sessionStorage.getItem(pressedKey);
      return stored === null ? null : (JSON.parse(stored) as Pressed);
    } catch {
      return null;
    }
  };
  const writePressed = (pressed: Pressed): void => {
    if (options.draftId === undefined) {
      return;
    }
    try {
      sessionStorage.setItem(pressedKey, JSON.stringify(pressed));
    } catch {
      // Storage refused; a pass after this one cannot tell Post was pressed.
    }
  };

  const outcomeAfterPress = async (
    pressed: Pressed,
  ): Promise<DriverPageResult> => {
    const href = await waitFor(
      () => toastLinks().find((link) => !pressed.earlierLinks.includes(link)),
      20_000,
    );
    const postedId = href?.match(/urn:li:activity:(\d+)/u)?.[1];
    if (!postedId) {
      return failure(
        "submission_unknown",
        "LinkedIn accepted the click without showing a link to the post. Check LinkedIn before trying again; Pluk did not retry.",
      );
    }
    return {
      state: "submission_succeeded",
      kind: "submission",
      ...landed,
      platform: "linkedin",
      postedId,
      postedUrl: `https://www.linkedin.com/feed/update/urn:li:activity:${postedId}/`,
      ...(pressed.linkPreview === null ? {} : { linkPreview: pressed.linkPreview }),
    };
  };

  // Opens the "Post as" list and switches to the one identity named `pageName`.
  const chooseIdentity = async (
    switcher: Element,
    pageName: string,
  ): Promise<Identity | DriverPageResult> => {
    const identities = await openIdentityList(switcher);
    if (!identities) {
      return failure(
        "unsupported",
        "LinkedIn did not list the pages you can post as. Nothing was submitted.",
      );
    }
    const available = identities.map((identity) => identity.name).join(", ");
    const matches = identities.filter(
      (identity) => nameKey(identity.name) === nameKey(pageName),
    );
    const [chosen] = matches;
    if (!chosen || matches.length > 1) {
      await closeIdentityListWithSwitcher(switcher);
      return failure(
        chosen ? "target_mismatch" : "target_not_found",
        chosen
          ? `More than one LinkedIn page is called "${pageName}". Pages you can post as: ${available}. Nothing was submitted.`
          : `LinkedIn does not list a page called "${pageName}". Pages you can post as: ${available}. Nothing was submitted.`,
      );
    }
    // Choosing a radio, even the checked one, also closes the list.
    (chosen.element as HTMLElement).click();
    const switched = await waitFor(
      () => nameKey(textOf(switcher)) === nameKey(chosen.name),
      5_000,
    );
    await closeIdentityListWithSwitcher(switcher);
    return switched
      ? chosen
      : failure(
          "target_mismatch",
          `LinkedIn did not switch to "${chosen.name}". Nothing was submitted.`,
        );
  };

  // `page` is a LinkedIn organization ID or a page name. An ID is turned into
  // the name its admin link shows in the left rail.
  const resolvePageName = async (
    switcher: Element,
    requested: string,
  ): Promise<string | DriverPageResult> => {
    if (!/^\d+$/u.test(requested)) {
      return requested;
    }
    const link = await waitFor(
      () => adminLinks().find((candidate) => candidate.id === requested),
      10_000,
    );
    if (link) {
      return link.name;
    }
    const identities = await openIdentityList(switcher);
    await closeIdentityListWithSwitcher(switcher);
    const names = (identities ?? adminLinks()).map((entry) => entry.name);
    return failure(
      "target_not_found",
      `LinkedIn does not show a page with ID ${requested}. Pages you can post as: ${names.join(", ")}. Nothing was submitted.`,
    );
  };

  const awaitLinkPreview = async (): Promise<boolean> => {
    const filled = await waitFor(() => {
      const card = document.querySelector('[componentkey="ShareBoxpreviewCard"]');
      return card !== null && textOf(card) !== "";
    }, 15_000);
    return filled !== null;
  };

  // Read back just before Post, which cannot be taken back: the page may have
  // changed under the typing.
  const mismatchBeforePost = (
    editor: Element,
    identityName: string,
    expectedText: string,
  ): DriverPageResult | null => {
    const switcher = identitySwitcher(editor);
    if (!switcher || nameKey(textOf(switcher)) !== nameKey(identityName)) {
      return failure(
        "target_mismatch",
        `The composer is no longer posting as "${identityName}". Nothing was submitted.`,
      );
    }
    if (textOf(editor) !== expectedText) {
      return failure(
        "unsupported",
        "LinkedIn changed the post text before it was sent. Nothing was submitted.",
      );
    }
    return null;
  };

  const submitPost = async (): Promise<DriverPageResult> => {
    const alreadyPressed = readPressed();
    if (alreadyPressed) {
      trace("post already pressed for this draft; reading the outcome");
      return outcomeAfterPress(alreadyPressed);
    }
    const requestedPage = clean(options.page);
    const text = options.text ?? "";
    const link = options.url ?? null;
    if (requestedPage === "" || text === "") {
      return failure(
        "unsupported",
        "The confirmed LinkedIn post has no page or text. Nothing was submitted.",
      );
    }
    try {
      sessionStorage.removeItem("wande:trace");
    } catch {
      // Storage refused; the trace is only a diagnostic.
    }
    trace(`start at ${window.location.href}`);
    const composer = await openComposer();
    if ("state" in composer) {
      return composer;
    }
    const { editor, box, switcher } = composer;

    const pageName = await resolvePageName(switcher, requestedPage);
    if (typeof pageName !== "string") {
      return pageName;
    }
    const chosen = await chooseIdentity(switcher, pageName);
    if ("state" in chosen) {
      return chosen;
    }
    trace(`posting as ${chosen.name}`);

    const typed = link === null ? text : `${text}\n${link}`;
    const expected = clean(typed);
    if (!(await typeIntoEditor(editor, typed, expected))) {
      return failure(
        "unsupported",
        "LinkedIn did not accept the post text. Check the visible composer and try again; nothing was submitted.",
      );
    }
    trace("typed");

    const linkPreview = link === null ? null : await awaitLinkPreview();
    trace(`link preview ${linkPreview}`);

    const mismatch = mismatchBeforePost(editor, chosen.name, expected);
    if (mismatch) {
      return mismatch;
    }
    const postButton = await waitFor(() => {
      const button = postButtonIn(box);
      return button && !isDisabled(button) ? button : null;
    }, 5_000);
    if (!postButton) {
      return failure(
        "unsupported",
        "LinkedIn did not enable the Post button for the confirmed post. Nothing was submitted.",
      );
    }
    const pressed = { earlierLinks: toastLinks(), linkPreview };
    writePressed(pressed);
    trace("post click");
    (postButton as HTMLElement).click();
    return outcomeAfterPress(pressed);
  };

  if (options.action === "read_pages") {
    return readPages();
  }
  if (options.action === "submit_post") {
    return submitPost();
  }
  return failure(
    "unsupported",
    "This LinkedIn action is not supported by the site driver.",
  );
}
