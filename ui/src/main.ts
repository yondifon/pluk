import "./style.css";
import { suppressWebViewContextMenu } from "./contextMenu.ts";
import { suppressWebViewKeyBeep } from "./keyBeep.ts";
import { zoom } from "./zoom.ts";
import { createShell } from "./shell.ts";
import { createSidebar, type SidebarState } from "./sidebar.ts";
import { emptyState, renderEmptyState } from "./emptyStates.ts";
import { mountIntegrationDetail } from "./integration-detail/index.ts";
import type { TabId } from "./integration-detail/tabs.ts";
import type { Integration as DetailIntegration, ConnHealth as DetailHealth } from "./integration-detail/types.ts";
import { renderGroupDetail } from "./groupDetail.ts";
import {
  renderGroupForm,
  renderTypeChooser,
  renderNameStep,
  renderConnectFieldsStep,
  renderToolsStep,
  renderCommandsStep,
  type RuleProblem,
} from "./forms/render.ts";
import {
  adopt,
  applyEnvironmentDefaults,
  draftFromConnection,
  emptyDraft,
  type Approvals,
  type ConnectionDraft,
} from "./forms/connectionDraft.ts";
import { wizardSteps } from "./forms/wizard.ts";
import { groupDraftFrom, serializeGroup, type GroupDraft } from "./forms/groupForm.ts";
import type { AdapterManifest as CatalogManifest, ToolState } from "./forms/catalog.ts";
import { renderConnectChromeStep } from "./integration-detail/browser-access.ts";
import { renderInstallStep } from "./integration-detail/agent-setup.ts";
import { toast, mountToaster } from "./toast.ts";
import { mountUpdates } from "./update.ts";
import { renderLoadingState } from "./primitives.ts";
import { openModal } from "./modal.ts";
import { injectMcpConfig, invoke, hasHost } from "./host.ts";
import { isMac } from "./platform.ts";
import type { Integration, Group, Environment, Health } from "./types.ts";
import { WANDE_TYPE } from "./integration-detail/types.ts";

const app = document.getElementById("app")!;

if (isMac()) {
  document.documentElement.classList.add("platform-macos");
  suppressWebViewKeyBeep();
}
suppressWebViewContextMenu(import.meta.env.DEV);

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
  approvals: Approvals;
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
  | { kind: "group"; id: string };

/** Which form the modal is showing, if any. `step` indexes into `wizardSteps(...)`. */
type FormState =
  | { kind: "new-integration"; step: number; savedId: string | null }
  | { kind: "edit-integration"; id: string; step: number }
  | { kind: "new-group" }
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
let form: FormState | null = null;
let formModal: { close: () => void; setTitle: (text: string) => void; content: HTMLElement } | null = null;
let formHost: HTMLElement | null = null;
let draft: ConnectionDraft | null = null;
let provisionalIntegrationId: string | null = null;
let creatingIntegration = false;
/** The rule the host refused on the last save attempt, shown beside its list. */
let ruleProblem: RuleProblem | null = null;
let groupDraft: GroupDraft | null = null;
/** Teardown for whatever the current wizard step is watching (Chrome pairing polling). */
let activeStepCleanup: (() => void) | null = null;
/** Which tab the next detail render opens on, when it should not be the usual one. */
let openDetailAt: TabId | null = null;
let detailHandle: { destroy: () => void; updateHealth: (next: DetailHealth | null) => void } | null = null;
let detachDetail: (() => void) | null = null;

type SidebarElement = HTMLElement & {
  _destroy?: () => void;
  _setHealth?: (health: Record<string, Health>) => void;
  _setSelectedId?: (id: string | null) => void;
};

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
    approvals: row.approvals,
    token: row.token,
    createdAt: row.createdAt,
  };
}

function report(error: unknown, title: string): void {
  const reason = error instanceof Error ? error.message : String(error);
  toast.error(title, { description: `${reason.replace(/\.?$/, ".")} Try again.` });
}

// ── Detail rendering ─────────────────────────────────────────────────────────

function renderDetail(mount: HTMLElement): void {
  detailHandle?.destroy();
  detailHandle = null;
  detachDetail?.();
  detachDetail = null;
  mount.innerHTML = "";

  const wrap = document.createElement("div");
  wrap.className = "detail-mount";
  mount.appendChild(wrap);

  switch (selection.kind) {
    case "none": {
      if (state.loading) {
        renderLoadingState(wrap);
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
          inject: injectMcpConfig,
        },
        openDetailAt ?? undefined,
      );
      openDetailAt = null;
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
        inject: injectMcpConfig,
      });
      return;
    }
  }
}

// ── Form modal ───────────────────────────────────────────────────────────────

const FORM_TITLES: Record<FormState["kind"], string> = {
  "new-integration": "New Integration",
  "edit-integration": "Edit Integration",
  "new-group": "New Group",
  "edit-group": "Edit Group",
};

const FORM_FOCUSABLE = "input, select, textarea, button";

function openForm(next: FormState): void {
  form = next;
  ruleProblem = null;
  if (!formModal) {
    formHost = document.createElement("div");
    formModal = openModal({
      title: FORM_TITLES[next.kind],
      size: "large",
      content: formHost,
      onClose: () => {
        const provisionalId = provisionalIntegrationId;
        provisionalIntegrationId = null;
        if (provisionalId) {
          void invoke("delete_integration", { id: provisionalId }).then(() => loadData()).catch(() => {});
        }
        activeStepCleanup?.();
        activeStepCleanup = null;
        form = null;
        formModal = null;
        formHost = null;
        draft = null;
        groupDraft = null;
      },
    });
    formModal.content.classList.add("modal-body-form");
  }
  formModal.setTitle(FORM_TITLES[next.kind]);
  renderForm();
}

function closeForm(): void {
  formModal?.close();
  activeStepCleanup?.();
  activeStepCleanup = null;
  form = null;
  formModal = null;
  formHost = null;
  draft = null;
  groupDraft = null;
}

/** Re-renders the open form, keeping the caret where the person left it. */
function renderForm(): void {
  const host = formHost;
  if (!host || !form) return;
  const active = document.activeElement as HTMLElement | null;
  const index = active ? Array.from(host.querySelectorAll<HTMLElement>(FORM_FOCUSABLE)).indexOf(active) : -1;
  const caret = active instanceof HTMLInputElement ? active.selectionStart : null;

  activeStepCleanup?.();
  activeStepCleanup = null;
  host.innerHTML = "";
  const built = buildForm(form);
  host.appendChild(built.el);
  activeStepCleanup = built.destroy ?? null;

  if (index < 0) return;
  const restored = host.querySelectorAll<HTMLElement>(FORM_FOCUSABLE)[index];
  restored?.focus();
  if (restored instanceof HTMLInputElement && caret != null) restored.setSelectionRange(caret, caret);
}

/** Moves the wizard step index for the integration currently open, if one is. */
function goToStep(delta: number): void {
  if (!form || (form.kind !== "new-integration" && form.kind !== "edit-integration")) return;
  form = { ...form, step: form.step + delta };
  ruleProblem = null;
  renderForm();
}

async function continueFromName(): Promise<void> {
  if (!form || !draft) return;
  if (form.kind !== "new-integration" || draft.type !== WANDE_TYPE || form.savedId || creatingIntegration) {
    goToStep(1);
    return;
  }

  creatingIntegration = true;
  try {
    const created = await invoke<HostIntegration>("create_integration", {
      payload: {
        name: draft.name,
        type: draft.type,
        config: draft.config,
        environment: draft.environment,
      },
    });
    if (!form) {
      await invoke("delete_integration", { id: created.id });
      return;
    }
    provisionalIntegrationId = created.id;
    form = { ...form, savedId: created.id, step: form.step + 1 };
    renderForm();
    await loadData();
    renderForm();
  } catch (error) {
    report(error, "Integration not saved");
  } finally {
    creatingIntegration = false;
  }
}

function buildForm(current: FormState): { el: HTMLElement; destroy?: () => void } {
  switch (current.kind) {
    case "new-integration":
    case "edit-integration": {
      const mode = current.kind === "edit-integration" ? "edit" : "create";
      const manifest = draft ? manifestFor(draft.type) : undefined;
      const steps = wizardSteps(manifest, mode);
      const stepKind = steps[current.step];

      if (stepKind === "type") {
        return {
          el: renderTypeChooser(manifests, chooseIntegrationType, {
            onCancel: closeForm,
            adaptersLoadFailed: state.adaptersLoadFailed,
            onRetry: () => void loadAdapters().then(renderForm),
          }),
        };
      }
      if (!draft || !manifest) return { el: document.createElement("div") };

      const pending = draft;
      const stepIndex = current.step + 1;
      const totalSteps = steps.length;
      const onBack = current.step > 0 ? () => goToStep(-1) : null;
      const onDraftChange = (next: ConnectionDraft) => {
        draft = next;
        renderForm();
      };

      switch (stepKind) {
        case "name":
          return { el: renderNameStep(pending, manifest, stepIndex, totalSteps, onDraftChange, onBack, closeForm, () => void continueFromName()) };
        case "connect": {
          if (manifest.configFields.length > 0) {
            return { el: renderConnectFieldsStep(pending, manifest, stepIndex, totalSteps, onDraftChange, onBack, closeForm, () => goToStep(1)) };
          }
          const integrationId = current.kind === "edit-integration" ? current.id : current.savedId;
          if (!integrationId) return { el: document.createElement("div") };
          return renderConnectChromeStep(integrationId, stepIndex, totalSteps, {
            onBack,
            onCancel: closeForm,
            onContinue: () => goToStep(1),
            onSkip: () => goToStep(1),
          });
        }
        case "tools": {
          const isLastContentStep = current.step === steps.length - 2;
          return {
            el: renderToolsStep(
              pending,
              stepIndex,
              totalSteps,
              isLastContentStep,
              onDraftChange,
              onBack,
              closeForm,
              isLastContentStep ? (d) => void saveIntegration(d) : () => goToStep(1),
            ),
          };
        }
        case "commands":
          return {
            el: renderCommandsStep(
              pending,
              stepIndex,
              totalSteps,
              onDraftChange,
              onBack,
              closeForm,
              (d) => void saveIntegration(d),
              ruleProblem,
            ),
          };
        case "install": {
          const targetId = current.kind === "edit-integration" ? current.id : current.savedId;
          const row = targetId ? hostIntegrations.find((r) => r.id === targetId) : null;
          if (!row) return { el: document.createElement("div") };
          return {
            el: renderInstallStep(row, manifest, injectMcpConfig, stepIndex, totalSteps, { onBack, onDone: closeForm }),
          };
        }
      }
    }
    case "new-group":
    case "edit-group": {
      if (!groupDraft) return { el: document.createElement("div") };
      return {
        el: renderGroupForm(
          groupDraft,
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
            renderForm();
          },
          (saved) => void saveGroup(saved),
          closeForm,
        ),
      };
    }
  }
}

// ── Actions ──────────────────────────────────────────────────────────────────

function startNewIntegration(): void {
  draft = null;
  openForm({ kind: "new-integration", step: 0, savedId: null });
}

function chooseIntegrationType(manifest: CatalogManifest): void {
  const base = draft ?? applyEnvironmentDefaults(emptyDraft());
  draft = adopt(base, manifest, true);
  if (form?.kind === "new-integration") form = { ...form, step: 1 };
  renderForm();
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
  const stored = { toolConfig: row.toolConfig, approvals: row.approvals };
  draft = manifest ? { ...adopt(base, manifest, false), ...stored } : { ...base, ...stored };
  openForm({ kind: "edit-integration", id, step: 0 });
}

function startNewGroup(): void {
  groupDraft = groupDraftFrom({ name: "", environment: null, members: [] });
  openForm({ kind: "new-group" });
}

function startEditGroup(id: string): void {
  const row = hostGroups.find((g) => g.id === id);
  if (!row) return;
  groupDraft = groupDraftFrom({
    name: row.name,
    environment: row.environment,
    members: row.members.map((m) => ({ id: m.id, overrides: m.overrides ?? {} })),
  });
  openForm({ kind: "edit-group", id });
}

async function saveIntegration(saved: ConnectionDraft): Promise<void> {
  if (!form || (form.kind !== "new-integration" && form.kind !== "edit-integration")) return;
  const payload = {
    name: saved.name,
    type: saved.type,
    config: saved.config,
    environment: saved.environment,
    toolConfig: saved.toolConfig,
    approvals: saved.approvals,
  };
  const existingId = form.kind === "edit-integration" ? form.id : form.savedId;
  try {
    ruleProblem = await invoke<RuleProblem | null>("check_approval_rules", {
      approvals: saved.approvals,
    });
    if (ruleProblem) {
      renderForm();
      return;
    }
    let targetId: string;
    if (existingId) {
      await invoke("update_integration", { id: existingId, payload });
      targetId = existingId;
    } else {
      const created = await invoke<HostIntegration>("create_integration", { payload });
      targetId = created.id;
    }
    provisionalIntegrationId = null;
    // Land on the integration's setup details behind the modal, which are what it needs next.
    selection = { kind: "integration", id: targetId };
    openDetailAt = "overview";
    await loadData();
    // The install step needs the record just saved, so the flow stays open until Done.
    const steps = wizardSteps(manifestFor(saved.type), form.kind === "edit-integration" ? "edit" : "create");
    const installStep = steps.length - 1;
    form = form.kind === "edit-integration"
      ? { kind: "edit-integration", id: existingId ?? targetId, step: installStep }
      : { kind: "new-integration", step: installStep, savedId: targetId };
    renderForm();
  } catch (error) {
    report(error, "Integration not saved");
  }
}

async function saveGroup(saved: GroupDraft): Promise<void> {
  const payload = {
    name: saved.name,
    environment: saved.environment ?? null,
    members: serializeGroup(saved, hostIntegrations),
  };
  try {
    if (form?.kind === "edit-group") {
      await invoke("update_group", { id: form.id, payload });
    } else {
      await invoke("create_group", { payload });
    }
    closeForm();
    await loadData();
  } catch (error) {
    report(error, "Group not saved");
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
    report(error, "Integration not duplicated");
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
    report(error, "Integration not deleted");
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
    report(error, "Group not deleted");
  }
}

async function testIntegration(id: string): Promise<{ ok: boolean; error?: string }> {
  const result = await invoke<{ ok: boolean; error?: string }>("test_connection", { id });
  await loadHealth();
  const h = state.health[id];
  detailHandle?.updateHealth(h ? { status: h.status, error: h.error ?? null, at: h.at } : null);
  refreshSidebar();
  return result;
}

function refreshSidebar(): void {
  if (!shellMounts) return;
  const sidebarWrap = shellMounts.root.querySelector(".shell-sidebar");
  if (sidebarWrap) {
    (sidebarWrap.firstElementChild as SidebarElement | null)?._setHealth?.(state.health);
  }
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
  const sidebar = shellMounts?.root.querySelector(".shell-sidebar")?.firstElementChild as SidebarElement | null;
  sidebar?._setSelectedId?.(sidebarSelection());
  renderDetail(detailEl);
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
    (sidebarWrap.firstElementChild as SidebarElement | null)?._destroy?.();
    sidebarWrap.innerHTML = "";
    sidebarWrap.appendChild(buildSidebar());
  }
  renderDetail(detailEl);
}

async function bootstrap(): Promise<void> {
  detailEl = document.createElement("div");
  detailEl.className = "detail-mount";
  shellMounts = createShell(buildSidebar(), detailEl);
  app.innerHTML = "";
  app.appendChild(shellMounts.root);
  mountToaster(shellMounts.toasterMount);
  void mountUpdates();

  if (!hasHost()) {
    state.loading = false;
    state.adaptersLoadFailed = true;
    refresh();
    return;
  }


  await Promise.all([loadAdapters(), loadHealth(), loadData()]);
  refresh();

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
