// Custom exclusions stay in the app-owned child, never the remote Settings DOM.
import { agentMissing, append, button, control, group, h, notice, pageHeader } from "../app.js";

export const title = "Excluded files and folders";
export const icon = "lock-simple";

const patterns = (list) => h("div", { class: "patterns" }, list.map(pattern => h("code", {}, pattern)));

async function load(app) {
  const view = app.view("privacy");
  try {
    view.deny = await control({ cmd: "deny" });
    view.loadError = null;
  } catch (error) {
    view.loadError = error.message;
  }
  // Drafts belong to the local view and survive status updates/hidden panes.
  if (app.page === "privacy" || app.page === "folders") app.render();
}

export const enter = load;
export function statusChanged(app) {
  if (app.page === "privacy" || app.page === "folders") load(app);
}

async function change(app, previous, pattern) {
  const view = app.view("privacy");
  view.busy = true;
  view.error = null;
  app.render();
  try {
    view.deny = await control({ cmd: "deny_extra", previous, pattern });
    view.editing = null;
    view.draft = "";
  } catch (error) {
    if (error.code !== "declined") view.error = error.message;
  }
  view.busy = false;
  app.render();
}

function editor(app) {
  const view = app.view("privacy");
  const editing = view.editing != null;
  const valid = !!view.draft?.trim();
  return h("form", { class: "row row--stack", onsubmit: event => {
    event.preventDefault();
    if (view.busy || !view.draft?.trim()) return;
    change(app, view.editing ?? null, view.draft.trim());
  } },
    h("label", { class: "row__body", for: "exclusion-pattern" },
      h("h3", {}, editing ? "Edit custom exclusion" : "Add an exclusion"),
      h("p", {}, "A name matches anywhere, like *.secret. Use / between folders, like Clients/Private.")),
    h("input", { id: "exclusion-pattern", class: "input", value: view.draft || "", placeholder: "*.secret or Clients/Private", maxlength: 512,
      "data-key": "exclusion-pattern", disabled: view.busy, oninput: event => {
        const wasValid = !!view.draft?.trim();
        view.draft = event.target.value;
        if (wasValid !== !!view.draft.trim()) app.render();
      } }),
    h("div", { class: "inline" },
      h("button", { type: "submit", class: "btn btn-primary", disabled: !valid || view.busy }, editing ? "Save exclusion" : "Add exclusion"),
      editing ? button("Cancel", () => { view.editing = null; view.draft = ""; view.error = null; app.render(); }, { disabled: view.busy }) : null));
}

export function render(app, embedded = false) {
  const view = app.view("privacy");
  const page = h(embedded ? "section" : "div", { class: embedded ? "section" : "page" },
    embedded ? h("header", { class: "section__header" }, h("h2", {}, title), h("p", {}, "These files stay private, even inside a shared folder.")) : pageHeader(title, "These files stay private, even inside a shared folder."));
  if (!app.status) { append(page, agentMissing(app)); return page; }
  if (view.error || view.loadError) append(page, notice(view.error || view.loadError));
  const deny = view.deny;
  if (!deny) { append(page, h("p", { class: "muted" }, "Loading…")); return page; }
  append(page, group(
    ...(deny.extra || []).map(pattern => h("div", { class: "row" },
      h("div", { class: "row__body" }, h("code", {}, pattern)),
      h("div", { class: "row__actions" },
        button("Edit", () => { view.editing = pattern; view.draft = pattern; view.error = null; app.render(); }, { disabled: view.busy, key: `edit-exclusion-${pattern}` }),
        button("Remove…", () => change(app, pattern, null), { kind: "btn-subtle-negative", disabled: view.busy, key: `remove-exclusion-${pattern}` })))),
    editor(app),
    h("details", { class: "row row--stack", open: !!view.builtinsOpen, ontoggle: event => { view.builtinsOpen = event.target.open; } },
      h("summary", {}, "Built-in protections"),
      h("p", {}, "Keys, passwords, mail and browser profiles stay private. These protections cannot be removed here."),
      patterns(deny.builtin || []),
      deny.removed?.length ? h("div", {}, h("h3", {}, "Changed in configuration"), h("p", {}, "These built-in patterns were removed manually and no longer apply."), patterns(deny.removed)) : null,
      h("p", { class: "note" }, deny.allow_hardlinks
        ? "Desktop's settings, keys, index and logs are protected. Hard links are allowed by your configuration."
        : "Desktop's settings, keys, index and logs, and files with multiple hard links, are also protected."))));
  return page;
}
export default { title, icon, render, enter, statusChanged };
