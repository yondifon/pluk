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
  type PostQuestion,
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

function postQuestion(overrides: Partial<PostQuestion> = {}): PostQuestion {
  return {
    integrationId: "wande-1",
    draftId: "d1",
    text: "Hello\nworld",
    parts: [],
    images: [],
    replyingTo: null,
    reposting: null,
    quoting: null,
    canQueue: true,
    closesAt: Date.now() + 60_000,
    ...overrides,
  };
}

const neverLoads = () => new Promise<string>(() => {});

describe("the post window", () => {
  test("shows the exact text and the three answers, Post now first to the keyboard", () => {
    const root = document.createElement("div");
    const answers: PostChoice[] = [];
    renderPost(root, postQuestion(), (choice) => answers.push(choice), neverLoads);
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
      postQuestion({ draftId: "d2", text: "Thanks", replyingTo: "https://x.com/a/status/1", canQueue: false }),
      () => {},
      neverLoads,
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Send this reply?");
    expect(root.querySelector(".confirm-reason")?.textContent).toBe("Replying to https://x.com/a/status/1");
    expect([...root.querySelectorAll(".confirm-actions .ui-button")].map((b) => b.textContent)).toEqual([
      "Discard",
      "Post now",
    ]);
  });

  test("a quote shows the post it quotes above its own words", () => {
    const root = document.createElement("div");
    renderPost(
      root,
      postQuestion({
        text: "Worth reading.",
        quoting: { url: "https://x.com/i/status/42", author: "Owner @owner", text: "The original." },
        canQueue: false,
      }),
      () => {},
      neverLoads,
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Post this quote?");
    const quoted = root.querySelector(".confirm-quoted");
    expect(quoted?.textContent).toBe("Quoting Owner @ownerThe original.");
    expect(quoted?.nextElementSibling?.className).toContain("confirm-post");
    expect(root.querySelector(".confirm-post")?.textContent).toBe("Worth reading.");

    renderPost(
      root,
      postQuestion({ quoting: { url: "https://x.com/i/status/42", author: null, text: null } }),
      () => {},
      neverLoads,
    );
    expect(root.querySelector(".confirm-quoted-text")?.textContent).toBe("https://x.com/i/status/42");
  });

  test("a thread is shown as the posts it becomes", () => {
    const root = document.createElement("div");
    renderPost(
      root,
      postQuestion({ draftId: "d3", text: "One\n\nTwo", parts: ["One", "Two"] }),
      () => {},
      neverLoads,
    );
    expect(root.querySelector(".confirm-title")?.textContent).toBe("Post this thread of 2?");
    expect([...root.querySelectorAll(".confirm-post-part")].map((b) => b.textContent)).toEqual(["One", "Two"]);
  });

  test("the countdown says nothing goes out on its own", () => {
    expect(postCountdownText(95)).toBe("Nothing goes out until you answer. Expires in 1:35.");
    expect(postCountdownText(0)).toBe("Time is up. Nothing was posted.");
  });

  async function flush(): Promise<void> {
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
  }

  test("sending stays disabled until every image has loaded, but Discard never waits", async () => {
    const root = document.createElement("div");
    const images = [
      { id: "img1", partIndex: 0, ordinal: 0, contentType: "image/png", bytes: 10 },
      { id: "img2", partIndex: 0, ordinal: 1, contentType: "image/png", bytes: 10 },
    ];
    const resolvers = new Map<string, (url: string) => void>();
    renderPost(
      root,
      postQuestion({ images }),
      () => {},
      (imageId) => new Promise((resolve) => resolvers.set(imageId, resolve)),
    );
    const send = () => [...root.querySelectorAll<HTMLButtonElement>(".confirm-actions .ui-button")];
    expect(send().find((b) => b.textContent === "Post now")?.disabled).toBe(true);
    expect(send().find((b) => b.textContent === "Add to queue")?.disabled).toBe(true);
    expect(send().find((b) => b.textContent === "Discard")?.disabled).toBe(false);
    expect(root.querySelectorAll(".confirm-image").length).toBe(2);

    resolvers.get("img1")?.("data:image/png;base64,AAAA");
    await flush();
    expect(send().find((b) => b.textContent === "Post now")?.disabled).toBe(true);

    resolvers.get("img2")?.("data:image/png;base64,BBBB");
    await flush();
    expect(send().find((b) => b.textContent === "Post now")?.disabled).toBe(false);
    expect(root.querySelectorAll(".confirm-image img").length).toBe(2);
  });

  test("a broken preview keeps sending disabled", async () => {
    const root = document.createElement("div");
    const images = [{ id: "img1", partIndex: 0, ordinal: 0, contentType: "image/png", bytes: 10 }];
    renderPost(root, postQuestion({ images }), () => {}, () => Promise.reject(new Error("gone")));
    await flush();
    const send = [...root.querySelectorAll<HTMLButtonElement>(".confirm-actions .ui-button")].find(
      (b) => b.textContent === "Post now",
    );
    expect(send?.disabled).toBe(true);
    expect(root.querySelector(".confirm-image-broken")).not.toBeNull();
  });

  test("a thread shows each part's own images beneath that part, never mixed together", async () => {
    const root = document.createElement("div");
    const images = [
      { id: "first-1", partIndex: 0, ordinal: 0, contentType: "image/png", bytes: 10 },
      { id: "third-1", partIndex: 2, ordinal: 0, contentType: "image/png", bytes: 10 },
      { id: "third-2", partIndex: 2, ordinal: 1, contentType: "image/png", bytes: 10 },
    ];
    renderPost(
      root,
      postQuestion({ parts: ["First.", "Second.", "Third."], images }),
      () => {},
      (imageId) => Promise.resolve(`data:image/png;base64,${imageId}`),
    );
    await flush();
    const parts = [...root.querySelectorAll<HTMLElement>(".confirm-post-part, .confirm-images")];
    // Part order is preserved, and the image-free middle part carries no
    // gallery at all rather than an empty one.
    expect(parts.map((el) => el.className)).toEqual([
      "confirm-post-part",
      "confirm-images",
      "confirm-post-part",
      "confirm-post-part",
      "confirm-images",
    ]);
    const galleries = [...root.querySelectorAll(".confirm-images")];
    expect(galleries[0]?.querySelectorAll(".confirm-image").length).toBe(1);
    expect(galleries[1]?.querySelectorAll(".confirm-image").length).toBe(2);
  });
});
