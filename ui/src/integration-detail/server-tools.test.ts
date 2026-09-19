import { describe, expect, test } from "bun:test";
import type { SignIn } from "./proxy-tools";
import { mountServerTools } from "./server-tools";
import type { Integration } from "./types";

type Row = ReturnType<typeof tool>;

/**
 * Stands in for the host: records what the panel asked it to do, and answers
 * the enable route the way the adapter does, so a ticked tool comes back
 * approved.
 */
function host(auth: SignIn, tools: Row[]) {
  const calls: Array<Record<string, unknown>> = [];
  const state = { auth, tools };
  Object.assign(window, {
    __TAURI__: {
      core: {
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, ...args });
          if (cmd !== "integration_api") return null;
          const subpath = String(args.subpath);
          if (subpath.includes("auth")) return { status: 200, body: { auth: state.auth } };
          if (subpath.endsWith("/proxy/enable")) {
            const body = JSON.parse(String(args.body)) as { names: string[]; enabled: boolean };
            state.tools = state.tools.map((row) =>
              body.enabled && body.names.includes(row.name)
                ? { ...row, state: "approved", present: true }
                : row,
            );
          }
          return { status: 200, body: { tools: state.tools } };
        },
      },
    },
  });
  return { calls, state };
}

function tool(name: string, state: string, category: string) {
  return {
    name,
    label: name[0].toUpperCase() + name.slice(1),
    description: `What ${name} does`,
    category,
    state,
    present: state !== "missing",
    updatedAt: "",
  };
}

function integration(): Integration {
  return {
    id: "i1",
    name: "Notion",
    type: "mcp",
    config: {},
    toolConfig: {},
    token: "t",
    createdAt: "",
    approvals: { ask: true, allow: [], deny: [] },
  };
}

const signedIn: SignIn = { kind: "oauth", status: "connected", required: "oauth" };

/** Mounted in the document so focus behaves the way it does on the real screen. */
function mount(auth: SignIn, tools: Row[]) {
  const { calls, state } = host(auth, tools);
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  const conn = integration();
  mountServerTools(root, conn);
  return { root, calls, state, conn, settled: new Promise((done) => setTimeout(done, 10)) };
}

const settle = () => new Promise((done) => setTimeout(done, 10));

const buttonLabels = (root: HTMLElement) =>
  [...root.querySelectorAll("button")].map((b) => b.textContent);

const ticks = (root: HTMLElement) => [
  ...root.querySelectorAll<HTMLInputElement>(".tool-head input"),
];

const subpaths = (calls: Array<Record<string, unknown>>) =>
  calls.map((c) => String(c.subpath ?? ""));

describe("mountServerTools", () => {
  test("shows what needs a decision first, and tells the states apart", async () => {
    const { root, settled } = mount(signedIn, [
      tool("search", "new", "read"),
      tool("edit", "approved", "write"),
      tool("old", "missing", "read"),
      tool("moved", "changed", "write"),
    ]);
    await settled;

    expect([...root.querySelectorAll(".tool-name")].map((n) => n.textContent)).toEqual([
      "Moved",
      "Search",
      "Edit",
      "Old",
    ]);
    expect([...root.querySelectorAll(".tool-row .ui-badge")].map((b) => b.textContent)).toEqual([
      "Changed",
      "New",
      "Unavailable",
    ]);
    expect(root.querySelector(".tool-gone")).not.toBeNull();
  });

  test("one tick turns a tool on, and nothing else is offered", async () => {
    const { root, calls, conn, settled } = mount(signedIn, [tool("search", "new", "read")]);
    await settled;

    expect(buttonLabels(root)).toEqual(["Sign out", "Check for new tools"]);
    const [tick] = ticks(root);
    expect(tick.checked).toBe(false);
    expect(tick.disabled).toBe(false);

    tick.checked = true;
    tick.dispatchEvent(new Event("change"));
    await settle();

    const enable = calls.find((c) => String(c.subpath ?? "").endsWith("/proxy/enable"));
    expect(JSON.parse(String(enable?.body))).toEqual({ names: ["search"], enabled: true });
    expect(conn.toolConfig.search).toEqual({ enabled: true, settings: {} });
    expect(ticks(root)[0].checked).toBe(true);
    expect(document.activeElement).toBe(ticks(root)[0]);
    expect(root.textContent).toContain("1 of 1 tools available to the agent.");
  });

  test("unticking turns a tool off and leaves it ready to go back on", async () => {
    const { root, calls, conn, settled } = mount(signedIn, [tool("search", "approved", "read")]);
    conn.toolConfig.search = { enabled: true, settings: {} };
    await settled;

    const [tick] = ticks(root);
    expect(tick.checked).toBe(true);
    tick.checked = false;
    tick.dispatchEvent(new Event("change"));
    await settle();

    const enable = calls.filter((c) => String(c.subpath ?? "").endsWith("/proxy/enable"));
    expect(JSON.parse(String(enable[0].body))).toEqual({ names: ["search"], enabled: false });
    expect(conn.toolConfig.search).toEqual({ enabled: false, settings: {} });
    expect(ticks(root)[0].checked).toBe(false);
  });

  test("a changed tool comes back unticked, and ticking it pins what it does now", async () => {
    const { root, calls, conn, settled } = mount(signedIn, [tool("moved", "changed", "write")]);
    conn.toolConfig.moved = { enabled: true, settings: {} };
    await settled;

    const [tick] = ticks(root);
    expect(tick.checked).toBe(false);
    expect(root.textContent).toContain("This tool changed. Read what it does now, then turn it back on.");
    expect(root.textContent).toContain("0 of 1 tools available to the agent.");

    tick.checked = true;
    tick.dispatchEvent(new Event("change"));
    await settle();

    const enable = calls.find((c) => String(c.subpath ?? "").endsWith("/proxy/enable"));
    expect(JSON.parse(String(enable?.body))).toEqual({ names: ["moved"], enabled: true });
    expect(root.querySelector(".tool-row .ui-badge")).toBeNull();
    expect(ticks(root)[0].checked).toBe(true);
  });

  test("a server Pluk can reach with nothing listed is asked exactly once", async () => {
    const { root, calls, settled } = mount(signedIn, []);
    await settled;
    await settle();

    expect(subpaths(calls).filter((s) => s.endsWith("/proxy/refresh"))).toHaveLength(1);
    expect(root.textContent).toContain("This server offers no tools yet.");
  });

  test("finishing the sign-in brings the tools in without a click", async () => {
    const realSetInterval = globalThis.setInterval;
    globalThis.setInterval = ((fn: () => void) => realSetInterval(fn, 1)) as never;
    try {
      const { root, calls, state, settled } = mount(
        { kind: "oauth", status: "not_connected", required: "oauth" },
        [],
      );
      await settled;
      expect(root.querySelector(".tool-name")).toBeNull();

      [...root.querySelectorAll("button")].find((b) => b.textContent === "Sign in")?.click();
      await settle();
      const before = calls.length;
      state.auth = signedIn;
      state.tools = [tool("search", "new", "read")];
      await settle();

      expect(subpaths(calls.slice(before)).some((s) => s.endsWith("/proxy/tools"))).toBe(true);
      expect([...root.querySelectorAll(".tool-name")].map((n) => n.textContent)).toEqual(["Search"]);
    } finally {
      globalThis.setInterval = realSetInterval;
    }
  });

  test("a server that lets anyone in offers no sign-in at all", async () => {
    const { root, settled } = mount({ kind: "none", status: "not_connected", required: "none" }, [
      tool("search", "approved", "read"),
    ]);
    await settled;

    expect(root.textContent).toContain("This server does not ask you to sign in.");
    expect(root.querySelector(".browser-status")).toBeNull();
    expect(buttonLabels(root)).not.toContain("Sign in");
    expect([...root.querySelectorAll(".tool-name")].map((n) => n.textContent)).toEqual(["Search"]);
  });

  test("a server that only takes a token says where to put it", async () => {
    const { root, settled } = mount(
      { kind: "none", status: "not_connected", required: "token" },
      [],
    );
    await settled;

    expect(root.textContent).toContain(
      "This server needs a token. Add one in this integration's settings.",
    );
    expect(buttonLabels(root)).not.toContain("Sign in");
  });

  test("a server that hands out no client IDs asks for one before the sign-in", async () => {
    const { root, settled } = mount(
      { kind: "none", status: "not_connected", required: "oauth", needsClientId: true },
      [],
    );
    await settled;

    expect(root.textContent).toContain(
      "This server needs a client ID. Add one in this integration's settings, then sign in.",
    );
    expect(buttonLabels(root)).not.toContain("Sign in");
  });

  test("an expired sign-in warns and offers a way back", async () => {
    const { root, settled } = mount(
      { kind: "oauth", status: "reconnect_needed", required: "oauth" },
      [],
    );
    await settled;

    expect(root.querySelector(".browser-status-disconnected")).not.toBeNull();
    expect(root.textContent).toContain("Your sign-in has expired");
    expect(buttonLabels(root)).toContain("Sign in again");
  });

  test("signing in sends the user to the browser", async () => {
    const { root, calls, settled } = mount(
      { kind: "oauth", status: "not_connected", required: "oauth" },
      [],
    );
    await settled;

    const signIn = [...root.querySelectorAll("button")].find((b) => b.textContent === "Sign in");
    expect(signIn).toBeDefined();
    signIn?.click();
    await settle();

    expect(calls.some((c) => c.cmd === "open_external")).toBe(true);
  });

  test("a first run ticks nothing, whatever the server says its tools do", async () => {
    const { root, calls, settled } = mount(signedIn, [
      tool("search", "new", "read"),
      tool("write", "new", "write"),
    ]);
    await settled;

    expect(ticks(root).map((t) => t.checked)).toEqual([false, false]);
    expect(subpaths(calls).some((s) => s.endsWith("/proxy/enable"))).toBe(false);
    expect(root.textContent).toContain("0 of 2 tools available to the agent.");
  });

  test("a server it cannot reach explains itself and offers a retry", async () => {
    Object.assign(window, {
      __TAURI__: { core: { invoke: async () => Promise.reject(new Error("connection refused")) } },
    });
    const root = document.createElement("div");
    mountServerTools(root, integration());
    await settle();

    expect(root.textContent).toContain("Pluk could not reach this server.");
    expect(buttonLabels(root)).toContain("Try again");
  });

  test("a route that refuses is shown in its own words", async () => {
    Object.assign(window, {
      __TAURI__: {
        core: {
          invoke: async () => ({ status: 502, body: { ok: false, error: "The server timed out" } }),
        },
      },
    });
    const root = document.createElement("div");
    mountServerTools(root, integration());
    await settle();

    expect(root.textContent).toContain("The server timed out");
  });
});
