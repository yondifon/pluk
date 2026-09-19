import { afterEach, describe, expect, it } from "vitest";
import { markFocus, restoreFocus } from "./focus.ts";
import { renderNameStep } from "./render.ts";
import { adopt, emptyDraft } from "./connectionDraft.ts";
import type { AdapterManifest } from "./catalog.ts";

const postgres: AdapterManifest = {
  id: "postgres",
  label: "PostgreSQL",
  category: "database",
  policyKind: "sql",
  agentHint: "",
  runsCommands: false,
  tools: [{ name: "query", label: "Query", description: "Run a query.", category: "read", defaultEnabled: true }],
  configFields: [{ key: "host", label: "Host", type: "text", required: true }],
};

/** Mirrors the app's form loop: one step on screen, redrawn whole whenever the draft changes. */
function mountNameStep() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  let draft = adopt(emptyDraft(), postgres, true);
  let redraws = 0;

  const draw = (keepFocus: boolean) => {
    const mark = keepFocus ? markFocus(host) : null;
    host.innerHTML = "";
    host.appendChild(
      renderNameStep(draft, postgres, 2, 5, (next) => {
        draft = next;
        redraws += 1;
        draw(true);
      }, null, () => {}, () => {}),
    );
    restoreFocus(host, mark);
  };
  draw(false);

  return {
    nameInput: () => host.querySelector<HTMLInputElement>("input[type='text']")!,
    heading: () => host.querySelector<HTMLHeadingElement>("h2")!,
    redraws: () => redraws,
  };
}

function typeCharacter(input: HTMLInputElement, character: string): void {
  input.value += character;
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("input"));
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  document.body.innerHTML = "";
});

describe("focus across a form redraw", () => {
  it("leaves the caret in the name field while someone types", async () => {
    const step = mountNameStep();
    step.nameInput().focus();

    typeCharacter(step.nameInput(), "A");
    await flush();
    expect(document.activeElement).toBe(step.nameInput());

    typeCharacter(step.nameInput(), "c");
    await flush();

    expect(step.redraws()).toBe(2);
    expect(document.activeElement).toBe(step.nameInput());
    expect(step.nameInput().value).toBe("Ac");
    expect(step.nameInput().selectionStart).toBe(2);
  });

  it("reads out the step heading on arrival, when nobody is working in the step", async () => {
    const step = mountNameStep();
    await flush();
    expect(document.activeElement).toBe(step.heading());
  });
});
