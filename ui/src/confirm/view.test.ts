import { describe, expect, test } from "bun:test";
import {
  countdownText,
  renderClosed,
  renderConfirm,
  type ConfirmChoice,
  type ConfirmQuestion,
} from "./view";

function question(): ConfirmQuestion {
  return {
    integrationId: "i1",
    integrationName: "Prod server",
    integrationKind: "SSH",
    tool: "run_command",
    command: "rm -rf /var/cache",
    reason: 'command not allowed: "rm"',
  };
}

describe("the confirm window", () => {
  test("names the integration and shows the command exactly", () => {
    const root = document.createElement("div");
    renderConfirm(root, question(), () => {});
    expect(root.querySelector(".confirm-source")?.textContent).toBe("SSH");
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Allow this on Prod server?");
    expect(root.querySelector(".confirm-command")?.textContent).toBe("rm -rf /var/cache");
    expect(root.querySelector(".confirm-reason")?.textContent).toContain('command not allowed: "rm"');
  });

  test("offers the four answers and reports the one that was clicked", () => {
    const root = document.createElement("div");
    const answers: ConfirmChoice[] = [];
    renderConfirm(root, question(), (choice) => answers.push(choice));
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(".confirm-actions .ui-button")];
    expect(buttons.map((b) => b.textContent)).toEqual([
      "Don’t run",
      "Always allow",
      "Allow until Pluk quits",
      "Allow once",
    ]);
    for (const button of buttons) button.click();
    expect(answers).toEqual(["deny", "always", "session", "once"]);
  });

  test("counts down, and says so when the time is up", () => {
    const root = document.createElement("div");
    const view = renderConfirm(root, question(), () => {});
    view.setSecondsLeft(58.4);
    expect(root.querySelector(".confirm-countdown")?.textContent).toBe(
      "Nothing runs if you don’t answer (59s).",
    );
    view.setSecondsLeft(0);
    expect(root.querySelector(".confirm-countdown")?.textContent).toBe("Time is up. Nothing ran.");
  });

  test("never shows a negative countdown", () => {
    expect(countdownText(-4)).toBe("Time is up. Nothing ran.");
  });

  test("an answered question leaves nothing left to click", () => {
    const root = document.createElement("div");
    const answers: ConfirmChoice[] = [];
    const view = renderConfirm(root, question(), (choice) => answers.push(choice));
    view.settle();
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(".confirm-actions .ui-button")];
    expect(buttons.every((b) => b.disabled)).toBe(true);
    expect(root.querySelector(".confirm-countdown")?.textContent).toBe("");
  });

  test("a question that is no longer waiting says so instead of going blank", () => {
    const root = document.createElement("div");
    renderClosed(root, "This request has closed.", "Nothing ran. The agent can ask again.");
    expect(root.querySelector(".confirm-title")?.textContent).toBe("This request has closed.");
    expect(root.querySelector(".confirm-actions")).toBeNull();
  });
});
