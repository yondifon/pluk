import { test, expect } from "bun:test";
import { assertMessageId, assertPositional, paging, range, sparkConfig, humanizeSparkError } from "./client.js";
import { sparkAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown> = {}): Integration {
  return { id: "s", name: "Spark", type: "spark", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

test("sparkConfig falls back to the installed CLI and safe limits", () => {
  expect(sparkConfig(conn())).toEqual({
    bin: "/usr/local/bin/spark",
    account: "",
    folder: "",
    team: "",
    maxPageSize: 25,
    timeoutMs: 30_000,
  });
});

test("sparkConfig honours explicit paths, defaults and limits", () => {
  const cfg = sparkConfig(conn({ spark_bin: " ~/bin/spark ", default_account: "me@co.com", max_page_size: 5, timeout_seconds: 10 }));
  expect(cfg.bin.endsWith("/bin/spark")).toBe(true);
  expect(cfg.bin.startsWith("~")).toBe(false);
  expect(cfg).toMatchObject({ account: "me@co.com", maxPageSize: 5, timeoutMs: 10_000 });
});

test("sparkConfig ignores nonsense limits rather than disabling the cap", () => {
  expect(sparkConfig(conn({ max_page_size: 0, timeout_seconds: -3 }))).toMatchObject({ maxPageSize: 25, timeoutMs: 30_000 });
});

test("assertPositional rejects a value that would read as a flag", () => {
  expect(assertPositional(" Inbox ", "folder")).toBe("Inbox");
  expect(() => assertPositional("--filter", "folder")).toThrow(/must not start with/);
  expect(() => assertPositional("  ", "folder")).toThrow(/required/);
});

test("assertMessageId accepts Spark ids and deep links, nothing else", () => {
  expect(assertMessageId(1234)).toBe("1234");
  expect(assertMessageId("https://sparkmailapp.com/dpl/bl?token=A")).toBe("https://sparkmailapp.com/dpl/bl?token=A");
  expect(assertMessageId("readdle-spark://bl=A")).toBe("readdle-spark://bl=A");
  for (const bad of ["", "--date", "12; rm -rf /", "abc"]) expect(() => assertMessageId(bad)).toThrow();
});

test("paging clamps the page size to the integration's cap", () => {
  const cfg = sparkConfig(conn({ max_page_size: 10 }));
  const args: string[] = [];
  paging(args, cfg, { page: 3, page_size: 500 });
  expect(args).toEqual(["--page", "3", "--page-size", "10"]);
});

test("paging defaults to the cap and omits page 1", () => {
  const args: string[] = [];
  paging(args, sparkConfig(conn()), {});
  expect(args).toEqual(["--page-size", "25"]);
});

test("range prefers explicit dates over the shortcut", () => {
  const shortcut: string[] = [];
  range(shortcut, { range: "week" });
  expect(shortcut).toEqual(["--week"]);

  const explicit: string[] = [];
  range(explicit, { range: "week", start: "2026-03-16", end: "2026-03-20" });
  expect(explicit).toEqual(["--start", "2026-03-16", "--end", "2026-03-20"]);
});

test("humanizeSparkError points at the two things a user can fix", () => {
  expect(humanizeSparkError(new Error("Connect failed - is Spark Desktop running with CLI server enabled?"))).toMatch(/Settings → AI Agents/);
  expect(humanizeSparkError(new Error("This account is read-only."))).toMatch(/Raise the account's access level/);
  expect(humanizeSparkError(new Error("no thread found"))).toBe("no thread found");
});

test("reads are on by default; every state change and both mail-emitting tools are off", () => {
  const specs = Object.fromEntries(sparkAdapter.toolSpecs.map((t) => [t.name, t]));

  expect(specs.accounts!.defaultEnabled).toBe(true);
  expect(specs.search_emails!.defaultEnabled).toBe(true);
  expect(specs.read_thread!.defaultEnabled).toBe(true);

  for (const name of ["draft", "comment", "email_action", "contact_action", "event_write", "delete_event", "send_draft", "unschedule_draft"]) {
    expect(specs[name]!.defaultEnabled).toBe(false);
  }
  // Sending mail is gated apart from ordinary writes, so a write-enabled
  // integration still can't put mail on the wire.
  expect(specs.send_draft!.category).toBe("admin");
  expect(specs.unschedule_draft!.category).toBe("admin");
  expect(specs.delete_event!.category).toBe("delete");
});

test("the adapter exposes the whole CLI surface once", () => {
  const names = sparkAdapter.toolSpecs.map((t) => t.name);
  expect(new Set(names).size).toBe(names.length);
  expect(names.sort()).toEqual(
    [
      "accounts", "availability", "comment", "contact_action", "delete_event", "draft", "email_action", "event_write",
      "find_contacts", "folders", "list_emails", "list_events", "list_meetings", "list_templates", "read_attachment",
      "read_meeting", "read_template", "read_thread", "search_emails", "send_draft", "team_info", "unschedule_draft",
    ].sort(),
  );
});

test("testConnection reports a missing CLI instead of shelling out", async () => {
  await expect(sparkAdapter.testConnection(conn({ spark_bin: "/nope/spark" }))).rejects.toThrow(/Spark CLI not found/);
});
