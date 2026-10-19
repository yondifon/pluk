import { createButton } from "../primitives";

export type ConfirmChoice = "once" | "session" | "always" | "deny";

export interface ConfirmQuestion {
  integrationId: string;
  integrationName: string;
  integrationKind: string;
  tool: string;
  command: string;
  reason: string;
}

export function renderConfirm(
  root: HTMLElement,
  question: ConfirmQuestion,
  onAnswer: (choice: ConfirmChoice) => void,
): { settle: () => void } {
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

  root.append(source, title, command, reason, actions);
  actions.querySelector<HTMLButtonElement>(".ui-button-primary")?.focus();

  return {
    /** After an answer the window waits to be closed; nothing else to click. */
    settle() {
      for (const button of actions.querySelectorAll("button")) button.disabled = true;
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
