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
  const runsCommands = commandsHere(app);

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

      // Run commands: only where changes are allowed, and on Windows only
      // where the daemon says this Windows can. A folder that allows them
      // keeps its switch even then, so they can be turned off.
      root.writable && !runsCommands && !root.allows_commands ? setting("Run commands", commandsUnavailable(app), null) : null,
      root.writable && (runsCommands || root.allows_commands) ? setting("Run commands", root.allows_commands
        ? "Chat with Work can run commands here, like tests or a build, in a sandbox that can change only this folder. Each is approved in Chat with Work, unless a chat allows everything."
        : "Turn on to let Chat with Work run commands here, like tests or a build, each approved in Chat with Work.",
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
        h("p", {}, `Let Chat with Work run commands in “${root.label}”? Each one is approved in Chat with Work, or runs without asking in a chat where you chose Allow everything. A command can change or delete anything in this folder, and nothing outside it.`),
        h("div", { class: "inline" },
          button("Cancel", () => {
            view.commanding = null;
            app.render();
          }),
          button("Allow Commands", () => {
            view.commanding = null;
            change(app, root.id, { cmd: "roots_commands", root: root.id, commands: true, network: false });
          }, { kind: "btn-primary" }))) : null,
      root.writable && root.allows_commands ? setting("Commands use the network", root.network
        ? "Commands here can reach the internet, to install packages or fetch dependencies."
        : "Commands here run without a network.",
      switchInput(root.network, (on) => change(app, root.id, { cmd: "roots_commands", root: root.id, commands: true, network: on }),
        { label: `Commands in ${root.label} use the network`, disabled: busy })) : null,
      bots(app, root, busy)));
}

// Whether commands can run here: everywhere but Windows, and there where
// the daemon says so (Windows 11 24H2 and later, docs/commands.md).
function commandsHere(app) {
  return app.info.platform !== "windows" || !!app.status?.commands?.available;
}

function commandsUnavailable(app) {
  const detail = app.status?.commands?.detail;
  return detail ? `${detail[0].toUpperCase()}${detail.slice(1)}.` : "Commands can't run on this computer.";
}

// "drive C:", "drives C: and D:".
function driveNames(drives) {
  const names = drives.map((drive) => String(drive.root).replace(/\\$/, ""));
  if (names.length === 1) return `drive ${names[0]}`;
  return `drives ${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

// Windows keeps the top of a drive from sandboxes, and Node.js, npm and
// cmd's dir look at it first: an administrator can let them see it
// (local.rs, prepare_command_drives).
function drivesRow(app, drives) {
  const view = app.view("folders");
  const names = driveNames(drives);
  return h("div", { class: "row" },
    h("div", { class: "row__body" },
      h("h3", {}, "Node.js and cmd in commands"),
      h("p", {}, `Node.js, npm and cmd's dir look at the top of ${names} first, which Windows keeps from sandboxes, so they fail in commands here. Let sandboxed programs read the details of ${names}, never what's in it? Windows asks for an administrator.`)),
    h("div", { class: "row__actions" },
      button(view.preparing ? "Allowing…" : "Allow…", async () => {
        view.preparing = true;
        view.error = null;
        view.note = null;
        app.render();
        try {
          await call("prepare_command_drives");
          view.note = `Commands can now see the top of ${names}, so Node.js, npm and cmd's dir work there.`;
        } catch (error) {
          view.error = error.message;
        }
        view.preparing = false;
        app.render();
      }, { disabled: !!view.preparing })));
}

// What lending a folder lets a bot do, in the words CONTROL.md asks for.
function lendQuestion(root) {
  const can = ["read the files in it"];
  if (root.writable) can.push("change them");
  if (root.writable && root.allows_commands) can.push("run commands");
  const reach = can.length > 1 ? `${can.slice(0, -1).join(", ")} and ${can[can.length - 1]}` : can[0];
  return `Lend “${root.label}” to this bot? Its chats can ${reach} here, and reach no other folder on this computer. Each change and command asks in Chat with Work first, unless the bot's own settings there let it run. You can stop lending at any time.`;
}

// Bots this folder is lent to (PR #18's roots_bot): one control request
// lends, changes unattended commands, and stops lending.
function bots(app, root, busy) {
  const view = app.view("folders");
  const loans = root.bots || [];
  const lending = view.lending?.root === root.id ? view.lending : null;
  const unattending = view.unattending?.root === root.id ? view.unattending : null;
  const commands = root.writable && root.allows_commands;
  const loanName = (loan) => loan.name || `Bot ${loan.id}`;
  const rows = [
    h("div", { class: "row" },
      h("div", { class: "row__body" },
        h("h3", {}, "Lent to bots"),
        h("p", {}, loans.length
          ? "These Chat with Work bots can use this folder, and no other on this computer."
          : "A Chat with Work bot reaches this computer only through folders you lend it here.")),
      !lending ? h("div", { class: "row__actions" }, button("Lend to a Bot…", () => {
        view.lending = { root: root.id, bot: "", name: "" };
        app.render();
        document.querySelector(`[data-key="bot-id-${CSS.escape(root.id)}"]`)?.focus();
      }, { disabled: busy })) : null),
  ];
  for (const loan of loans) {
    rows.push(h("div", { class: "row" },
      h("div", { class: "row__body" },
        h("h3", {}, loanName(loan)),
        h("span", { class: "micro" }, `Bot ${loan.id}${loan.unattended ? " · commands start without approval" : ""}`)),
      h("div", { class: "row__actions" },
        commands ? h("label", { class: "check" },
          switchInput(loan.unattended || unattending?.bot === loan.id, (on) => {
            if (on) {
              view.unattending = { root: root.id, bot: loan.id };
              app.render();
            } else {
              change(app, root.id, { cmd: "roots_bot", root: root.id, bot: loan.id, lent: true, unattended: false });
            }
          }, { label: `Unattended commands for ${loanName(loan)}`, disabled: busy || !!unattending }),
          "Unattended") : null,
        button("Stop Lending", () =>
          change(app, root.id, { cmd: "roots_bot", root: root.id, bot: loan.id, lent: false }),
        { kind: "btn-subtle-negative", disabled: busy }))));
    if (unattending?.bot === loan.id) {
      rows.push(h("div", { class: "confirm" },
        h("p", {}, `Let ${loanName(loan)}'s commands in “${root.label}” start without anyone approving each one? They run in the same sandbox and limits as any command here, and can change or delete anything in this folder, and nothing outside it.`),
        h("div", { class: "inline" },
          button("Cancel", () => {
            view.unattending = null;
            app.render();
          }),
          button("Allow Unattended Commands", () => {
            view.unattending = null;
            change(app, root.id, { cmd: "roots_bot", root: root.id, bot: loan.id, lent: true, unattended: true });
          }, { kind: "btn-primary" }))));
    }
  }
  if (lending) {
    const id = lending.bot.trim();
    const valid = /^[0-9]{1,20}$/.test(id);
    rows.push(h("form", { class: "confirm", onsubmit: (event) => {
      event.preventDefault();
      if (!valid) return;
      view.lending = null;
      change(app, root.id, { cmd: "roots_bot", root: root.id, bot: id, name: lending.name.trim() || undefined, lent: true, unattended: false });
    } },
      h("div", { class: "inline" },
        h("input", { class: "input", inputMode: "numeric", placeholder: "Bot ID, like 42", "aria-label": "Bot ID",
          value: lending.bot, "data-key": `bot-id-${root.id}`, oninput: (event) => {
            const wasValid = /^[0-9]{1,20}$/.test(lending.bot.trim());
            lending.bot = event.target.value;
            if (wasValid !== /^[0-9]{1,20}$/.test(lending.bot.trim())) app.render();
          } }),
        h("input", { class: "input", placeholder: "Name, to recognize it (optional)", "aria-label": "Bot name", maxLength: 80,
          value: lending.name, "data-key": `bot-name-${root.id}`, oninput: (event) => (lending.name = event.target.value) })),
      h("p", { class: "note" }, "The bot's ID is the number in its address in Chat with Work (/bots/42); its page shows it."),
      h("p", {}, lendQuestion(root)),
      h("div", { class: "inline" },
        button("Cancel", () => {
          view.lending = null;
          app.render();
        }),
        h("button", { type: "submit", class: "btn btn-primary", disabled: !valid }, "Lend Folder"))));
  }
  return rows;
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
  const unprepared = commandsHere(app) && status.roots.some((root) => root.allows_commands)
    ? (status.commands?.drives || []).filter((drive) => !drive.prepared)
    : [];
  append(page,
    status.roots.length === 0
      ? group(h("div", { class: "empty" },
        h("strong", {}, "Nothing is shared yet"),
        h("p", {}, "Add a folder to let Chat with Work search it. Share narrow folders: anything inside can be read, except the private files listed under Always Private.")))
      : group(...status.roots.map((root) => folderRow(app, root))),
    unprepared.length ? group(drivesRow(app, unprepared)) : null,
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
