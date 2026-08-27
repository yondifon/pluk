import "./style.css";
import { renderTypeChooser, renderIntegrationForm, renderGroupForm } from "./forms/render.ts";
import type { AdapterManifest } from "./forms/catalog.ts";
import { emptyDraft, adopt, setEnvironment, canSave } from "./forms/connectionDraft.ts";
import type { GroupDraft } from "./forms/groupForm.ts";

const app = document.querySelector<HTMLDivElement>("#app");

if (app) {
  // Minimal demo shell: catalog-driven chooser -> form, plus group form toggle
  let adapters: AdapterManifest[] = [];
  let draft = emptyDraft();
  let picking = true;
  let activeManifest: AdapterManifest | undefined;

  async function loadCatalog() {
    try {
      const res = await fetch("/api/adapters");
      if (res.ok) {
        const data = (await res.json()) as { adapters: AdapterManifest[] };
        adapters = data.adapters;
      }
    } catch {
      // offline/demo fallback
    }
    if (!adapters.length) {
      adapters = fallbackAdapters();
    }
    render();
  }

  function fallbackAdapters(): AdapterManifest[] {
    return [
      {
        id: "postgres",
        label: "PostgreSQL",
        category: "database",
        policyKind: "sql",
        tools: [
          { name: "query", description: "Run a SQL query.", category: "read", defaultEnabled: true, settings: [{ key: "mode", label: "Statements", type: "select", options: [{ value: "read-only", label: "Read-only" }, { value: "mutations", label: "Mutations" }], default: "read-only" }] },
          { name: "list_tables", description: "List tables.", category: "read", defaultEnabled: true },
        ],
        configFields: [
          { key: "host", label: "Host", type: "text", group: "Connection", required: true, placeholder: "localhost" },
          { key: "port", label: "Port", type: "number", group: "Connection", required: true, placeholder: "5432" },
          { key: "use_ssl", label: "Use SSL", type: "toggle", group: "SSL", default: "true" },
          { key: "ssl_mode", label: "Mode", type: "select", group: "SSL", options: [{ value: "disable", label: "Disable" }, { value: "require", label: "Require" }], showIf: { key: "use_ssl", equals: "true" } },
        ],
      },
      {
        id: "linear",
        label: "Linear",
        category: "issue-tracker",
        policyKind: "action",
        tools: [
          { name: "search_issues", description: "Search issues.", category: "read", defaultEnabled: true },
          { name: "create_issue", description: "Create issue.", category: "write", defaultEnabled: false },
        ],
        configFields: [
          { key: "api_key", label: "API Key", type: "password", required: true, placeholder: "lin_api_…" },
          { key: "team_key", label: "Team Key", type: "text", placeholder: "ENG" },
        ],
      },
    ];
  }

  function render() {
    if (!app) return;
    app.innerHTML = "";
    const nav = document.createElement("div");
    nav.style.display = "flex"; nav.style.gap = "8px"; nav.style.marginBottom = "16px";
    const btnConn = document.createElement("button"); btnConn.className = "btn"; btnConn.textContent = picking ? "Pick type" : "Integration form";
    const btnGroup = document.createElement("button"); btnGroup.className = "btn"; btnGroup.textContent = "Group form";
    nav.append(btnConn, btnGroup);
    app.appendChild(nav);

    if (picking) {
      const chooser = renderTypeChooser(adapters, (m) => {
        activeManifest = m;
        draft = adopt(draft, m, true);
        picking = false;
        render();
      });
      app.appendChild(chooser);
    } else if (activeManifest) {
      const form = renderIntegrationForm(
        draft,
        activeManifest,
        (next) => {
          // Environment change must go through setEnvironment to preserve seeded semantics
          if (next.environment !== draft.environment) {
            draft = setEnvironment({ ...draft, ...next, environment: draft.environment }, next.environment);
            // also apply config changes
            draft = { ...draft, config: next.config, name: next.name, toolConfig: next.toolConfig };
            // re-apply env rule with config updates
            const tmp = { ...draft, config: next.config };
            draft = setEnvironment(tmp, next.environment);
            draft.name = next.name;
          } else {
            draft = next;
          }
          // keep save gate in sync
          const saveBtn = form.querySelector<HTMLButtonElement>(".btn-primary");
          if (saveBtn) saveBtn.disabled = !canSave(draft);
          // Re-render to reflect visibility/tools
          render();
        },
        (d) => {
          app.innerHTML = `<div class="card">Saved ${d.name} (${d.type})</div>`;
        },
        () => { picking = true; render(); },
        () => { picking = true; render(); },
      );
      app.appendChild(form);
    }

    btnGroup.addEventListener("click", () => {
      const groupDraft: GroupDraft = { name: "", environment: null, included: new Set(), overrides: {} };
      const conns = [
        { id: "c1", name: "Prod DB", type: "postgres", environment: "production", config: { host: "db.example.com" } as Record<string, string> },
        { id: "c2", name: "Linear", type: "linear", environment: "production", config: { api_key: "lin_xxx", team_key: "ENG" } as Record<string, string> },
      ];
      app.innerHTML = "";
      app.appendChild(nav);
      let gd = groupDraft;
      const renderGroup = () => {
        app.querySelector(".form-body")?.remove();
        const gForm = renderGroupForm(gd, conns, adapters, (n) => { gd = n; renderGroup(); }, () => { app.innerHTML = `<div class="card">Saved group ${gd.name}</div>`; }, () => render());
        app.appendChild(gForm);
      };
      renderGroup();
    });
  }

  loadCatalog();
}
