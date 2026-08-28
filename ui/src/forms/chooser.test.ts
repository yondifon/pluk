import { describe, it, expect } from "vitest";
import { renderTypeChooser } from "./render.ts";
import type { AdapterManifest } from "./catalog.ts";
import { emptyDraft, adopt } from "./connectionDraft.ts";

function manifest(id: string, label: string, category = "database"): AdapterManifest {
  return {
    id,
    label,
    category,
    policyKind: "sql",
    agentHint: "",
    tools: [{ name: "query", description: "Run", category: "read", defaultEnabled: true }],
    configFields: [{ key: "host", label: "Host", type: "text" as const, required: true }],
  };
}

describe("type chooser draws from live catalog", () => {
  it("renders one button per adapter, grouped by category", () => {
    const adapters = [manifest("postgres", "PostgreSQL", "database"), manifest("linear", "Linear", "issue-tracker"), manifest("redis", "Redis", "database")];
    let chosen: AdapterManifest | null = null;
    const el = renderTypeChooser(adapters, (m) => (chosen = m));
    const btns = el.querySelectorAll<HTMLButtonElement>(".chooser-row");
    expect(btns.length).toBe(3);
    expect(el.textContent).toContain("PostgreSQL");
    expect(el.textContent).toContain("Linear");
    expect(el.textContent).toContain("Redis");
    // grouped: database and issue-tracker headings
    expect(el.textContent).toContain("Database");
    expect(el.textContent).toContain("Issue Tracker");
    // click chooses
    btns[0].click();
    expect(chosen!.id).toBe("postgres");
  });

  it("not hardcoded: different catalog produces different buttons", () => {
    const a = [manifest("sqlite", "SQLite")];
    const el = renderTypeChooser(a, () => {});
    expect(el.querySelectorAll(".chooser-row").length).toBe(1);
    expect(el.textContent).toContain("SQLite");
    expect(el.textContent).not.toContain("PostgreSQL");
  });
});

describe("chooser picking adopts service fields and tool defaults", () => {
  it("adopt seeds config defaults and tool states when reset", () => {
    const m = manifest("postgres", "PostgreSQL");
    m.configFields = [{ key: "port", label: "Port", type: "text" as const, default: "5432" }];
    m.tools = [{ name: "query", description: "", category: "read", defaultEnabled: true, settings: [{ key: "mode", label: "Mode", type: "select" as const, default: "read-only" }] }];
    const d = adopt(emptyDraft(), m, true);
    expect(d.type).toBe("postgres");
    expect(d.config["port"]).toBe("5432");
    expect(d.toolConfig["query"]).toBeDefined();
    expect(d.toolConfig["query"].enabled).toBe(true);
    // field list adopted
    expect(d.fields.map((f) => f.key)).toContain("port");
  });
});

describe("chooser cancel returns without half-created integration", () => {
  it("has a Cancel button that calls onCancel", () => {
    let cancelled = false;
    const el = renderTypeChooser([manifest("postgres", "PostgreSQL")], () => {}, { onCancel: () => (cancelled = true) });
    const cancel = [...el.querySelectorAll("button")].find((b) => b.textContent === "Cancel");
    expect(cancel).toBeDefined();
    cancel!.click();
    expect(cancelled).toBe(true);
  });

  it("Escape triggers cancel", () => {
    let cancelled = false;
    const el = renderTypeChooser([manifest("postgres", "PostgreSQL")], () => {}, { onCancel: () => (cancelled = true) });
    el.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(cancelled).toBe(true);
  });
});

describe("chooser accessibility", () => {
  it("has heading and aria labels and keyboard reachable buttons", () => {
    const el = renderTypeChooser([manifest("postgres", "PostgreSQL"), manifest("linear", "Linear")], () => {}, { onCancel: () => {} });
    const heading = el.querySelector("h2");
    expect(heading).not.toBeNull();
    expect(heading!.textContent).toBe("Choose an integration");
    expect(el.getAttribute("aria-label")).toBe("Choose an integration");
    const btns = el.querySelectorAll<HTMLButtonElement>(".chooser-row");
    for (const b of btns) {
      expect(b.getAttribute("aria-label")).toBeTruthy();
      expect(b.type).toBe("button");
    }
    // sections have group role
    const sections = el.querySelectorAll('section[role="group"]');
    expect(sections.length).toBeGreaterThan(0);
  });

  it("shows loading and error without internal vocab", () => {
    const loading = renderTypeChooser([], () => {}, {});
    expect(loading.textContent).toContain("Loading integrations");
    const failed = renderTypeChooser([], () => {}, { adaptersLoadFailed: true, onCancel: () => {} });
    expect(failed.textContent).toContain("Couldn’t load integrations");
    expect(failed.textContent).not.toMatch(/manifest|projection|slug/i);
  });
});
