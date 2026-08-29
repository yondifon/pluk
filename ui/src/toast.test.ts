import { describe, test, expect, beforeEach, afterEach, vi } from "bun:test";
import { toast, mountToaster, AUTO_DISMISS_MS, VISIBLE_TOASTS } from "./toast";

let container: HTMLElement;
let unmount: () => void;

/** Leaving toasts linger for their exit animation, so only live ones count. */
function toasts(): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(".toast:not([data-exit])")];
}

/** DOM order is oldest first, so the newest toast — the one at the corner — is last. */
function front(): HTMLElement {
  return toasts()[toasts().length - 1];
}

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.appendChild(container);
  unmount = mountToaster(container);
});

afterEach(() => {
  toast.clear();
  unmount();
  container.remove();
  vi.useRealTimers();
});

describe("stacking", () => {
  test("newest sits at the corner with the older ones behind it", () => {
    toast.error("First");
    toast.error("Second");

    const stack = toasts();
    expect(stack.map((el) => el.querySelector(".toast-title")!.textContent)).toEqual(["First", "Second"]);
    expect(front().style.getPropertyValue("--toast-scale")).toBe("1");
    expect(stack[0].style.getPropertyValue("--toast-scale")).toBe("0.96");
  });

  test("past three, the extra toasts collapse behind a count", () => {
    for (let i = 0; i < VISIBLE_TOASTS + 1; i++) toast.error(`Error ${i}`);

    const hidden = toasts().filter((el) => el.hasAttribute("data-hidden"));
    expect(hidden.map((el) => el.querySelector(".toast-title")!.textContent)).toEqual(["Error 0"]);
    expect(container.querySelector(".toast-overflow")!.textContent).toBe("+1 more");
  });

  test("hovering the stack expands it and drops the count", () => {
    for (let i = 0; i < VISIBLE_TOASTS + 1; i++) toast.error(`Error ${i}`);

    container.dispatchEvent(new MouseEvent("mouseenter"));

    expect(toasts().some((el) => el.hasAttribute("data-hidden"))).toBe(false);
    expect(container.querySelector(".toast-overflow")).toBeNull();
  });
});

describe("dismissal", () => {
  test("success clears itself after a few seconds", () => {
    toast.success("Endpoint URL copied");
    expect(toasts()).toHaveLength(1);

    vi.advanceTimersByTime(AUTO_DISMISS_MS + 500);

    expect(toasts()).toHaveLength(0);
  });

  test("error waits for the person", () => {
    toast.error("Couldn’t connect");

    vi.advanceTimersByTime(AUTO_DISMISS_MS * 4);

    expect(toasts()).toHaveLength(1);
  });

  test("hovering holds the countdown, leaving resumes it", () => {
    toast.success("Endpoint URL copied");
    container.dispatchEvent(new MouseEvent("mouseenter"));

    vi.advanceTimersByTime(AUTO_DISMISS_MS * 2);
    expect(toasts()).toHaveLength(1);

    container.dispatchEvent(new MouseEvent("mouseleave"));
    vi.advanceTimersByTime(AUTO_DISMISS_MS + 500);
    expect(toasts()).toHaveLength(0);
  });

  test("the close control removes just that toast", () => {
    toast.error("First");
    toast.error("Second");

    front().querySelector<HTMLButtonElement>("button[aria-label='Dismiss notification']")!.click();

    expect(toasts().map((el) => el.querySelector(".toast-title")!.textContent)).toEqual(["First"]);
  });

  test("Escape dismisses the focused toast", () => {
    toast.error("Couldn’t connect");

    front().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    expect(toasts()).toHaveLength(0);
  });
});

describe("pending toasts", () => {
  test("a pending toast becomes its own result rather than a second toast", () => {
    const pending = toast.pending("Prod DB", { description: "Testing connection…" });
    const element = front();
    expect(element.dataset.variant).toBe("pending");

    pending.success("Prod DB", { description: "Connected." });

    expect(toasts()).toHaveLength(1);
    expect(toasts()[0]).toBe(element);
    expect(element.dataset.variant).toBe("success");
    expect(element.querySelector(".toast-description")!.textContent).toBe("Connected.");
  });

  test("resolving to an error keeps the toast until it is dismissed", () => {
    toast.pending("Prod DB").error("Prod DB", { description: "Couldn’t connect." });

    vi.advanceTimersByTime(AUTO_DISMISS_MS * 4);

    expect(front().dataset.variant).toBe("error");
  });
});

describe("long detail", () => {
  test("the summary shows and the full text stays one control away", () => {
    toast.error("Install didn’t finish", { description: "Couldn’t update Cursor", detail: "line one\nline two" });

    const toggle = front().querySelector<HTMLButtonElement>(".toast-detail-toggle")!;
    const detail = front().querySelector<HTMLElement>(".toast-detail")!;
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-controls")).toBe(detail.id);
    expect(detail.hidden).toBe(true);

    toggle.click();

    expect(detail.hidden).toBe(false);
    expect(detail.textContent).toBe("line one\nline two");
    expect(toggle.textContent).toBe("Hide details");
  });
});

describe("accessibility", () => {
  test("the stack is a polite live region", () => {
    expect(container.getAttribute("role")).toBe("region");
    expect(container.getAttribute("aria-live")).toBe("polite");
    expect(container.getAttribute("aria-label")).toBe("Notifications");
  });

  test("errors announce assertively, everything else politely", () => {
    toast.success("Endpoint URL copied");
    toast.error("Couldn’t connect");

    const [success, error] = toasts();
    expect(success.getAttribute("role")).toBe("status");
    expect(success.getAttribute("aria-live")).toBe("polite");
    expect(error.getAttribute("role")).toBe("alert");
    expect(error.getAttribute("aria-live")).toBe("assertive");
  });
});
