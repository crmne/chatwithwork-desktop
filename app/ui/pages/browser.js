// Browser controls stay local. Native code confirms wider access and applies
// the same site policy in the daemon and extension.
import { agentMissing, append, button, call, control, group, h, notice, pageHeader, section, switchInput } from "../app.js";

export const title = "Browser";
export const icon = "browser";
const EXTENSION_URL = "https://github.com/crmne/chatwithwork-desktop/tree/main/browser#readme";
const SITE = /^(https?:\/\/[^\s/?#]+\/?|[^\s/?#@:*\\]+\.[^\s/?#@:*\\]+|localhost)$/i;

async function perform(app, action) {
  const view = app.view("browser");
  view.busy = true;
  view.error = null;
  app.render();
  try { await action(); }
  catch (error) { if (error.code !== "declined") view.error = error.message; }
  view.busy = false;
  app.render();
}
const change = (app, request) => perform(app, () => control(request));

function useBrowser(app, browser, on) {
  if (on && browser.extension?.stopped) return;
  if (on && !browser.enabled) return perform(app, () => call("browser_enable", { enabled: true }));
  return change(app, { cmd: "browser_stopped", stopped: !on });
}

const setting = (titleText, text, control) =>
  h("div", { class: "row" }, h("div", { class: "row__body" }, h("h3", {}, titleText), text ? h("p", {}, text) : null), h("div", { class: "row__actions" }, control));

function connection(browser) {
  const ext = browser.extension;
  if (!ext) return h("div", { class: "row" },
    h("div", { class: "row__body" }, h("h3", {}, "No browser connected"), h("p", {}, "Install the extension in Chrome, Edge, Brave or Chromium.")),
    h("div", { class: "row__actions" }, button("Get extension", () => call("open_url", { url: EXTENSION_URL }))));
  const updateNeeded = (browser.all_sites || browser.blocked_sites.length > 0) && (ext.site_policy_version || 0) < 2;
  return h("div", { class: "row" }, h("div", { class: "row__body" },
    h("h3", {}, `${ext.browser || "Browser"} connected`,
      h("span", { class: `state state--${ext.stopped || browser.stopped || updateNeeded ? "attention" : "positive"}` }, updateNeeded ? "Update needed" : ext.stopped || browser.stopped ? "Stopped" : "Ready")),
    updateNeeded ? h("p", {}, "Update the browser extension to use this site policy. Browser access is paused until then.") : null),
    updateNeeded ? h("div", { class: "row__actions" }, button("Get extension", () => call("open_url", { url: EXTENSION_URL }))) : null);
}

function siteList(app, browser, blocked) {
  const view = app.view("browser");
  const key = blocked ? "blockedSite" : "allowedSite";
  const label = blocked ? "Blocked sites" : "Allowed sites";
  const entries = blocked ? browser.blocked_sites : browser.sites;
  const request = (site, present) => blocked
    ? { cmd: "browser_blocked_site", site, blocked: present }
    : { cmd: "browser_site", site, allowed: present };
  const valid = SITE.test((view[key] || "").trim());
  return [
    h("div", { class: "row row--stack" }, h("div", { class: "row__body" }, h("h3", {}, label)),
      !entries.length ? h("p", { class: "muted" }, blocked ? "No blocked sites." : "Add the sites Chat with Work may use.") : null),
    ...entries.map(site => h("div", { class: "row" },
      h("div", { class: "row__body" }, h("code", {}, site)),
      h("div", { class: "row__actions" }, button("Remove", () => change(app, request(site, false)), { disabled: view.busy, key: `${key}-remove-${site}` })))),
    h("form", { class: "row", "data-key": `${key}-form`, onsubmit: event => {
      event.preventDefault();
      const site = (view[key] || "").trim();
      if (view.busy || !SITE.test(site)) return;
      perform(app, async () => { await control(request(site, true)); view[key] = ""; });
    } },
      h("div", { class: "row__body" }, h("input", { class: "input", placeholder: "example.com", "aria-label": blocked ? "Site to block" : "Site to allow",
        value: view[key] || "", "data-key": key, disabled: view.busy, oninput: event => {
          const was = SITE.test((view[key] || "").trim());
          view[key] = event.target.value;
          if (was !== SITE.test(view[key].trim())) app.render();
        } })),
      h("div", { class: "row__actions" }, h("button", { type: "submit", class: "btn", disabled: !valid || view.busy }, blocked ? "Block" : "Allow")))
  ];
}

function sites(app, browser) {
  const view = app.view("browser");
  const mode = (all, label) => h("button", { type: "button", "data-key": `browser-mode-${all}`, "aria-pressed": String(browser.all_sites === all), disabled: view.busy,
    onclick: () => { if (browser.all_sites !== all) change(app, { cmd: "browser_all_sites", all_sites: all }); } }, label);
  return section("Sites", null, group(
    h("div", { class: "row row--stack" },
      h("div", { class: "tabs", role: "group", "aria-label": "Browser site access" }, mode(false, "Only these sites"), mode(true, "All sites except…")),
      h("p", { class: "note" }, "Blocked sites always take priority.")),
    ...(!browser.all_sites ? siteList(app, browser, false) : []),
    ...siteList(app, browser, true)));
}

export function render(app) {
  const view = app.view("browser");
  const page = h("div", { class: "page" }, pageHeader("Browser", "Use your browser through the extension, in its Chat with Work tab group."));
  if (!app.status) { append(page, agentMissing(app)); return page; }
  const browser = { sites: [], blocked_sites: [], all_sites: false, ask_before_actions: true, bots: [], ...app.status.browser };
  const stoppedByExtension = !!browser.extension?.stopped;
  const on = browser.enabled && !browser.stopped && !stoppedByExtension;
  append(page, group(
    setting("Use your browser", stoppedByExtension ? "Resume in the browser extension." : null,
      switchInput(on, enabled => useBrowser(app, browser, enabled), { label: "Use your browser", key: "use-browser", disabled: view.busy || stoppedByExtension })),
    browser.enabled ? connection(browser) : null,
    browser.enabled ? setting("Allow actions", "Click, type and navigate on permitted sites.",
      switchInput(browser.changes, changes => change(app, { cmd: "browser_changes", changes }), { label: "Allow actions in the browser", key: "browser-actions", disabled: view.busy })) : null,
    browser.enabled ? setting("Ask before browser actions", browser.ask_before_actions
      ? "Approve actions in Chat with Work."
      : "Actions run without asking, including sending, buying and deleting.",
      switchInput(browser.ask_before_actions, ask => change(app, { cmd: "browser_ask_before_actions", ask }),
        { label: "Ask before browser actions", key: "browser-approvals", disabled: view.busy || !browser.changes })) : null));
  if (browser.enabled) append(page, sites(app, browser),
    browser.bots.length ? section("Shared with bots", "Choose access in each bot’s settings.",
      h("p", {}, browser.bots.map(bot => bot.name || `Bot ${bot.id}`).join(", "))) : null);
  if (view.error) append(page, notice(view.error));
  return page;
}
export default { title, icon, render };
