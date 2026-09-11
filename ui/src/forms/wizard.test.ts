import { describe, expect, test } from "bun:test";
import { wizardSteps } from "./wizard";
import { adopt, emptyDraft } from "./connectionDraft";
import { renderTypeChooser, renderNameStep, renderConnectFieldsStep, renderToolsStep, renderCommandsStep } from "./render";
import { renderConnectChromeStep } from "../integration-detail/browser-access";
import { renderInstallStep } from "../integration-detail/agent-setup";
import type { AdapterManifest } from "./catalog";

const wande: AdapterManifest = {
  id: "wande",
  label: "Wande",
  category: "social",
  policyKind: "none",
  agentHint: "",
  runsCommands: false,
  tools: [{ name: "x_post", label: "Post", description: "Post exact text.", category: "write", defaultEnabled: true }],
  configFields: [],
};

const ssh: AdapterManifest = {
  id: "ssh",
  label: "SSH",
  category: "infrastructure",
  policyKind: "none",
  agentHint: "",
  runsCommands: true,
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
    expect(wizardSteps(wande, "create")).toEqual(["type", "name", "connect", "tools", "install"]);
  });

  test("an adapter that runs commands gets an extra screen after tools", () => {
    expect(wizardSteps(ssh, "create")).toEqual(["type", "name", "connect", "tools", "commands", "install"]);
  });

  test("editing skips the type step entirely, it does not just hide it", () => {
    expect(wizardSteps(wande, "edit")).toEqual(["name", "connect", "tools", "install"]);
    expect(wizardSteps(ssh, "edit")).toEqual(["name", "connect", "tools", "commands", "install"]);
  });
});

describe("Wande flow, step by step", () => {
  const steps = wizardSteps(wande, "create");

  test("step 1 groups Wande under its own category and picking it advances", () => {
    let chosen: AdapterManifest | null = null;
    const el = renderTypeChooser([wande], (m) => (chosen = m));
    el.querySelector<HTMLButtonElement>(".chooser-row")!.click();
    expect(chosen!.id).toBe("wande");
  });

  test("step 2, Name it: blocks on an empty name, continues once filled", () => {
    let continued = false;
    let draft = adopt(emptyDraft(), wande, true);
    const onContinue = () => (continued = true);
    let el = renderNameStep(draft, wande, 2, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    expect(el.querySelector("h2")!.textContent).toBe("Name it");
    expect(el.textContent).toContain("Agents will see this name when they use it.");

    primary(el).click();
    expect(continued).toBe(false);
    expect(el.querySelector(".field-error")!.textContent).toBe("Enter a name to continue.");

    // Real usage re-renders the step on every keystroke; a fresh element
    // with the updated draft stands in for that here.
    draft = { ...draft, name: "My Wande" };
    el = renderNameStep(draft, wande, 2, steps.length, (next) => (draft = next), null, () => {}, onContinue);
    primary(el).click();
    expect(continued).toBe(true);
  });

  test("step 3, Connect Chrome: Continue stays off until the poll reports connected", () => {
    let onChange: ((connected: boolean) => void) | null = null;
    let continued = false;
    const { el } = renderConnectChromeStep(
      "integration-1",
      3,
      steps.length,
      { onBack: null, onCancel: () => {}, onContinue: () => (continued = true), onSkip: () => {} },
      {
        fetchPlukId: async () => ({ id: "pluk-123" }),
        watch: (cb) => {
          onChange = cb;
          return () => {};
        },
      },
    );
    expect(el.querySelector("h2")!.textContent).toBe("Connect Chrome");
    expect(primary(el).disabled).toBe(true);

    onChange!(false);
    expect(primary(el).disabled).toBe(true);
    primary(el).click();
    expect(continued).toBe(false);

    onChange!(true);
    expect(primary(el).disabled).toBe(false);
    primary(el).click();
    expect(continued).toBe(true);
  });

  test("step 4, tools: saves directly since Wande has no commands step", () => {
    let saved: unknown = null;
    const draft = adopt(emptyDraft(), wande, true);
    const toolsIndex = steps.indexOf("tools");
    const el = renderToolsStep(draft, toolsIndex + 1, steps.length, toolsIndex === steps.length - 2, () => {}, null, () => {}, (d) => (saved = d));
    expect(primary(el).textContent).toBe("Save integration");
    primary(el).click();
    expect(saved).toBe(draft);
  });

  test("step 5, install: reaches the agent's own name in the helper, Done closes the flow", () => {
    let done = false;
    const el = renderInstallStep(
      { name: "My Wande", environment: "development", token: "tok" },
      wande,
      async () => ({ status: "added", path: "" }),
      5,
      steps.length,
      { onBack: null, onDone: () => (done = true) },
      { installed: [] },
    );
    expect(el.querySelector("h2")!.textContent).toBe("Install into your agent");
    expect(el.textContent).toContain("so it can reach Wande.");
    expect(primary(el).textContent).toBe("Done");
    primary(el).click();
    expect(done).toBe(true);
  });
});

describe("SSH flow, step by step", () => {
  const steps = wizardSteps(ssh, "create");

  test("step 3, Connect: shows the adapter's own fields instead of a pairing card", () => {
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

describe("Skip for now", () => {
  test("advances past pairing even while still not connected", () => {
    let continued = false;
    let skipped = false;
    const { el } = renderConnectChromeStep(
      "integration-1",
      3,
      5,
      { onBack: null, onCancel: () => {}, onContinue: () => (continued = true), onSkip: () => (skipped = true) },
      { fetchPlukId: async () => ({ id: "pluk-123" }), watch: () => () => {} },
    );
    const skip = [...el.querySelectorAll("button")].find((b) => b.textContent === "Skip for now")!;
    expect(skip.classList.contains("wizard-skip")).toBe(true);
    expect(primary(el).disabled).toBe(true);

    skip.click();
    expect(skipped).toBe(true);
    expect(continued).toBe(false);
  });
});
