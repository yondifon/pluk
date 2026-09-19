import { describe, expect, test } from "bun:test";
import { mountServerTools } from "./server-tools";
import type { Integration } from "./types";

/** Stands in for the host, and records what the panel asked it to do. */
function host(auth: { kind: string; status: string }, tools: unknown[]) {
  const calls: Array<Record<string, unknown>> = [];
  Object.assign(window, {
    __TAURI__: {
      core: {
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, ...args });
          if (cmd !== "integration_api") return null;
          const body = String(args.subpath).includes("auth") ? { auth } : { tools };
          return { status: 200, body };
        },
      },
    },
  });
  return calls;
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

function mount(auth: { kind: string; status: string }, tools: unknown[]) {
  const calls = host(auth, tools);
  const root = document.createElement("div");
  mountServerTools(root, integration());
  return { root, calls, settled: new Promise((done) => setTimeout(done, 10)) };
}

const buttonLabels = (root: HTMLElement) =>
  [...root.querySelectorAll("button")].map((b) => b.textContent);

describe("mountServerTools", () => {
  test("shows what needs a decision first, and tells the states apart", async () => {
    const { root, settled } = mount({ kind: "oauth", status: "connected" }, [
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

  test("a tool cannot be switched on before it is approved", async () => {
    const { root, settled } = mount({ kind: "oauth", status: "connected" }, [
      tool("moved", "changed", "write"),
      tool("search", "new", "read"),
      tool("edit", "approved", "write"),
    ]);
    await settled;

    const switches = [...root.querySelectorAll<HTMLInputElement>(".tool-head input")];
    expect(switches.map((s) => s.disabled)).toEqual([true, true, false]);
  });

  test("an expired sign-in warns and offers a way back", async () => {
    const { root, settled } = mount({ kind: "oauth", status: "reconnect_needed" }, []);
    await settled;

    expect(root.querySelector(".browser-status-disconnected")).not.toBeNull();
    expect(root.textContent).toContain("Your sign-in has expired");
    expect(buttonLabels(root)).toContain("Sign in again");
  });

  test("signing in sends the user to the browser", async () => {
    const { root, calls, settled } = mount({ kind: "oauth", status: "not_connected" }, []);
    await settled;

    const signIn = [...root.querySelectorAll("button")].find((b) => b.textContent === "Sign in");
    expect(signIn).toBeDefined();
    signIn?.click();
    await new Promise((done) => setTimeout(done, 10));

    expect(calls.some((c) => c.cmd === "open_external")).toBe(true);
  });

  test("a first run ticks what only reads and approves nothing on its own", async () => {
    const { root, calls, settled } = mount({ kind: "oauth", status: "connected" }, [
      tool("search", "new", "read"),
      tool("write", "new", "write"),
    ]);
    await settled;

    const ticks = [...root.querySelectorAll<HTMLInputElement>(".tool-head input")];
    expect(ticks.map((t) => t.checked)).toEqual([true, false]);
    expect(calls.some((c) => String(c.subpath ?? "").includes("approve"))).toBe(false);
    expect(buttonLabels(root)).toContain("Approve ticked tools");
  });

  test("a server it cannot reach explains itself and offers a retry", async () => {
    Object.assign(window, {
      __TAURI__: { core: { invoke: async () => Promise.reject(new Error("connection refused")) } },
    });
    const root = document.createElement("div");
    mountServerTools(root, integration());
    await new Promise((done) => setTimeout(done, 10));

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
    await new Promise((done) => setTimeout(done, 10));

    expect(root.textContent).toContain("The server timed out");
  });
});
