import type { AdapterManifest, ConfigFieldDef, ToolDef } from "./catalog.ts";
import { visibleFields, groupedFields } from "./catalog.ts";
import type { ConnectionDraft, Environment } from "./connectionDraft.ts";
import { canSave, setEnvironment, splitTools } from "./connectionDraft.ts";
import type { GroupDraft } from "./groupForm.ts";
import { overridableFields, inheritPlaceholder, canSaveGroup } from "./groupForm.ts";
import { createIcon } from "../icon";
import { createButton, createBadge } from "../primitives";
import { typeBadge } from "../glyph";

export function renderTypeChooser(
  adapters: AdapterManifest[],
  onChoose: (m: AdapterManifest) => void,
  opts?: { onCancel?: () => void; adaptersLoadFailed?: boolean; onRetry?: () => void },
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "form-chooser";
  wrap.setAttribute("role", "region");
   wrap.setAttribute("aria-label", "Choose an integration");

  const heading = document.createElement("h2");
  heading.className = "ui-card-title";
  heading.id = "chooser-heading";
   heading.textContent = "Choose an integration";
  heading.setAttribute("tabindex", "-1");
  wrap.appendChild(heading);

  if (!adapters.length) {
    const card = document.createElement("div");
     card.className = "ui-card";
    card.setAttribute("role", "status");
    if (opts?.adaptersLoadFailed) {
      const title = document.createElement("h3");
       title.className = "ui-card-title";
       title.textContent = "Couldn’t load integrations";
      const body = document.createElement("p");
      body.className = "hint";
       body.textContent = "The integration catalog is unavailable. Check that the server is running and try again.";
      card.append(title, body);
      if (opts?.onRetry) {
        const retry = createButton("Try again", { size: "sm", ariaLabel: "Try again", onClick: opts.onRetry });
        card.appendChild(retry);
      }
    } else {
      const body = document.createElement("p");
      body.className = "hint";
       body.textContent = "Loading integrations…";
      card.appendChild(body);
    }
    wrap.appendChild(card);
  } else {
    const grid = document.createElement("div");
    grid.className = "chooser-grid";
    grid.setAttribute("role", "group");
    grid.setAttribute("aria-labelledby", "chooser-heading");
    for (const a of adapters) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "chooser-row";
      btn.setAttribute("aria-label", `${a.label}`);
      btn.innerHTML = `<span>${a.label}</span><span class="chooser-chevron" aria-hidden="true"></span>`;
      btn.prepend(typeBadge(a.id, a.label));
      btn.querySelector(".chooser-chevron")?.appendChild(createIcon("chevron-right"));
      btn.addEventListener("click", () => onChoose(a));
      grid.appendChild(btn);
    }
    wrap.appendChild(grid);
  }

  const footer = document.createElement("div");
  footer.className = "form-footer";
  const cancel = createButton("Cancel", { ariaLabel: "Cancel", onClick: () => opts?.onCancel?.() });
  footer.appendChild(cancel);
  wrap.appendChild(footer);

  wrap.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      opts?.onCancel?.();
    }
  });

  queueMicrotask(() => {
    const first = wrap.querySelector<HTMLButtonElement>(".chooser-row");
    if (first) first.focus();
    else heading.focus();
  });

  return wrap;
}

/** Flags a required control the person left empty, once they try to save. */
function markMissing(control: HTMLElement, wrap: HTMLElement, message: string): void {
  control.setAttribute("aria-invalid", "true");
  control.focus();
  if (wrap.querySelector(".field-error")) return;
  const error = document.createElement("div");
  error.className = "field-error";
  error.setAttribute("role", "alert");
  error.textContent = message;
  wrap.appendChild(error);
}

export function renderField(field: ConfigFieldDef, value: string, onChange: (v: string) => void): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row";
  row.dataset.fieldKey = field.key;
  const label = document.createElement("div");
  label.className = "inspector-label";
  label.textContent = field.label;
  if (field.required) label.textContent += " *";
  row.appendChild(label);

  const controlWrap = document.createElement("div");
   controlWrap.className = "field-control";

  const help = field.help ? (() => { const p = document.createElement("div"); p.className = "hint"; p.id = `help-${field.key}`; p.textContent = field.help!; return p; })() : null;

  switch (field.type) {
    case "toggle": {
      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = value === "true";
       input.addEventListener("change", () => onChange(input.checked ? "true" : "false"));
       const toggleLabel = document.createElement("label");
       toggleLabel.className = "field-toggle";
       toggleLabel.append(input, document.createTextNode(field.label));
       controlWrap.appendChild(toggleLabel);
      break;
    }
    case "select": {
      const sel = document.createElement("select");
      sel.className = "field-select";
      if (help) sel.setAttribute("aria-describedby", help.id);
      for (const opt of field.options ?? []) {
        const o = document.createElement("option");
        o.value = opt.value;
        o.textContent = opt.label;
        o.title = opt.label;
        if (opt.value === value) o.selected = true;
        sel.appendChild(o);
      }
      sel.addEventListener("change", () => {
        sel.title = sel.selectedOptions[0]?.text ?? "";
        onChange(sel.value);
      });
      sel.title = sel.selectedOptions[0]?.text ?? "";
      controlWrap.appendChild(sel);
      break;
    }
    case "file": {
      const row2 = document.createElement("div");
       row2.className = "field-file-row";
      const text = document.createElement("input");
      text.type = "text";
      text.placeholder = field.placeholder ?? "";
      text.value = value;
      text.className = "field-input mono";
      text.addEventListener("input", () => onChange(text.value));
      const file = document.createElement("input");
      file.type = "file";
      if (field.fileTypes?.length) file.accept = field.fileTypes.map((e) => "." + e).join(",");
      file.style.display = "none";
       const btn = createButton("Choose…", { size: "sm" });
       btn.addEventListener("click", async () => {
         const dialog = (window as unknown as { __TAURI__?: { dialog?: { open: (options: unknown) => Promise<string | null> } } }).__TAURI__?.dialog;
         if (dialog) {
           const picked = await dialog.open({ multiple: false, directory: false, title: `Choose ${field.label.toLowerCase()}` });
           if (picked) onChange(picked);
         } else file.click();
       });
      file.addEventListener("change", () => {
        if (file.files?.[0]) onChange(file.files[0].name);
      });
      row2.append(text, btn, file);
      controlWrap.appendChild(row2);
      break;
    }
    case "number": {
      const input = document.createElement("input");
      input.type = "number";
      input.placeholder = field.placeholder ?? "";
      input.value = value;
      input.className = "field-input mono";
      if (help) input.setAttribute("aria-describedby", help.id);
       input.classList.add("field-number");
       input.inputMode = "numeric";
       input.step = "1";
       if (help) input.setAttribute("aria-describedby", help.id);
      input.addEventListener("input", () => onChange(input.value));
      controlWrap.appendChild(input);
      break;
    }
    case "password": {
      const input = document.createElement("input");
      input.type = "password";
      input.placeholder = field.placeholder ?? "••••••";
      input.value = value;
      input.className = "field-input mono";
      if (help) input.setAttribute("aria-describedby", help.id);
      input.addEventListener("input", () => onChange(input.value));
      controlWrap.appendChild(input);
      break;
    }
    default: {
      const input = document.createElement("input");
      input.type = "text";
      input.placeholder = field.placeholder ?? "";
      input.value = value;
       input.className = "field-input mono";
       if (help) input.setAttribute("aria-describedby", help.id);
      input.addEventListener("input", () => onChange(input.value));
      controlWrap.appendChild(input);
      break;
    }
  }
  if (help) controlWrap.appendChild(help);
  row.appendChild(controlWrap);
  return row;
}

export function renderToolsSection(
  draft: ConnectionDraft,
  onToggle: (tool: string, enabled: boolean) => void,
  onSettingChange: (tool: string, key: string, value: string) => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "ui-card";
  const title = document.createElement("h3");
  title.className = "ui-card-title";
  title.textContent = "Tools";
  wrap.appendChild(title);

  const enabledCount = draft.tools.filter((t) => (draft.toolConfig[t.name]?.enabled ?? t.defaultEnabled)).length;
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = `${enabledCount} of ${draft.tools.length} on. Enable tools to give the agent more, disable to shrink what it sees.`;
  wrap.appendChild(hint);

  const { defaults, extras } = splitTools(draft.tools);

  const renderList = (tools: ToolDef[]) => {
    for (const tool of tools) {
      const enabled = draft.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled;
      const row = document.createElement("div");
      row.className = enabled ? "tool-row tool-on" : "tool-row tool-off";
      const toggle = document.createElement("input");
      toggle.type = "checkbox";
      toggle.checked = enabled;
      toggle.setAttribute("aria-label", tool.name);
      toggle.addEventListener("change", () => onToggle(tool.name, toggle.checked));
      const info = document.createElement("div");
       info.className = "tool-info";
      const nameRow = document.createElement("div");
      nameRow.className = "tool-name-row";
       nameRow.innerHTML = `<span class="mono">${tool.name}</span><span class="tool-category">${tool.category}</span>`;
       if (!enabled) {
         const state = document.createElement("span");
         state.className = "tool-state";
         state.textContent = "Off — enable to include";
         nameRow.appendChild(state);
       }
      const desc = document.createElement("div");
      desc.className = "tool-summary";
      desc.textContent = tool.description;
      info.append(nameRow, desc);

      // Settings expanded when enabled
      if (enabled && tool.settings?.length) {
        const settingsWrap = document.createElement("div");
       settingsWrap.className = "tool-settings";
        for (const s of tool.settings) {
          const isDangerOn = s.danger && (draft.toolConfig[tool.name]?.settings[s.key] ?? s.default ?? "") === "true";
          const sRow = renderSettingRow(tool, s, draft.toolConfig[tool.name]?.settings[s.key] ?? s.default ?? "", onSettingChange);
          if (isDangerOn) {
             sRow.classList.add("danger-setting");
            const warn = document.createElement("div");
            warn.className = "hint danger-copy";
            warn.append(createIcon("error"));
            const warning = document.createElement("strong");
            warning.textContent = `Warning: ${dangerousCopy(s)}`;
            warn.appendChild(warning);
            sRow.appendChild(warn);
          }
          settingsWrap.appendChild(sRow);
        }
        info.appendChild(settingsWrap);
      }

      row.append(toggle, info);
      wrap.appendChild(row);
    }
  };

  renderList(defaults);
  if (extras.length) {
     const moreTitle = document.createElement("h4");
      moreTitle.className = "ui-card-title more-tools-title";
    moreTitle.textContent = "More tools";
    const moreHint = document.createElement("p");
    moreHint.className = "hint";
    moreHint.textContent = "Off by default — enable the ones you need.";
    wrap.append(moreTitle, moreHint);
    renderList(extras);
  }
  return wrap;
}

function dangerousCopy(setting: ConfigFieldDef): string {
  // Concrete consequence without lecturing
  if (setting.key === "mode" || setting.label.toLowerCase().includes("destructive")) {
    return "When on, the agent can delete or overwrite data.";
  }
  return "When on, this allows state-changing actions.";
}

function renderSettingRow(
  tool: ToolDef,
  setting: ConfigFieldDef,
  value: string,
  onChange: (tool: string, key: string, value: string) => void,
): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const label = document.createElement("div");
  label.className = "inspector-label";
  label.textContent = setting.label;
  row.appendChild(label);
  const wrap = document.createElement("div");
   wrap.className = "field-control";
   if (setting.type === "toggle") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = value === "true";
    input.setAttribute("aria-label", setting.label);
    input.addEventListener("change", () => onChange(tool.name, setting.key, input.checked ? "true" : "false"));
    wrap.appendChild(input);
     if (setting.danger && input.checked) wrap.classList.add("danger-setting");
   } else if (setting.type === "select") {
    const sel = document.createElement("select");
    if (setting.help) sel.setAttribute("aria-describedby", `help-${tool.name}-${setting.key}`);
      for (const opt of setting.options ?? []) {
      const o = document.createElement("option");
      o.value = opt.value; o.textContent = opt.label; o.title = opt.label;
      if (opt.value === value) o.selected = true;
      sel.appendChild(o);
    }
    sel.addEventListener("change", () => {
      sel.title = sel.selectedOptions[0]?.text ?? "";
      onChange(tool.name, setting.key, sel.value);
    });
    sel.title = sel.selectedOptions[0]?.text ?? "";
    wrap.appendChild(sel);
  } else if (setting.type === "number") {
    const inp = document.createElement("input");
     inp.type = "number"; inp.value = value; inp.inputMode = "numeric"; inp.step = "1"; inp.className = "field-input mono field-setting-number";
     if (setting.help) inp.setAttribute("aria-describedby", `help-${tool.name}-${setting.key}`);
    inp.addEventListener("input", () => onChange(tool.name, setting.key, inp.value));
    wrap.appendChild(inp);
  } else {
    const inp = document.createElement("input");
     inp.type = setting.type === "password" ? "password" : "text";
     inp.placeholder = setting.placeholder ?? ""; inp.value = value; inp.className = "field-input mono";
     if (setting.help) inp.setAttribute("aria-describedby", `help-${tool.name}-${setting.key}`);
    inp.addEventListener("input", () => onChange(tool.name, setting.key, inp.value));
    wrap.appendChild(inp);
  }
  if (setting.help) { const h = document.createElement("div"); h.className = "hint"; h.textContent = setting.help; wrap.appendChild(h); }
  row.appendChild(wrap);
  return row;
}

export function renderIntegrationForm(
  draft: ConnectionDraft,
  manifest: AdapterManifest | undefined,
  onDraftChange: (next: ConnectionDraft) => void,
  onSave: (d: ConnectionDraft) => void,
  onCancel: () => void,
  onTypeChangeClick?: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "form-body";

  // Name + Type row
  const general = document.createElement("div");
  general.className = "ui-card";
  general.innerHTML = `<h3 class="ui-card-title">General</h3>`;
  const nameRow = document.createElement("div");
  nameRow.className = "inspector-row";
  nameRow.innerHTML = `<div class="inspector-label">Name *</div>`;
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = manifest ? `My ${manifest.label}` : "My Service";
  nameInput.value = draft.name; nameInput.className = "field-input"; nameInput.id = "integration-name";
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  const nameWrap = document.createElement("div"); nameWrap.className = "field-control"; nameWrap.appendChild(nameInput);
  nameRow.appendChild(nameWrap);
  general.appendChild(nameRow);

  if (manifest) {
    const typeRow = document.createElement("div");
    typeRow.className = "inspector-row";
    typeRow.innerHTML = `<div class="inspector-label">Type</div>`;
    const typeValue = document.createElement("div");
    typeValue.className = "field-control field-inline";
    typeValue.innerHTML = `<span class="mono">${manifest.label}</span>`;
    if (onTypeChangeClick) {
      typeValue.appendChild(createButton("Change", { size: "sm", onClick: onTypeChangeClick }));
    }
    typeRow.appendChild(typeValue);
    general.appendChild(typeRow);
  }

  const envRow = document.createElement("div");
  envRow.className = "inspector-row";
  envRow.innerHTML = `<div class="inspector-label">Environment</div>`;
  const envPicker = document.createElement("select");
  envPicker.className = "field-select";
  envPicker.setAttribute("aria-label", "Environment");
  for (const env of ["production", "staging", "development", "local"] as Environment[]) {
    const o = document.createElement("option");
    o.value = env; o.textContent = env[0].toUpperCase() + env.slice(1);
    if (env === draft.environment) o.selected = true;
    envPicker.appendChild(o);
  }
  envPicker.addEventListener("change", () => onDraftChange(setEnvironment(draft, envPicker.value as Environment)));
  const envWrap = document.createElement("div"); envWrap.className = "field-control"; envWrap.appendChild(envPicker);
  envRow.appendChild(envWrap);
  general.appendChild(envRow);
  wrap.appendChild(general);

  if (draft.policyKind === "sql" && (draft.environment === "development" || draft.environment === "local") && draft.toolConfig.query?.settings.mode === "mutations") {
    const environmentHint = document.createElement("p");
    environmentHint.className = "hint environment-hint";
    environmentHint.textContent = "Development and local setups allow write actions by default.";
    wrap.appendChild(environmentHint);
  }

  if (manifest) {
    for (const { group, fields } of groupedFields(manifest)) {
      const shown = visibleFields(fields, draft.config);
      if (!shown.length) continue;
      const card = document.createElement("div");
       card.className = "ui-card";
       const h = document.createElement("h3"); h.className = "ui-card-title"; h.textContent = group;
      card.appendChild(h);
      for (const f of shown) {
        const row = renderField(f, draft.config[f.key] ?? "", (v) => {
          onDraftChange({ ...draft, config: { ...draft.config, [f.key]: v } });
        });
        card.appendChild(row);
      }
      wrap.appendChild(card);
    }
    const toolsEl = renderToolsSection(
      draft,
      (tool, enabled) => {
        const next = { ...draft, toolConfig: { ...draft.toolConfig, [tool]: { ...(draft.toolConfig[tool] ?? { enabled: false, settings: {} }), enabled } } };
        onDraftChange(next);
      },
      (tool, key, value) => {
        const prev = draft.toolConfig[tool] ?? { enabled: true, settings: {} };
        onDraftChange({ ...draft, toolConfig: { ...draft.toolConfig, [tool]: { ...prev, settings: { ...prev.settings, [key]: value } } } });
      },
    );
    wrap.appendChild(toolsEl);
  }

  const footer = document.createElement("div");
  footer.className = "form-footer";
  const cancel = createButton("Cancel", { onClick: onCancel });
  const save = createButton("Save", { variant: "primary" });
  save.addEventListener("click", () => {
    if (!draft.name.trim()) {
      markMissing(nameInput, nameWrap, "Enter a name to continue.");
      return;
    }
    if (canSave(draft)) {
      onSave(draft);
      return;
    }
    const invalid = visibleFields(draft.fields, draft.config).find((field) => field.required && (draft.config[field.key] ?? "") === "");
    const invalidRow = invalid ? wrap.querySelector<HTMLElement>(`[data-field-key="${invalid.key}"]`) : null;
    const control = invalidRow?.querySelector<HTMLElement>("input, select");
    if (control) {
      control.setAttribute("aria-invalid", "true");
      control.focus();
      if (!invalidRow?.querySelector(".field-error")) {
        const error = document.createElement("div"); error.className = "field-error"; error.setAttribute("role", "alert"); error.textContent = `${invalid?.label ?? "This field"} is required.`; invalidRow?.querySelector(".field-control")?.appendChild(error);
      }
    }
  });
  footer.append(cancel, save);
  wrap.appendChild(footer);

  return wrap;
}

export function renderGroupForm(
  draft: GroupDraft,
  connections: Array<{ id: string; name: string; type: string; environment?: string; config: Record<string, string> }>,
  adapters: AdapterManifest[],
  onDraftChange: (next: GroupDraft) => void,
  onSave: (d: GroupDraft) => void,
  onCancel: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "form-body";

  const nameRow = document.createElement("div");
  nameRow.className = "ui-card";
  nameRow.innerHTML = `<h3 class="ui-card-title">Group</h3>`;
  const row = document.createElement("div"); row.className = "inspector-row";
  row.innerHTML = `<div class="inspector-label">Name *</div>`;
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = "Group name"; nameInput.value = draft.name; nameInput.className = "field-input"; nameInput.id = "group-name";
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  const w = document.createElement("div"); w.className = "field-control"; w.appendChild(nameInput); row.appendChild(w);
  nameRow.appendChild(row);

  // Environment picker with Any
  const envRow = document.createElement("div"); envRow.className = "inspector-row";
  envRow.innerHTML = `<div class="inspector-label">Environment</div>`;
  const sel = document.createElement("select");
  sel.className = "field-select";
  const anyOpt = document.createElement("option"); anyOpt.value = ""; anyOpt.textContent = "Any (mixed)"; if (draft.environment == null) anyOpt.selected = true; sel.appendChild(anyOpt);
  for (const env of ["production", "staging", "development", "local"]) {
    const o = document.createElement("option"); o.value = env; o.textContent = env[0].toUpperCase() + env.slice(1); if (draft.environment === env) o.selected = true; sel.appendChild(o);
  }
  sel.addEventListener("change", () => onDraftChange({ ...draft, environment: sel.value || null }));
  const ew = document.createElement("div"); ew.className = "field-control"; ew.appendChild(sel); envRow.appendChild(ew);
  nameRow.appendChild(envRow);
  wrap.appendChild(nameRow);

  // Checklist
  const listCard = document.createElement("div"); listCard.className = "ui-card";
  listCard.innerHTML = `<h3 class="ui-card-title">Integrations</h3>`;
  if (!connections.length) {
    const empty = document.createElement("div"); empty.className = "empty"; empty.textContent = "No integrations yet — add one first."; listCard.appendChild(empty);
  } else {
    for (const conn of connections) {
      const on = draft.included.has(conn.id);
      const row2 = document.createElement("div"); row2.className = "member-form-row";
      const header2 = document.createElement("label"); header2.className = "member-form-label";
      const cb = document.createElement("input"); cb.type = "checkbox"; cb.checked = on;
      cb.addEventListener("change", () => {
        const next = new Set(draft.included);
        if (cb.checked) next.add(conn.id); else next.delete(conn.id);
        onDraftChange({ ...draft, included: next });
      });
      const nameEl = document.createElement("span"); nameEl.textContent = conn.name;
       const envTag = createBadge(conn.environment ?? "development", "environment");
      header2.append(cb, nameEl, envTag);
      row2.appendChild(header2);

      if (on) {
        const manifest = adapters.find((a) => a.id === conn.type);
        const fields = overridableFields(manifest);
        if (fields.length) {
          const panel = document.createElement("div"); panel.className = "member-form-panel";
          const hint = document.createElement("div"); hint.className = "hint"; hint.textContent = "Overrides for this group (blank = inherit)";
          panel.appendChild(hint);
          for (const f of fields) {
            const orRow = document.createElement("div"); orRow.className = "inspector-row";
            orRow.innerHTML = `<div class="inspector-label">${f.label}</div>`;
            const inp = document.createElement("input");
            inp.type = "text"; inp.className = "field-input mono";
             inp.placeholder = "Inherited";
             const inherited = inheritPlaceholder(conn.config, f);
             if (inherited !== "Inherited") inp.title = inherited;
            inp.value = draft.overrides[conn.id]?.[f.key] ?? "";
            inp.addEventListener("input", () => {
              const ov = { ...(draft.overrides[conn.id] ?? {}) };
              const trimmed = inp.value.trim();
              if (trimmed === "") delete ov[f.key]; else ov[f.key] = inp.value;
              onDraftChange({ ...draft, overrides: { ...draft.overrides, [conn.id]: ov } });
            });
             const ww = document.createElement("div"); ww.className = "field-control"; ww.appendChild(inp); orRow.appendChild(ww);
            panel.appendChild(orRow);
          }
          row2.appendChild(panel);
        }
      }
      listCard.appendChild(row2);
    }
  }
  wrap.appendChild(listCard);

  const footer = document.createElement("div"); footer.className = "form-footer";
  const cancel = createButton("Cancel", { onClick: onCancel });
  const save = createButton("Save", { variant: "primary" });
  save.addEventListener("click", () => {
    if (!draft.name.trim()) { markMissing(nameInput, w, "Enter a name to continue."); return; }
    if (canSaveGroup(draft)) onSave(draft);
  });
  footer.append(cancel, save); wrap.appendChild(footer);
  return wrap;
}
