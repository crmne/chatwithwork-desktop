// Runs only in the app-owned child webview in an existing bot integration.
// Scope/name come from the paired daemon's authenticated settings context.
import { agentMissing, append, control, group, h, notice, pageHeader, switchInput } from "../app.js";

export const title = "Share with bot";
export const icon = "folder-simple";

async function change(app, request) {
  const view = app.view("bot-sharing");
  view.busy = true;
  view.error = null;
  app.render();
  try {
    await control(request);
  } catch (error) {
    view.error = error.message;
  }
  view.busy = false;
  app.render();
}

export function render(app) {
  const view = app.view("bot-sharing");
  const bot = app.context?.bot;
  const page = h("div", { class: "page" }, pageHeader("Share this computer", bot
    ? `Choose what ${bot.name} can use. This computer asks before sharing more.`
    : "Open this from a bot’s computer integration."));
  if (!app.status) { append(page, agentMissing(app)); return page; }
  if (!bot) return page;
  const id = String(bot.id);
  const folders = app.status.roots.map(root => {
    const grant = (root.bots || []).find(grant => String(grant.id) === id);
    const set = (lent, unattended = false) => change(app, { cmd: "roots_bot", root: root.id, bot: id, name: bot.name, lent, unattended });
    return h("div", { class: "row row--stack" },
      h("div", { class: "row__head" }, h("div", { class: "row__body" },
        h("h3", {}, root.label),
        h("p", {}, root.writable ? "Can read and change files with approval." : "Read-only.")),
      switchInput(!!grant, on => set(on), { label: `Share ${root.label} with ${bot.name}`, disabled: view.busy })),
      grant && root.allows_commands ? h("div", { class: "row__head" }, h("div", { class: "row__body" },
        h("h3", {}, "Unattended commands"),
        h("p", {}, "Commands can start without a person approving each one. The folder’s sandbox and network limits still apply.")),
      switchInput(grant.unattended, on => set(true, on), { label: `Unattended commands in ${root.label}`, disabled: view.busy })) : null);
  });
  append(page, group(...(folders.length ? folders : [h("div", { class: "empty" },
    h("p", {}, "Add a shared folder in Settings, This computer, Shared folders first."))])));
  const browser = app.status.browser;
  if (browser?.enabled) {
    const shared = browser.bots.some(grant => String(grant.id) === id);
    append(page, group(h("div", { class: "row" }, h("div", { class: "row__body" },
      h("h3", {}, "Browser"), h("p", {}, "Uses the allowed sites in your Chat with Work tab group. Changes still ask for approval.")),
    switchInput(shared, lent => change(app, { cmd: "browser_bot", bot: id, name: bot.name, lent }),
      { label: `Share browser with ${bot.name}`, disabled: view.busy }))));
  }
  if (view.error) append(page, notice(view.error));
  return page;
}

export default { title, icon, render };
