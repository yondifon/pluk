import type { AdapterManifest, ConfigFieldDef, ToolDef } from "./catalog.ts";
import { groupedByCategory, prettyCategory, visibleFields, groupedFields } from "./catalog.ts";
import type { ConnectionDraft, Environment } from "./connectionDraft.ts";
import { canSave, splitTools } from "./connectionDraft.ts";
import type { GroupDraft } from "./groupForm.ts";
import { overridableFields, inheritPlaceholder, canSaveGroup } from "./groupForm.ts";

export function renderTypeChooser(
  adapters: AdapterManifest[],
  onChoose: (m: AdapterManifest) => void,
  opts?: { onCancel?: () => void; adaptersLoadFailed?: boolean; onRetry?: () => void },
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "form-chooser";
  wrap.setAttribute("role", "region");
  wrap.setAttribute("aria-label", "Choose a service");

  const heading = document.createElement("h2");
  heading.className = "card-title";
  heading.id = "chooser-heading";
  heading.textContent = "Choose a service";
  heading.setAttribute("tabindex", "-1");
  wrap.appendChild(heading);

  const hint = document.createElement("p");
  hint.className = "hint";
  hint.id = "chooser-hint";
  hint.textContent = "Select a service to set up a new integration.";
  wrap.appendChild(hint);

  if (!adapters.length) {
    const card = document.createElement("div");
    card.className = "card";
    card.setAttribute("role", "status");
    if (opts?.adaptersLoadFailed) {
      const title = document.createElement("h3");
      title.className = "card-title";
      title.textContent = "Couldn’t load services";
      const body = document.createElement("p");
      body.className = "hint";
      body.textContent = "The service catalog is unavailable. Check that the server is running and try again.";
      card.append(title, body);
      if (opts?.onRetry) {
        const retry = document.createElement("button");
        retry.className = "btn btn-sm";
        retry.textContent = "Try again";
        retry.setAttribute("aria-label", "Try again");
        retry.addEventListener("click", opts.onRetry);
        card.appendChild(retry);
      }
    } else {
      const body = document.createElement("p");
      body.className = "hint";
      body.textContent = "Loading services…";
      card.appendChild(body);
    }
    wrap.appendChild(card);
  } else {
    for (const { category, items } of groupedByCategory(adapters)) {
      const section = document.createElement("section");
      section.className = "card";
      section.setAttribute("role", "group");
      const h = document.createElement("h3");
      h.className = "card-title";
      const cid = `chooser-cat-${category}`;
      h.id = cid;
      h.textContent = prettyCategory(category);
      section.setAttribute("aria-labelledby", cid);
      section.appendChild(h);
      for (const a of items) {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "chooser-row";
        btn.setAttribute("aria-label", `${a.label}`);
        btn.innerHTML = `<span class="type-badge" aria-hidden="true">${a.id[0].toUpperCase()}</span><span>${a.label}</span><span class="chooser-chevron" aria-hidden="true">›</span>`;
        btn.addEventListener("click", () => onChoose(a));
        section.appendChild(btn);
      }
      wrap.appendChild(section);
    }
  }

  const footer = document.createElement("div");
  footer.style.display = "flex";
  footer.style.justifyContent = "flex-end";
  footer.style.marginTop = "8px";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn";
  cancel.textContent = "Cancel";
  cancel.setAttribute("aria-label", "Cancel");
  cancel.addEventListener("click", () => opts?.onCancel?.());
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

export function renderField(field: ConfigFieldDef, value: string, onChange: (v: string) => void): HTMLElement {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const label = document.createElement("div");
  label.className = "inspector-label";
  label.textContent = field.label;
  if (field.required) label.textContent += " *";
  row.appendChild(label);

  const controlWrap = document.createElement("div");
  controlWrap.style.flex = "1";

  const help = field.help ? (() => { const p = document.createElement("div"); p.className = "hint"; p.textContent = field.help!; return p; })() : null;

  switch (field.type) {
    case "toggle": {
      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = value === "true";
      input.setAttribute("aria-label", field.label);
      input.addEventListener("change", () => onChange(input.checked ? "true" : "false"));
      controlWrap.appendChild(input);
      break;
    }
    case "select": {
      const sel = document.createElement("select");
      sel.className = "field-select";
      for (const opt of field.options ?? []) {
        const o = document.createElement("option");
        o.value = opt.value;
        o.textContent = opt.label;
        if (opt.value === value) o.selected = true;
        sel.appendChild(o);
      }
      sel.addEventListener("change", () => onChange(sel.value));
      controlWrap.appendChild(sel);
      break;
    }
    case "file": {
      const row2 = document.createElement("div");
      row2.style.display = "flex";
      row2.style.gap = "8px";
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
      const btn = document.createElement("button");
      btn.className = "btn btn-sm";
      btn.textContent = "Choose…";
      btn.addEventListener("click", () => file.click());
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
      input.style.width = "120px";
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
  wrap.className = "card";
  const title = document.createElement("h3");
  title.className = "card-title";
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
      info.style.flex = "1";
      const nameRow = document.createElement("div");
      nameRow.className = "tool-name-row";
      nameRow.innerHTML = `<span class="mono">${tool.name}</span><span class="tool-category">${tool.category}</span>`;
      const desc = document.createElement("div");
      desc.className = "tool-summary";
      desc.textContent = tool.description;
      info.append(nameRow, desc);

      // Settings expanded when enabled
      if (enabled && tool.settings?.length) {
        const settingsWrap = document.createElement("div");
        settingsWrap.style.marginTop = "8px";
        settingsWrap.style.paddingLeft = "24px";
        for (const s of tool.settings) {
          const isDangerOn = s.danger && (draft.toolConfig[tool.name]?.settings[s.key] ?? s.default ?? "") === "true";
          const sRow = renderSettingRow(tool, s, draft.toolConfig[tool.name]?.settings[s.key] ?? s.default ?? "", onSettingChange);
          if (isDangerOn) {
            sRow.style.color = "#dc2626";
            const warn = document.createElement("div");
            warn.className = "hint";
            warn.style.color = "#dc2626";
            warn.textContent = dangerousCopy(s);
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
    const moreTitle = document.createElement("div");
    moreTitle.className = "card-title";
    moreTitle.style.marginTop = "16px";
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
  wrap.style.flex = "1";
  if (setting.type === "toggle") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = value === "true";
    input.setAttribute("aria-label", setting.label);
    input.addEventListener("change", () => onChange(tool.name, setting.key, input.checked ? "true" : "false"));
    wrap.appendChild(input);
    if (setting.danger && input.checked) wrap.style.color = "#dc2626";
  } else if (setting.type === "select") {
    const sel = document.createElement("select");
    for (const opt of setting.options ?? []) {
      const o = document.createElement("option");
      o.value = opt.value; o.textContent = opt.label;
      if (opt.value === value) o.selected = true;
      sel.appendChild(o);
    }
    sel.addEventListener("change", () => onChange(tool.name, setting.key, sel.value));
    wrap.appendChild(sel);
  } else if (setting.type === "number") {
    const inp = document.createElement("input");
    inp.type = "number"; inp.value = value; inp.className = "field-input mono"; inp.style.width = "90px";
    inp.addEventListener("input", () => onChange(tool.name, setting.key, inp.value));
    wrap.appendChild(inp);
  } else {
    const inp = document.createElement("input");
    inp.type = setting.type === "password" ? "password" : "text";
    inp.placeholder = setting.placeholder ?? ""; inp.value = value; inp.className = "field-input mono";
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

  // Header with environment picker
  const header = document.createElement("div");
  header.className = "detail-header";
  header.innerHTML = `<div class="detail-title">${draft.name || "Integration settings"}</div>`;
  const envPicker = document.createElement("select");
  envPicker.className = "field-select";
  envPicker.setAttribute("aria-label", "Environment");
  for (const env of ["production", "staging", "development", "local"] as Environment[]) {
    const o = document.createElement("option");
    o.value = env; o.textContent = env[0].toUpperCase() + env.slice(1);
    if (env === draft.environment) o.selected = true;
    envPicker.appendChild(o);
  }
  envPicker.addEventListener("change", () => onDraftChange({ ...draft, environment: envPicker.value as Environment } as ConnectionDraft));
  // Proxy through setEnvironment logic would happen at caller via connectionDraft.setEnvironment
  const headerRow = document.createElement("div");
  headerRow.style.display = "flex"; headerRow.style.justifyContent = "space-between"; headerRow.append(header, envPicker);
  wrap.appendChild(headerRow);

  // Name + Type row
  const general = document.createElement("div");
  general.className = "card";
  general.innerHTML = `<h3 class="card-title">General</h3>`;
  const nameRow = document.createElement("div");
  nameRow.className = "inspector-row";
  nameRow.innerHTML = `<div class="inspector-label">Name *</div>`;
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = manifest ? `My ${manifest.label}` : "My Service";
  nameInput.value = draft.name; nameInput.className = "field-input";
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  const nameWrap = document.createElement("div"); nameWrap.style.flex = "1"; nameWrap.appendChild(nameInput);
  nameRow.appendChild(nameWrap);
  general.appendChild(nameRow);

  if (manifest) {
    const typeRow = document.createElement("div");
    typeRow.className = "inspector-row";
    typeRow.innerHTML = `<div class="inspector-label">Type</div><span class="mono">${manifest.label}</span>`;
    if (onTypeChangeClick) {
      const btn = document.createElement("button"); btn.className = "btn btn-sm"; btn.textContent = "Change"; btn.addEventListener("click", onTypeChangeClick);
      typeRow.appendChild(btn);
    }
    general.appendChild(typeRow);
  }
  wrap.appendChild(general);

  if (manifest) {
    for (const { group, fields } of groupedFields(manifest)) {
      const shown = visibleFields(fields, draft.config);
      if (!shown.length) continue;
      const card = document.createElement("div");
      card.className = "card";
      const h = document.createElement("h3"); h.className = "card-title"; h.textContent = group;
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
  footer.style.display = "flex"; footer.style.gap = "8px"; footer.style.justifyContent = "flex-end"; footer.style.marginTop = "16px";
  const cancel = document.createElement("button"); cancel.className = "btn"; cancel.textContent = "Cancel"; cancel.addEventListener("click", onCancel);
  const save = document.createElement("button"); save.className = "btn btn-primary"; save.textContent = "Save"; save.disabled = !canSave(draft);
  if (!save.disabled) save.addEventListener("click", () => onSave(draft));
  footer.append(cancel, save);
  wrap.appendChild(footer);

  // Validation messages
  if (draft.name.trim() === "") {
    const msg = document.createElement("div"); msg.className = "hint"; msg.textContent = "Enter a name to continue.";
    wrap.appendChild(msg);
  }
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
  nameRow.className = "card";
  nameRow.innerHTML = `<h3 class="card-title">Group</h3>`;
  const row = document.createElement("div"); row.className = "inspector-row";
  row.innerHTML = `<div class="inspector-label">Name *</div>`;
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = "Group name"; nameInput.value = draft.name; nameInput.className = "field-input";
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  const w = document.createElement("div"); w.style.flex = "1"; w.appendChild(nameInput); row.appendChild(w);
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
  const ew = document.createElement("div"); ew.style.flex = "1"; ew.appendChild(sel); envRow.appendChild(ew);
  nameRow.appendChild(envRow);
  wrap.appendChild(nameRow);

  // Checklist
  const listCard = document.createElement("div"); listCard.className = "card";
  listCard.innerHTML = `<h3 class="card-title">Integrations</h3>`;
  if (!connections.length) {
    const empty = document.createElement("div"); empty.className = "empty"; empty.textContent = "No integrations yet — add one first."; listCard.appendChild(empty);
  } else {
    for (const conn of connections) {
      const on = draft.included.has(conn.id);
      const row2 = document.createElement("div"); row2.style.padding = "8px 0"; row2.style.borderBottom = "1px solid #f3f4f6";
      const header2 = document.createElement("label"); header2.style.display = "flex"; header2.style.gap = "8px"; header2.style.alignItems = "center";
      const cb = document.createElement("input"); cb.type = "checkbox"; cb.checked = on;
      cb.addEventListener("change", () => {
        const next = new Set(draft.included);
        if (cb.checked) next.add(conn.id); else next.delete(conn.id);
        onDraftChange({ ...draft, included: next });
      });
      const nameEl = document.createElement("span"); nameEl.textContent = conn.name;
      const envTag = document.createElement("span"); envTag.className = "tag"; envTag.textContent = conn.environment ?? "development";
      header2.append(cb, nameEl, envTag);
      row2.appendChild(header2);

      if (on) {
        const manifest = adapters.find((a) => a.id === conn.type);
        const fields = overridableFields(manifest);
        if (fields.length) {
          const hint = document.createElement("div"); hint.className = "hint"; hint.textContent = "Overrides for this group (blank = inherit)";
          row2.appendChild(hint);
          for (const f of fields) {
            const orRow = document.createElement("div"); orRow.className = "inspector-row";
            orRow.innerHTML = `<div class="inspector-label">${f.label}</div>`;
            const inp = document.createElement("input");
            inp.type = "text"; inp.className = "field-input mono";
            inp.placeholder = inheritPlaceholder(conn.config, f);
            inp.value = draft.overrides[conn.id]?.[f.key] ?? "";
            inp.addEventListener("input", () => {
              const ov = { ...(draft.overrides[conn.id] ?? {}) };
              const trimmed = inp.value.trim();
              if (trimmed === "") delete ov[f.key]; else ov[f.key] = inp.value;
              onDraftChange({ ...draft, overrides: { ...draft.overrides, [conn.id]: ov } });
            });
            const ww = document.createElement("div"); ww.style.flex = "1"; ww.appendChild(inp); orRow.appendChild(ww);
            row2.appendChild(orRow);
          }
        }
      }
      listCard.appendChild(row2);
    }
  }
  wrap.appendChild(listCard);

  const footer = document.createElement("div"); footer.style.display = "flex"; footer.style.gap = "8px"; footer.style.justifyContent = "flex-end";
  const cancel = document.createElement("button"); cancel.className = "btn"; cancel.textContent = "Cancel"; cancel.addEventListener("click", onCancel);
  const save = document.createElement("button"); save.className = "btn btn-primary"; save.textContent = "Save"; save.disabled = !canSaveGroup(draft);
  if (!save.disabled) save.addEventListener("click", () => onSave(draft));
  footer.append(cancel, save); wrap.appendChild(footer);
  if (draft.name.trim() === "") { const m = document.createElement("div"); m.className = "hint"; m.textContent = "Enter a name to continue."; wrap.appendChild(m); }
  return wrap;
}
