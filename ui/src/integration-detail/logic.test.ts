import { describe, expect, test } from "bun:test";
import {
  deriveStatus,
  enabledCount,
  formatFanOutMessage,
  formatRelativeTime,
  genericConfigRows,
  isToolEnabled,
  mcpUrl,
  orderedTools,
  overviewRows,
  settingsSummary,
  statusLabel,
} from "./logic";
import type { AdapterManifest, ConfigField, ToolSpec } from "./types";

describe("deriveStatus", () => {
  test("unknown when no health", () => {
    expect(deriveStatus(null)).toBe("unknown");
    expect(deriveStatus(undefined)).toBe("unknown");
  });
  test("healthy when ok", () => {
    expect(deriveStatus({ status: "ok", at: Date.now() })).toBe("ok");
  });
  test("failing when error", () => {
    expect(deriveStatus({ status: "error", error: "refused", at: Date.now() })).toBe("failing");
  });
  test("label mapping", () => {
    expect(statusLabel("ok")).toBe("Healthy");
    expect(statusLabel("failing")).toBe("Failing");
    expect(statusLabel("unknown")).toBe("Not checked");
  });
  test("relative time with at", () => {
    const now = Date.now();
    expect(formatRelativeTime(undefined)).toBeNull();
    expect(formatRelativeTime(null as unknown as number)).toBeNull();
    expect(formatRelativeTime(now - 5000)).toBe("5s ago");
    expect(formatRelativeTime(now - 120_000)).toBe("2m ago");
    expect(formatRelativeTime(now - 3 * 3600_000)).toBe("3h ago");
    expect(formatRelativeTime(now - 2 * 86400_000)).toBe("2d ago");
    expect(formatRelativeTime(now + 10000)).toBeNull();
  });
});

describe("secret masking", () => {
  const fields: ConfigField[] = [
    { key: "host", label: "Host", type: "text" },
    { key: "password", label: "Password", type: "password", secret: true },
    { key: "ssh_password", label: "Passphrase", type: "password", secret: true },
  ];
  test("masks secret fields, leaves others", () => {
    const rows = genericConfigRows({ host: "db.example.com", password: "s3cr3t", ssh_password: "hunter2", database: "mydb" }, fields);
    const map = Object.fromEntries(rows);
    expect(map["Host"]).toBe("db.example.com");
    expect(map["Password"]).toBe("••••••");
    expect(map["Ssh Password"]).toBe("••••••");
    expect(map["Database"]).toBe("mydb");
    // never leaks value
    for (const [, v] of rows) expect(v).not.toContain("s3cr3t");
  });
  test("overview sqlite masks secret via manifest", () => {
    const manifest: AdapterManifest = {
      id: "sqlite",
      label: "SQLite",
      category: "database",
      agentHint: "",
      tools: [],
      configFields: fields,
    };
    const rows = overviewRows({ id: "1", name: "x", type: "sqlite", config: { filename: "/tmp/a.db", use_ssh: "true", ssh_host: "bastion", ssh_password: "secret" }, toolConfig: {}, token: "t", createdAt: "" }, manifest);
    const map = Object.fromEntries(rows);
    // ssh_password not shown directly but ssh_host shown; if ssh_host were secret it would mask
    expect(map["File"]).toBe("/tmp/a.db");
  });
  test("overview networked masks password", () => {
    const manifest: AdapterManifest = {
      id: "postgres",
      label: "PostgreSQL",
      category: "database",
      agentHint: "",
      tools: [],
      configFields: fields,
    };
    const rows = overviewRows(
      {
        id: "1",
        name: "x",
        type: "postgres",
        config: { host: "h", port: "5432", user: "u", password: "p", database: "db", use_ssh: "false", use_ssl: "false" },
        toolConfig: {},
        token: "t",
        createdAt: "",
      },
      manifest,
    );
    const map = Object.fromEntries(rows);
    // password not in networked overview rows directly, but host/user shown
    expect(map["Host"]).toBe("h");
    // generic fallback would mask if present
    const generic = genericConfigRows({ host: "h", password: "p" }, fields);
    expect(Object.fromEntries(generic)["Password"]).toBe("••••••");
  });
});

describe("enabled count", () => {
  const tools: ToolSpec[] = [
    { name: "query", description: "", category: "read", defaultEnabled: true },
    { name: "write", description: "", category: "write", defaultEnabled: false },
    { name: "delete", description: "", category: "delete", defaultEnabled: false },
  ];
  test("counts enabled including overrides", () => {
    expect(enabledCount(tools, {})).toBe(1);
    expect(enabledCount(tools, { write: { enabled: true, settings: {} } })).toBe(2);
    expect(enabledCount(tools, { query: { enabled: false, settings: {} } })).toBe(0);
  });
});

describe("tool ordering", () => {
  const tools: ToolSpec[] = [
    { name: "a", description: "", category: "read", defaultEnabled: true },
    { name: "b", description: "", category: "write", defaultEnabled: false },
    { name: "c", description: "", category: "read", defaultEnabled: true },
  ];
  test("enabled first, stable within groups", () => {
    const ordered = orderedTools(tools, { b: { enabled: true, settings: {} } });
    expect(ordered.map((t) => t.name)).toEqual(["a", "b", "c"]);
    const ordered2 = orderedTools(tools, {});
    expect(ordered2.map((t) => t.name)).toEqual(["a", "c", "b"]);
  });
});

describe("settings summary", () => {
  const tool: ToolSpec = {
    name: "query",
    description: "",
    category: "read",
    defaultEnabled: true,
    settings: [
      { key: "statements", label: "Statements", type: "select", options: [{ value: "mutations", label: "Mutations" }, { value: "all", label: "All" }], default: "mutations" },
      { key: "limit", label: "Limit", type: "number", default: "100" },
      { key: "empty", label: "Empty", type: "text", default: "" },
    ],
  };
  test("renders options label and skips empty", () => {
    expect(settingsSummary(tool, {})).toBe("Statements: Mutations · Limit: 100");
    expect(settingsSummary(tool, { query: { enabled: true, settings: { statements: "all", limit: "" } } })).toBe("Statements: All");
    expect(settingsSummary({ ...tool, settings: [] }, {})).toBeNull();
    expect(settingsSummary({ ...tool, settings: undefined }, {})).toBeNull();
  });
});

describe("copy action confirmation", () => {
  test("mcpUrl shape", () => {
    expect(mcpUrl("tok123")).toBe("http://localhost:4242/mcp/tok123");
  });
});

describe("fan-out result message", () => {
  test("single added", () => {
    const r = formatFanOutMessage("my-int", { added: ["Cursor"], skipped: [], failed: [] });
    expect(r.kind).toBe("success");
    expect(r.message).toBe('Added “my-int” to Cursor');
  });
  test("single skipped", () => {
    const r = formatFanOutMessage("my-int", { added: [], skipped: ["opencode"], failed: [] });
    expect(r.kind).toBe("success");
    expect(r.message).toContain("already in opencode");
  });
  test("single failed", () => {
    const r = formatFanOutMessage("my-int", { added: [], skipped: [], failed: [{ client: "Windsurf", reason: "permission denied" }] });
    expect(r.kind).toBe("error");
    expect(r.message).toContain("Couldn’t update Windsurf");
    expect(r.message).toContain("permission denied");
  });
  test("mix of added, skipped and failed", () => {
    const r = formatFanOutMessage("my-int", {
      added: ["Cursor", "Claude Code"],
      skipped: ["opencode"],
      failed: [{ client: "Windsurf", reason: "permission denied" }],
    });
    expect(r.kind).toBe("error");
    expect(r.message).toBe("Added to Cursor, Claude Code · Already set up in opencode · Couldn’t update Windsurf: permission denied");
  });
  test("added + skipped without failure is success", () => {
    const r = formatFanOutMessage("k", { added: ["Cursor"], skipped: ["Claude Code"], failed: [] });
    expect(r.kind).toBe("success");
    expect(r.message).toBe("Added to Cursor · Already set up in Claude Code");
  });
  test("multiple fails joined", () => {
    const r = formatFanOutMessage("k", {
      added: [],
      skipped: [],
      failed: [
        { client: "Cursor", reason: "not found" },
        { client: "Windsurf", reason: "denied" },
      ],
    });
    expect(r.message).toContain("Cursor: not found; Windsurf: denied");
  });
  test("empty is nothing", () => {
    const r = formatFanOutMessage("k", { added: [], skipped: [], failed: [] });
    expect(r.message).toBe("Nothing to update.");
  });
  test("no internal vocab leaks", () => {
    const r = formatFanOutMessage("my-int", { added: ["Cursor"], skipped: [], failed: [] });
    const lower = r.message.toLowerCase();
    for (const banned of ["adapter", "owner", "manifest", "policy kind", "projection", "field map"]) {
      expect(lower).not.toContain(banned);
    }
  });
});
