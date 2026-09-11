import { invoke } from "../host";
import { createButton } from "../primitives";
import { toast } from "../toast";

const TITLE_ID = "wande-waiting-title";
const QUEUE_TITLE_ID = "wande-queue-title";
const REFRESH_MS = 5000;
const TICK_MS = 1000;

export interface WaitingPost {
  id: string;
  text: string;
  /** The post this one replies to, when it is a reply. */
  replyingTo: string | null;
  expiresAt: number;
  canQueue: boolean;
}

export interface QueuedPost {
  id: string;
  text: string;
  scheduledAt: number;
  status: string;
}

export interface WandePosts {
  chromeConnected: boolean;
  /** A post is going out right now, so the next one can only be queued. */
  sending: boolean;
  waiting: WaitingPost[];
  queued: QueuedPost[];
}

/** Minutes and seconds left, counting the part-second up so 0:00 means gone. */
export function countdown(msLeft: number): string {
  const seconds = Math.max(0, Math.ceil(msLeft / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** What each queue slot ended up doing, in the words the app uses for it. */
export function queueStatus(status: string): string {
  switch (status) {
    case "reserved":
      return "Waiting";
    case "committed":
      return "Posted";
    case "released":
      return "Not posted";
    default:
      return "Outcome unknown";
  }
}

function daysApart(at: number, now: number): number {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  return Math.floor((at - start.getTime()) / 86_400_000);
}

export function slotLabel(at: number, now: number): string {
  const when = new Date(at);
  const time = when.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  switch (daysApart(at, now)) {
    case 0:
      return `Today at ${time}`;
    case 1:
      return `Tomorrow at ${time}`;
    default:
      return `${when.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} at ${time}`;
  }
}

function line(text: string, className: string): HTMLParagraphElement {
  const element = document.createElement("p");
  element.className = className;
  element.textContent = text;
  return element;
}

function cardTitle(id: string, text: string): HTMLElement {
  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.id = id;
  title.textContent = text;
  return title;
}

/**
 * The two lists behind Wande's half of publishing: what has been asked for
 * and is waiting on a person, and what is holding a slot in the queue.
 *
 * Nothing here posts on its own. Sending one is always a click — that click
 * is what fills the composer in Chrome and submits — and a post that runs out
 * of time stays on screen saying so rather than vanishing.
 */
export function mountWandePosts(container: HTMLElement): { destroy: () => void } {
  container.innerHTML = "";
  container.className = "stack-lg";

  const waitingCard = document.createElement("section");
  waitingCard.className = "ui-card";
  waitingCard.setAttribute("aria-labelledby", TITLE_ID);
  const queueCard = document.createElement("section");
  queueCard.className = "ui-card";
  queueCard.setAttribute("aria-labelledby", QUEUE_TITLE_ID);
  container.append(waitingCard, queueCard);

  let loaded = false;
  let unreachable = false;
  let chromeConnected = true;
  let sending = false;
  let waiting: WaitingPost[] = [];
  let queued: QueuedPost[] = [];
  let expired: WaitingPost[] = [];
  let busy = false;
  const handled = new Set<string>();
  const countdowns = new Map<string, { element: HTMLElement; expiresAt: number }>();
  let painted = "";
  let alive = true;

  async function refresh(): Promise<void> {
    try {
      const posts = await invoke<WandePosts>("list_wande_posts");
      if (!alive) return;
      const gone = new Set(expired.map((post) => post.id));
      const arrived = new Set(posts.waiting.map((post) => post.id));
      for (const post of waiting) {
        if (!arrived.has(post.id) && !handled.has(post.id) && !gone.has(post.id)) {
          expired.push(post);
          gone.add(post.id);
        }
      }
      for (const id of handled) if (!arrived.has(id)) handled.delete(id);
      waiting = posts.waiting.filter((post) => !gone.has(post.id));
      queued = posts.queued;
      chromeConnected = posts.chromeConnected;
      sending = posts.sending;
      unreachable = false;
    } catch {
      if (!alive) return;
      unreachable = true;
    }
    loaded = true;
    repaintIfChanged();
  }

  // Everything the panel draws from, so a poll that changed nothing leaves
  // the buttons under the pointer alone.
  function signature(): string {
    return JSON.stringify([
      loaded,
      unreachable,
      chromeConnected,
      sending,
      busy,
      waiting.map((post) => post.id),
      expired.map((post) => post.id),
      queued.map((post) => [post.id, post.status, post.scheduledAt]),
    ]);
  }

  async function act(
    id: string,
    run: () => Promise<void>,
    done: { title: string; description: string },
    failed: string,
  ): Promise<void> {
    busy = true;
    handled.add(id);
    render();
    try {
      await run();
      toast.success(done.title, { description: done.description });
    } catch (error) {
      handled.delete(id);
      toast.error(failed, { description: error instanceof Error ? error.message : String(error) });
    }
    busy = false;
    await refresh();
  }

  function postNow(post: WaitingPost): void {
    void act(
      post.id,
      () => invoke("send_wande_post", { draftId: post.id, queue: false }),
      {
        title: "On its way",
        description: chromeConnected
          ? "Wande is posting it in Chrome."
          : "It goes out as soon as Chrome is connected.",
      },
      "Couldn’t post this",
    );
  }

  function addToQueue(post: WaitingPost): void {
    void act(
      post.id,
      () => invoke("send_wande_post", { draftId: post.id, queue: true }),
      { title: "Added to the queue", description: "It goes out at the next free time." },
      "Couldn’t add this to the queue",
    );
  }

  function discard(post: WaitingPost): void {
    void act(
      post.id,
      () => invoke("discard_wande_post", { draftId: post.id }),
      { title: "Discarded", description: "This post won’t go out." },
      "Couldn’t discard this",
    );
  }

  function cancelQueued(post: QueuedPost): void {
    void act(
      post.id,
      () => invoke("cancel_queued_wande_post", { draftId: post.id }),
      { title: "Taken out of the queue", description: "This post won’t go out." },
      "Couldn’t take this out of the queue",
    );
  }

  function dismiss(id: string): void {
    expired = expired.filter((post) => post.id !== id);
    render();
  }

  function postBody(post: { text: string; replyingTo?: string | null }): HTMLElement {
    const body = document.createElement("div");
    body.className = "wande-post-body";
    if (post.replyingTo) body.appendChild(line(`Replying to ${post.replyingTo}`, "hint"));
    body.appendChild(line(post.text, "wande-post-text"));
    return body;
  }

  function waitingItem(post: WaitingPost, now: number): HTMLElement {
    const item = document.createElement("li");
    item.className = "wande-post";
    item.appendChild(postBody(post));

    const footer = document.createElement("div");
    footer.className = "wande-post-footer";
    const left = document.createElement("span");
    left.className = "wande-post-countdown";
    left.textContent = `${countdown(post.expiresAt - now)} left to post`;
    countdowns.set(post.id, { element: left, expiresAt: post.expiresAt });
    const actions = document.createElement("div");
    actions.className = "wande-post-actions";
    // One post goes out at a time: while one is on its way, the next can
    // only take a queue slot.
    const buttons = [
      ...(sending
        ? []
        : [createButton("Post now", { variant: "primary", size: "sm", onClick: () => postNow(post) })]),
      ...(post.canQueue
        ? [createButton("Add to queue", { variant: sending ? "primary" : "secondary", size: "sm", onClick: () => addToQueue(post) })]
        : []),
      createButton("Discard", { variant: "secondary", size: "sm", onClick: () => discard(post) }),
    ];
    for (const button of buttons) button.disabled = busy;
    actions.append(...buttons);
    footer.append(left, actions);
    item.appendChild(footer);
    return item;
  }

  function expiredItem(post: WaitingPost): HTMLElement {
    const item = document.createElement("li");
    item.className = "wande-post wande-post-expired";
    item.appendChild(postBody(post));
    const footer = document.createElement("div");
    footer.className = "wande-post-footer";
    const note = line("Time ran out, so this never went out. Ask for it again to post it.", "wande-post-note");
    note.setAttribute("role", "status");
    const actions = document.createElement("div");
    actions.className = "wande-post-actions";
    actions.appendChild(createButton("Dismiss", { variant: "secondary", size: "sm", onClick: () => dismiss(post.id) }));
    footer.append(note, actions);
    item.appendChild(footer);
    return item;
  }

  function queuedItem(post: QueuedPost, now: number): HTMLElement {
    const item = document.createElement("li");
    item.className = "wande-post";
    const head = document.createElement("div");
    head.className = "wande-post-footer";
    head.append(line(slotLabel(post.scheduledAt, now), "wande-post-slot"), line(queueStatus(post.status), "hint"));
    item.append(head, postBody(post));
    if (post.status === "reserved") {
      const actions = document.createElement("div");
      actions.className = "wande-post-actions";
      const cancel = createButton("Take out of the queue", {
        variant: "secondary",
        size: "sm",
        onClick: () => cancelQueued(post),
      });
      cancel.disabled = busy;
      actions.appendChild(cancel);
      item.appendChild(actions);
    }
    return item;
  }

  function renderWaiting(now: number): void {
    waitingCard.innerHTML = "";
    waitingCard.append(
      cardTitle(TITLE_ID, "Waiting for you"),
      line("Nothing goes out until you send it.", "hint"),
    );

    if (unreachable) {
      const failure = line("Pluk can’t show this right now. Restart Pluk and try again.", "empty");
      failure.setAttribute("role", "alert");
      waitingCard.appendChild(failure);
      return;
    }
    if (!loaded) {
      const pending = line("Loading…", "hint");
      pending.setAttribute("role", "status");
      waitingCard.appendChild(pending);
      return;
    }
    if (sending && waiting.length) {
      const inFlight = line("A post is going out now. The next one can wait in the queue.", "wande-post-note");
      inFlight.setAttribute("role", "status");
      waitingCard.appendChild(inFlight);
    }
    if (!chromeConnected && waiting.length) {
      const offline = line("Chrome isn’t connected. Paste your Pluk ID into Wande in Chrome first.", "wande-post-note");
      offline.setAttribute("role", "status");
      waitingCard.appendChild(offline);
    }
    if (!waiting.length && !expired.length) {
      waitingCard.appendChild(line("No posts waiting. Anything written through Wande lands here for you to send.", "empty"));
      return;
    }
    const list = document.createElement("ul");
    list.className = "wande-post-list";
    for (const post of waiting) list.appendChild(waitingItem(post, now));
    for (const post of expired) list.appendChild(expiredItem(post));
    waitingCard.appendChild(list);
  }

  function renderQueue(now: number): void {
    queueCard.hidden = !queued.length;
    queueCard.innerHTML = "";
    if (!queued.length) return;
    queueCard.appendChild(cardTitle(QUEUE_TITLE_ID, "Going out later"));
    const list = document.createElement("ul");
    list.className = "wande-post-list";
    for (const post of queued) list.appendChild(queuedItem(post, now));
    queueCard.appendChild(list);
  }

  function render(): void {
    const now = Date.now();
    countdowns.clear();
    renderWaiting(now);
    renderQueue(now);
    painted = signature();
  }

  function repaintIfChanged(): void {
    if (signature() !== painted) render();
  }

  // One second of the clock: the deadlines move, and anything that has run
  // out joins the posts that never went out.
  function tick(): void {
    const now = Date.now();
    const ranOut = waiting.filter((post) => post.expiresAt <= now);
    if (ranOut.length) {
      expired = [...expired, ...ranOut];
      waiting = waiting.filter((post) => post.expiresAt > now);
      render();
      return;
    }
    for (const { element, expiresAt } of countdowns.values()) {
      element.textContent = `${countdown(expiresAt - now)} left to post`;
    }
  }

  render();
  void refresh();
  const poll = setInterval(() => void refresh(), REFRESH_MS);
  const ticker = setInterval(tick, TICK_MS);

  return {
    destroy() {
      alive = false;
      clearInterval(poll);
      clearInterval(ticker);
      container.innerHTML = "";
    },
  };
}
