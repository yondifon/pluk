import { afterEach, describe, expect, it } from "vitest";
import { suppressWebViewKeyBeep } from "./keyBeep";

let stop = () => {};

afterEach(() => {
  stop();
  stop = () => {};
  document.body.innerHTML = "";
});

function keydown(target: Element, init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

describe("suppressWebViewKeyBeep", () => {
  it("swallows plain character keys that land on the frame", () => {
    const label = document.createElement("span");
    document.body.appendChild(label);
    stop = suppressWebViewKeyBeep();

    expect(keydown(label, { key: "a" }).defaultPrevented).toBe(true);
    expect(keydown(label, { key: " " }).defaultPrevented).toBe(true);
  });

  it("leaves typing, shortcuts and keyboard navigation alone", () => {
    const input = document.createElement("input");
    const button = document.createElement("button");
    const label = document.createElement("span");
    document.body.append(input, button, label);
    stop = suppressWebViewKeyBeep();

    expect(keydown(input, { key: "a" }).defaultPrevented).toBe(false);
    expect(keydown(button, { key: " " }).defaultPrevented).toBe(false);
    expect(keydown(label, { key: "n", metaKey: true }).defaultPrevented).toBe(false);
    expect(keydown(label, { key: "Tab" }).defaultPrevented).toBe(false);
    expect(keydown(label, { key: "ArrowDown" }).defaultPrevented).toBe(false);
    expect(keydown(label, { key: "Escape" }).defaultPrevented).toBe(false);
  });
});
