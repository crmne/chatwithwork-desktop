// First run: start the agent, pair, and choose what to share. Nothing is
// shared until the person clicks a button that says so.

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

// A numbered step with a check mark once it's done.
const step = (number, titleText, done, ...body) =>
  group(h("div", { class: done ? "step step--done" : "step" },
    h("span", { class: "step__mark", "aria-hidden": "true" }, done ? "✓" : String(number)),
    h("div", { class: "step__body" }, h("h2", {}, titleText, done ? h("span", { class: "sr-only" }, " (done)") : null), ...body)));

async function finish(app) {
  const view = app.view("welcome");
  const general = app.view("general");
  await call("set_onboarded", { onboarded: true });
  app.info.onboarded = true;
  if (general.autostart !== undefined && view.autostart !== general.autostart) await setAutostart(app, view.autostart);
  // Paired and sharing: straight into Chat with Work.
  if (app.status?.server && app.status.roots.length) call("show_main");
  app.go("folders");
}

export function render(app) {
  const view = app.view("welcome");
  const folders = app.view("folders");
  const crash = app.view("general").crash;
  const status = app.status;
  const paired = !!status?.server;
  const shared = !!status?.roots?.length;
  const busy = folders.adding || folders.picking;

  const steps = [
    step(1, "Run the Local Agent", !!status,
      status ? h("p", {}, "It's running in the background.") : agentMissing(app)),
    step(2, "Connect to Chat with Work", paired && !app.view("pairing").active,
      !status ? h("p", {}, "Start the Local Agent first.")
        : paired && !app.view("pairing").active ? h("p", {}, `Paired with ${host(status.server)}.`)
          : pairingSection(app, status)),
    step(3, "Choose what to share", shared,
      !status ? h("p", {}, "Start the Local Agent first.")
        : shared ? h("p", {}, `Sharing ${status.roots.map((r) => r.label).join(", ")}.`)
          : [
            folders.documents
              ? h("p", { class: "ink" }, `Share your Documents folder (${displayPath(folders.documents.path, app.info.home)})? Chat with Work will be able to search and read the files in it, except private ones such as keys and passwords.`)
              : h("p", {}, "Choose a folder for Chat with Work to search."),
            h("div", { class: "inline" },
              folders.documents ? button("Share Documents", () => addFolder(app, folders.documents.path), { kind: "btn-primary", disabled: busy }) : null,
              button("Choose Another Folder…", () => pickFolder(app), { disabled: busy })),
            folders.error ? notice(folders.error) : null,
          ]),
  ];
  if (crash?.available) {
    steps.push(step(4, "Crash reports", crash.enabled !== null && crash.enabled !== undefined,
      h("p", { class: "ink" }, crash.about),
      crash.enabled === null || crash.enabled === undefined
        ? h("div", { class: "inline" },
          button("Send Crash Reports", () => setCrashReports(app, true), { kind: "btn-primary" }),
          button("Don't Send", () => setCrashReports(app, false)))
        : h("p", {}, crash.enabled ? "Crash reports are on. You can change this under General." : "Crash reports are off. You can change this under General.")));
  }

  const done = paired && shared;
  return h("div", { class: "welcome" },
    h("div", { class: "welcome__hero" },
      h("img", { src: "mark.svg", alt: "" }),
      h("h1", {}, "Welcome to Chat with Work"),
      h("p", {}, "Chat with Work can search and read the folders you choose on this computer. Nothing is shared until you say so.")),
    ...steps,
    h("div", { class: "welcome__end" },
      h("label", { class: "check" },
        switchInput(view.autostart, (on) => {
          view.autostart = on;
          app.render();
        }, { label: "Start at login", key: "welcome-autostart" }),
        `Show Chat with Work in the ${app.info.tray_place} when you log in`),
      button(done ? "Done" : "Skip for Now", () => finish(app), { kind: done ? "btn-primary" : "" })));
}

export default { title, icon, render, enter, statusChanged };
