import { afterEach, describe, expect, it } from "vitest";
import { suppressWebViewContextMenu } from "./contextMenu";

let stop = () => {};

afterEach(() => {
  stop();
  stop = () => {};
  document.body.innerHTML = "";
  document.getSelection()?.removeAllRanges();
});

function contextMenu(target: Element): MouseEvent {
  const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

describe("suppressWebViewContextMenu", () => {
  it("suppresses the menu on non-editable content", () => {
    const label = document.createElement("span");
    document.body.appendChild(label);
    stop = suppressWebViewContextMenu(false);

    expect(contextMenu(label).defaultPrevented).toBe(true);
  });

  it("keeps the menu for editable content and selected text", () => {
    const input = document.createElement("input");
    const label = document.createElement("span");
    label.textContent = "Copy me";
    document.body.append(input, label);
    stop = suppressWebViewContextMenu(false);

    expect(contextMenu(input).defaultPrevented).toBe(false);

    const range = document.createRange();
    range.selectNodeContents(label);
    document.getSelection()?.addRange(range);
    expect(contextMenu(label).defaultPrevented).toBe(false);
  });

  it("leaves the context menu available during development", () => {
    const label = document.createElement("span");
    document.body.appendChild(label);
    stop = suppressWebViewContextMenu(true);

    expect(contextMenu(label).defaultPrevented).toBe(false);
  });
});
