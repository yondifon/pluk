import { describe, expect, test } from "bun:test";
import { draftFromConnection, emptyDraft, adopt, parseRules } from "./connectionDraft";
import { renderApprovalsSection, renderToolsStep, renderCommandsStep } from "./render";
import { wizardSteps } from "./wizard";
import type { AdapterManifest } from "./catalog";

function manifest(id: string, label: string, runsCommands: boolean): AdapterManifest {
  const tool = id === "ssh"
    ? { name: "run_command", label: "Run command", description: "Run a shell command.", category: "read", defaultEnabled: true }
    : { name: "x_post", label: "Post", description: "Post exact text.", category: "write", defaultEnabled: true };
  return {
    id,
    label,
    category: id === "ssh" ? "infrastructure" : "social",
    policyKind: "none",
    agentHint: "",
    runsCommands,
    tools: [
      tool,
      { name: "optional_tool", label: "Optional tool", description: "Use another tool.", category: "read", defaultEnabled: false },
    ],
    configFields: [],
  };
}

describe("approval rules", () => {
  test("one rule per line, blanks dropped", () => {
    expect(parseRules("git pull*\n\n  rm -rf * \n")).toEqual(["git pull*", "rm -rf *"]);
  });

  test("a fresh integration asks before it refuses", () => {
    expect(emptyDraft().approvals).toEqual({ ask: true, allow: [], deny: [] });
  });

  test("stored rules are read back with the tool switches", () => {
    const draft = draftFromConnection({
      name: "Prod",
      type: "ssh",
      config: {},
      queryPolicy: JSON.stringify({
        tools: { run_command: { enabled: true } },
        approvals: { ask: false, allow: ["git pull*"], deny: ["rm *"] },
      }),
    });
    expect(draft.approvals).toEqual({ ask: false, allow: ["git pull*"], deny: ["rm *"] });
    expect(draft.toolConfig.run_command?.enabled).toBe(true);
  });

  test("an integration saved before rules existed still asks", () => {
    const draft = draftFromConnection({
      name: "Prod",
      type: "ssh",
      config: {},
      queryPolicy: JSON.stringify({ tools: {} }),
    });
    expect(draft.approvals).toEqual({ ask: true, allow: [], deny: [] });
  });

  test("editing a rule list reports the rules, not the raw text", () => {
    let seen = { ask: true, allow: [] as string[], deny: [] as string[] };
    const section = renderApprovalsSection(seen, (next) => {
      seen = next;
    });
    const allow = section.querySelector<HTMLTextAreaElement>("#control-approvals-allow")!;
    allow.value = "git pull*\n\ndocker ps";
    allow.dispatchEvent(new Event("change"));
    expect(seen.allow).toEqual(["git pull*", "docker ps"]);
  });

  test("a rule the host refuses is shown beside the list that holds it", () => {
    const approvals = { ask: true, allow: [] as string[], deny: ["rm [a-"] };
    const section = renderApprovalsSection(approvals, () => {}, {
      list: "deny",
      message: "Never allow: “rm [a-” is not a pattern Pluk can match — a [ … ] group is not closed properly. Fix or remove it to save.",
    });
    const deny = section.querySelector<HTMLTextAreaElement>("#control-approvals-deny")!;
    const allow = section.querySelector<HTMLTextAreaElement>("#control-approvals-allow")!;
    const error = section.querySelector<HTMLElement>(".field-error")!;
    expect(error.textContent).toContain("rm [a-");
    expect(error.getAttribute("role")).toBe("alert");
    expect(deny.closest(".inspector-row")?.contains(error)).toBe(true);
    expect(deny.getAttribute("aria-invalid")).toBe("true");
    expect(allow.getAttribute("aria-invalid")).toBeNull();
  });

  test("no rule problem leaves both lists unflagged", () => {
    const section = renderApprovalsSection({ ask: true, allow: [], deny: [] }, () => {});
    expect(section.querySelector(".field-error")).toBeNull();
  });

  test("asking can be turned off", () => {
    let seen = { ask: true, allow: [] as string[], deny: [] as string[] };
    const section = renderApprovalsSection(seen, (next) => {
      seen = next;
    });
    const ask = section.querySelector<HTMLInputElement>("#control-approvals-ask")!;
    ask.checked = false;
    ask.dispatchEvent(new Event("change"));
    expect(seen.ask).toBe(false);
  });

  test("Wande has no commands step and its tools step saves directly", () => {
    const wande = manifest("wande", "Wande", false);
    const steps = wizardSteps(wande, "create");
    expect(steps).not.toContain("commands");
    const draft = adopt(emptyDraft(), wande, true);
    const toolsIndex = steps.indexOf("tools");
    const form = renderToolsStep(draft, toolsIndex + 1, steps.length, toolsIndex === steps.length - 2, () => {}, null, () => {}, () => {});
    expect(form.textContent).not.toContain("What the agent may run");
    expect(form.querySelector(".ui-button-primary")?.textContent).toBe("Save integration");
  });

  test("SSH gets a commands step after tools, which is what saves", () => {
    const ssh = manifest("ssh", "SSH", true);
    const steps = wizardSteps(ssh, "create");
    expect(steps).toContain("commands");
    const draft = adopt(emptyDraft(), ssh, true);
    const toolsIndex = steps.indexOf("tools");
    const toolsForm = renderToolsStep(draft, toolsIndex + 1, steps.length, toolsIndex === steps.length - 2, () => {}, null, () => {}, () => {});
    expect(toolsForm.textContent).not.toContain("What the agent may run");
    expect(toolsForm.querySelector(".ui-button-primary")?.textContent).toBe("Continue");

    const commandsIndex = steps.indexOf("commands");
    const commandsForm = renderCommandsStep(draft, commandsIndex + 1, steps.length, () => {}, null, () => {}, () => {});
    expect(commandsForm.textContent).toContain("What the agent may run");
    expect(commandsForm.querySelector(".ui-button-primary")?.textContent).toBe("Save integration");
  });

  test("tool rows use labels and keep ids secondary", () => {
    const ssh = manifest("ssh", "SSH", true);
    const draft = adopt(emptyDraft(), ssh, true);
    const form = renderToolsStep(draft, 1, 1, true, () => {}, null, () => {}, () => {});
    const row = form.querySelector(".tool-row")!;
    const offRow = form.querySelector(".tool-off")!;
    expect(row.querySelector(".tool-name")?.textContent).toBe("Run command");
    expect(row.querySelector("code")?.textContent).toBe("run_command");
    expect(offRow.querySelector(".tool-state")?.textContent).toBe("Off");
    expect(form.querySelector(".more-tools-title + .hint")?.textContent).toBe("Turn on the ones the agent should have.");
  });
});
