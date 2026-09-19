import { describe, expect, test } from "bun:test";
import {
  awaitingSignIn,
  canEnable,
  orderedProxyTools,
  signInView,
  stateBadge,
  stateNote,
  type ProxyToolRow,
  type ProxyToolState,
  type SignIn,
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

describe("badges and notes", () => {
  test("only unsettled states are marked", () => {
    expect(stateBadge("new")).toBe("New");
    expect(stateBadge("changed")).toBe("Changed");
    expect(stateBadge("missing")).toBe("Unavailable");
    expect(stateBadge("approved")).toBeNull();
  });

  test("only a changed or withdrawn tool needs a line of its own", () => {
    expect(stateNote("changed")).toBe(
      "This tool changed. Read what it does now, then turn it back on.",
    );
    expect(stateNote("missing")).toBe("This server no longer offers this tool.");
    expect(stateNote("new")).toBeNull();
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

describe("signInView", () => {
  const view = (auth: Partial<SignIn>) =>
    signInView({ kind: "none", status: "not_connected", required: "none", ...auth });

  test("a server that lets anyone in is never asked to be signed in to", () => {
    expect(view({})).toEqual({
      message: "This server does not ask you to sign in.",
      status: null,
      action: null,
    });
  });

  test("a server that only takes a token points at the settings, not a button", () => {
    expect(view({ required: "token" })).toEqual({
      message: "This server needs a token. Add one in this integration's settings.",
      status: null,
      action: null,
    });
  });

  test("a saved token reads as signed in", () => {
    expect(view({ kind: "token", status: "connected", required: "token" })).toEqual({
      message: "Signed in with the token you saved.",
      status: "connected",
      action: null,
    });
  });

  test("a server that hands out no client IDs asks for one first", () => {
    expect(view({ required: "oauth", needsClientId: true })).toEqual({
      message:
        "This server needs a client ID. Add one in this integration's settings, then sign in.",
      status: null,
      action: null,
    });
  });

  test("a sign-in server offers the button its state calls for", () => {
    expect(view({ required: "oauth" }).action).toBe("sign-in");
    expect(view({ required: "oauth", status: "reconnect_needed" })).toEqual({
      message: "Your sign-in has expired. Sign in again to keep using this server.",
      status: "reconnect_needed",
      action: "sign-in-again",
    });
    expect(view({ kind: "oauth", required: "oauth", status: "connected" })).toEqual({
      message: "Signed in.",
      status: "connected",
      action: "sign-out",
    });
  });
});

describe("awaitingSignIn", () => {
  test("only a server still waiting on the user holds its tools back", () => {
    expect(awaitingSignIn({ kind: "none", status: "not_connected", required: "none" })).toBe(false);
    expect(awaitingSignIn({ kind: "none", status: "not_connected", required: "oauth" })).toBe(true);
    expect(awaitingSignIn({ kind: "none", status: "not_connected", required: "token" })).toBe(true);
    expect(awaitingSignIn({ kind: "token", status: "connected", required: "token" })).toBe(false);
  });
});
