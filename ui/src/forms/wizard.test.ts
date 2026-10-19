import { describe, expect, test } from "bun:test";
import { wizardSteps } from "./wizard";
import { adopt, emptyDraft } from "./connectionDraft";
import { renderTypeChooser, renderNameStep, renderConnectFieldsStep, renderToolsStep, renderCommandsStep } from "./render";
import type { AdapterManifest } from "./catalog";

const postgres: AdapterManifest = {
  id: "postgres",
  label: "PostgreSQL",
  category: "database",
  policyKind: "sql",
  agentHint: "",
  runsCommands: false,
  offeredForSetup: true,
  tools: [{ name: "query", label: "Query", description: "Run a query.", category: "read", defaultEnabled: true }],
  configFields: [{ key: "host", label: "Host", type: "text", required: true, group: "Connection" }],
};

const ssh: AdapterManifest = {
  id: "ssh",
  label: "SSH",
  category: "infrastructure",
  policyKind: "none",
  agentHint: "",
  runsCommands: true,
  offeredForSetup: true,
  tools: [{ name: "run_command", label: "Run command", description: "Run a shell command.", category: "read", defaultEnabled: true }],
  configFields: [{ key: "host", label: "Host", type: "text", required: true, group: "Connection" }],
};

/** Scoped to the wizard's own footer: an install step also has the mcp
    section's own primary "Install" button, which is not the one this file
    is stepping through. */
function primary(el: HTMLElement): HTMLButtonElement {
  return el.querySelector<HTMLButtonElement>(".form-footer .ui-button-primary")!;
}

describe("wizardSteps", () => {
  test("a plain adapter walks type, name, connect, tools, install", () => {
    expect(wizardSteps(postgres, "create")).toEqual(["type", "name", "connect", "tools", "install"]);
  });

  test("an adapter that runs commands gets an extra screen after tools", () => {
    expect(wizardSteps(ssh, "create")).toEqual(["type", "name", "connect", "tools", "commands", "install"]);
  });

  test("editing skips the type step entirely, it does not just hide it", () => {
    expect(wizardSteps(postgres, "edit")).toEqual(["name", "connect", "tools", "install"]);
    expect(wizardSteps(ssh, "edit")).toEqual(["name", "connect", "tools", "commands", "install"]);
  });
});

describe("PostgreSQL flow, step by step", () => {
  const steps = wizardSteps(postgres, "create");

  test("step 2, Name it: blocks on an empty name, continues once filled", () => {
    let continued = false;
    let draft = adopt(emptyDraft(), postgres, true);
    const onContinue = () => (continued = true);
    let el = renderNameStep(draft, postgres, 2, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    expect(el.querySelector("h2")!.textContent).toBe("Name it");
    expect(el.textContent).toContain("Agents will see this name when they use it.");

    primary(el).click();
    expect(continued).toBe(false);
    expect(el.querySelector(".field-error")!.textContent).toBe("Enter a name to continue.");

    // Real usage re-renders the step on every keystroke; a fresh element
    // with the updated draft stands in for that here.
    draft = { ...draft, name: "Analytics" };
    el = renderNameStep(draft, postgres, 2, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    primary(el).click();
    expect(continued).toBe(true);
  });

  test("step 4, tools: saves directly since a database has no commands step", () => {
    let saved: unknown = null;
    const draft = adopt(emptyDraft(), postgres, true);
    const toolsIndex = steps.indexOf("tools");
    const el = renderToolsStep(draft, toolsIndex + 1, steps.length, toolsIndex === steps.length - 2, () => {}, null, () => {}, (d) => (saved = d));
    expect(primary(el).textContent).toBe("Save integration");
    primary(el).click();
    expect(saved).toBe(draft);
  });

});

describe("SSH flow, step by step", () => {
  const steps = wizardSteps(ssh, "create");

  test("step 3, Connect: blocks on a missing required field, continues once filled", () => {
    let continued = false;
    let draft = adopt(emptyDraft(), ssh, true);
    const onContinue = () => (continued = true);
    let el = renderConnectFieldsStep(draft, ssh, 3, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    expect(el.querySelector("h2")!.textContent).toBe("Connect");
    expect(el.textContent).toContain("Fill in what Pluk needs to reach SSH.");

    primary(el).click();
    expect(continued).toBe(false);
    expect(el.querySelector(".field-error")!.textContent).toBe("Host is required.");

    // Real usage re-renders the step on every keystroke; a fresh element
    // with the updated draft stands in for that here.
    draft = { ...draft, config: { ...draft.config, host: "server.example.com" } };
    el = renderConnectFieldsStep(draft, ssh, 3, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    primary(el).click();
    expect(continued).toBe(true);
  });

  test("step 4, tools: only continues, a commands step still stands between it and saving", () => {
    let advanced = false;
    const draft = adopt(emptyDraft(), ssh, true);
    const toolsIndex = steps.indexOf("tools");
    const el = renderToolsStep(draft, toolsIndex + 1, steps.length, toolsIndex === steps.length - 2, () => {}, null, () => {}, () => (advanced = true));
    expect(primary(el).textContent).toBe("Continue");
    primary(el).click();
    expect(advanced).toBe(true);
  });

  test("step 5, What it's allowed to run: is the screen that actually saves", () => {
    let saved: unknown = null;
    const draft = adopt(emptyDraft(), ssh, true);
    const commandsIndex = steps.indexOf("commands");
    const el = renderCommandsStep(draft, commandsIndex + 1, steps.length, () => {}, null, () => {}, (d) => (saved = d));
    expect(el.querySelector("h2")!.textContent).toBe("What it’s allowed to run");
    expect(el.textContent).toContain("What the agent may run");
    expect(primary(el).textContent).toBe("Save integration");
    primary(el).click();
    expect(saved).toBe(draft);
  });
});
