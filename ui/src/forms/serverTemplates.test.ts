import { describe, it, expect } from "vitest";
import { renderTypeChooser } from "./render.ts";
import type { AdapterManifest } from "./catalog.ts";
import {
  LOCAL_CATEGORIES,
  LOCAL_TEMPLATES,
  SERVER_TEMPLATES,
  SERVERS_SHOWN,
  addServer,
  availableName,
  type ServerHost,
  type ServerTemplate,
} from "./serverTemplates.ts";

const allowedLocalCategories = [
  "Browser & testing",
  "Search & web",
  "Developer tools",
  "Databases",
  "Cloud & infrastructure",
  "Productivity & docs",
  "Design",
  "AI & memory",
];

function manifest(id: string, label: string, category = "database"): AdapterManifest {
  return {
    id,
    label,
    category,
    policyKind: "sql",
    agentHint: "",
    runsCommands: false,
    offeredForSetup: true,
    tools: [],
    configFields: [{ key: "url", label: "Server URL", type: "text" as const, required: true }],
  };
}

/** A host that records what a tile asked the app to do. */
function spyHost(takenNames: string[] = []) {
  const created: unknown[] = [];
  const revealed: string[] = [];
  const prefilled: Array<{ name: string; template: ServerTemplate }> = [];
  const host: ServerHost = {
    takenNames,
    create: async (payload) => {
      created.push(payload);
      return { id: "new-1" };
    },
    reveal: (id) => {
      revealed.push(id);
    },
    prefill: (name, template) => {
      prefilled.push({ name, template });
    },
  };
  return { host, created, revealed, prefilled };
}

function chooser(host: ServerHost, opts: { serverUrls?: string[]; adapters?: AdapterManifest[] } = {}) {
  return renderTypeChooser(opts.adapters ?? [manifest("mcp", "MCP server", "other")], () => {}, {
    onPickServer: (template) => addServer(template, host),
    serverUrls: opts.serverUrls,
  });
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the server list", () => {
  it("has a unique id and an https address for every entry", () => {
    const ids = SERVER_TEMPLATES.map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const template of SERVER_TEMPLATES) {
      expect(template.url.startsWith("https://")).toBe(true);
    }
  });

  it("shows a tile for each vendor that moved off its own entry", () => {
    const el = chooser(spyHost().host);
    for (const id of ["linear", "sentry", "slack"]) {
      expect(el.querySelector(`[data-server="${id}"]`)).not.toBeNull();
    }
    expect(el.textContent).toContain("Linear, Sentry, and Slack connect through their official MCP servers.");
  });

  it("steps around a name already in use", () => {
    expect(availableName("Linear", [])).toBe("Linear");
    expect(availableName("Linear", ["Linear"])).toBe("Linear 2");
    expect(availableName("Linear", ["Linear", "Linear 2"])).toBe("Linear 3");
  });
});

describe("picking a server that signs in", () => {
  it("creates it once and opens it", async () => {
    const { host, created, revealed } = spyHost(["Linear"]);
    const el = chooser(host);
    el.querySelector<HTMLButtonElement>('[data-server="linear"]')!.click();
    await flush();

    expect(created).toEqual([
      {
        name: "Linear 2",
        type: "mcp",
        config: { url: "https://mcp.linear.app/mcp" },
        environment: null,
      },
    ]);
    expect(revealed).toEqual(["new-1"]);
  });

  it("holds the tile while it works", async () => {
    const { host } = spyHost();
    const el = chooser(host);
    const tile = el.querySelector<HTMLButtonElement>('[data-server="linear"]')!;
    tile.click();
    expect(tile.disabled).toBe(true);
    expect(tile.getAttribute("aria-busy")).toBe("true");
    await flush();
    expect(tile.disabled).toBe(false);
    expect(tile.hasAttribute("aria-busy")).toBe(false);
  });
});

describe("picking a server that needs a token", () => {
  it("opens the form prefilled and creates nothing", async () => {
    const { host, created, revealed, prefilled } = spyHost();
    const el = chooser(host);
    el.querySelector<HTMLButtonElement>('[data-server="github"]')!.click();
    await flush();

    expect(created).toEqual([]);
    expect(revealed).toEqual([]);
    expect(prefilled.length).toBe(1);
    expect(prefilled[0].name).toBe("GitHub");
    expect(prefilled[0].template).toMatchObject({ url: "https://api.githubcopilot.com/mcp/" });
    expect(prefilled[0].template.tokenHint).toBeTruthy();
  });
});

describe("picking a server that runs on this Mac", () => {
  it("opens the form on its command, even with no token to paste", async () => {
    const { host, created, prefilled } = spyHost();
    const el = chooser(host);
    el.querySelector<HTMLButtonElement>('[data-server="playwright"]')!.click();
    await flush();

    expect(created).toEqual([]);
    expect(prefilled[0].template).toMatchObject({ command: "npx", args: ["-y", "@playwright/mcp@latest"] });
  });

  it("gives every local server an allowed category", () => {
    expect(LOCAL_CATEGORIES).toEqual(allowedLocalCategories);
    for (const template of LOCAL_TEMPLATES) {
      expect(allowedLocalCategories).toContain(template.category);
    }
  });

  it("has a unique id across both lists and a key hint wherever a key is read", () => {
    const ids = [...SERVER_TEMPLATES, ...LOCAL_TEMPLATES].map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const template of LOCAL_TEMPLATES) {
      expect(Boolean(template.tokenEnv)).toBe(Boolean(template.tokenHint));
    }
  });

  it("renders every non-empty category as a closed group with all its tiles", () => {
    const categories = LOCAL_CATEGORIES.filter((category) => LOCAL_TEMPLATES.some((template) => template.category === category));
    const el = chooser(spyHost().host);
    const rendered = [...el.querySelectorAll<HTMLDetailsElement>("details.server-category")];

    expect(rendered).toHaveLength(categories.length);
    rendered.forEach((details, index) => {
      const category = categories[index];
      const templates = LOCAL_TEMPLATES.filter((template) => template.category === category);
      const grid = details.querySelector<HTMLElement>(".server-grid");
      expect(details.open).toBe(false);
      expect(details.querySelector("summary")?.textContent).toBe(`${category} · ${templates.length}`);
      expect(grid?.getAttribute("aria-label")).toBe(category);
      expect(grid?.querySelectorAll(".server-tile")).toHaveLength(templates.length);
      expect([...details.querySelectorAll("button")].some((button) => button.textContent?.startsWith("Show "))).toBe(false);
    });
  });
});

describe("a server already added", () => {
  it("is marked, trailing slash or not, and still works", async () => {
    const { host, created } = spyHost();
    const el = chooser(host, { serverUrls: ["https://mcp.sentry.dev/mcp/"] });
    const sentry = el.querySelector<HTMLButtonElement>('[data-server="sentry"]')!;
    expect(sentry.textContent).toContain("Added");
    expect(sentry.getAttribute("aria-label")).toBe("Add another Sentry");
    expect(el.querySelector<HTMLButtonElement>('[data-server="linear"]')!.textContent).not.toContain("Added");

    sentry.click();
    await flush();
    expect(created.length).toBe(1);
  });
});

describe("the popular servers section", () => {
  it("shows the first few, then the rest on request", () => {
    const { host } = spyHost();
    const el = chooser(host);
    const popular = el.querySelector('[aria-label="Popular servers"]')!;
    expect(popular.querySelectorAll(".server-tile").length).toBe(SERVERS_SHOWN);

    const more = [...el.querySelectorAll("button")].find((b) => b.textContent?.startsWith("Show "))!;
    more.click();
    expect(popular.querySelectorAll(".server-tile").length).toBe(SERVER_TEMPLATES.length);
    expect(el.contains(more)).toBe(false);
  });

  it("names each tile by what clicking it does", () => {
    const { host } = spyHost();
    const el = chooser(host);
    for (const tile of el.querySelectorAll<HTMLButtonElement>(".server-tile")) {
      expect(tile.type).toBe("button");
      expect(tile.getAttribute("aria-label")).toMatch(/^Add /);
    }
  });

  it("stays away when the catalog has no server type to create", () => {
    const { host } = spyHost();
    const el = chooser(host, { adapters: [manifest("postgres", "PostgreSQL")] });
    expect(el.querySelectorAll(".server-tile").length).toBe(0);
    expect(el.textContent).not.toContain("Popular servers");
  });
});

describe("the type picker beside it", () => {
  it("still renders and still picks", () => {
    const { host } = spyHost();
    let chosen: AdapterManifest | null = null;
    const el = renderTypeChooser(
      [manifest("mcp", "MCP server", "other"), manifest("postgres", "PostgreSQL")],
      (m) => (chosen = m),
      { onPickServer: (template) => addServer(template, host) },
    );
    const rows = [...el.querySelectorAll<HTMLButtonElement>(".chooser-row")];
    expect(rows.length).toBe(2);
    expect(el.textContent).toContain("Everything else");
    rows.find((row) => row.textContent?.includes("PostgreSQL"))!.click();
    expect(chosen!.id).toBe("postgres");
  });
});
