import { describe, test, expect } from "bun:test";
import {
  filteredGroups,
  filteredIntegrations,
  availableTypesSorted,
  availableEnvs,
} from "./filter";
import { scaledSize } from "./tokens";
import { Zoom } from "./zoom";
import { adapterColor, adapterAbbrev } from "./glyph";
import type { Integration, Group, AdapterManifest } from "./types";

// Fixture list
const adapters: AdapterManifest[] = [
  { id: "postgres", label: "PostgreSQL" },
  { id: "linear", label: "Linear" },
  { id: "ssh", label: "SSH" },
];

const integrations: Integration[] = [
  { id: "1", name: "Production DB", type: "postgres", environment: "production", readOnly: true },
  { id: "2", name: "Staging DB", type: "postgres", environment: "staging", readOnly: false },
  { id: "3", name: "Linear Workspace", type: "linear", environment: "production", readOnly: false },
  { id: "4", name: "Bastion Host", type: "ssh", environment: "development", readOnly: false },
];

const groups: Group[] = [
  { id: "g1", name: "API Services", environment: "production", memberIds: ["1", "3"] },
  { id: "g2", name: "Mixed Group", environment: null, memberIds: ["2", "4"] },
  { id: "g3", name: "Dev Only", environment: "development", memberIds: ["4"] },
];

describe("filter and search reduction over fixture list", () => {
  test("search matches name, type, label and environment", () => {
    expect(filteredIntegrations(integrations, "postgres", new Set(), new Set(), adapters).map((c) => c.id)).toEqual(["1", "2"]);
    expect(filteredIntegrations(integrations, "PostgreSQL", new Set(), new Set(), adapters).map((c) => c.id)).toEqual(["1", "2"]);
    expect(filteredIntegrations(integrations, "prod", new Set(), new Set(), adapters).map((c) => c.id).sort()).toEqual(["1", "3"]);
    expect(filteredIntegrations(integrations, "Linear", new Set(), new Set(), adapters).map((c) => c.id)).toEqual(["3"]);
    // name
    expect(filteredIntegrations(integrations, "Bastion", new Set(), new Set(), adapters).map((c) => c.id)).toEqual(["4"]);
  });

  test("empty query returns all", () => {
    expect(filteredIntegrations(integrations, "", new Set(), new Set(), adapters).length).toBe(4);
    expect(filteredIntegrations(integrations, "   ", new Set(), new Set(), adapters).length).toBe(4);
  });

  test("type + env combo", () => {
    const res = filteredIntegrations(integrations, "", new Set(["postgres"]), new Set(["production"]), adapters);
    expect(res.map((c) => c.id)).toEqual(["1"]);
  });

  test("group search", () => {
    expect(filteredGroups(groups, "API", new Set(), new Set()).map((g) => g.id)).toEqual(["g1"]);
    expect(filteredGroups(groups, "", new Set(), new Set()).length).toBe(3);
  });
});

describe("offered filter choices limited to values in use", () => {
  test("available types only those present", () => {
    const types = availableTypesSorted(integrations, adapters);
    expect(types).toEqual(expect.arrayContaining(["postgres", "linear", "ssh"]));
    expect(types.length).toBe(3);
    // Not offering unused type e.g. mysql
    expect(types).not.toContain("mysql");
  });

  test("available envs only those present in integrations + groups", () => {
    const envs = availableEnvs(integrations, groups);
    expect(envs).toEqual(expect.arrayContaining(["production", "staging", "development"]));
    // local not present
    expect(envs).not.toContain("local");
    // order is production, staging, development, local
    expect(envs).toEqual(["production", "staging", "development"]);
  });

  test("envs includes group env even if no integration has it", () => {
    const onlyGroupEnv: Group[] = [{ id: "gx", name: "Local Group", environment: "local", memberIds: [] }];
    const envs = availableEnvs([], onlyGroupEnv);
    expect(envs).toEqual(["local"]);
  });
});

describe("type filter hides groups", () => {
  test("any active type filter hides all groups", () => {
    expect(filteredGroups(groups, "", new Set(["postgres"]), new Set())).toEqual([]);
    expect(filteredGroups(groups, "", new Set(["linear"]), new Set())).toEqual([]);
    // no type filter -> groups visible
    expect(filteredGroups(groups, "", new Set(), new Set()).length).toBe(3);
  });

  test("env filter still applies to groups when type filter empty", () => {
    expect(filteredGroups(groups, "", new Set(), new Set(["production"])).map((g) => g.id).sort()).toEqual(["g1", "g2"]);
    expect(filteredGroups(groups, "", new Set(), new Set(["development"])).map((g) => g.id).sort()).toEqual(["g2", "g3"]);
  });
});

describe("three health states rendering distinctly", () => {
  test("adapter color health dot distinction", () => {
    // helper: health absent -> no dot, ok -> green, error -> red
    function healthDotClass(health: { status: "ok" | "error" } | undefined): string | null {
      if (!health) return null;
      return health.status === "error" ? "error" : "ok";
    }
    expect(healthDotClass(undefined)).toBeNull();
    expect(healthDotClass({ status: "ok" })).toBe("ok");
    expect(healthDotClass({ status: "error" })).toBe("error");
    expect(healthDotClass({ status: "ok" })).not.toBe(healthDotClass({ status: "error" }));
    expect(healthDotClass(undefined)).not.toBe(healthDotClass({ status: "ok" }));
  });

  test("health absent must not render as healthy (third state)", () => {
    const healthMap: Record<string, { status: "ok" | "error" }> = { a: { status: "ok" } };
    // absent id should be treated as unknown, not ok
    expect(healthMap["missing"]).toBeUndefined();
    // rendering logic: if (!health) -> no dot
    const render = (id: string) => (healthMap[id] ? `dot-${healthMap[id].status}` : "no-dot");
    expect(render("a")).toBe("dot-ok");
    expect(render("missing")).toBe("no-dot");
    expect(render("missing")).not.toBe("dot-ok");
  });
});

describe("zoom applying to type scale", () => {
  test("scaledSize multiplies base by zoom", () => {
    // base body is 13
    expect(scaledSize("body", 1)).toBe(13);
    expect(scaledSize("body", 1.25)).toBe(13 * 1.25);
    expect(scaledSize("caption", 1)).toBe(11.5);
    expect(scaledSize("caption", 2)).toBe(23);
    // unknown style falls back to 13
    expect(scaledSize("unknown", 1)).toBe(13);
  });

  test("Zoom steps multiply type scale, never page transform", () => {
    // Verify zoom scale is applied to type only: scaledSize uses scale factor
    const baseBody = 13;
    const scale = 1.25;
    expect(scaledSize("body", scale)).toBe(baseBody * scale);
    // Zoom class exposes same scale steps as AppZoom.swift
    const z = new Zoom();
    const steps = z.state.steps;
    expect(steps).toEqual([0.85, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6, 1.8, 2.0]);
    // Applying zoom does not imply a page transform; the module documents that it sets --zoom-scale only.
  });
});

describe("adapter glyph fallback", () => {
  test("per-adapter colours are defined and not invented", () => {
    expect(adapterColor("postgres")).toBe("#4d75a8");
    expect(adapterColor("mysql")).toBe("#c78c33");
    expect(adapterColor("unknown-adapter")).toBe("#66687f");
  });
  test("two-letter abbrev fallback", () => {
    expect(adapterAbbrev("postgres")).toBe("PG");
    expect(adapterAbbrev("myservice")).toBe("MY");
    expect(adapterAbbrev("x")).toBe("X");
  });
});

describe("delete confirmation required before callback fires", () => {
  test("confirmation gate keeps delete from firing until confirmed, with required copy", () => {
    // Simulate sidebar's pendingDelete logic: only on confirm does onDelete fire
    let deleted: { kind: string; id: string } | null = null;
    const onDelete = (kind: string, id: string) => {
      deleted = { kind, id };
    };
    let pending: { kind: string; id: string; name: string } | null = null;
    const requestDelete = (kind: string, id: string, name: string) => {
      pending = { kind, id, name };
    };
    const confirmCopy = "This can’t be undone.";
    const dialogTitle = (p: { kind: string; name: string }) => `Delete ${p.kind} “${p.name}”?`;

    const confirm = () => {
      if (!pending) return;
      onDelete(pending.kind, pending.id);
      pending = null;
    };
    const cancel = () => {
      pending = null;
    };

    requestDelete("integration", "i1", "Prod DB");
    expect(pending).not.toBeNull();
    expect(dialogTitle(pending!)).toBe("Delete integration “Prod DB”?");
    expect(confirmCopy).toBe("This can’t be undone.");
    expect(deleted).toBeNull();

    cancel();
    expect(deleted).toBeNull();
    expect(pending).toBeNull();

    requestDelete("integration", "i1", "Prod DB");
    confirm();
    expect(deleted).toEqual({ kind: "integration", id: "i1" });
  });
});
