import { describe, it, expect } from "vitest";
import type { ToolDef } from "./catalog.ts";
import {
  availableTools,
  clearMemberTools,
  groupDraftFrom,
  hasPickedTools,
  memberTools,
  serializeGroup,
  setMemberTool,
  type GroupDraft,
  type GroupFormConnection,
} from "./groupForm.ts";

const TOOLS: ToolDef[] = [
  { name: "query", label: "Query", description: "Run a query", category: "read", defaultEnabled: true },
  { name: "list_tables", label: "List tables", description: "List tables", category: "read", defaultEnabled: true },
  { name: "drop_table", label: "Drop table", description: "Drop a table", category: "delete", defaultEnabled: false },
];

function connection(toolConfig: GroupFormConnection["toolConfig"] = {}): GroupFormConnection {
  return {
    id: "c1",
    name: "Metrics DB",
    type: "postgres",
    environment: "production",
    config: {},
    tools: TOOLS,
    toolConfig,
  };
}

function draft(tools: Record<string, string[]> = {}): GroupDraft {
  return { name: "G", environment: null, included: new Set(["c1"]), overrides: {}, tools };
}

describe("what a group member can offer", () => {
  it("offers the tools the integration has on and no others", () => {
    expect(availableTools(connection()).map((t) => t.name)).toEqual(["query", "list_tables"]);
  });

  it("follows the integration when it turns a tool on or off", () => {
    const conn = connection({
      list_tables: { enabled: false, settings: {} },
      drop_table: { enabled: true, settings: {} },
    });
    expect(availableTools(conn).map((t) => t.name)).toEqual(["query", "drop_table"]);
  });

  it("hands over everything it has on until someone picks", () => {
    expect(memberTools(draft(), connection())).toEqual(["query", "list_tables"]);
    expect(hasPickedTools(draft(), "c1")).toBe(false);
  });

  it("drops a picked tool the integration has since turned off", () => {
    const conn = connection({ list_tables: { enabled: false, settings: {} } });
    expect(memberTools(draft({ c1: ["query", "list_tables"] }), conn)).toEqual(["query"]);
  });

  it("does not hand over a tool the integration never turned on", () => {
    expect(memberTools(draft({ c1: ["query", "drop_table"] }), connection())).toEqual(["query"]);
  });
});

describe("picking a member's tools", () => {
  it("turns the first one off and leaves the rest", () => {
    const next = setMemberTool(draft(), connection(), "list_tables", false);
    expect(next.tools["c1"]).toEqual(["query"]);
    expect(hasPickedTools(next, "c1")).toBe(true);
  });

  it("keeps the integration's own order when one comes back", () => {
    const picked = draft({ c1: ["list_tables"] });
    expect(setMemberTool(picked, connection(), "query", true).tools["c1"]).toEqual([
      "query",
      "list_tables",
    ]);
  });

  it("can end up handing over nothing", () => {
    let next = setMemberTool(draft(), connection(), "query", false);
    next = setMemberTool(next, connection(), "list_tables", false);
    expect(next.tools["c1"]).toEqual([]);
    expect(memberTools(next, connection())).toEqual([]);
  });

  it("goes back to following the integration", () => {
    const next = clearMemberTools(draft({ c1: ["query"] }), "c1");
    expect(hasPickedTools(next, "c1")).toBe(false);
    expect(memberTools(next, connection())).toEqual(["query", "list_tables"]);
  });

  it("leaves other members alone", () => {
    const two = draft({ c2: ["ping"] });
    expect(setMemberTool(two, connection(), "query", false).tools["c2"]).toEqual(["ping"]);
    expect(clearMemberTools(two, "c1").tools["c2"]).toEqual(["ping"]);
  });
});

describe("saving and reopening a group", () => {
  it("sends a pick and stays silent when there is none", () => {
    const d = draft({ c1: ["query"] });
    d.included.add("c2");
    expect(serializeGroup(d, [{ id: "c1" }, { id: "c2" }])).toEqual([
      { id: "c1", overrides: {}, tools: ["query"] },
      { id: "c2", overrides: {} },
    ]);
  });

  it("sends an empty pick rather than reading it as all", () => {
    expect(serializeGroup(draft({ c1: [] }), [{ id: "c1" }])).toEqual([
      { id: "c1", overrides: {}, tools: [] },
    ]);
  });

  it("reopens on the same picks", () => {
    const reopened = groupDraftFrom({
      name: "G",
      environment: null,
      members: [
        { id: "c1", overrides: {}, tools: ["query"] },
        { id: "c2", overrides: {} },
      ],
    });
    expect(reopened.tools).toEqual({ c1: ["query"] });
    expect(hasPickedTools(reopened, "c2")).toBe(false);
  });
});
