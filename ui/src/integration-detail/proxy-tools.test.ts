import { describe, expect, test } from "bun:test";
import {
  attentionCount,
  canEnable,
  orderedProxyTools,
  signInMessage,
  stateBadge,
  stateNote,
  type ProxyToolRow,
  type ProxyToolState,
} from "./proxy-tools";

function row(name: string, state: ProxyToolState, category = "read"): ProxyToolRow {
  return {
    name,
    label: name,
    description: `${name} description`,
    category,
    state,
    present: state !== "missing",
    updatedAt: "2026-01-01T00:00:00Z",
  };
}

describe("orderedProxyTools", () => {
  test("puts what needs a decision first, gone tools last", () => {
    const ordered = orderedProxyTools([
      row("approved_one", "approved"),
      row("missing_one", "missing"),
      row("new_one", "new"),
      row("changed_one", "changed"),
    ]);
    expect(ordered.map((r) => r.name)).toEqual([
      "changed_one",
      "new_one",
      "approved_one",
      "missing_one",
    ]);
  });

  test("sorts by label inside one state", () => {
    const ordered = orderedProxyTools([row("beta", "new"), row("alpha", "new")]);
    expect(ordered.map((r) => r.name)).toEqual(["alpha", "beta"]);
  });

  test("leaves the given list alone", () => {
    const rows = [row("b", "approved"), row("a", "new")];
    orderedProxyTools(rows);
    expect(rows.map((r) => r.name)).toEqual(["b", "a"]);
  });
});

describe("attentionCount", () => {
  test("counts new and changed only", () => {
    expect(
      attentionCount([
        row("a", "new"),
        row("b", "changed"),
        row("c", "approved"),
        row("d", "missing"),
      ]),
    ).toBe(2);
  });
});

describe("badges and notes", () => {
  test("only unsettled states are marked", () => {
    expect(stateBadge("new")).toBe("New");
    expect(stateBadge("changed")).toBe("Changed");
    expect(stateBadge("missing")).toBe("Unavailable");
    expect(stateBadge("approved")).toBeNull();
  });

  test("a changed tool asks to be approved again", () => {
    expect(stateNote("changed")).toBe(
      "This tool changed since you approved it. Review and approve it again.",
    );
    expect(stateNote("approved")).toBeNull();
  });
});

describe("canEnable", () => {
  test("only an approved tool may be switched on", () => {
    expect(canEnable("approved")).toBe(true);
    expect(canEnable("new")).toBe(false);
    expect(canEnable("changed")).toBe(false);
    expect(canEnable("missing")).toBe(false);
  });
});

describe("signInMessage", () => {
  test("a server with no sign in says so", () => {
    expect(signInMessage("none", "not_connected")).toBe("This server does not ask you to sign in.");
  });

  test("a saved key reads the same whatever the status", () => {
    expect(signInMessage("token", "connected")).toBe("Signed in with the token you saved.");
  });

  test("an expired sign in asks for a new one", () => {
    expect(signInMessage("oauth", "reconnect_needed")).toBe(
      "Your sign-in has expired. Sign in again to keep using this server.",
    );
    expect(signInMessage("oauth", "connected")).toBe("Signed in.");
    expect(signInMessage("oauth", "not_connected")).toBe(
      "Sign in to see what this server offers.",
    );
  });
});
