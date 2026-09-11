import { describe, expect, test } from "bun:test";
import {
  countdownText,
  postCountdownText,
  renderClosed,
  renderConfirm,
  renderPost,
  type ConfirmChoice,
  type ConfirmQuestion,
  type PostChoice,
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

describe("the post window", () => {
  test("shows the exact text and the three answers, Post now first to the keyboard", () => {
    const root = document.createElement("div");
    const answers: PostChoice[] = [];
    renderPost(
      root,
      { draftId: "d1", text: "Hello\nworld", parts: [], replyingTo: null, canQueue: true, closesAt: Date.now() + 60_000 },
      (choice) => answers.push(choice),
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Post this?");
    expect(root.querySelector(".confirm-post")?.textContent).toBe("Hello\nworld");
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(".confirm-actions .ui-button")];
    expect(buttons.map((b) => b.textContent)).toEqual(["Discard", "Add to queue", "Post now"]);
    buttons[2].click();
    expect(answers).toEqual(["postNow"]);
  });

  test("a reply names what it answers and cannot be queued", () => {
    const root = document.createElement("div");
    renderPost(
      root,
      { draftId: "d2", text: "Thanks", parts: [], replyingTo: "https://x.com/a/status/1", canQueue: false, closesAt: Date.now() },
      () => {},
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Send this reply?");
    expect(root.querySelector(".confirm-reason")?.textContent).toBe("Replying to https://x.com/a/status/1");
    expect([...root.querySelectorAll(".confirm-actions .ui-button")].map((b) => b.textContent)).toEqual([
      "Discard",
      "Post now",
    ]);
  });

  test("a thread is shown as the posts it becomes", () => {
    const root = document.createElement("div");
    renderPost(
      root,
      { draftId: "d3", text: "One\n\nTwo", parts: ["One", "Two"], replyingTo: null, canQueue: true, closesAt: Date.now() },
      () => {},
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Post this thread of 2?");
    expect([...root.querySelectorAll(".confirm-post-part")].map((b) => b.textContent)).toEqual(["One", "Two"]);
  });

  test("the countdown says nothing goes out on its own", () => {
    expect(postCountdownText(95)).toBe("Nothing goes out until you answer. Expires in 1:35.");
    expect(postCountdownText(0)).toBe("Time is up. Nothing was posted.");
  });
});
