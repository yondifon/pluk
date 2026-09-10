import { describe, expect, test } from "bun:test";
import { draftFromConnection, emptyDraft, parseRules } from "./connectionDraft";
import { renderApprovalsSection } from "./render";

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
});
