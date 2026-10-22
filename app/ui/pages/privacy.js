// The deny list, as information: what is never shared, even inside a
// shared folder. It can only be changed in config.toml, on purpose.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader } from "../app.js";

export const title = "Always Private";
export const icon = "lock-simple";

const patterns = (list, removed = false) =>
  h("div", { class: removed ? "patterns patterns--removed" : "patterns" }, list.map((pattern) => h("code", {}, pattern)));

async function load(app) {
  const view = app.view("privacy");
  view.loading = true;
  try {
    view.deny = await control({ cmd: "deny" });
    view.error = null;
  } catch (error) {
    view.error = error.message;
  }
  view.loading = false;
  if (app.page === "privacy") app.render();
}

// Reloaded on every visit and change: an edited config.toml can change it.
export const enter = load;

export function statusChanged(app) {
  if (app.page === "privacy") load(app);
}

export function render(app) {
  const view = app.view("privacy");
  const page = h("div", { class: "page" },
    pageHeader("Always Private", "These files are never shared, even inside a shared folder. Chat with Work can't change this list."));
  if (!app.status) {
    append(page, agentMissing(app));
    return page;
  }
  if (view.error) {
    append(page, notice(view.error));
    return page;
  }
  const deny = view.deny;
  if (!deny) {
    append(page, h("p", { class: "muted" }, "Loading…"));
    return page;
  }
  append(page, 
    group(
      h("div", { class: "row row--stack" }, h("div", { class: "row__body" },
        h("h3", {}, "Built in"),
        h("p", {}, "Keys, credentials, password stores, mail, messages and browser profiles. A pattern matches a file or folder name anywhere in a path."),
        patterns(deny.builtin || []))),
      deny.extra?.length ? h("div", { class: "row row--stack" }, h("div", { class: "row__body" },
        h("h3", {}, "Added by you"), patterns(deny.extra))) : null,
      deny.removed?.length ? h("div", { class: "row row--stack" }, h("div", { class: "row__body" },
        h("h3", {}, "Removed by you"), h("p", {}, "These built-in patterns no longer apply."), patterns(deny.removed, true))) : null,
      h("div", { class: "row" }, h("div", { class: "row__body" },
        h("h3", {}, "Also private"),
        h("p", {}, deny.allow_hardlinks
          ? "The Local Agent's own settings, keys, index and logs. Files with more than one hard link are allowed by your configuration."
          : "The Local Agent's own settings, keys, index and logs, and files with more than one hard link, which could point outside a shared folder.")))),
    h("div", { class: "section" },
      h("p", { class: "muted" }, "To change this list, edit the ", h("code", {}, "[deny]"), " section of config.toml, and it applies right away."),
      h("div", { class: "inline" }, button("Open config.toml", () => call("open_file", { which: "config" }).catch((error) => {
        view.error = error.message;
        app.render();
      })))));
  return page;
}

export default { title, icon, render, enter, statusChanged };
