import { describe, expect, test } from "bun:test";
import {
  afterSave,
  canSave,
  describeImportError,
  nameProblem,
  reviewItems,
  type ImportedServer,
  type ParsedImport,
  type ServerDraft,
} from "./importConfig";
import { renderImportFlow, type ImportHost } from "./importConfigView";
import { pendingOffNote } from "../integration-detail/proxy-tools";

function server(overrides: Partial<ServerDraft> = {}): ServerDraft {
  return {
    name: "sentry-selfhosted",
    connection: "local",
    url: null,
    headers: [],
    command: "node",
    args: ["/path/to/sentry-mcp/build/index.js"],
    env: [
      { name: "SENTRY_AUTH_TOKEN", value: "sntrys_secret", secret: true },
      { name: "SENTRY_URL", value: "https://sentry.internal.domain", secret: false },
    ],
    cwd: null,
    disabledTools: ["create_sentry_issue_comment", "update_sentry_issue_status"],
    notImported: [],
    viaMcpRemote: false,
    turnedOff: false,
    sse: false,
    ...overrides,
  };
}

function remote(name: string, overrides: Partial<ServerDraft> = {}): ServerDraft {
  return server({
    name,
    connection: "remote",
    url: `https://${name}.example/mcp`,
    command: null,
    args: [],
    env: [],
    disabledTools: [],
    headers: [{ name: "Authorization", value: "Bearer abc", secret: true }],
    ...overrides,
  });
}

/** Stands in for the app: answers parse and save the way the host would, and records the calls. */
function fakeHost(opts: {
  parsed?: ParsedImport;
  parseError?: unknown;
  taken?: string[];
  save?: (servers: ServerDraft[]) => ImportedServer[];
}) {
  const calls = { parse: [] as string[], save: [] as ServerDraft[][], done: [] as string[][], back: 0, cancel: 0 };
  const host: ImportHost = {
    takenNames: () => opts.taken ?? [],
    parse: async (text) => {
      calls.parse.push(text);
      if (opts.parseError) throw opts.parseError;
      return opts.parsed ?? { servers: [], problems: [] };
    },
    save: async (servers) => {
      calls.save.push(servers);
      return (opts.save ?? ((s) => s.map((d, i) => ({ name: d.name, integration: { id: `id-${i}` } }))))(servers);
    },
    onDone: (ids) => void calls.done.push(ids),
    onBack: () => void calls.back++,
    onCancel: () => void calls.cancel++,
  };
  return { host, calls };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function button(root: HTMLElement, label: string): HTMLButtonElement {
  const found = [...root.querySelectorAll("button")].find((b) => b.textContent === label);
  if (!found) throw new Error(`no ${label} button in: ${root.textContent}`);
  return found;
}

async function pasteAndReview(root: HTMLElement, text = "{}"): Promise<void> {
  const box = root.querySelector("textarea")!;
  box.value = text;
  box.dispatchEvent(new Event("input"));
  button(root, "Review servers").click();
  await tick();
}

function typeName(root: HTMLElement, index: number, name: string): void {
  const input = root.querySelector<HTMLInputElement>(`#import-name-${index}`)!;
  input.value = name;
  input.dispatchEvent(new Event("input"));
}

describe("name checks", () => {
  test("a name taken in Pluk or twice in the list is a problem, and skipping clears it", () => {
    const items = reviewItems({ servers: [remote("linear"), remote("docs"), remote("Docs ")], problems: [] });
    expect(nameProblem(items, 0, ["Linear"])).toBe("This name is already in Pluk. Choose another.");
    expect(nameProblem(items, 1, [])).toBe("Another server in this list has this name. Choose another.");
    expect(canSave(items, [])).toBe(false);
    items[2].skip = true;
    expect(nameProblem(items, 1, [])).toBeNull();
    expect(canSave(items, [])).toBe(true);
  });

  test("a server the config turned off starts skipped", () => {
    const items = reviewItems({ servers: [remote("a", { turnedOff: true }), remote("b")], problems: [] });
    expect(items.map((i) => i.skip)).toEqual([true, false]);
  });

  test("parse errors lead with where they are", () => {
    expect(describeImportError({ message: "This JSON has a mistake: EOF.", line: 3, column: 9 })).toBe(
      "Line 3, column 9: This JSON has a mistake: EOF.",
    );
    expect(describeImportError({ message: "Paste a server config first." })).toBe("Paste a server config first.");
  });

  test("after a save only the servers that failed stay, with their reason", () => {
    const items = reviewItems({ servers: [remote("a"), remote("b"), remote("c")], problems: [] });
    items[2].skip = true;
    const left = afterSave(items, [
      { name: "a", integration: { id: "1" } },
      { name: "b", error: "b is already in Pluk. Choose another name." },
    ]);
    expect(left.map((i) => [i.draft.name, i.error])).toEqual([["b", "b is already in Pluk. Choose another name."]]);
  });
});

describe("review flow", () => {
  test("the Sentry paste reviews as a local server and saves with the secret flags the user chose", async () => {
    const { host, calls } = fakeHost({
      parsed: { servers: [server(), remote("datadog", { notImported: ["autoApprove"] })], problems: [] },
    });
    const root = renderImportFlow(host);
    await pasteAndReview(root, '{"mcpServers":{}}');
    expect(calls.parse).toEqual(['{"mcpServers":{}}']);

    const cards = root.querySelectorAll(".import-card");
    expect(cards).toHaveLength(2);
    const sentry = cards[0];
    expect(sentry.textContent).toContain("Local server");
    expect(sentry.textContent).toContain("node");
    expect(sentry.textContent).toContain("Tools to turn off once Pluk finds them: create_sentry_issue_comment, update_sentry_issue_status");
    expect(sentry.textContent).not.toContain("sntrys_secret");
    const tokenSecret = sentry.querySelector<HTMLInputElement>('input[aria-label="Keep SENTRY_AUTH_TOKEN secret"]')!;
    const urlSecret = sentry.querySelector<HTMLInputElement>('input[aria-label="Keep SENTRY_URL secret"]')!;
    expect(tokenSecret.checked).toBe(true);
    expect(urlSecret.checked).toBe(false);
    expect(cards[1].textContent).toContain("Remote server");
    expect(cards[1].textContent).toContain("Not imported: autoApprove");

    urlSecret.checked = true;
    urlSecret.dispatchEvent(new Event("change"));
    const add = button(root, "Add 2 servers");
    expect(add.disabled).toBe(false);
    add.click();
    await tick();

    expect(calls.save).toHaveLength(1);
    const sent = calls.save[0];
    expect(sent.map((d) => d.name)).toEqual(["sentry-selfhosted", "datadog"]);
    expect(sent[0].env.map((r) => [r.name, r.secret])).toEqual([
      ["SENTRY_AUTH_TOKEN", true],
      ["SENTRY_URL", true],
    ]);
    expect(calls.done).toEqual([["id-0", "id-1"]]);
  });

  test("a clashing name blocks the save until it is renamed or skipped", async () => {
    const { host, calls } = fakeHost({
      parsed: { servers: [remote("linear"), remote("docs")], problems: [] },
      taken: ["Linear"],
    });
    const root = renderImportFlow(host);
    await pasteAndReview(root);

    const name = root.querySelector<HTMLInputElement>("#import-name-0")!;
    const error = root.querySelector<HTMLElement>("#import-name-error-0")!;
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("This name is already in Pluk. Choose another.");
    expect(name.getAttribute("aria-invalid")).toBe("true");
    expect(button(root, "Add 2 servers").disabled).toBe(true);

    typeName(root, 0, "docs");
    expect(root.querySelector<HTMLElement>("#import-name-error-1")!.textContent).toBe(
      "Another server in this list has this name. Choose another.",
    );

    typeName(root, 0, "Linear work");
    expect(error.hidden).toBe(true);
    expect(name.hasAttribute("aria-invalid")).toBe(false);

    const skip = root.querySelectorAll<HTMLInputElement>(".import-skip input")[1];
    skip.checked = true;
    skip.dispatchEvent(new Event("change"));
    const add = button(root, "Add 1 server");
    expect(add.disabled).toBe(false);
    add.click();
    await tick();
    expect(calls.save[0].map((d) => d.name)).toEqual(["Linear work"]);
  });

  test("a partial save keeps the failed server with its reason, and Done keeps what was added", async () => {
    const { host, calls } = fakeHost({
      parsed: { servers: [remote("a"), remote("b")], problems: [] },
      save: () => [
        { name: "a", integration: { id: "id-a" } },
        { name: "b", error: "Authorization is already sent by the sign-in. Remove this header." },
      ],
    });
    const root = renderImportFlow(host);
    await pasteAndReview(root);
    button(root, "Add 2 servers").click();
    await tick();

    expect(root.querySelector('[role="status"]')!.textContent).toBe(
      "Added 1 server. 1 server wasn't added. Fix the problem below, or skip it.",
    );
    const cards = root.querySelectorAll(".import-card");
    expect(cards).toHaveLength(1);
    expect(cards[0].textContent).toContain("Authorization is already sent by the sign-in.");
    expect(calls.done).toEqual([]);

    button(root, "Done").click();
    expect(calls.done).toEqual([["id-a"]]);
  });

  test("entries that could not be read are listed, and with nothing left the empty state shows", async () => {
    const { host } = fakeHost({
      parsed: { servers: [], problems: [{ name: "broken", message: "This entry has no URL or command." }] },
    });
    const root = renderImportFlow(host);
    await pasteAndReview(root);

    expect(root.querySelector(".import-problems")!.textContent).toContain("broken: This entry has no URL or command.");
    expect(root.querySelector(".import-empty")!.textContent).toContain("No servers to add");
    expect(button(root, "Add 0 servers").disabled).toBe(true);

    button(root, "Back").click();
    expect(root.querySelector("textarea")!.value).toBe("{}");
  });

  test("a parse error shows under the paste box with its line, and the text stays", async () => {
    const { host } = fakeHost({ parseError: { message: "This JSON has a mistake: EOF while parsing an object.", line: 4, column: 1 } });
    const root = renderImportFlow(host);
    await pasteAndReview(root, '{"mcpServers": {');

    const error = root.querySelector('[role="alert"]')!;
    expect(error.textContent).toBe("Line 4, column 1: This JSON has a mistake: EOF while parsing an object.");
    expect(root.querySelector("textarea")!.value).toBe('{"mcpServers": {');
    expect(root.querySelector("textarea")!.getAttribute("aria-invalid")).toBe("true");
  });

  test("an mcp-remote server says Pluk connects to the URL itself", async () => {
    const { host } = fakeHost({ parsed: { servers: [remote("linear", { viaMcpRemote: true })], problems: [] } });
    const root = renderImportFlow(host);
    await pasteAndReview(root);
    expect(root.textContent).toContain("This config ran mcp-remote to reach this server. Pluk connects to the URL directly.");
  });
});

describe("tools still to turn off", () => {
  const row = (name: string, present = true) => ({
    name,
    label: name,
    description: "",
    category: "write",
    state: "new" as const,
    present,
    updatedAt: "",
  });

  test("names the ones the server has not listed, and nothing once all are found", () => {
    expect(pendingOffNote(["a", "b"], null)).toContain("turned off a, b.");
    expect(pendingOffNote(["a", "b"], [row("a"), row("b", false)])).toContain("turned off b.");
    expect(pendingOffNote(["a"], [row("a")])).toBeNull();
    expect(pendingOffNote(undefined, [])).toBeNull();
  });
});
