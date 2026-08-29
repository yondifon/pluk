import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { renderMcpSection, type InjectFn } from "./mcp-section";
import { toast, mountToaster } from "../toast";

let toaster: HTMLElement;
let unmountToaster: () => void;

beforeEach(() => {
  toaster = document.createElement("div");
  document.body.appendChild(toaster);
  unmountToaster = mountToaster(toaster);
});

afterEach(() => {
  toast.clear();
  unmountToaster();
  toaster.remove();
});

function currentToast(): HTMLElement {
  return toaster.querySelector<HTMLElement>(".toast:not([data-exit])")!;
}

function toastText(selector: string): string {
  return currentToast().querySelector(selector)!.textContent ?? "";
}

function detailLines(): string[] {
  return (currentToast().querySelector(".toast-detail")!.textContent ?? "").split("\n");
}

const target = { key: "marketing-db-production", url: "http://localhost:4242/mcp/tok" };

type Call = Parameters<InjectFn>[0];

function mount(
  inject: InjectFn,
  opts?: { pickProjectDir?: (title: string) => Promise<string | null> },
): HTMLElement {
  const root = document.createElement("div");
  renderMcpSection(root, target, inject, {
    installed: ["cursor", "claudeCode", "codex"],
    pickProjectDir: opts?.pickProjectDir ?? (async () => "/repo"),
  });
  return root;
}

function installButton(root: HTMLElement): HTMLButtonElement {
  return root.querySelector<HTMLButtonElement>('button[aria-label="Install into selected clients"]')!;
}

async function clickInstall(root: HTMLElement): Promise<void> {
  installButton(root).click();
  await new Promise((r) => setTimeout(r, 0));
}

describe("mcp section", () => {
  test("endpoint and install controls share one heading", () => {
    const root = mount(async () => ({ status: "added", path: "" }));
    const headings = [...root.querySelectorAll(".ui-card-title")].map((h) => h.textContent);

    expect(headings).toEqual(["MCP endpoint"]);
    expect(root.querySelector(".inspector-row .mono")!.textContent).toBe(target.url);
    expect(root.querySelector('button[aria-label="Copy endpoint URL"]')).not.toBeNull();
    expect(installButton(root)).not.toBeNull();
  });

  test("shows the adapter's agent hint beside the endpoint", () => {
    const root = document.createElement("div");
    renderMcpSection(root, { ...target, agentHint: "Ask for a table before querying." }, async () => ({ status: "added", path: "" }), {
      installed: [],
    });

    const labels = [...root.querySelectorAll(".inspector-label")].map((l) => l.textContent);
    expect(labels).toEqual(["URL", "Agent hint"]);
  });
});

describe("agent setup install", () => {
  test("project scope writes every detected client that has a project file", async () => {
    const calls: Call[] = [];
    const root = mount(async (args) => {
      calls.push(args);
      return { status: "added", path: `/repo/${args.client}.json` };
    });

    await clickInstall(root);

    expect(calls.map((c) => c.client)).toEqual(["cursor", "claudeCode"]);
    expect(calls.every((c) => c.scope === "project" && c.projectDir === "/repo")).toBe(true);
    expect(calls[0].key).toBe(target.key);
    expect(calls[0].url).toBe(target.url);
  });

  test("global scope also reaches the clients that have no project file", async () => {
    const calls: Call[] = [];
    const root = mount(async (args) => {
      calls.push(args);
      return { status: "added", path: "~/.codex/config.toml" };
    });
    const scope = root.querySelector<HTMLSelectElement>("#pluk-scope-select")!;
    scope.value = "global";
    scope.dispatchEvent(new Event("change"));

    await clickInstall(root);

    expect(calls.map((c) => c.client).sort()).toEqual(["claudeCode", "codex", "cursor"]);
    expect(calls.every((c) => c.scope === "global" && c.projectDir === null)).toBe(true);
  });

  test("cancelling the folder chooser says so instead of going quiet", async () => {
    let called = false;
    const root = mount(
      async () => {
        called = true;
        return { status: "added", path: "" };
      },
      { pickProjectDir: async () => null },
    );

    await clickInstall(root);

    expect(called).toBe(false);
    expect(toastText(".toast-title")).toBe("Nothing installed");
    expect(toastText(".toast-description")).toBe("Choose a project folder to install into.");
  });

  test("reports the file behind every client, written or already there", async () => {
    const root = mount(async (args) =>
      args.client === "cursor"
        ? { status: "added", path: "/repo/.cursor/mcp.json" }
        : { status: "skipped", path: "/repo/.mcp.json" },
    );

    await clickInstall(root);

    expect(detailLines()).toEqual([
      "Cursor — added to /repo/.cursor/mcp.json",
      "Claude Code — already set up in /repo/.mcp.json",
    ]);
    expect(currentToast().dataset.variant).toBe("success");
  });

  test("a failing client reports its reason and does not stop the others", async () => {
    const root = mount(async (args) => {
      if (args.client === "claudeCode") throw new Error("Couldn't write /repo/.mcp.json: denied");
      return { status: "added", path: "/repo/.cursor/mcp.json" };
    });

    await clickInstall(root);

    expect(detailLines()).toEqual([
      "Cursor — added to /repo/.cursor/mcp.json",
      "Claude Code — Couldn't write /repo/.mcp.json: denied",
    ]);
    expect(currentToast().dataset.variant).toBe("error");
    expect(installButton(root).textContent).toBe("Install");
  });

  test("a single client installs only itself", async () => {
    const calls: Call[] = [];
    const root = mount(async (args) => {
      calls.push(args);
      return { status: "added", path: "~/.codex/config.toml" };
    });
    const client = root.querySelector<HTMLSelectElement>("#pluk-client-select")!;
    client.value = "codex";
    client.dispatchEvent(new Event("change"));

    await clickInstall(root);

    expect(calls.map((c) => c.client)).toEqual(["codex"]);
    // Codex has no project file, so the scope falls back to global.
    expect(calls[0].scope).toBe("global");
    expect(toastText(".toast-description")).toBe(`Added “${target.key}” to ~/.codex/config.toml`);
  });
});
