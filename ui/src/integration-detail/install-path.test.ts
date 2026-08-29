/**
 * The whole click path with nothing stubbed but the host itself: the Install
 * button, the real folder chooser and inject bridge in `host.ts`, down to the
 * `invoke` calls the window puts on the wire. The Rust end of the same path is
 * covered by `commands::inject_command_tests`.
 */

import { describe, test, expect, afterEach } from "bun:test";
import { renderMcpSection } from "./mcp-section";
import { injectMcpConfig } from "../host";

const target = { key: "marketing-db-production", url: "http://localhost:4242/mcp/tok" };

type Invocation = { cmd: string; args: Record<string, unknown> };

function attachHost(answer: (cmd: string) => unknown): Invocation[] {
  const calls: Invocation[] = [];
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        calls.push({ cmd, args });
        return answer(cmd);
      },
    },
  } as never;
  return calls;
}

function answerHappyPath(cmd: string): unknown {
  if (cmd === "list_installed_mcp_clients") return ["cursor"];
  if (cmd === "plugin:dialog|open") return "/repo";
  if (cmd === "inject_mcp_config") return { status: "added", path: "/repo/.cursor/mcp.json" };
  throw new Error(`unexpected command ${cmd}`);
}

async function mountAndInstall(root: HTMLElement): Promise<void> {
  renderMcpSection(root, target, injectMcpConfig);
  await settle();
  root.querySelector<HTMLButtonElement>('button[aria-label="Install into selected clients"]')!.click();
  await settle();
}

function settle(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

afterEach(() => {
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
});

describe("install click path", () => {
  test("reaches the host command that writes the file", async () => {
    const calls = attachHost(answerHappyPath);
    const root = document.createElement("div");

    await mountAndInstall(root);

    expect(calls.map((c) => c.cmd)).toEqual([
      "list_installed_mcp_clients",
      "plugin:dialog|open",
      "inject_mcp_config",
    ]);
  });

  test("asks the host for a folder before writing anything", async () => {
    const calls = attachHost(answerHappyPath);

    await mountAndInstall(document.createElement("div"));

    expect(calls.find((c) => c.cmd === "plugin:dialog|open")!.args).toEqual({
      options: { directory: true, multiple: false, title: "Choose the project folder" },
    });
  });

  test("sends the argument names Tauri resolves, not the Rust ones", async () => {
    const calls = attachHost(answerHappyPath);

    await mountAndInstall(document.createElement("div"));

    const args = calls.find((c) => c.cmd === "inject_mcp_config")!.args;
    expect(args).toEqual({
      client: "cursor",
      scope: "project",
      projectDir: "/repo",
      key: target.key,
      url: target.url,
    });
    expect(Object.keys(args)).not.toContain("project_dir");
  });

  test("confirms the written file to the user", async () => {
    attachHost(answerHappyPath);
    const root = document.createElement("div");

    await mountAndInstall(root);

    expect(root.querySelector(".toast")!.textContent).toBe(
      `Added “${target.key}” to /repo/.cursor/mcp.json`,
    );
    expect([...root.querySelectorAll(".install-outcome .target-row")].map((r) => r.textContent)).toEqual([
      "Cursor — added to /repo/.cursor/mcp.json",
    ]);
  });

  test("a folder chooser the host refuses is reported, not swallowed", async () => {
    attachHost((cmd) => {
      if (cmd === "list_installed_mcp_clients") return ["cursor"];
      throw new Error("dialog.open not allowed");
    });
    const root = document.createElement("div");

    await mountAndInstall(root);

    const toast = root.querySelector(".toast")!;
    expect(toast.textContent).toBe("Couldn’t open the folder chooser: dialog.open not allowed");
    expect(toast.className).toContain("toast-error");
  });

  test("a host that rejects the write names the reason", async () => {
    attachHost((cmd) => {
      if (cmd === "list_installed_mcp_clients") return ["cursor"];
      if (cmd === "plugin:dialog|open") return "/repo";
      throw new Error("Couldn't write /repo/.cursor/mcp.json: Permission denied");
    });
    const root = document.createElement("div");

    await mountAndInstall(root);

    expect(root.querySelector(".toast")!.textContent).toContain("Permission denied");
    expect(root.querySelector(".toast")!.className).toContain("toast-error");
  });

  test("no host attached says so instead of doing nothing", async () => {
    delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
    const root = document.createElement("div");

    await mountAndInstall(root);

    expect(root.querySelector(".toast")!.textContent).toContain("No Pluk host attached");
    expect(root.querySelector(".toast")!.className).toContain("toast-error");
  });
});
