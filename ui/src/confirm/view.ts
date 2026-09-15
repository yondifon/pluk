import { createButton } from "../primitives";

export type ConfirmChoice = "once" | "session" | "always" | "deny";
export type PostChoice = "postNow" | "queue" | "discard" | "later";

export interface PostQuestion {
  draftId: string;
  text: string;
  /** The posts of a thread, in order. Empty for a plain post. */
  parts: string[];
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

/**
 * A post an agent asked for, put to the person before anything reaches the
 * page. Every word here comes from Pluk, never from the page.
 */
export function renderPost(
  root: HTMLElement,
  question: PostQuestion,
  onAnswer: (choice: PostChoice) => void,
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

  const text = document.createElement("div");
  text.className = "confirm-command confirm-post";
  for (const part of thread ? question.parts : [question.text]) {
    const block = document.createElement("pre");
    block.className = "confirm-post-part";
    block.textContent = part;
    text.appendChild(block);
  }

  const context = document.createElement("p");
  context.className = "confirm-reason";
  context.textContent = question.replyingTo ? `Replying to ${question.replyingTo}` : "";
  context.hidden = !question.replyingTo;

  const countdown = document.createElement("p");
  countdown.className = "confirm-countdown";
  countdown.setAttribute("aria-hidden", "true");

  const actions = document.createElement("div");
  actions.className = "confirm-actions";
  const buttons: Array<[string, PostChoice, "default" | "primary"]> = [
    ["Discard", "discard", "default"],
    ...(question.canQueue ? [["Add to queue", "queue", "default"] as [string, PostChoice, "default"]] : []),
    ["Post now", "postNow", "primary"],
  ];
  for (const [label, choice, variant] of buttons) {
    actions.appendChild(createButton(label, { variant, onClick: () => onAnswer(choice) }));
  }

  root.append(source, title, text, context, countdown, actions);
  actions.querySelector<HTMLButtonElement>(".ui-button-primary")?.focus();

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
