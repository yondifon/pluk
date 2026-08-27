import "./style.css";
import { zoom } from "./zoom.ts";
import { createShell, renderBanners } from "./shell.ts";
import { createSidebar, type SidebarState } from "./sidebar.ts";
import { emptyState, renderEmptyState } from "./emptyStates.ts";
import { mountIntegrationDetail } from "./integration-detail/index.ts";
import type { Integration as DetailIntegration, ConnHealth as DetailHealth } from "./integration-detail/types.ts";
import { renderGroupDetail } from "./groupDetail.ts";
import { renderIntegrationForm, renderGroupForm, renderTypeChooser } from "./forms/render.ts";
import {
  adopt,
  applyEnvironmentDefaults,
  draftFromConnection,
  emptyDraft,
  type ConnectionDraft,
} from "./forms/connectionDraft.ts";
import { groupDraftFrom, serializeGroup, type GroupDraft } from "./forms/groupForm.ts";
import type { AdapterManifest as CatalogManifest, ToolState } from "./forms/catalog.ts";
import { ToastCenter, renderToasts } from "./toast.ts";
import { humanizeHealthError } from "./health.ts";
import { invoke, hasHost } from "./host.ts";
import type { Integration, Group, Environment, Health } from "./types.ts";

const app = document.getElementById("app")!;

zoom.apply();
zoom.syncFromHost();
zoom.bindKeyboard();

type HostIntegration = {
  id: string;
  name: string;
  type: string;
  config: Record<string, unknown>;
  environment: string | null;
  toolConfig: Record<string, ToolState>;
  token: string;
  createdAt: string;
};

type HostGroup = {
  id: string;
  name: string;
  environment: string | null;
  members: Array<{ id: string; overrides?: Record<string, string> }>;
  token: string;
  createdAt: string;
};

/** What the sidebar and detail screens are showing. */
type Selection =
  | { kind: "none" }
  | { kind: "integration"; id: string }
  | { kind: "group"; id: string }
  | { kind: "choose-integration-type" }
  | { kind: "new-integration" }
  | { kind: "new-group" }
  | { kind: "edit-integration"; id: string }
  | { kind: "edit-group"; id: string };

let state: SidebarState = {
  integrations: [],
  groups: [],
  adapters: [],
  health: {},
  adaptersLoadFailed: false,
  loading: true,
};
let manifests: CatalogManifest[] = [];
let hostIntegrations: HostIntegration[] = [];
let hostGroups: HostGroup[] = [];
let selection: Selection = { kind: "none" };
let previousSelection: Selection = { kind: "none" };
let draft: ConnectionDraft | null = null;
let groupDraft: GroupDraft | null = null;
let detailHandle: { destroy: () => void; updateHealth: (next: DetailHealth | null) => void } | null = null;
let detachDetail: (() => void) | null = null;

const toasts = new ToastCenter();

function manifestFor(type: string): CatalogManifest | undefined {
  return manifests.find((m) => m.id === type);
}

function toDetailIntegration(row: HostIntegration): DetailIntegration {
  const config: Record<string, string> = {};
  for (const [key, value] of Object.entries(row.config)) {
    config[key] = value == null ? "" : String(value);
  }
  return {
    id: row.id,
    name: row.name,
    type: row.type,
    environment: (row.environment ?? undefined) as DetailIntegration["environment"],
    config,
    toolConfig: row.toolConfig,
    token: row.token,
    createdAt: row.createdAt,
  };
}

function report(error: unknown, integrationId: string, title: string): void {
  const reason = error instanceof Error ? error.message : String(error);
  toasts.present({
    integrationId,
    title,
    message: `${reason.replace(/\.?$/, ".")} Try again.`,
    kind: "error",
  });
}

// ── Detail rendering ─────────────────────────────────────────────────────────

function renderDetail(mount: HTMLElement): void {
  detailHandle?.destroy();
  detailHandle = null;
  detachDetail?.();
  detachDetail = null;
  mount.innerHTML = "";

  const wrap = document.createElement("div");
  wrap.style.height = "100%";
  mount.appendChild(wrap);

  switch (selection.kind) {
    case "none": {
      if (state.loading) {
        wrap.className = "detail-loading";
        wrap.textContent = "Loading…";
      } else if (!state.integrations.length && !state.groups.length) {
        renderEmptyState(wrap, emptyState("no-integrations"), (action) => {
          if (action === "new-integration") startNewIntegration();
        });
      } else {
        renderEmptyState(wrap, emptyState("nothing-selected"));
      }
      return;
    }
    case "integration": {
      const { id } = selection;
      const row = hostIntegrations.find((c) => c.id === id);
      if (!row) return renderEmptyState(wrap, emptyState("nothing-selected"));
      wrap.setAttribute("data-key", row.id);
      const mounted = mountIntegrationDetail(
        wrap,
        toDetailIntegration(row),
        manifestFor(row.type),
        state.health[row.id],
        {
          onEdit: () => startEditIntegration(row.id),
          onDuplicate: () => void duplicateIntegration(row.id),
          onDelete: () => void deleteIntegration(row.id),
          onTest: () => testIntegration(row.id),
          inject: injectClientConfig,
        },
      );
      detailHandle = mounted;
      detachDetail = mounted.destroy;
      return;
    }
    case "group": {
      const { id } = selection;
      const row = hostGroups.find((g) => g.id === id);
      if (!row) return renderEmptyState(wrap, emptyState("nothing-selected"));
      wrap.setAttribute("data-key", row.id);
      renderGroupDetail(wrap, {
        group: {
          id: row.id,
          name: row.name,
          environment: row.environment,
          token: row.token,
          members: row.members.map((m) => ({ id: m.id, overrides: m.overrides ?? {} })),
        },
        integrations: hostIntegrations.map(toDetailIntegration),
        adapters: manifests,
        onEdit: () => startEditGroup(row.id),
        onDelete: () => void deleteGroup(row.id),
        onEditIntegration: (id) => select({ kind: "integration", id }),
        toastCenter: toasts,
      });
      return;
    }
    case "choose-integration-type": {
      const chooser = renderTypeChooser(
        manifests,
        (m) => chooseIntegrationType(m),
        {
          onCancel: () => {
            draft = null;
            select(previousSelection);
          },
          adaptersLoadFailed: state.adaptersLoadFailed,
          onRetry: () => void loadAdapters().then(refresh),
        },
      );
      wrap.appendChild(chooser);
      return;
    }
    case "new-integration":
    case "edit-integration": {
      if (!draft) return;
      const current = draft;
      const isNew = selection.kind === "new-integration";
      wrap.appendChild(
        renderIntegrationForm(
          current,
          manifestFor(current.type),
          (next) => {
            draft = next;
            renderDetail(mount);
          },
          (saved) => void saveIntegration(saved),
          () => {
            draft = null;
            select({ kind: "none" });
          },
          isNew
            ? () => {
                previousSelection = selection;
                select({ kind: "choose-integration-type" });
              }
            : undefined,
        ),
      );
      return;
    }
    case "new-group":
    case "edit-group": {
      if (!groupDraft) return;
      const current = groupDraft;
      wrap.appendChild(
        renderGroupForm(
          current,
          hostIntegrations.map((c) => ({
            id: c.id,
            name: c.name,
            type: c.type,
            environment: c.environment ?? undefined,
            config: Object.fromEntries(
              Object.entries(c.config).map(([k, v]) => [k, v == null ? "" : String(v)]),
            ),
          })),
          manifests,
          (next) => {
            groupDraft = next;
            renderDetail(mount);
          },
          (saved) => void saveGroup(saved),
          () => select({ kind: "none" }),
        ),
      );
      return;
    }
  }
}

// ── Actions ──────────────────────────────────────────────────────────────────

function startNewIntegration(): void {
  previousSelection = selection;
  draft = null;
  select({ kind: "choose-integration-type" });
}

function chooseIntegrationType(manifest: CatalogManifest): void {
  const base = draft ?? applyEnvironmentDefaults(emptyDraft());
  draft = adopt(base, manifest, true);
  select({ kind: "new-integration" });
}

function startEditIntegration(id: string): void {
  const row = hostIntegrations.find((c) => c.id === id);
  if (!row) return;
  const base = draftFromConnection({
    name: row.name,
    type: row.type,
    config: row.config,
    environment: (row.environment ?? "development") as Environment,
  });
  const manifest = manifestFor(row.type);
  draft = manifest ? { ...adopt(base, manifest, false), toolConfig: row.toolConfig } : base;
  select({ kind: "edit-integration", id });
}

function startNewGroup(): void {
  groupDraft = groupDraftFrom({ name: "", environment: null, members: [] });
  select({ kind: "new-group" });
}

function startEditGroup(id: string): void {
  const row = hostGroups.find((g) => g.id === id);
  if (!row) return;
  groupDraft = groupDraftFrom({
    name: row.name,
    environment: row.environment,
    members: row.members.map((m) => ({ id: m.id, overrides: m.overrides ?? {} })),
  });
  select({ kind: "edit-group", id });
}

async function saveIntegration(saved: ConnectionDraft): Promise<void> {
  const payload = {
    name: saved.name,
    type: saved.type,
    config: saved.config,
    environment: saved.environment,
    toolConfig: saved.toolConfig,
  };
  try {
    if (selection.kind === "edit-integration") {
      await invoke("update_integration", { id: selection.id, payload });
    } else {
      await invoke("create_integration", { payload });
    }
    await loadData();
    select({ kind: "none" });
  } catch (error) {
    report(error, selection.kind === "edit-integration" ? selection.id : "", "Integration not saved");
  }
}

async function saveGroup(saved: GroupDraft): Promise<void> {
  const payload = {
    name: saved.name,
    environment: saved.environment ?? null,
    members: serializeGroup(saved, hostIntegrations),
  };
  try {
    if (selection.kind === "edit-group") {
      await invoke("update_group", { id: selection.id, payload });
    } else {
      await invoke("create_group", { payload });
    }
    await loadData();
    select({ kind: "none" });
  } catch (error) {
    report(error, "", "Group not saved");
  }
}

async function duplicateIntegration(id: string): Promise<void> {
  const row = hostIntegrations.find((c) => c.id === id);
  if (!row) return;
  try {
    await invoke("create_integration", {
      payload: {
        name: `${row.name} copy`,
        type: row.type,
        config: row.config,
        environment: row.environment,
      },
    });
    await loadData();
  } catch (error) {
    report(error, id, "Integration not duplicated");
  }
}

async function deleteIntegration(id: string): Promise<void> {
  try {
    await invoke("delete_integration", { id });
    if (selection.kind !== "none" && "id" in selection && selection.id === id) {
      selection = { kind: "none" };
    }
    await loadData();
  } catch (error) {
    report(error, id, "Integration not deleted");
  }
}

async function deleteGroup(id: string): Promise<void> {
  try {
    await invoke("delete_group", { id });
    if (selection.kind !== "none" && "id" in selection && selection.id === id) {
      selection = { kind: "none" };
    }
    await loadData();
  } catch (error) {
    report(error, "", "Group not deleted");
  }
}

async function testIntegration(id: string): Promise<{ ok: boolean; error?: string }> {
  const result = await invoke<{ ok: boolean; error?: string }>("test_connection", { id });
  await loadHealth();
  const row = hostIntegrations.find((c) => c.id === id);
  const name = row?.name ?? "Integration";
  if (result.ok) {
    toasts.present({ integrationId: id, title: name, message: "Connected — your integration is working.", kind: "success" });
  } else {
    const msg = humanizeHealthError(result.error ?? null);
    toasts.present({ integrationId: id, title: name, message: msg, kind: "error" });
  }
  const h = state.health[id];
  detailHandle?.updateHealth(h ? { status: h.status, error: h.error ?? null, at: h.at } : null);
  refreshSidebar();
  return result;
}

function refreshSidebar(): void {
  if (!shellMounts) return;
  const sidebarWrap = shellMounts.root.querySelector(".shell-sidebar");
  if (sidebarWrap) {
    sidebarWrap.innerHTML = "";
    sidebarWrap.appendChild(buildSidebar());
  }
}

async function injectClientConfig(args: {
  client: string;
  scope: string;
  projectDir: string | null;
  key: string;
  url: string;
}): Promise<{ status: "added" | "skipped"; path: string }> {
  return invoke<{ status: "added" | "skipped"; path: string }>("inject_mcp_config", {
    client: args.client,
    scope: args.scope,
    project_dir: args.projectDir,
    key: args.key,
    url: args.url,
  });
}

// ── Data ─────────────────────────────────────────────────────────────────────

async function loadAdapters(): Promise<void> {
  try {
    manifests = await invoke<CatalogManifest[]>("list_adapters");
    state.adapters = manifests.map((m) => ({ id: m.id, label: m.label }));
    state.adaptersLoadFailed = manifests.length === 0;
  } catch {
    manifests = [];
    state.adapters = [];
    state.adaptersLoadFailed = true;
  }
}

async function loadHealth(): Promise<void> {
  try {
    state.health = await invoke<Record<string, Health>>("get_health");
  } catch {
    state.health = {};
  }
}

async function loadData(): Promise<void> {
  const [integrations, groups] = await Promise.all([
    invoke<HostIntegration[]>("list_integrations"),
    invoke<HostGroup[]>("list_groups"),
  ]);
  hostIntegrations = integrations;
  hostGroups = groups;
  state.integrations = integrations.map(
    (row): Integration => ({
      id: row.id,
      name: row.name,
      type: row.type,
      environment: (row.environment ?? "development") as Environment,
      readOnly: false,
    }),
  );
  state.groups = groups.map(
    (row): Group => ({
      id: row.id,
      name: row.name,
      environment: (row.environment ?? null) as Environment | null,
      memberIds: row.members.map((m) => m.id),
    }),
  );
  state.loading = false;
  refresh();
}

// ── Shell ────────────────────────────────────────────────────────────────────

let detailEl = document.createElement("div");
let shellMounts: ReturnType<typeof createShell> | null = null;

function select(next: Selection): void {
  selection = next;
  refresh();
}

function sidebarSelection(): string | null {
  return selection.kind !== "none" && "id" in selection ? selection.id : null;
}

function buildSidebar(): HTMLElement {
  return createSidebar(state, sidebarSelection(), {
    onSelect: (id) => {
      const isGroup = hostGroups.some((g) => g.id === id);
      select(isGroup ? { kind: "group", id } : { kind: "integration", id });
    },
    onCreateIntegration: startNewIntegration,
    onCreateGroup: startNewGroup,
    onDuplicate: (id) => void duplicateIntegration(id),
    onDelete: (kind, id) =>
      void (kind === "integration" ? deleteIntegration(id) : deleteGroup(id)),
    onRetryAdapters: () => void loadAdapters().then(refresh),
  });
}

function refresh(): void {
  if (!shellMounts) return;
  const sidebarWrap = shellMounts.root.querySelector(".shell-sidebar");
  if (sidebarWrap) {
    sidebarWrap.innerHTML = "";
    sidebarWrap.appendChild(buildSidebar());
  }
  renderDetail(detailEl);
}

async function bootstrap(): Promise<void> {
  detailEl = document.createElement("div");
  detailEl.style.height = "100%";
  shellMounts = createShell(buildSidebar(), detailEl);
  app.innerHTML = "";
  app.appendChild(shellMounts.root);
  renderToasts(shellMounts.toastMount, toasts, (id) => void testIntegration(id));
  renderBanners(shellMounts.bottomMount, { serverStatus: "running" }, () => {}, () => {});

  if (!hasHost()) {
    state.loading = false;
    state.adaptersLoadFailed = true;
    refresh();
    return;
  }

  await loadAdapters();
  await loadHealth();
  await loadData();

  setInterval(
    () =>
      void loadHealth().then(() => {
        refreshSidebar();
        if (selection.kind === "integration") {
          const h = state.health[selection.id];
          detailHandle?.updateHealth(h ? { status: h.status, error: h.error ?? null, at: h.at } : null);
        }
      }),
    15000,
  );
}

void bootstrap();
