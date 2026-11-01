// Printers Chat with Work may use: this computer's own, chosen here, never
// from the server. Each bot's printers are in that bot's settings.

import { agentMissing, append, control, group, h, notice, pageHeader, switchInput } from "../app.js";

export const title = "Printers";
export const icon = "printer";

async function load(app) {
  const view = app.view("printers");
  try {
    const answer = await control({ cmd: "printers" });
    view.system = answer.system;
    view.left = answer.pages_left_today;
    view.limit = answer.pages_per_day;
    view.error = null;
  } catch (error) {
    view.system = [];
    view.error = error.message;
  }
  app.render();
}

async function change(app, request) {
  const view = app.view("printers");
  view.busy = true;
  view.error = null;
  app.render();
  try {
    await control(request);
    await load(app);
  } catch (error) {
    view.error = error.message;
  }
  view.busy = false;
  app.render();
}

export function render(app) {
  const view = app.view("printers");
  const page = h("div", { class: "page page--printers" }, pageHeader("Printers",
    "Chats can print PDFs on the printers you allow. You approve each job in Chat with Work."));
  if (!app.status) { append(page, agentMissing(app)); return page; }
  if (view.system === undefined) { view.system = null; load(app); }
  const allowed = app.status.printers || [];
  const rows = (view.system || []).map(printer => {
    const found = allowed.find(entry => entry.queue === printer.queue);
    const shared = found?.bots?.length || 0;
    return h("div", { class: "row" },
      h("div", { class: "row__body" },
        h("h3", {}, found?.label || printer.queue.replaceAll("_", " ")),
        h("p", {}, [printer.state === "stopped" ? "Stopped" : "Ready",
          printer.duplex ? "Both sides" : null,
          shared ? `Shared with ${shared} ${shared === 1 ? "bot" : "bots"}` : null].filter(Boolean).join(" · "))),
      switchInput(!!found?.chats, on => change(app, on
        ? { cmd: "printers_add", queue: printer.queue, chats: true }
        : shared ? { cmd: "printers_add", queue: printer.queue, chats: false } : { cmd: "printers_remove", printer: found.id }),
      { label: `Let chats print on ${printer.queue}`, disabled: view.busy }));
  });
  if (view.system) {
    append(page, group(...(rows.length ? rows : [h("div", { class: "empty" },
      h("p", {}, "This computer has no printers yet. Add one in its system settings."))])));
  }
  if (view.limit) {
    append(page, h("p", { class: "note" }, `${view.left} of ${view.limit} pages left today, the most this computer prints in a day.`));
  }
  if (view.error) append(page, notice(view.error));
  return page;
}

export default { title, icon, render };
