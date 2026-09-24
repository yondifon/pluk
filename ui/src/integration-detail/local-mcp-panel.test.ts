import { describe, expect, test } from "bun:test";
import { mountLocalMcp } from "./local-mcp-panel";
import type { LaunchPreview, McpServerStatus } from "../host";
import type { Integration } from "./types";

type Row = ReturnType<typeof tool>;

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

function preview(overrides: Partial<LaunchPreview> = {}): LaunchPreview {
  return {
    program: "/opt/homebrew/bin/node",
    args: ["/path/to/sentry-mcp/build/index.js"],
    cwd: "/Users/isern",
    env: [
      { name: "SENTRY_URL", secret: false },
      { name: "SENTRY_AUTH_TOKEN", secret: true },
    ],
    warnings: [],
    launchHash: "hash-1",
    approved: false,
    ...overrides,
  };
}

/**
 * Stands in for the host: records what the panel asked it to do, and answers
 * each Tauri command and adapter route the way the real ones would.
 */
function host(initial: { preview: LaunchPreview; status?: McpServerStatus; output?: string[]; tools?: Row[] }) {
  const calls: Array<Record<string, unknown>> = [];
  const state = {
    preview: initial.preview,
    status: initial.status ?? { state: "stopped" as const },
    output: initial.output ?? [],
    tools: initial.tools ?? [],
  };
  Object.assign(window, {
    __TAURI__: {
      core: {
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, ...args });
          switch (cmd) {
            case "mcp_launch_preview":
              return state.preview;
            case "approve_mcp_launch":
              state.preview = { ...state.preview, approved: true };
              return null;
            case "mcp_server_status":
              return state.status;
            case "mcp_server_output":
              return state.output;
            case "stop_mcp_server":
              state.status = { state: "stopped" };
              return null;
            case "restart_mcp_server":
              state.status = { state: "starting" };
              return null;
            case "integration_api":
              return { status: 200, body: { tools: state.tools } };
            default:
              return null;
          }
        },
      },
    },
  });
  return { calls, state };
}

function integration(): Integration {
  return {
    id: "sentry-1",
    name: "Sentry",
    type: "mcp",
    config: { connection: "local" },
    toolConfig: {},
    token: "t",
    createdAt: "",
  };
}

const settle = () => new Promise((done) => setTimeout(done, 10));

function mount(initial: Parameters<typeof host>[0]) {
  const { calls, state } = host(initial);
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  mountLocalMcp(root, integration());
  return { root, calls, state };
}

describe("the approval card", () => {
  test("shows the resolved command and asks for approval before it has one", async () => {
    const { root } = mount({ preview: preview() });
    await settle();

    expect(root.textContent).toContain("/opt/homebrew/bin/node");
    expect(root.textContent).toContain("/path/to/sentry-mcp/build/index.js");
    expect(root.textContent).toContain("/Users/isern");
    expect(root.textContent).toContain("SENTRY_URL");
    expect(root.textContent).toContain("SENTRY_AUTH_TOKEN");
    expect(root.textContent).toContain("runs with your full access to this Mac");
    expect([...root.querySelectorAll("button")].some((b) => b.textContent === "Approve and start")).toBe(true);

    // Nothing about a secret's value is ever rendered: there is none on the wire to leak.
    expect(root.querySelector("input[type=password]")).toBeNull();
  });

  test("says it is waiting for approval, not that something has failed", async () => {
    const { root } = mount({ preview: preview() });
    await settle();
    expect(root.textContent).toContain("Approve the command above");
    expect(root.querySelector(".field-error")).toBeNull();
  });

  test("a command Pluk could not resolve is shown as an error with a retry, not a crash", async () => {
    const calls: Array<Record<string, unknown>> = [];
    Object.assign(window, {
      __TAURI__: {
        core: {
          invoke: async (cmd: string, args: Record<string, unknown>) => {
            calls.push({ cmd, ...args });
            if (cmd === "mcp_launch_preview") {
              throw "Pluk could not find sentry-mcp. Check that it is installed, or use its full path.";
            }
            if (cmd === "integration_api") return { status: 200, body: { tools: [] } };
            return null;
          },
        },
      },
    });
    const root = document.createElement("div");
    document.body.replaceChildren(root);
    mountLocalMcp(root, integration());
    await settle();

    expect(root.textContent).toContain("Pluk could not find sentry-mcp");
    expect([...root.querySelectorAll("button")].some((b) => b.textContent === "Try again")).toBe(true);
  });

  test("approving calls the host with the launch hash the preview showed", async () => {
    const { root, calls, state } = mount({ preview: preview() });
    await settle();

    const approve = [...root.querySelectorAll("button")].find((b) => b.textContent === "Approve and start")!;
    approve.click();
    await settle();

    expect(calls).toContainEqual({ cmd: "approve_mcp_launch", id: "sentry-1", launchHash: "hash-1" });
    expect(state.preview.approved).toBe(true);
    expect(root.textContent).toContain("Approved");
    expect(root.textContent).not.toContain("Approve the command above");
  });
});

describe("the status card", () => {
  test("a running server shows its pid and offers to stop it", async () => {
    const { root } = mount({ preview: preview({ approved: true }), status: { state: "running", pid: 4242 } });
    await settle();

    expect(root.textContent).toContain("Running");
    expect(root.textContent).toContain("4242");
    const stop = [...root.querySelectorAll("button")].find((b) => b.textContent === "Stop") as HTMLButtonElement;
    expect(stop.disabled).toBe(false);
  });

  test("a server that keeps stopping says so and points at the fix", async () => {
    const { root } = mount({ preview: preview({ approved: true }), status: { state: "crashed" } });
    await settle();
    expect(root.textContent).toContain("Keeps stopping");
    expect(root.textContent).toContain("restart it");
  });

  test("stopping calls the host and the status updates to stopped", async () => {
    const { root, calls } = mount({ preview: preview({ approved: true }), status: { state: "running", pid: 1 } });
    await settle();
    const stop = [...root.querySelectorAll("button")].find((b) => b.textContent === "Stop")!;
    stop.click();
    await settle();
    expect(calls).toContainEqual({ cmd: "stop_mcp_server", id: "sentry-1" });
    expect(root.textContent).toContain("Stopped");
  });

  test("opening Recent output fetches and shows the redacted tail", async () => {
    const { root, calls } = mount({
      preview: preview({ approved: true }),
      status: { state: "running", pid: 1 },
      output: ["starting with token <redacted>"],
    });
    await settle();

    expect(calls.every((c) => c.cmd !== "mcp_server_output")).toBe(true);
    const details = root.querySelector("details.output-disclosure") as HTMLDetailsElement;
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await settle();

    expect(calls).toContainEqual({ cmd: "mcp_server_output", id: "sentry-1" });
    expect(root.textContent).toContain("starting with token <redacted>");
  });
});

describe("the tools card", () => {
  test("waits for approval instead of asking the server for anything", async () => {
    const { root, calls } = mount({ preview: preview() });
    await settle();
    expect(root.textContent).toContain("Approve the command above to see what it offers");
    expect(calls.some((c) => c.subpath === "/proxy/refresh")).toBe(false);
  });

  test("once approved, discovers and lists what the server offers", async () => {
    const { root } = mount({
      preview: preview({ approved: true }),
      status: { state: "running", pid: 1 },
      tools: [tool("echo", "new", "read")],
    });
    await settle();
    expect(root.textContent).toContain("Echo");
  });
});
