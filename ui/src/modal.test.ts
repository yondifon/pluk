import { afterEach, describe, expect, it, vi } from "vitest";
import { openModal } from "./modal";

afterEach(() => {
  vi.runAllTimers();
  vi.useRealTimers();
  document.body.innerHTML = "";
  document.body.style.overflow = "";
});

function modalContent(): HTMLElement {
  const content = document.createElement("div");
  const first = document.createElement("button");
  first.textContent = "First";
  const last = document.createElement("button");
  last.textContent = "Last";
  content.append(first, last);
  return content;
}

describe("openModal", () => {
  it("labels the dialog, locks page scrolling, and closes on Escape", () => {
    vi.useFakeTimers();
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    openModal({ title: "Delete integration", size: "small", content: modalContent() });

    const dialog = document.querySelector("[role='dialog']") as HTMLElement;
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.body.style.overflow).toBe("hidden");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    vi.runAllTimers();
    expect(document.querySelector("[role='dialog']")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("keeps focus inside the dialog", () => {
    vi.useFakeTimers();
    const modal = openModal({ title: "Response", size: "large", content: modalContent() });
    const focusable = document.querySelectorAll<HTMLElement>(".modal button");
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    last.focus();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab" }));
    expect(document.activeElement).toBe(first);
    modal.close();
  });
});
