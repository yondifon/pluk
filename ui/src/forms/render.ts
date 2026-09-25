import type { AdapterManifest, ConfigFieldDef, ToolDef } from "./catalog.ts";
import { visibleFields, groupedFields, groupedByCategory, prettyCategory } from "./catalog.ts";
import type { ConnectionDraft, Environment } from "./connectionDraft.ts";
import { forgetSecret, isFilled, parseRules, setEnvironment, splitTools } from "./connectionDraft.ts";
import type { Approvals } from "./connectionDraft.ts";
import type { GroupDraft, GroupFormConnection } from "./groupForm.ts";
import {
  availableTools,
  canSaveGroup,
  clearMemberTools,
  hasPickedTools,
  inheritPlaceholder,
  memberTools,
  overridableFields,
  setMemberTool,
} from "./groupForm.ts";
import { isWorkingIn } from "./focus.ts";
import { emptyRow, keepsSaved, type ConfigProblem, type KeyValueRow } from "./keyValue.ts";
import { LOCAL_CATEGORIES, LOCAL_TEMPLATES, SERVER_TEMPLATES, SERVERS_SHOWN, isAdded, type ServerTemplate } from "./serverTemplates.ts";
import { createIcon } from "../icon";
import { createButton, createBadge, wizardStepHeader, wizardStepFooter } from "../primitives";
import { faviconBadge, typeBadge } from "../glyph";
import { MCP_TYPE } from "../integration-detail/types.ts";

/** One well-known server, ready to add with a click. */
function serverTile(
  template: ServerTemplate,
  serverUrls: string[],
  onPick: (template: ServerTemplate) => Promise<void>,
): HTMLButtonElement {
  const added = isAdded(template, serverUrls);
  const tile = document.createElement("button");
  tile.type = "button";
  tile.className = "server-tile";
  tile.dataset.server = template.id;
  tile.title = template.summary;
  tile.setAttribute("aria-label", added ? `Add another ${template.name}` : `Add ${template.name}`);
  tile.appendChild(faviconBadge(template.name, template.site));

  const text = document.createElement("span");
  text.className = "server-tile-text";
  const name = document.createElement("span");
  name.className = "server-tile-name";
  name.textContent = template.name;
  const line = document.createElement("span");
  line.className = "server-tile-line";
  line.textContent = template.summary;
  text.append(name, line);
  tile.appendChild(text);

  if (added) {
    const mark = createBadge("Added");
    mark.classList.add("server-tile-added");
    tile.appendChild(mark);
  }

  tile.addEventListener("click", () => {
    tile.disabled = true;
    tile.setAttribute("aria-busy", "true");
    line.textContent = "Adding…";
    void onPick(template).finally(() => {
      tile.disabled = false;
      tile.removeAttribute("aria-busy");
      line.textContent = template.summary;
    });
  });
  return tile;
}

/** The first `shown` tiles, then a button that reveals the rest in place. */
function appendTileGrid(section: HTMLElement, label: string, tiles: HTMLButtonElement[], shown: number): void {
  const grid = document.createElement("div");
  grid.className = "server-grid";
  grid.setAttribute("role", "group");
  grid.setAttribute("aria-label", label);
  section.appendChild(grid);
  for (const tile of tiles.slice(0, shown)) grid.appendChild(tile);

  if (tiles.length > shown) {
    const rest = tiles.slice(shown);
    const more = createButton(`Show ${rest.length} more`, { size: "sm" });
    more.addEventListener("click", () => {
      for (const tile of rest) grid.appendChild(tile);
      more.remove();
      rest[0].focus();
    });
    section.appendChild(more);
  }
}

function appendLocalServerCategories(section: HTMLElement, serverUrls: string[], onPick: (template: ServerTemplate) => Promise<void>): void {
  for (const category of LOCAL_CATEGORIES) {
    const templates = LOCAL_TEMPLATES.filter((template) => template.category === category);
    if (!templates.length) continue;

    const details = document.createElement("details");
    details.className = "server-category";
    const summary = document.createElement("summary");
    summary.textContent = `${category} · ${templates.length}`;
    const grid = document.createElement("div");
    grid.className = "server-grid";
    grid.setAttribute("role", "group");
    grid.setAttribute("aria-label", category);
    for (const template of templates) grid.appendChild(serverTile(template, serverUrls, onPick));
    details.append(summary, grid);
    section.appendChild(details);
  }
}

function renderPopularServers(
  serverUrls: string[],
  onPick: (template: ServerTemplate) => Promise<void>,
  onPasteConfig?: () => void,
): HTMLElement {
  const section = document.createElement("section");
  section.className = "server-section";

  const title = document.createElement("h3");
  title.className = "ui-card-title";
  title.textContent = "Popular servers";
  const note = document.createElement("p");
  note.className = "hint";
  note.textContent = "Linear, Sentry, and Slack connect through their official MCP servers. Pick one below.";
  section.append(title, note);

  appendTileGrid(section, "Popular servers", SERVER_TEMPLATES.map((template) => serverTile(template, serverUrls, onPick)), SERVERS_SHOWN);

  const localTitle = document.createElement("h4");
  localTitle.className = "ui-card-title";
  localTitle.textContent = "Runs on this Mac";
  const localNote = document.createElement("p");
  localNote.className = "hint";
  localNote.textContent = "Pluk starts these itself. You check the command before it runs.";
  section.append(localTitle, localNote);
  appendLocalServerCategories(section, serverUrls, onPick);
  if (onPasteConfig) {
    const paste = createButton("Paste a server config", { size: "sm", variant: "secondary", onClick: onPasteConfig });
    paste.classList.add("server-paste");
    const pasteHint = document.createElement("p");
    pasteHint.className = "hint";
    pasteHint.textContent = "Already set up a server in another app? Paste its config to add it here.";
    section.append(pasteHint, paste);
  }
  return section;
}

export function renderTypeChooser(
  catalog: AdapterManifest[],
  onChoose: (m: AdapterManifest) => void,
  opts?: {
    onCancel?: () => void;
    adaptersLoadFailed?: boolean;
    onRetry?: () => void;
    /** Adding a well-known server, which only the live app can carry out. */
    onPickServer?: (template: ServerTemplate) => Promise<void>;
    /** Addresses of the servers already added, so a tile can say so. */
    serverUrls?: string[];
    /** Adding servers from a config copied out of another app. */
    onPasteConfig?: () => void;
  },
): HTMLElement {
  const adapters = catalog.filter((a) => a.offeredForSetup);
  const wrap = document.createElement("div");
  wrap.className = "form-chooser";
  wrap.setAttribute("role", "region");
   wrap.setAttribute("aria-label", "Choose what to connect");

  const heading = document.createElement("h2");
  heading.className = "ui-card-title";
  heading.id = "chooser-heading";
   heading.textContent = "Choose what to connect";
  heading.setAttribute("tabindex", "-1");
  wrap.appendChild(heading);

  const helper = document.createElement("p");
  helper.className = "hint";
  helper.textContent = "Pick what Pluk should talk to.";
  wrap.appendChild(helper);

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
        const retry = createButton("Try again", { variant: "secondary", size: "sm", ariaLabel: "Try again", onClick: opts.onRetry });
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
    // A tile creates an MCP integration, so the tiles wait for that type to be in the catalog.
    const onPickServer = opts?.onPickServer;
    const showServers = onPickServer != null && adapters.some((a) => a.id === MCP_TYPE);
    if (showServers) {
      wrap.appendChild(renderPopularServers(opts?.serverUrls ?? [], onPickServer, opts?.onPasteConfig));
      const rest = document.createElement("h3");
      rest.className = "ui-card-title";
      rest.textContent = "Everything else";
      wrap.appendChild(rest);
    }
    for (const { category, items } of groupedByCategory(adapters)) {
      const section = document.createElement("div");
      section.className = "chooser-section";
      const label = prettyCategory(category);
      const sectionTitle = document.createElement(showServers ? "h4" : "h3");
      sectionTitle.className = "ui-card-title";
      sectionTitle.textContent = label;
      section.appendChild(sectionTitle);

      const grid = document.createElement("div");
      grid.className = "chooser-grid";
      grid.setAttribute("role", "group");
      grid.setAttribute("aria-label", label);
      for (const a of items) {
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
      section.appendChild(grid);
      wrap.appendChild(section);
    }
  }

  const footer = document.createElement("div");
  footer.className = "form-footer";
  const cancel = createButton("Cancel", { variant: "secondary", ariaLabel: "Cancel", onClick: () => opts?.onCancel?.() });
  footer.appendChild(cancel);
  wrap.appendChild(footer);

  wrap.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      opts?.onCancel?.();
    }
  });

  queueMicrotask(() => {
    const first = wrap.querySelector<HTMLButtonElement>(".server-tile, .chooser-row");
    if (first) first.focus();
    else heading.focus();
  });

  return wrap;
}

/** Flags a required control the person left empty, once they try to save. */
export function markMissing(control: HTMLElement, wrap: HTMLElement, message: string): void {
  control.setAttribute("aria-invalid", "true");
  control.focus();
  if (wrap.querySelector(".field-error")) return;
  const error = document.createElement("div");
  error.className = "field-error";
  error.setAttribute("role", "alert");
  error.textContent = message;
  wrap.appendChild(error);
}

/** A label in the shared column, the control on the shared axis, and anything
    explaining it stacked underneath the control. */
function settingRow(key: string, labelText: string): { row: HTMLElement; slot: HTMLElement; controlId: string } {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const controlId = `control-${key}`;
  const label = document.createElement("label");
  label.className = "inspector-label";
  label.htmlFor = controlId;
  label.textContent = labelText;
  const slot = document.createElement("div");
  slot.className = "field-slot";
  row.append(label, slot);
  return { row, slot, controlId };
}

function helpText(id: string, text: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "hint";
  el.id = id;
  el.textContent = text;
  return el;
}

/**
 * One config input. `onForget` marks a secret that already has a saved value:
 * the input starts blank, says the value is saved, and offers to remove it.
 */
export function renderField(
  field: ConfigFieldDef,
  value: string,
  onChange: (v: string) => void,
  onForget?: () => void,
): HTMLElement {
  const { row, slot, controlId } = settingRow(field.key, field.required ? `${field.label} *` : field.label);
  row.dataset.fieldKey = field.key;

  const help = field.help ? helpText(`help-${field.key}`, field.help) : null;
  const saved = onForget ? helpText(`saved-${field.key}`, "Saved. Leave this blank to keep it.") : null;
  const describedBy = [saved?.id, help?.id].filter(Boolean).join(" ");
  const describe = (el: HTMLElement) => { if (describedBy) el.setAttribute("aria-describedby", describedBy); };

  switch (field.type) {
    case "toggle": {
      const input = document.createElement("input");
      input.type = "checkbox";
      input.id = controlId;
      input.checked = value === "true";
      describe(input);
      input.addEventListener("change", () => onChange(input.checked ? "true" : "false"));
      slot.appendChild(input);
      break;
    }
    case "select": {
      const sel = document.createElement("select");
      sel.className = "field-select";
      sel.id = controlId;
      describe(sel);
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
      slot.appendChild(sel);
      break;
    }
    case "file": {
      const text = document.createElement("input");
      text.type = "text";
      text.id = controlId;
      text.placeholder = field.placeholder ?? "";
      text.value = value;
      text.className = "field-input mono";
      describe(text);
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
      slot.append(text, btn, file);
      break;
    }
    case "number": {
      const input = document.createElement("input");
      input.type = "number";
      input.id = controlId;
      input.placeholder = field.placeholder ?? "";
      input.value = value;
      input.className = "field-input mono field-number";
      input.inputMode = "numeric";
      input.step = "1";
      describe(input);
      input.addEventListener("input", () => onChange(input.value));
      slot.appendChild(input);
      break;
    }
    default: {
      const input = document.createElement("input");
      input.type = field.type === "password" ? "password" : "text";
      input.id = controlId;
      input.placeholder = saved ? "Saved" : field.placeholder ?? (field.type === "password" ? "••••••" : "");
      input.value = value;
      input.className = "field-input mono";
      describe(input);
      input.addEventListener("input", () => onChange(input.value));
      slot.appendChild(input);
      if (onForget) {
        slot.appendChild(createButton("Remove", { size: "sm", onClick: onForget, ariaLabel: `Remove saved ${field.label.toLowerCase()}` }));
      }
      break;
    }
  }
  if (saved) row.appendChild(saved);
  if (help) row.appendChild(help);
  return row;
}

/** What one row of a key/value field is called in labels: "Headers" gives "header". */
function rowNoun(field: ConfigFieldDef): string {
  return field.label.toLowerCase().replace(/s$/, "");
}

export function renderKeyValueField(
  field: ConfigFieldDef,
  rows: KeyValueRow[],
  onChange: (rows: KeyValueRow[]) => void,
): HTMLElement {
  const { row: wrap, slot, controlId } = settingRow(field.key, field.label);
  wrap.dataset.fieldKey = field.key;
  wrap.classList.add("inspector-row-wrap");
  slot.classList.add("kv-list");
  const noun = rowNoun(field);
  const help = field.help ? helpText(`help-${field.key}`, field.help) : null;
  const update = (index: number, next: Partial<KeyValueRow>) =>
    onChange(rows.map((row, i) => (i === index ? { ...row, ...next } : row)));

  rows.forEach((row, index) => {
    const line = document.createElement("div");
    line.className = "kv-row";
    line.dataset.row = String(index);
    const named = row.name.trim() || `${noun} ${index + 1}`;

    const name = document.createElement("input");
    name.type = "text";
    name.className = "field-input mono kv-name";
    if (index === 0) name.id = controlId;
    name.placeholder = "Name";
    name.spellcheck = false;
    name.value = row.name;
    name.setAttribute("aria-label", `Name of ${noun} ${index + 1}`);
    name.addEventListener("input", () => update(index, { name: name.value }));

    const saved = keepsSaved(row) ? helpText(`saved-${field.key}-${index}`, "Saved. Leave this blank to keep it.") : null;
    const value = document.createElement("input");
    value.type = row.secret ? "password" : "text";
    value.className = "field-input mono kv-value";
    value.placeholder = saved ? "Saved" : "Value";
    value.spellcheck = false;
    value.value = row.value;
    value.setAttribute("aria-label", `Value of ${named}`);
    if (saved) value.setAttribute("aria-describedby", saved.id);
    value.addEventListener("input", () => update(index, { value: value.value }));

    const secretLabel = document.createElement("label");
    secretLabel.className = "kv-secret";
    const secret = document.createElement("input");
    secret.type = "checkbox";
    secret.checked = row.secret;
    secret.setAttribute("aria-label", `Keep ${named} secret`);
    secret.addEventListener("change", () => update(index, { secret: secret.checked }));
    secretLabel.append(secret, document.createTextNode("Secret"));

    const remove = createButton("Remove", {
      size: "sm",
      ariaLabel: `Remove ${named}`,
      onClick: () => onChange(rows.filter((_, i) => i !== index)),
    });

    line.append(name, value, secretLabel, remove);
    slot.appendChild(line);
    if (saved) slot.appendChild(saved);
  });

  slot.appendChild(
    createButton(`Add ${noun}`, { size: "sm", onClick: () => onChange([...rows, emptyRow(field.defaultSecret ?? true)]) }),
  );
  if (help) wrap.appendChild(help);
  return wrap;
}

export function renderListField(
  field: ConfigFieldDef,
  items: string[],
  onChange: (items: string[]) => void,
): HTMLElement {
  const { row: wrap, slot, controlId } = settingRow(field.key, field.label);
  wrap.dataset.fieldKey = field.key;
  wrap.classList.add("inspector-row-wrap");
  slot.classList.add("kv-list");
  const noun = rowNoun(field);
  const help = field.help ? helpText(`help-${field.key}`, field.help) : null;
  const update = (index: number, value: string) =>
    onChange(items.map((item, i) => (i === index ? value : item)));

  items.forEach((item, index) => {
    const line = document.createElement("div");
    line.className = "kv-row";
    line.dataset.row = String(index);

    const input = document.createElement("input");
    input.type = "text";
    input.className = "field-input mono kv-value";
    if (index === 0) input.id = controlId;
    input.spellcheck = false;
    input.value = item;
    input.setAttribute("aria-label", `${field.label} ${index + 1}`);
    input.addEventListener("input", () => update(index, input.value));

    const remove = createButton("Remove", {
      size: "sm",
      ariaLabel: `Remove ${noun} ${index + 1}`,
      onClick: () => onChange(items.filter((_, i) => i !== index)),
    });

    line.append(input, remove);
    slot.appendChild(line);
  });

  slot.appendChild(createButton(`Add ${noun}`, { size: "sm", onClick: () => onChange([...items, ""]) }));
  if (help) wrap.appendChild(help);
  return wrap;
}

/** Shows a save the host would refuse beside the field and row it names. */
export function markProblem(host: HTMLElement, problem: ConfigProblem): void {
  const field = host.querySelector<HTMLElement>(`[data-field-key="${problem.field}"]`);
  if (!field) return;
  const line = problem.row != null ? field.querySelector<HTMLElement>(`.kv-row[data-row="${problem.row}"]`) : null;
  const control = (line ?? field).querySelector<HTMLElement>("input, select");
  if (control) markMissing(control, line ?? field, problem.message);
}

export function renderToolsSection(
  draft: ConnectionDraft,
  onToggle: (tool: string, enabled: boolean) => void,
  onSettingChange: (tool: string, key: string, value: string) => void,
  onToggleAll?: (enabled: boolean) => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "ui-card";
  const header = document.createElement("div");
  header.className = "tools-header";
  const title = document.createElement("h3");
  title.className = "ui-card-title";
  title.textContent = "Tools";
  header.appendChild(title);

  const enabledCount = draft.tools.filter((t) => (draft.toolConfig[t.name]?.enabled ?? t.defaultEnabled)).length;
  if (onToggleAll && draft.tools.length) {
    const allOn = enabledCount === draft.tools.length;
    header.appendChild(
      createButton(allOn ? "Turn all off" : "Turn all on", {
        size: "sm",
        onClick: () => onToggleAll(!allOn),
      }),
    );
  }
  wrap.appendChild(header);
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = draft.tools.length
    ? `${enabledCount} of ${draft.tools.length} on. Enable tools to give the agent more, disable to shrink what it sees.`
    : "Nothing to choose yet. Finish here, then open this integration to see what it offers.";
  wrap.appendChild(hint);

  const { defaults, extras } = splitTools(draft.tools);

  const renderList = (tools: ToolDef[]) => {
    for (const tool of tools) {
      const enabled = draft.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled;
      const row = document.createElement("div");
      row.className = enabled ? "tool-row tool-on" : "tool-row tool-off";

      const head = document.createElement("label");
      head.className = "tool-head";
      const toggle = document.createElement("input");
      toggle.type = "checkbox";
      toggle.checked = enabled;
      toggle.setAttribute("aria-label", tool.label);
      toggle.setAttribute("aria-describedby", `tool-desc-${tool.name}`);
      toggle.addEventListener("change", () => onToggle(tool.name, toggle.checked));
      const name = document.createElement("span");
      name.className = "tool-name";
      name.id = `tool-name-${tool.name}`;
      name.textContent = tool.label;
      const id = document.createElement("code");
      id.className = "tool-id tool-category mono";
      id.textContent = tool.name;
      const category = document.createElement("span");
      category.className = "tool-category";
      category.textContent = tool.category;
      head.append(toggle, name, id, category);
      if (!enabled) {
        const state = document.createElement("span");
        state.className = "tool-state";
        state.textContent = "Off";
        head.appendChild(state);
      }

      const body = document.createElement("div");
      body.className = "tool-body";
      const desc = document.createElement("div");
      desc.className = "tool-summary";
      desc.id = `tool-desc-${tool.name}`;
      desc.textContent = tool.description;
      body.appendChild(desc);

      // Settings expanded when enabled
      if (enabled && tool.settings?.length) {
        const settingsWrap = document.createElement("div");
        settingsWrap.className = "tool-settings";
        settingsWrap.setAttribute("role", "group");
        settingsWrap.setAttribute("aria-labelledby", name.id);
        for (const s of tool.settings) {
          const value = draft.toolConfig[tool.name]?.settings[s.key] ?? s.default ?? "";
          settingsWrap.appendChild(renderSettingRow(tool, s, value, onSettingChange));
        }
        body.appendChild(settingsWrap);
      }

      row.append(head, body);
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
    moreHint.textContent = "Turn on the ones the agent should have.";
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
  const key = `${tool.name}-${setting.key}`;
  const { row, slot, controlId } = settingRow(key, setting.label);
  const help = setting.help ? helpText(`help-${key}`, setting.help) : null;
  const describe = (el: HTMLElement) => { if (help) el.setAttribute("aria-describedby", help.id); };

  if (setting.type === "toggle") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.id = controlId;
    input.checked = value === "true";
    describe(input);
    input.addEventListener("change", () => onChange(tool.name, setting.key, input.checked ? "true" : "false"));
    slot.appendChild(input);
  } else if (setting.type === "select") {
    const sel = document.createElement("select");
    sel.className = "field-select";
    sel.id = controlId;
    describe(sel);
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
    slot.appendChild(sel);
  } else if (setting.type === "number") {
    const inp = document.createElement("input");
    inp.type = "number"; inp.id = controlId; inp.value = value; inp.inputMode = "numeric"; inp.step = "1";
    inp.className = "field-input mono field-number";
    describe(inp);
    inp.addEventListener("input", () => onChange(tool.name, setting.key, inp.value));
    slot.appendChild(inp);
  } else {
    const inp = document.createElement("input");
    inp.type = setting.type === "password" ? "password" : "text";
    inp.id = controlId;
    inp.placeholder = setting.placeholder ?? ""; inp.value = value; inp.className = "field-input mono";
    describe(inp);
    inp.addEventListener("input", () => onChange(tool.name, setting.key, inp.value));
    slot.appendChild(inp);
  }
  if (help) row.appendChild(help);

  if (setting.danger && value === "true") {
    row.classList.add("danger-setting");
    const warn = document.createElement("div");
    warn.className = "hint danger-copy";
    warn.append(createIcon("error"));
    const warning = document.createElement("strong");
    warning.textContent = `Warning: ${dangerousCopy(setting)}`;
    warn.appendChild(warning);
    row.appendChild(warn);
  }
  return row;
}

/** One rule list: a label, a text box holding one pattern per line. */
function ruleList(
  key: string,
  label: string,
  rules: string[],
  text: { placeholder: string; help?: string; error?: string },
  onChange: (next: string[]) => void,
): HTMLElement {
  const { row, slot, controlId } = settingRow(key, label);
  const box = document.createElement("textarea");
  box.id = controlId;
  box.rows = 3;
  box.spellcheck = false;
  box.className = "field-input mono rule-list";
  box.placeholder = text.placeholder;
  box.value = rules.join("\n");
  box.addEventListener("change", () => onChange(parseRules(box.value)));
  slot.appendChild(box);
  if (text.help) {
    box.setAttribute("aria-describedby", `${controlId}-help`);
    row.appendChild(helpText(`${controlId}-help`, text.help));
  }
  if (text.error) {
    box.setAttribute("aria-invalid", "true");
    const message = document.createElement("div");
    message.className = "field-error";
    message.setAttribute("role", "alert");
    message.textContent = text.error;
    row.appendChild(message);
  }
  return row;
}

/** The first rule Pluk cannot save, and which list holds it. */
export interface RuleProblem {
  list: "allow" | "deny";
  message: string;
}

export function renderApprovalsSection(
  approvals: Approvals,
  onChange: (next: Approvals) => void,
  problem?: RuleProblem | null,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "ui-card";
  const title = document.createElement("h3");
  title.className = "ui-card-title";
  title.textContent = "What the agent may run";
  wrap.appendChild(title);

  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent =
    "One pattern per line, matched against the whole command. * stands for any text, ? for one character.";
  wrap.appendChild(hint);

  wrap.appendChild(
    ruleList(
      "approvals-allow",
      "Always allow",
      approvals.allow,
      {
        placeholder: "git pull*",
        error: problem?.list === "allow" ? problem.message : undefined,
      },
      (allow) => onChange({ ...approvals, allow }),
    ),
  );
  wrap.appendChild(
    ruleList(
      "approvals-deny",
      "Never allow",
      approvals.deny,
      {
        placeholder: "rm -rf *",
        help: "Wins over Always allow.",
        error: problem?.list === "deny" ? problem.message : undefined,
      },
      (deny) => onChange({ ...approvals, deny }),
    ),
  );

  const ask = settingRow("approvals-ask", "Ask me first");
  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  toggle.id = ask.controlId;
  toggle.checked = approvals.ask;
  toggle.setAttribute("aria-describedby", "approvals-ask-help");
  toggle.addEventListener("change", () => onChange({ ...approvals, ask: toggle.checked }));
  ask.slot.appendChild(toggle);
  ask.row.appendChild(
    helpText(
      "approvals-ask-help",
      "When neither list matches, Pluk asks you before refusing. Off means it refuses straight away.",
    ),
  );
  wrap.appendChild(ask.row);

  return wrap;
}

/** Step 2, Name it: the one field every adapter type asks for, plus environment. */
export function renderNameStep(
  draft: ConnectionDraft,
  manifest: AdapterManifest | undefined,
  stepIndex: number,
  totalSteps: number,
  onDraftChange: (next: ConnectionDraft) => void,
  onBack: (() => void) | null,
  onCancel: () => void,
  onContinue: () => void,
): HTMLElement {
  const wrap = wizardStepHeader(stepIndex, totalSteps, "Name it", "Agents will see this name when they use it.");
  const card = document.createElement("div");
  card.className = "ui-card";

  const name = settingRow("integration-name", "Name *");
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = manifest ? `My ${manifest.label}` : "My Service";
  nameInput.value = draft.name; nameInput.className = "field-input"; nameInput.id = name.controlId;
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  name.slot.appendChild(nameInput);
  card.appendChild(name.row);

  const env = settingRow("environment", "Environment");
  const envPicker = document.createElement("select");
  envPicker.className = "field-select";
  envPicker.id = env.controlId;
  const noneOpt = document.createElement("option");
  noneOpt.value = ""; noneOpt.textContent = "None";
  if (draft.environment == null) noneOpt.selected = true;
  envPicker.appendChild(noneOpt);
  for (const value of ["production", "staging", "development", "local"] as Environment[]) {
    const o = document.createElement("option");
    o.value = value; o.textContent = value[0].toUpperCase() + value.slice(1);
    if (value === draft.environment) o.selected = true;
    envPicker.appendChild(o);
  }
  envPicker.addEventListener("change", () => onDraftChange(setEnvironment(draft, (envPicker.value || null) as Environment | null)));
  env.slot.appendChild(envPicker);
  card.appendChild(env.row);

  if (draft.policyKind === "sql" && (draft.environment === "development" || draft.environment === "local") && draft.toolConfig.query?.settings.mode === "mutations") {
    const environmentHint = document.createElement("p");
    environmentHint.className = "hint environment-hint";
    environmentHint.textContent = "Development and local setups allow write actions by default.";
    card.appendChild(environmentHint);
  }
  wrap.appendChild(card);

  const { el: footer } = wizardStepFooter({
    onBack,
    onCancel,
    primaryLabel: "Continue",
    onPrimary: () => {
      if (!draft.name.trim()) {
        markMissing(nameInput, name.row, "Enter a name to continue.");
        return;
      }
      onContinue();
    },
  });
  wrap.appendChild(footer);
  return wrap;
}

/** Step 3 for an adapter with connection fields: the fields grouped as they already are, instead of a pairing card. */
export function renderConnectFieldsStep(
  draft: ConnectionDraft,
  manifest: AdapterManifest,
  stepIndex: number,
  totalSteps: number,
  onDraftChange: (next: ConnectionDraft) => void,
  onBack: (() => void) | null,
  onCancel: () => void,
  onContinue: () => void,
  /** The field to open on, and the line above it saying what belongs there. */
  landOn?: { field: string; text: string },
  /** Asks the host what saving this draft would refuse, before moving on. */
  check?: (draft: ConnectionDraft) => Promise<ConfigProblem | null>,
): HTMLElement {
  const wrap = wizardStepHeader(stepIndex, totalSteps, "Connect", `Fill in what Pluk needs to reach ${manifest.label}.`);
  const body = document.createElement("div");
  body.className = "wizard-body";

  for (const { group, fields } of groupedFields(manifest)) {
    const shown = visibleFields(fields, draft.config);
    if (!shown.length) continue;
    const card = document.createElement("div");
    card.className = "ui-card";
    const h = document.createElement("h3"); h.className = "ui-card-title"; h.textContent = group;
    card.appendChild(h);
    for (const f of shown) {
      if (landOn?.field === f.key) card.appendChild(helpText(`land-on-${f.key}`, landOn.text));
      if (f.type === "keyvalue") {
        card.appendChild(renderKeyValueField(f, draft.rows[f.key] ?? [], (rows) => {
          onDraftChange({ ...draft, rows: { ...draft.rows, [f.key]: rows } });
        }));
        continue;
      }
      if (f.type === "list") {
        card.appendChild(renderListField(f, draft.lists[f.key] ?? [], (items) => {
          onDraftChange({ ...draft, lists: { ...draft.lists, [f.key]: items } });
        }));
        continue;
      }
      const onForget = draft.savedSecrets.includes(f.key)
        ? () => onDraftChange({ ...forgetSecret(draft, f.key), config: { ...draft.config, [f.key]: "" } })
        : undefined;
      const row = renderField(f, draft.config[f.key] ?? "", (v) => {
        onDraftChange({ ...draft, config: { ...draft.config, [f.key]: v } });
      }, onForget);
      card.appendChild(row);
    }
    body.appendChild(card);
  }
  wrap.appendChild(body);

  if (landOn) {
    const inputs = [...body.querySelectorAll<HTMLInputElement>(`[data-field-key="${landOn.field}"] input`)];
    const control = inputs.find((input) => input.value === "") ?? inputs[0];
    const described = [control?.getAttribute("aria-describedby"), `land-on-${landOn.field}`];
    control?.setAttribute("aria-describedby", described.filter(Boolean).join(" "));
    // Arriving lands on the field that still needs something; a redraw mid-edit leaves them be.
    queueMicrotask(() => {
      if (!isWorkingIn(wrap)) control?.focus();
    });
  }

  const { el: footer } = wizardStepFooter({
    onBack,
    onCancel,
    primaryLabel: "Continue",
    onPrimary: () => {
      const invalid = visibleFields(draft.fields, draft.config).find((field) => field.required && !isFilled(draft, field));
      if (!invalid) {
        if (!check) {
          onContinue();
          return;
        }
        void check(draft).then((problem) => {
          if (problem) markProblem(wrap, problem);
          else onContinue();
        });
        return;
      }
      const invalidRow = wrap.querySelector<HTMLElement>(`[data-field-key="${invalid.key}"]`);
      const control = invalidRow?.querySelector<HTMLElement>("input, select");
      if (control) {
        control.setAttribute("aria-invalid", "true");
        control.focus();
        if (!invalidRow?.querySelector(".field-error")) {
          const error = document.createElement("div");
          error.className = "field-error";
          error.setAttribute("role", "alert");
          error.textContent = `${invalid.label} is required.`;
          invalidRow?.appendChild(error);
        }
      }
    },
  });
  wrap.appendChild(footer);
  return wrap;
}

/**
 * Step 4, Choose what the agent can do: the same tool list as today. When
 * there is no commands step after it (nothing runs shell-like commands),
 * this is where the integration is actually saved.
 */
export function renderToolsStep(
  draft: ConnectionDraft,
  stepIndex: number,
  totalSteps: number,
  isLastContentStep: boolean,
  onDraftChange: (next: ConnectionDraft) => void,
  onBack: (() => void) | null,
  onCancel: () => void,
  onNext: (d: ConnectionDraft) => void,
): HTMLElement {
  const wrap = wizardStepHeader(
    stepIndex,
    totalSteps,
    "Choose what the agent can do",
    "Turn off anything you don’t want an agent posting or reading.",
  );
  const body = document.createElement("div");
  body.className = "wizard-body";
  body.appendChild(
    renderToolsSection(
      draft,
      (tool, enabled) => {
        const next = { ...draft, toolConfig: { ...draft.toolConfig, [tool]: { ...(draft.toolConfig[tool] ?? { enabled: false, settings: {} }), enabled } } };
        onDraftChange(next);
      },
      (tool, key, value) => {
        const prev = draft.toolConfig[tool] ?? { enabled: true, settings: {} };
        onDraftChange({ ...draft, toolConfig: { ...draft.toolConfig, [tool]: { ...prev, settings: { ...prev.settings, [key]: value } } } });
      },
      (enabled) => {
        const toolConfig = { ...draft.toolConfig };
        for (const tool of draft.tools) {
          toolConfig[tool.name] = { ...(toolConfig[tool.name] ?? { enabled, settings: {} }), enabled };
        }
        onDraftChange({ ...draft, toolConfig });
      },
    ),
  );
  wrap.appendChild(body);

  const { el: footer } = wizardStepFooter({
    onBack,
    onCancel,
    primaryLabel: isLastContentStep ? "Save integration" : "Continue",
    onPrimary: () => onNext(draft),
  });
  wrap.appendChild(footer);
  return wrap;
}

/** The extra screen for adapters that run commands: today's allow/deny/ask rules, always the step that saves. */
export function renderCommandsStep(
  draft: ConnectionDraft,
  stepIndex: number,
  totalSteps: number,
  onDraftChange: (next: ConnectionDraft) => void,
  onBack: (() => void) | null,
  onCancel: () => void,
  onSave: (d: ConnectionDraft) => void,
  ruleProblem?: RuleProblem | null,
): HTMLElement {
  const wrap = wizardStepHeader(
    stepIndex,
    totalSteps,
    "What it’s allowed to run",
    "Rules for what it can run without asking you first.",
  );
  const body = document.createElement("div");
  body.className = "wizard-body";
  body.appendChild(renderApprovalsSection(draft.approvals, (approvals) => onDraftChange({ ...draft, approvals }), ruleProblem));
  wrap.appendChild(body);

  const { el: footer } = wizardStepFooter({
    onBack,
    onCancel,
    primaryLabel: "Save integration",
    onPrimary: () => onSave(draft),
  });
  wrap.appendChild(footer);
  return wrap;
}

/** What the count line under a member's Tools heading says. */
function memberToolsHint(picked: number, available: number, name: string): string {
  if (!available) return `Nothing to pick yet. Turn on tools for ${name} first.`;
  if (!picked) return `0 of ${available} tools. This one gives the agent nothing here.`;
  return `${picked} of ${available} tools. Uncheck what the agent should not reach here.`;
}

/**
 * One member's tools: a row per tool the integration has on, and a way back to
 * handing over all of them.
 *
 * The reset sits after the list, never before it. The form rebuilds on every
 * change and puts focus back by position, so a control that comes and goes
 * ahead of the checkboxes would move focus to the wrong row mid-edit.
 */
function renderMemberTools(
  draft: GroupDraft,
  conn: GroupFormConnection,
  onDraftChange: (next: GroupDraft) => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "member-tools";
  const available = availableTools(conn);
  const exposed = new Set(memberTools(draft, conn));

  const title = document.createElement("p");
  title.className = "member-tools-title";
  title.id = `${conn.id}-tools-title`;
  title.textContent = "Tools";
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = memberToolsHint(exposed.size, available.length, conn.name);
  wrap.append(title, hint);
  if (!available.length) return wrap;

  const list = document.createElement("div");
  list.setAttribute("role", "group");
  list.setAttribute("aria-labelledby", title.id);
  for (const tool of available) {
    const on = exposed.has(tool.name);
    const row = document.createElement("label");
    row.className = on ? "member-tool-row" : "member-tool-row tool-off";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = on;
    box.setAttribute("aria-label", tool.label);
    box.addEventListener("change", () =>
      onDraftChange(setMemberTool(draft, conn, tool.name, box.checked)),
    );
    const label = document.createElement("span");
    label.className = "tool-name";
    label.textContent = tool.label;
    const id = document.createElement("code");
    id.className = "tool-id tool-category mono";
    id.textContent = tool.name;
    row.append(box, label, id);
    list.appendChild(row);
  }
  wrap.appendChild(list);

  if (hasPickedTools(draft, conn.id)) {
    wrap.appendChild(
      createButton("Use all tools", {
        size: "sm",
        onClick: () => onDraftChange(clearMemberTools(draft, conn.id)),
      }),
    );
  }
  return wrap;
}

export function renderGroupForm(
  draft: GroupDraft,
  connections: GroupFormConnection[],
  adapters: AdapterManifest[],
  onDraftChange: (next: GroupDraft) => void,
  onSave: (d: GroupDraft) => void,
  onCancel: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "form-body";

  const card = document.createElement("div");
  card.className = "ui-card";
  card.innerHTML = `<h3 class="ui-card-title">Group</h3>`;
  const name = settingRow("group-name", "Name *");
  const nameInput = document.createElement("input");
  nameInput.type = "text"; nameInput.placeholder = "Group name"; nameInput.value = draft.name; nameInput.className = "field-input"; nameInput.id = name.controlId;
  nameInput.addEventListener("input", () => onDraftChange({ ...draft, name: nameInput.value }));
  name.slot.appendChild(nameInput);
  card.appendChild(name.row);

  // Environment picker with Any
  const environment = settingRow("group-environment", "Environment");
  const sel = document.createElement("select");
  sel.className = "field-select";
  sel.id = environment.controlId;
  const anyOpt = document.createElement("option"); anyOpt.value = ""; anyOpt.textContent = "Any (mixed)"; if (draft.environment == null) anyOpt.selected = true; sel.appendChild(anyOpt);
  for (const env of ["production", "staging", "development", "local"]) {
    const o = document.createElement("option"); o.value = env; o.textContent = env[0].toUpperCase() + env.slice(1); if (draft.environment === env) o.selected = true; sel.appendChild(o);
  }
  sel.addEventListener("change", () => onDraftChange({ ...draft, environment: sel.value || null }));
  environment.slot.appendChild(sel);
  card.appendChild(environment.row);
  wrap.appendChild(card);

  // Checklist
  const listCard = document.createElement("div"); listCard.className = "ui-card";
  listCard.innerHTML = `<h3 class="ui-card-title">Integrations</h3>`;
  if (!connections.length) {
    const empty = document.createElement("div"); empty.className = "empty"; empty.textContent = "No integrations yet. Add one first."; listCard.appendChild(empty);
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
      header2.append(cb, nameEl);
      if (conn.environment) header2.appendChild(createBadge(conn.environment, "environment"));
      row2.appendChild(header2);

      if (on) {
        const manifest = adapters.find((a) => a.id === conn.type);
        const fields = overridableFields(manifest);
        const panel = document.createElement("div"); panel.className = "member-form-panel";
        if (fields.length) {
          const hint = document.createElement("div"); hint.className = "hint"; hint.textContent = "Overrides for this group (blank = inherit)";
          panel.appendChild(hint);
          for (const f of fields) {
            const override = settingRow(`${conn.id}-${f.key}`, f.label);
            const inp = document.createElement("input");
            inp.type = "text"; inp.className = "field-input mono"; inp.id = override.controlId;
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
            override.slot.appendChild(inp);
            panel.appendChild(override.row);
          }
        }
        panel.appendChild(renderMemberTools(draft, conn, onDraftChange));
        row2.appendChild(panel);
      }
      listCard.appendChild(row2);
    }
  }
  wrap.appendChild(listCard);

  const footer = document.createElement("div"); footer.className = "form-footer";
  const cancel = createButton("Cancel", { variant: "secondary", onClick: onCancel });
  const save = createButton("Save", { variant: "primary" });
  save.addEventListener("click", () => {
    if (!draft.name.trim()) { markMissing(nameInput, name.row, "Enter a name to continue."); return; }
    if (canSaveGroup(draft)) onSave(draft);
  });
  footer.append(cancel, save); wrap.appendChild(footer);
  return wrap;
}
