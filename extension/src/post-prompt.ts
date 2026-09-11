import type { PostChoice } from "./protocol";

/** What the overlay shows. Everything here comes from Pluk, never the page. */
export interface PostPrompt {
  readonly account: string;
  readonly text: string;
  readonly replyingTo: string | null;
  readonly canQueue: boolean;
  /** When to give up and leave the post waiting, in epoch milliseconds. */
  readonly closesAt: number;
}

/** The words on the overlay, in one place so a test can hold them to account. */
export const PROMPT_COPY = {
  post: "Post this?",
  reply: "Send this reply?",
  from: "Posting as",
  postNow: "Post now",
  queue: "Queue for later",
  discard: "Discard",
  dismissal: "Press Esc to decide later — it waits for you in Pluk.",
} as const;

/**
 * The question Pluk draws over the page it has just written into.
 *
 * This runs through `chrome.scripting.executeScript`, so it must stay
 * self-contained: Chrome serializes the function and runs it with no access
 * to anything in this module. Everything it needs arrives in `prompt`, and
 * the copy is inlined for the same reason.
 *
 * It runs in the extension's isolated world, and its markup lives in a closed
 * shadow root, so page scripts can neither read the question nor reach the
 * buttons. The answer leaves as this function's return value — straight to
 * the service worker, never through the page.
 */
export function postPromptScript(prompt: PostPrompt): Promise<PostChoice> {
  const copy = {
    post: "Post this?",
    reply: "Send this reply?",
    from: "Posting as",
    postNow: "Post now",
    queue: "Queue for later",
    discard: "Discard",
    dismissal: "Press Esc to decide later — it waits for you in Pluk.",
  };
  const HOST_ID = "pluk-post-prompt";
  document.getElementById(HOST_ID)?.remove();

  const host = document.createElement("div");
  host.id = HOST_ID;
  host.style.cssText = "all: initial; position: fixed; z-index: 2147483647;";
  const root = host.attachShadow({ mode: "closed" });

  const style = document.createElement("style");
  style.textContent = `
    :host { all: initial; }
    .backdrop {
      position: fixed; inset: 0; display: flex; align-items: center;
      justify-content: center; padding: 24px; background: rgb(16 20 18 / 55%);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    }
    .card {
      box-sizing: border-box; width: 100%; max-width: 440px; max-height: 80vh;
      overflow-y: auto; padding: 20px; border-radius: 14px; background: #fffdf8;
      color: #17211b; box-shadow: 0 24px 60px rgb(16 20 18 / 35%);
    }
    .mark { margin: 0 0 10px; font-size: 11px; font-weight: 700;
      letter-spacing: 0.12em; text-transform: uppercase; color: #155b51; }
    .title { margin: 0 0 14px; font-size: 19px; font-weight: 700; line-height: 1.25; }
    .label { margin: 0; font-size: 12px; color: #667169; }
    .account { margin: 2px 0 0; font-size: 14px; font-weight: 600; }
    .context { margin: 12px 0 0; font-size: 13px; color: #667169;
      overflow-wrap: anywhere; }
    .text { margin: 12px 0 0; padding: 12px; border: 1px solid #d8d8cf;
      border-radius: 10px; background: #f4f1ea; font-size: 15px;
      line-height: 1.45; white-space: pre-wrap; overflow-wrap: anywhere;
      -webkit-user-select: text; user-select: text; }
    .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 18px; }
    button { flex: 1 1 auto; min-height: 38px; padding: 0 14px; border-radius: 9px;
      border: 1px solid #d8d8cf; background: #fffdf8; color: #17211b;
      font-family: inherit; font-size: 14px; font-weight: 600; cursor: pointer; }
    button:hover { background: #f4f1ea; }
    button.primary { border-color: #155b51; background: #155b51; color: #fffdf8; }
    button.primary:hover { background: #10473f; }
    button:focus-visible { outline: 2px solid #155b51; outline-offset: 2px; }
    .footnote { margin: 12px 0 0; font-size: 12px; color: #667169; text-align: center; }
    @media (prefers-color-scheme: dark) {
      .card { background: #1b201d; color: #eef1ec; box-shadow: 0 24px 60px rgb(0 0 0 / 55%); }
      .mark { color: #7fc7b8; }
      .label, .context, .footnote { color: #a2aca5; }
      .text { border-color: #333c37; background: #12160f; }
      button { border-color: #3a443e; background: #242b27; color: #eef1ec; }
      button:hover { background: #2d3531; }
      button.primary { border-color: #2f8d7d; background: #2f8d7d; color: #0d110f; }
      button.primary:hover { background: #37a08e; }
    }
  `;

  const backdrop = document.createElement("div");
  backdrop.className = "backdrop";
  const card = document.createElement("div");
  card.className = "card";
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-modal", "true");

  const mark = document.createElement("p");
  mark.className = "mark";
  mark.textContent = "PLUK";
  const title = document.createElement("h2");
  title.className = "title";
  title.id = "pluk-post-prompt-title";
  title.textContent = prompt.replyingTo ? copy.reply : copy.post;
  card.setAttribute("aria-labelledby", title.id);
  card.append(mark, title);

  const fromLabel = document.createElement("p");
  fromLabel.className = "label";
  fromLabel.textContent = copy.from;
  const account = document.createElement("p");
  account.className = "account";
  account.textContent = prompt.account;
  card.append(fromLabel, account);

  if (prompt.replyingTo) {
    const context = document.createElement("p");
    context.className = "context";
    context.textContent = `Replying to “${prompt.replyingTo}”`;
    card.appendChild(context);
  }

  const body = document.createElement("p");
  body.className = "text";
  body.textContent = prompt.text;
  card.appendChild(body);

  const actions = document.createElement("div");
  actions.className = "actions";
  const choices: ReadonlyArray<[PostChoice, string, boolean]> = [
    ["postNow", copy.postNow, true],
    ...(prompt.canQueue
      ? ([["queue", copy.queue, false]] as ReadonlyArray<
          [PostChoice, string, boolean]
        >)
      : []),
    ["discard", copy.discard, false],
  ];

  return new Promise<PostChoice>((resolve) => {
    let settled = false;
    const settle = (choice: PostChoice) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      document.removeEventListener("keydown", onKeydown, true);
      host.remove();
      resolve(choice);
    };
    // Anything the page could synthesize is not an answer: only a real click
    // on a button inside this closed shadow root counts.
    const onChoice = (choice: PostChoice) => (event: MouseEvent) => {
      if (!event.isTrusted) return;
      settle(choice);
    };
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !event.isTrusted) return;
      event.stopPropagation();
      settle("later");
    };

    for (const [choice, label, primary] of choices) {
      const button = document.createElement("button");
      button.type = "button";
      if (primary) button.className = "primary";
      button.textContent = label;
      button.addEventListener("click", onChoice(choice));
      actions.appendChild(button);
    }
    card.appendChild(actions);

    const footnote = document.createElement("p");
    footnote.className = "footnote";
    footnote.textContent = copy.dismissal;
    card.appendChild(footnote);

    backdrop.addEventListener("click", (event) => {
      if (event.isTrusted && event.target === backdrop) settle("later");
    });
    backdrop.appendChild(card);
    root.append(style, backdrop);
    document.documentElement.appendChild(host);
    const firstButton = actions.querySelector("button");
    if (firstButton instanceof HTMLElement) firstButton.focus();

    const timer = window.setTimeout(
      () => settle("later"),
      Math.max(0, prompt.closesAt - Date.now()),
    );
    document.addEventListener("keydown", onKeydown, true);
  });
}
