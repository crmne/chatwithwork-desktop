// Shared folders: add with the native picker, rename, remove, allow changes
// and commands, and see each folder's index state.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader, switchInput, thousands } from "../app.js";

export const title = "Shared Folders";
export const icon = "folder-simple";

// The index state of a folder, as a label and a tone.
export function indexState(root) {
  if (!root.available) return ["Folder not found", "attention"];
  switch (root.index) {
    case "ready": {
      const files = root.indexed_files || 0;
      return [`Indexed · ${thousands(files)} ${files === 1 ? "file" : "files"}`, "positive"];
    }
    case "indexing": return ["Indexing…", "live"];
    case "pending": return ["Waiting to index", ""];
    case "error": return ["Index error", "negative"];
    case "disabled": return ["Searched live", ""];
    default: return [root.index || "", ""];
  }
}

// Home as ~, as the egui app showed paths.
export function displayPath(path, home) {
  if (!path) return "";
  if (home && (path === home || path.startsWith(home + "/") || path.startsWith(home + "\\"))) {
    return "~" + path.slice(home.length);
  }
  return path;
}

// The Documents folder, if it exists and isn't shared yet.
export async function documentsToOffer(app) {
  try {
    const { suggestions } = await control({ cmd: "suggested_roots" });
    return (suggestions || []).find((s) => s.exists && !s.shared) || null;
  } catch {
    return null;
  }
}

// Adds a folder and says what happened. Shared with the welcome page.
export async function addFolder(app, path) {
  const view = app.view("folders");
  view.adding = true;
  view.error = null;
  view.note = null;
  app.render();
  try {
    const answer = await control({ cmd: "roots_add", path });
    view.note = `Sharing “${answer.root?.label}”. Chat with Work can search it once it's indexed.`;
    view.documents = undefined;
  } catch (error) {
    view.error = error.message;
  }
  view.adding = false;
  app.render();
}

export async function pickFolder(app) {
  const view = app.view("folders");
  view.picking = true;
  app.render();
  try {
    const { path } = await call("pick_folder");
    view.picking = false;
    if (path) await addFolder(app, path);
  } catch (error) {
    view.error = error.message;
  }
  view.picking = false;
  app.render();
}

async function change(app, id, request) {
  const view = app.view("folders");
  view.busy = id;
  view.error = null;
  app.render();
  try {
    await control(request);
  } catch (error) {
    view.error = error.message;
  }
  view.busy = null;
  app.render();
}

export function enter(app) {
  const view = app.view("folders");
  documentsToOffer(app).then((documents) => {
    view.documents = documents;
    if (app.page === "folders") app.render();
  });
}

export function statusChanged(app) {
  if (app.page === "folders") enter(app);
}

function folderRow(app, root) {
  const view = app.view("folders");
  const busy = view.busy === root.id;
  const renaming = view.renaming?.id === root.id;
  const removing = view.removing === root.id;
  const allowing = view.allowing === root.id;
  const commanding = view.commanding === root.id;
  const [indexLabel, tone] = indexState(root);
  const unix = app.info.platform !== "windows";

  const setting = (titleText, text, control) =>
    h("div", { class: "row" }, h("div", { class: "row__body" }, h("h3", {}, titleText), h("p", {}, text)), h("div", { class: "row__actions" }, control));

  return h("div", { class: "row row--stack" },
    h("div", { class: "row__head" },
      h("div", { class: "row__body" },
        h("h3", {}, root.label, h("span", { class: `state state--${tone}` }, indexLabel)),
        h("span", { class: "micro truncate", title: `Chat with Work sees this folder as “${root.id}:”` },
          displayPath(root.local_path, app.info.home))),
      !renaming && !removing ? h("div", { class: "row__actions" },
        button("Rename…", () => {
          view.renaming = { id: root.id, label: root.label };
          view.removing = null;
          app.render();
          document.querySelector(`[data-key="rename-${CSS.escape(root.id)}"]`)?.select();
        }, { disabled: busy }),
        button("Remove…", () => {
          view.removing = root.id;
          view.renaming = null;
          app.render();
        }, { disabled: busy })) : null),

    renaming ? h("form", { class: "inline", onsubmit: (event) => {
      event.preventDefault();
      const label = view.renaming.label.trim();
      view.renaming = null;
      if (label && label !== root.label) change(app, root.id, { cmd: "roots_label", root: root.id, label });
      else app.render();
    } },
      h("input", { class: "input", value: view.renaming.label, maxLength: 80, placeholder: "Label", "aria-label": "Label",
        "data-key": `rename-${root.id}`, oninput: (event) => (view.renaming.label = event.target.value),
        onkeydown: (event) => {
          if (event.key === "Escape") {
            view.renaming = null;
            app.render();
          }
        } }),
      button("Cancel", () => {
        view.renaming = null;
        app.render();
      }),
      h("button", { type: "submit", class: "btn btn-primary" }, "Save")) : null,

    removing ? h("div", { class: "confirm confirm--negative" },
      h("p", {}, `Stop sharing “${root.label}”? Chat with Work will no longer be able to search or read these files. Nothing on this computer is deleted.`),
      h("div", { class: "inline" },
        button("Cancel", () => {
          view.removing = null;
          app.render();
        }),
        button("Stop Sharing", () => {
          view.removing = null;
          change(app, root.id, { cmd: "roots_remove", root: root.id });
        }, { kind: "btn-subtle-negative" }))) : null,

    h("div", { class: "group group--sunken" },
      // Allow changes: on asks first, off is at once.
      setting("Allow changes", root.writable
        ? "Chat with Work can create, edit, move and delete files here, each after you approve it. Old versions and deleted files go to the trash."
        : "Read-only. Turn on to let Chat with Work create, edit, move and delete files here, each after you approve it.",
      switchInput(root.writable || allowing, (on) => {
        if (on) {
          view.allowing = root.id;
          view.removing = null;
          app.render();
        } else {
          change(app, root.id, { cmd: "roots_writable", root: root.id, writable: false });
        }
      }, { label: `Allow changes in ${root.label}`, disabled: busy || allowing })),
      allowing ? h("div", { class: "confirm" },
        h("p", {}, `Let Chat with Work change files in “${root.label}”? It asks you in Chat with Work before each change, and old versions and deleted files go to the trash on this computer.`),
        h("div", { class: "inline" },
          button("Cancel", () => {
            view.allowing = null;
            app.render();
          }),
          button("Allow Changes", () => {
            view.allowing = null;
            change(app, root.id, { cmd: "roots_writable", root: root.id, writable: true });
          }, { kind: "btn-primary" }))) : null,

      // Run commands: only where changes are allowed, and not on Windows.
      root.writable && unix ? setting("Run commands", root.allows_commands
        ? "Chat with Work can run commands here, like tests or a build, each after you approve it, in a sandbox that can change only this folder."
        : "Turn on to let Chat with Work run commands here, like tests or a build, each after you approve it.",
      switchInput(root.allows_commands || commanding, (on) => {
        if (on) {
          view.commanding = root.id;
          view.removing = null;
          app.render();
        } else {
          change(app, root.id, { cmd: "roots_commands", root: root.id, commands: false, network: false });
        }
      }, { label: `Run commands in ${root.label}`, disabled: busy || commanding })) : null,
      commanding ? h("div", { class: "confirm" },
        h("p", {}, `Let Chat with Work run commands in “${root.label}”? It asks you in Chat with Work before each one. A command can change or delete anything in this folder, and nothing outside it.`),
        h("div", { class: "inline" },
          button("Cancel", () => {
            view.commanding = null;
            app.render();
          }),
          button("Allow Commands", () => {
            view.commanding = null;
            change(app, root.id, { cmd: "roots_commands", root: root.id, commands: true, network: false });
          }, { kind: "btn-primary" }))) : null,
      root.writable && unix && root.allows_commands ? setting("Commands use the network", root.network
        ? "Commands here can reach the internet, to install packages or fetch dependencies."
        : "Commands here run without a network.",
      switchInput(root.network, (on) => change(app, root.id, { cmd: "roots_commands", root: root.id, commands: true, network: on }),
        { label: `Commands in ${root.label} use the network`, disabled: busy })) : null));
}

export function render(app) {
  const view = app.view("folders");
  const status = app.status;
  const page = h("div", { class: "page" },
    pageHeader("Shared Folders", "Chat with Work can search and read the files in these folders, and nothing else on this computer. It changes files only in folders where you allow it."));
  if (!status) {
    append(page, agentMissing(app));
    return page;
  }
  const busy = view.adding || view.picking;
  append(page, 
    status.roots.length === 0
      ? group(h("div", { class: "empty" },
        h("strong", {}, "Nothing is shared yet"),
        h("p", {}, "Add a folder to let Chat with Work search it. Share narrow folders: anything inside can be read, except the private files listed under Always Private.")))
      : group(...status.roots.map((root) => folderRow(app, root))),
    h("div", { class: "section" },
      h("div", { class: "inline" },
        button(view.adding ? "Adding…" : "Add Folder…", () => pickFolder(app), { kind: "btn-primary", disabled: busy }),
        view.documents ? button("Share Documents", () => addFolder(app, view.documents.path), { disabled: busy }) : null),
      view.error ? notice(view.error) : null,
      view.note ? notice(view.note, "positive") : null,
      h("p", { class: "note" }, "Labels are what Chat with Work sees. It never sees where a folder is on this computer.")));
  return page;
}

export default { title, icon, render, enter, statusChanged };
