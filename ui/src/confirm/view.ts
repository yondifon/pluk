import { createButton } from "../primitives";

export type ConfirmChoice = "once" | "session" | "always" | "deny";
export type PostChoice = "postNow" | "queue" | "discard" | "later";

/** One image a post was asked with, as far as this window ever knows: never
 * a path. The picture itself comes from `loadImage`, one id at a time. */
export interface DraftImage {
  id: string;
  /** Which post of a thread this attaches to (0 for a plain post's one
   * implicit part). */
  partIndex: number;
  ordinal: number;
  contentType: string;
  bytes: number;
}

export interface PostQuestion {
  integrationId: string;
  draftId: string;
  text: string;
  /** The posts of a thread, in order. Empty for a plain post. */
  parts: string[];
  /** Every image asked for, across every part. Group by `partIndex` to show
   * each post its own. */
  images: DraftImage[];
  replyingTo: string | null;
  canQueue: boolean;
  /** When the post stops being sendable, in epoch milliseconds. */
  closesAt: number;
}

/** The line under a waiting post: how long it stays sendable. */
export function postCountdownText(secondsLeft: number): string {
  const seconds = Math.max(0, Math.ceil(secondsLeft));
  if (seconds === 0) return "Time is up. Nothing was posted.";
  const minutes = Math.floor(seconds / 60);
  const rest = String(seconds % 60).padStart(2, "0");
  return `Nothing goes out until you answer. Expires in ${minutes}:${rest}.`;
}

export interface ConfirmQuestion {
  integrationId: string;
  integrationName: string;
  integrationKind: string;
  tool: string;
  command: string;
  reason: string;
}

/** The line that counts down to the question closing itself. */
export function countdownText(secondsLeft: number): string {
  const seconds = Math.max(0, Math.ceil(secondsLeft));
  if (seconds === 0) return "Time is up. Nothing ran.";
  return `Nothing runs if you don’t answer (${seconds}s).`;
}

export function renderConfirm(
  root: HTMLElement,
  question: ConfirmQuestion,
  onAnswer: (choice: ConfirmChoice) => void,
): { setSecondsLeft: (seconds: number) => void; settle: () => void } {
  root.innerHTML = "";
  root.className = "confirm";

  const source = document.createElement("p");
  source.className = "confirm-source";
  source.textContent = question.integrationKind;

  const title = document.createElement("h1");
  title.className = "confirm-title";
  title.textContent = `Allow this on ${question.integrationName}?`;

  const command = document.createElement("pre");
  command.className = "confirm-command";
  command.textContent = question.command;

  const reason = document.createElement("p");
  reason.className = "confirm-reason";
  reason.textContent = `Pluk blocks this by default. ${question.reason}`;

  // The countdown is read as it changes, so it is not announced: the
  // consequence of waiting is already in the text above it.
  const countdown = document.createElement("p");
  countdown.className = "confirm-countdown";
  countdown.setAttribute("aria-hidden", "true");

  const actions = document.createElement("div");
  actions.className = "confirm-actions";
  const buttons: Array<[string, ConfirmChoice, "default" | "primary"]> = [
    ["Don’t run", "deny", "default"],
    ["Always allow", "always", "default"],
    ["Allow until Pluk quits", "session", "default"],
    ["Allow once", "once", "primary"],
  ];
  for (const [label, choice, variant] of buttons) {
    actions.appendChild(createButton(label, { variant, onClick: () => onAnswer(choice) }));
  }

  root.append(source, title, command, reason, countdown, actions);
  actions.querySelector<HTMLButtonElement>(".ui-button-primary")?.focus();

  return {
    setSecondsLeft(seconds: number) {
      countdown.textContent = countdownText(seconds);
    },
    /** After an answer the window waits to be closed; nothing else to click. */
    settle() {
      for (const button of actions.querySelectorAll("button")) button.disabled = true;
      countdown.textContent = "";
    },
  };
}

/** The line under a post's images while at least one preview is still
 * loading, or once one has failed to. */
function imagesStatusText(pending: number, failed: boolean): string {
  if (failed) return "Couldn’t load one of these images. Discard and ask again.";
  if (pending > 0) return "Loading the exact images you’d be sending…";
  return "";
}

/**
 * A post an agent asked for, put to the person before anything reaches the
 * page. Every word here comes from Pluk, never from the page.
 *
 * `loadImage` fetches one preview by id — the exact bytes this draft was
 * staged with, never a path this window could point anywhere else. Sending
 * stays disabled until every image has loaded; Discard never waits on them.
 */
export function renderPost(
  root: HTMLElement,
  question: PostQuestion,
  onAnswer: (choice: PostChoice) => void,
  loadImage: (imageId: string) => Promise<string>,
): { setSecondsLeft: (seconds: number) => void; settle: () => void } {
  root.innerHTML = "";
  root.className = "confirm";

  const source = document.createElement("p");
  source.className = "confirm-source";
  source.textContent = "Wande";

  const thread = question.parts.length > 1;
  const title = document.createElement("h1");
  title.className = "confirm-title";
  title.textContent = question.replyingTo
    ? "Send this reply?"
    : thread
      ? `Post this thread of ${question.parts.length}?`
      : "Post this?";

  const parts = thread ? question.parts : [question.text];
  const images = question.images;
  const text = document.createElement("div");
  text.className = "confirm-command confirm-post";
  const galleries: HTMLElement[] = [];
  for (const [index, part] of parts.entries()) {
    const block = document.createElement("pre");
    block.className = "confirm-post-part";
    block.textContent = part;
    text.appendChild(block);
    const partImages = images.filter((image) => image.partIndex === index);
    if (partImages.length === 0) continue;
    const gallery = document.createElement("div");
    gallery.className = "confirm-images";
    text.appendChild(gallery);
    galleries[index] = gallery;
  }

  const imagesStatus = document.createElement("p");
  imagesStatus.className = "confirm-images-status";
  imagesStatus.setAttribute("role", "status");
  imagesStatus.hidden = images.length === 0;

  const context = document.createElement("p");
  context.className = "confirm-reason";
  context.textContent = question.replyingTo ? `Replying to ${question.replyingTo}` : "";
  context.hidden = !question.replyingTo;

  const countdown = document.createElement("p");
  countdown.className = "confirm-countdown";
  countdown.setAttribute("aria-hidden", "true");

  const actions = document.createElement("div");
  actions.className = "confirm-actions";
  const buttons: Array<[string, PostChoice, "default" | "primary", boolean]> = [
    ["Discard", "discard", "default", false],
    ...(question.canQueue
      ? [["Add to queue", "queue", "default", true] as [string, PostChoice, "default", boolean]]
      : []),
    ["Post now", "postNow", "primary", true],
  ];
  const sendButtons: HTMLButtonElement[] = [];
  for (const [label, choice, variant, waitsOnImages] of buttons) {
    const button = createButton(label, { variant, onClick: () => onAnswer(choice) });
    if (waitsOnImages) {
      button.disabled = images.length > 0;
      sendButtons.push(button);
    }
    actions.appendChild(button);
  }

  root.append(source, title, text, imagesStatus, context, countdown, actions);
  (
    actions.querySelector<HTMLButtonElement>(".ui-button-primary:not(:disabled)") ??
    actions.querySelector<HTMLButtonElement>(".ui-button:not(:disabled)")
  )?.focus();

  if (images.length > 0) {
    let pending = images.length;
    let failed = false;
    const updateStatus = () => {
      imagesStatus.textContent = imagesStatusText(pending, failed);
    };
    updateStatus();
    for (const image of images) {
      const gallery = galleries[image.partIndex];
      if (!gallery) continue;
      const partCount = images.filter((other) => other.partIndex === image.partIndex).length;
      const frame = document.createElement("div");
      frame.className = "confirm-image";
      frame.setAttribute("role", "img");
      frame.setAttribute("aria-label", `Image ${image.ordinal + 1} of ${partCount}, loading`);
      gallery.appendChild(frame);
      loadImage(image.id)
        .then((dataUrl) => {
          const picture = document.createElement("img");
          picture.src = dataUrl;
          picture.alt = `Image ${image.ordinal + 1} of ${partCount}`;
          frame.replaceChildren(picture);
          frame.removeAttribute("role");
          frame.removeAttribute("aria-label");
        })
        .catch(() => {
          failed = true;
          frame.classList.add("confirm-image-broken");
          frame.setAttribute("aria-label", `Image ${image.ordinal + 1} of ${partCount} could not load`);
        })
        .finally(() => {
          pending -= 1;
          updateStatus();
          if (pending === 0 && !failed) {
            for (const button of sendButtons) button.disabled = false;
          }
        });
    }
  }

  return {
    setSecondsLeft(seconds: number) {
      countdown.textContent = postCountdownText(seconds);
    },
    settle() {
      for (const button of actions.querySelectorAll("button")) button.disabled = true;
      countdown.textContent = "";
    },
  };
}

/** Shown when there is no longer a question to answer. */
export function renderClosed(root: HTMLElement, primary: string, secondary: string): void {
  root.innerHTML = "";
  root.className = "confirm confirm-closed";
  const title = document.createElement("h1");
  title.className = "confirm-title";
  title.textContent = primary;
  const detail = document.createElement("p");
  detail.className = "confirm-reason";
  detail.textContent = secondary;
  root.append(title, detail);
}
