// First run opens the chat. Pairing and sharing folders are optional,
// explicit choices, available here and later in this computer's settings.

import { agentMissing, button, call, group, h, host, notice, switchInput } from "../app.js";
import { pairingSection } from "./account.js";
import { addFolder, displayPath, documentsToOffer, pickFolder } from "./folders.js";
import { load as loadGeneral, setAutostart, setCrashReports } from "./general.js";

export const title = "Welcome";
export const icon = "hand-waving";

export function enter(app) {
  const view = app.view("welcome");
  view.autostart ??= true;
  loadGeneral(app);
  documentsToOffer(app).then((documents) => {
    app.view("folders").documents = documents;
    if (app.page === "welcome") app.render();
  });
}

export function statusChanged(app) {
  if (app.page === "welcome") enter(app);
}

const step = (number, titleText, done, ...body) =>
  group(h("div", { class: done ? "step step--done" : "step" },
    h("span", { class: "step__mark", "aria-hidden": "true" }, done ? "✓" : String(number)),
    h("div", { class: "step__body" }, h("h2", {}, titleText, done ? h("span", { class: "sr-only" }, " (done)") : null), ...body)));

async function finish(app) {
  const view = app.view("welcome");
  const general = app.view("general");
  if (view.finishing) return;
  view.finishing = true;
  view.error = null;
  app.render();
  try {
    if (general.autostart !== undefined && view.autostart !== general.autostart) {
      await setAutostart(app, view.autostart);
      if (general.error) throw { message: general.error };
    }
    // Chat works without pairing this computer or sharing any folders.
    await call("show_main");
    await call("set_onboarded", { onboarded: true });
    app.info.onboarded = true;
    app.go("folders");
  } catch (error) {
    view.error = error.message;
  } finally {
    view.finishing = false;
    app.render();
  }
}

export function render(app) {
  const view = app.view("welcome");
  const folders = app.view("folders");
  const general = app.view("general");
  const crash = general.crash;
  const status = app.status;
  const paired = !!status?.server;
  const shared = !!status?.roots?.length;
  const busy = folders.adding || folders.picking;
  const pairing = app.view("pairing");

  return h("div", { class: "welcome" },
    h("div", { class: "welcome__hero" },
      h("img", { src: "mark.svg", alt: "" }),
      h("h1", {}, "Welcome to Chat with Work Desktop"),
      h("p", {}, "Your chats and work tools, together in one app. Sign in to pick up where you left off."),
      button(view.finishing ? "Opening…" : "Open Chat with Work", () => finish(app), {
        kind: "btn-primary", key: "welcome-open", disabled: view.finishing || pairing.active || busy,
      }),
      pairing.active ? h("p", { class: "note" }, "Finish or cancel pairing below to continue.") : null,
      view.error ? notice(view.error) : null),
    h("details", { class: "welcome__sharing", open: !!view.sharing, ontoggle: (event) => { view.sharing = event.target.open; } },
      h("summary", {},
        h("span", {}, "Use files from this computer"),
        h("span", { class: "micro" }, "Optional")),
      h("div", { class: "welcome__steps" },
        h("p", { class: "muted" }, "Share only the folders you choose. You can set this up now or later in Shared Folders."),
        step(1, "Connect this computer", paired && !pairing.active,
          !status ? agentMissing(app)
            : paired && !pairing.active ? h("p", {}, `Connected to ${host(status.server)}.`)
              : pairingSection(app, status)),
        step(2, "Choose a folder", shared,
          !status ? h("p", {}, "Start the background agent above to choose a folder.")
            : shared ? h("p", {}, `Sharing ${status.roots.map((r) => r.label).join(", ")}. You can stop sharing at any time.`)
              : [
                folders.documents
                  ? h("p", {}, `Share Documents (${displayPath(folders.documents.path, app.info.home)}) or choose another folder. Chat with Work can search and read it; private files such as keys and passwords stay excluded.`)
                  : h("p", {}, "Choose a folder for Chat with Work to search and read. Private files such as keys and passwords stay excluded."),
                h("div", { class: "inline" },
                  folders.documents ? button("Share Documents", () => addFolder(app, folders.documents.path), { disabled: busy }) : null,
                  button(folders.documents ? "Choose Another Folder…" : "Choose a Folder…", () => pickFolder(app), { disabled: busy })),
                folders.error ? notice(folders.error) : null,
              ]))),
    crash?.available ? group(h("div", { class: "row row--stack" },
      h("div", { class: "row__body" }, h("h2", {}, "Help improve Chat with Work"), h("p", {}, crash.about)),
      crash.enabled === null || crash.enabled === undefined
        ? h("div", { class: "inline" },
          button("Send Crash Reports", () => setCrashReports(app, true)),
          button("Don't Send", () => setCrashReports(app, false)))
        : h("p", { class: "muted" }, crash.enabled ? "Crash reports are on. You can change this under General." : "Crash reports are off. You can change this under General."),
      general.crashError ? notice(general.crashError) : null)) : null,
    h("div", { class: "welcome__end" },
      h("label", { class: "check" },
        switchInput(view.autostart, (on) => {
          view.autostart = on;
          app.render();
        }, { label: "Start at login", key: "welcome-autostart", disabled: view.finishing }),
        `Show Chat with Work in the ${app.info.tray_place} when you log in`)));
}

export default { title, icon, render, enter, statusChanged };
