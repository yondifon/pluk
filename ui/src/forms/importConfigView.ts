/** The paste-and-review flow. It keeps its own state, so the modal only opens and closes it. */

import { createBadge, createButton } from "../primitives";
import {
  addedDrafts,
  afterSave,
  canSave,
  chosen,
  describeImportError,
  nameProblem,
  partialMessage,
  reviewItems,
  savedIds,
  saveLabel,
  type DraftRow,
  type ImportedServer,
  type ParsedImport,
  type ReviewItem,
  type ServerDraft,
  type ServerProblem,
} from "./importConfig";

export interface ImportHost {
  /** The names already in use, read fresh each time a name is checked. */
  takenNames(): string[];
  parse(text: string): Promise<ParsedImport>;
  save(servers: ServerDraft[]): Promise<ImportedServer[]>;
  /** Every server the user chose is added, or they are done after a partial save. */
  onDone(ids: string[], added: ServerDraft[]): void | Promise<void>;
  onBack(): void;
  onCancel(): void;
}

const MASK = "••••••••";

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

export function renderImportFlow(host: ImportHost): HTMLElement {
  const root = el("div", "import-flow");
  let text = "";
  let pasteError: string | null = null;
  let items: ReviewItem[] = [];
  let problems: ServerProblem[] = [];
  let busy = false;
  const addedIds: string[] = [];
  const added: ServerDraft[] = [];
  const taken = () => [...host.takenNames(), ...added.map((d) => d.name)];

  function showPaste(): void {
    root.innerHTML = "";
    const heading = el("h2", "ui-card-title", "Paste a server config");
    heading.tabIndex = -1;
    const hint = el(
      "p",
      "hint",
      "Paste the JSON or TOML that lists your MCP servers, from Claude Code, Cursor, Windsurf, opencode, Codex or a similar app. You'll review each server before anything is added.",
    );
    hint.id = "import-paste-hint";
    const box = el("textarea", "field-input mono import-paste");
    box.rows = 12;
    box.spellcheck = false;
    box.value = text;
    box.placeholder = '{ "mcpServers": { … } }';
    box.setAttribute("aria-label", "Server config");
    box.setAttribute("aria-describedby", "import-paste-hint");
    box.addEventListener("input", () => {
      text = box.value;
    });
    root.append(heading, hint, box);
    if (pasteError) {
      const error = el("p", "field-error", pasteError);
      error.setAttribute("role", "alert");
      box.setAttribute("aria-invalid", "true");
      root.appendChild(error);
    }
    const review = createButton("Review servers", { variant: "primary", onClick: () => void readText() });
    review.disabled = busy;
    root.appendChild(footer(createButton("Back", { variant: "secondary", onClick: host.onBack }), review));
    box.focus();
  }

  async function readText(): Promise<void> {
    busy = true;
    try {
      const parsed = await host.parse(text);
      pasteError = null;
      items = reviewItems(parsed);
      problems = parsed.problems;
      busy = false;
      showReview();
    } catch (error) {
      pasteError = describeImportError(error);
      busy = false;
      showPaste();
    }
  }

  async function saveChosen(): Promise<void> {
    const drafts = chosen(items).map((item) => item.draft);
    busy = true;
    showReview();
    let outcomes: ImportedServer[];
    try {
      outcomes = await host.save(drafts);
    } catch (error) {
      busy = false;
      showReview(describeImportError(error));
      return;
    }
    busy = false;
    addedIds.push(...savedIds(outcomes));
    added.push(...addedDrafts(drafts, outcomes));
    items = afterSave(items, outcomes);
    problems = [];
    if (items.length === 0) {
      await host.onDone(addedIds, added);
      return;
    }
    showReview(addedIds.length > 0 ? partialMessage(addedIds.length, items.length) : undefined);
  }

  function showReview(status?: string): void {
    root.innerHTML = "";
    const heading = el("h2", "ui-card-title", "Review servers");
    heading.tabIndex = -1;
    root.appendChild(heading);

    if (status) {
      const line = el("p", "hint import-status", status);
      line.setAttribute("role", "status");
      root.appendChild(line);
    } else if (items.length > 0) {
      const found = items.length === 1 ? "Pluk found 1 server." : `Pluk found ${items.length} servers.`;
      root.appendChild(el("p", "hint", `${found} Check each one, then add the ones you want.`));
    }

    if (problems.length > 0) root.appendChild(problemList(problems));

    if (items.length === 0) {
      const empty = el("div", "ui-card import-empty");
      empty.setAttribute("role", "status");
      empty.append(
        el("h3", "ui-card-title", "No servers to add"),
        el("p", "hint", "None of the servers in this config could be read. Go back and fix the config, or paste another one."),
      );
      root.appendChild(empty);
    }

    const checks: Array<() => void> = [];
    items.forEach((item, index) => {
      const { card, check } = serverCard(item, index, () => refresh());
      checks.push(check);
      root.appendChild(card);
    });

    const add = createButton(saveLabel(items), { variant: "primary", onClick: () => void saveChosen() });
    const refresh = () => {
      for (const check of checks) check();
      add.textContent = busy ? "Adding…" : saveLabel(items);
      add.disabled = busy || !canSave(items, taken());
    };
    refresh();

    const back = createButton("Back", {
      variant: "secondary",
      onClick: () => {
        pasteError = null;
        showPaste();
      },
    });
    back.disabled = busy;
    const leave = addedIds.length > 0
      ? createButton("Done", { variant: "secondary", onClick: () => void host.onDone(addedIds, added) })
      : createButton("Cancel", { variant: "secondary", onClick: host.onCancel });
    root.appendChild(footer(back, leave, add));
    heading.focus();
  }

  /** One server's card. `check` redraws what depends on the other cards. */
  function serverCard(item: ReviewItem, index: number, onChange: () => void): { card: HTMLElement; check: () => void } {
    const draft = item.draft;
    const card = el("section", "ui-card import-card");
    card.dataset.server = String(index);

    const head = el("div", "import-card-head");
    const nameId = `import-name-${index}`;
    const label = el("label", "inspector-label", "Name");
    label.htmlFor = nameId;
    const name = el("input", "field-input");
    name.type = "text";
    name.id = nameId;
    name.value = draft.name;
    name.spellcheck = false;
    name.addEventListener("input", () => {
      item.draft = { ...item.draft, name: name.value };
      onChange();
    });
    head.append(label, name, createBadge(draft.connection === "local" ? "Local server" : "Remote server"));
    card.appendChild(head);

    const clash = el("p", "field-error");
    clash.id = `import-name-error-${index}`;
    clash.setAttribute("role", "alert");
    card.appendChild(clash);

    for (const note of notes(draft)) card.appendChild(el("p", "hint", note));
    card.appendChild(details(draft));
    if (draft.headers.length > 0) {
      card.appendChild(rowList("Headers", draft.headers, (headers) => {
        item.draft = { ...item.draft, headers };
      }));
    }
    if (draft.env.length > 0) {
      card.appendChild(rowList("Environment variables", draft.env, (env) => {
        item.draft = { ...item.draft, env };
      }));
    }
    if (draft.disabledTools.length > 0) {
      card.appendChild(el("p", "hint", `Tools to turn off once Pluk finds them: ${draft.disabledTools.join(", ")}`));
    }
    if (draft.notImported.length > 0) {
      card.appendChild(el("p", "hint import-not-imported", `Not imported: ${draft.notImported.join(", ")}`));
    }
    if (item.error) {
      const error = el("p", "field-error", item.error);
      error.setAttribute("role", "alert");
      card.appendChild(error);
    }

    const skipLabel = el("label", "import-skip");
    const skip = el("input");
    skip.type = "checkbox";
    skip.checked = item.skip;
    skip.addEventListener("change", () => {
      item.skip = skip.checked;
      onChange();
    });
    skipLabel.append(skip, document.createTextNode("Skip this server"));
    card.appendChild(skipLabel);

    const check = () => {
      const problem = nameProblem(items, index, taken());
      clash.textContent = problem ?? "";
      clash.hidden = problem === null;
      if (problem) {
        name.setAttribute("aria-invalid", "true");
        name.setAttribute("aria-describedby", clash.id);
      } else {
        name.removeAttribute("aria-invalid");
        name.removeAttribute("aria-describedby");
      }
      card.classList.toggle("is-skipped", item.skip);
      name.disabled = item.skip || busy;
    };
    return { card, check };
  }

  showPaste();
  return root;
}

function footer(...buttons: HTMLElement[]): HTMLElement {
  const bar = el("div", "form-footer");
  bar.append(...buttons);
  return bar;
}

function problemList(problems: ServerProblem[]): HTMLElement {
  const box = el("div", "ui-card import-problems");
  box.setAttribute("role", "alert");
  box.appendChild(el("h3", "ui-card-title", problems.length === 1 ? "1 server couldn't be read" : `${problems.length} servers couldn't be read`));
  const list = el("ul");
  for (const problem of problems) list.appendChild(el("li", undefined, `${problem.name}: ${problem.message}`));
  box.appendChild(list);
  return box;
}

function notes(draft: ServerDraft): string[] {
  const lines: string[] = [];
  if (draft.viaMcpRemote) lines.push("This config ran mcp-remote to reach this server. Pluk connects to the URL directly.");
  if (draft.sse) lines.push("This config used SSE. If the server doesn't answer, check that it offers a streamable HTTP address.");
  if (draft.turnedOff) lines.push("This server was turned off in the config, so it starts skipped.");
  return lines;
}

/** The mapped fields, labelled as the settings form labels them. */
function details(draft: ServerDraft): HTMLElement {
  const list = el("dl", "import-details");
  const row = (term: string, value: string | null | undefined) => {
    if (!value) return;
    list.append(el("dt", undefined, term), el("dd", "mono", value));
  };
  if (draft.connection === "remote") {
    row("Server URL", draft.url);
  } else {
    row("Command", draft.command);
    row("Arguments", draft.args.join(" "));
    row("Working folder", draft.cwd);
  }
  return list;
}

/** Names and values, with a Secret switch per row. A secret value is masked. */
function rowList(title: string, rows: DraftRow[], onChange: (rows: DraftRow[]) => void): HTMLElement {
  const group = el("div", "import-rows");
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", title);
  group.appendChild(el("h4", "ui-card-title", title));
  let current = rows;
  current.forEach((row, index) => {
    const line = el("div", "kv-row");
    const value = el("span", "mono kv-value", row.secret ? MASK : row.value);
    const label = el("label", "kv-secret");
    const secret = el("input");
    secret.type = "checkbox";
    secret.checked = row.secret;
    secret.setAttribute("aria-label", `Keep ${row.name} secret`);
    secret.addEventListener("change", () => {
      current = current.map((r, i) => (i === index ? { ...r, secret: secret.checked } : r));
      value.textContent = secret.checked ? MASK : row.value;
      onChange(current);
    });
    label.append(secret, document.createTextNode("Secret"));
    line.append(el("span", "mono kv-name", row.name), value, label);
    group.appendChild(line);
  });
  return group;
}
