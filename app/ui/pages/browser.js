// Your browser (docs/browser.md): turn it on, see the browser the extension
// connected from, allow actions, stop it, and the sites and bots it's for.
// Turning it on goes through `browser_enable`, since only `cww` registers
// itself with the browsers; the rest are control requests.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader, section, switchInput } from "../app.js";

export const title = "Your Browser";
export const icon = "browser";

const EXTENSION_URL = "https://github.com/crmne/chatwithwork-desktop/tree/main/browser#readme";
const SITE = /^(https?:\/\/[^\s/?#]+\/?|[^\s/?#@:*\\]+\.[^\s/?#@:*\\]+|localhost)$/i;

async function change(app, request) {
  const view = app.view("browser");
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

async function enable(app, enabled) {
  const view = app.view("browser");
  view.busy = true;
  view.error = null;
  view.note = null;
  app.render();
  try {
    await call("browser_enable", { enabled });
    view.note = enabled ? "Chat with Work Desktop is registered with your browsers. Install the extension if you haven't." : null;
  } catch (error) {
    view.error = error.message;
  }
  view.busy = false;
  app.render();
}

const setting = (titleText, text, control) =>
  h("div", { class: "row" }, h("div", { class: "row__body" }, h("h3", {}, titleText), h("p", {}, text)), h("div", { class: "row__actions" }, control));

function connection(browser) {
  const ext = browser.extension;
  if (!ext) {
    return h("div", { class: "row" },
      h("div", { class: "row__body" },
        h("h3", {}, "No browser connected", h("span", { class: "state state--attention" }, "Waiting")),
        h("p", {}, "Install the Chat with Work extension in Chrome, Edge, Brave or Chromium. It connects here when the browser is open.")),
      h("div", { class: "row__actions" }, button("Get the Extension", () => call("open_url", { url: EXTENSION_URL }))));
  }
  const name = ext.browser || "A browser";
  return h("div", { class: "row" },
    h("div", { class: "row__body" },
      h("h3", {}, `${name} is connected`,
        ext.stopped
          ? h("span", { class: "state state--attention" }, "Stopped in the extension")
          : h("span", { class: "state state--positive" }, "Ready")),
      h("span", { class: "micro" }, `Extension ${ext.version || ""}`.trim())));
}

function sites(app, browser) {
  const view = app.view("browser");
  const entry = (view.site ||= "");
  const valid = SITE.test(entry.trim());
  const rows = browser.sites.map((site) =>
    h("div", { class: "row" },
      h("div", { class: "row__body" }, h("h3", {}, site),
        h("span", { class: "micro" }, site.includes("://") ? "Exactly this address" : "With its subdomains, over HTTPS")),
      h("div", { class: "row__actions" },
        button("Remove", () => change(app, { cmd: "browser_site", site, allowed: false }), { kind: "btn-subtle-negative", disabled: view.busy }))));
  return section("Allowed sites", "Chat with Work opens, reads and acts only on these sites. Every other page is refused, in the extension and here.",
    group(
      ...(rows.length ? rows : [h("div", { class: "empty" }, h("strong", {}, "No sites yet"), h("p", {}, "Allow the sites Chat with Work may use, like github.com."))]),
      h("form", { class: "row", onsubmit: (event) => {
        event.preventDefault();
        if (!valid) return;
        view.site = "";
        change(app, { cmd: "browser_site", site: entry.trim(), allowed: true });
      } },
        h("div", { class: "row__body" },
          h("input", { class: "input", placeholder: "github.com, or http://localhost:3000", "aria-label": "Site to allow",
            value: entry, "data-key": "browser-site", oninput: (event) => {
              const was = SITE.test(view.site.trim());
              view.site = event.target.value;
              if (was !== SITE.test(view.site.trim())) app.render();
            } })),
        h("div", { class: "row__actions" },
          h("button", { type: "submit", class: "btn btn-primary", disabled: !valid || view.busy }, "Allow Site")))));
}

function bots(app, browser) {
  const view = app.view("browser");
  const lending = view.lending;
  const loanName = (loan) => loan.name || `Bot ${loan.id}`;
  const rows = [
    h("div", { class: "row" },
      h("div", { class: "row__body" },
        h("h3", {}, "Lent to bots"),
        h("p", {}, browser.bots.length
          ? "These Chat with Work bots can use your browser too, on the same sites, each change asking in Chat with Work."
          : "A Chat with Work bot uses your browser only if you lend it here.")),
      !lending ? h("div", { class: "row__actions" }, button("Lend to a Bot…", () => {
        view.lending = { bot: "", name: "" };
        app.render();
      }, { disabled: view.busy })) : null),
    ...browser.bots.map((loan) =>
      h("div", { class: "row" },
        h("div", { class: "row__body" }, h("h3", {}, loanName(loan)), h("span", { class: "micro" }, `Bot ${loan.id}`)),
        h("div", { class: "row__actions" },
          button("Stop Lending", () => change(app, { cmd: "browser_bot", bot: loan.id, lent: false }),
            { kind: "btn-subtle-negative", disabled: view.busy })))),
  ];
  if (lending) {
    const id = lending.bot.trim();
    const valid = /^[0-9]{1,20}$/.test(id);
    rows.push(h("form", { class: "confirm", onsubmit: (event) => {
      event.preventDefault();
      if (!valid) return;
      view.lending = null;
      change(app, { cmd: "browser_bot", bot: id, name: lending.name.trim() || undefined, lent: true });
    } },
      h("div", { class: "inline" },
        h("input", { class: "input", inputMode: "numeric", placeholder: "Bot ID, like 42", "aria-label": "Bot ID",
          value: lending.bot, "data-key": "browser-bot-id", oninput: (event) => {
            const was = /^[0-9]{1,20}$/.test(lending.bot.trim());
            lending.bot = event.target.value;
            if (was !== /^[0-9]{1,20}$/.test(lending.bot.trim())) app.render();
          } }),
        h("input", { class: "input", placeholder: "Name, to recognize it (optional)", "aria-label": "Bot name", maxLength: 80,
          value: lending.name, "data-key": "browser-bot-name", oninput: (event) => (lending.name = event.target.value) })),
      h("p", { class: "note" }, "The bot's ID is the number in its address in Chat with Work (/bots/42)."),
      h("p", {}, "Lend your browser to this bot? Its chats can read, and if you allow actions act, in the browser's “Chat with Work” tab group, on the sites above, with your sign-ins there. Each action asks in Chat with Work first."),
      h("div", { class: "inline" },
        button("Cancel", () => {
          view.lending = null;
          app.render();
        }),
        h("button", { type: "submit", class: "btn btn-primary", disabled: !valid }, "Lend Browser"))));
  }
  return section("Bots", null, group(...rows));
}

export function render(app) {
  const view = app.view("browser");
  const page = h("div", { class: "page" },
    pageHeader("Your Browser", "With the Chat with Work extension, Chat with Work can use your own Chrome, Edge, Brave or Chromium, with your sign-ins, only in its own “Chat with Work” tab group and only on the sites you allow here."));
  const status = app.status;
  if (!status) {
    append(page, agentMissing(app));
    return page;
  }
  const browser = { sites: [], bots: [], ...status.browser };
  const busy = view.busy;

  append(page, group(
    setting("Use your browser", browser.enabled
      ? "Chat with Work can read pages in its tab group, on the sites below."
      : "Off. Turn on to let Chat with Work use your browser, with the extension.",
    switchInput(browser.enabled || view.enabling, (on) => {
      if (on) {
        view.enabling = true;
        app.render();
      } else {
        enable(app, false);
      }
    }, { label: "Use your browser", disabled: busy || view.enabling })),
    view.enabling ? h("div", { class: "confirm" },
      h("p", {}, "Let Chat with Work use your browser? This registers Chat with Work Desktop with the browsers on this computer so the extension can reach it. Chat with Work then reads pages only in its own tab group, on the sites you allow, and never password, card or one-time code fields."),
      h("div", { class: "inline" },
        button("Cancel", () => {
          view.enabling = false;
          app.render();
        }),
        button("Turn On", () => {
          view.enabling = false;
          enable(app, true);
        }, { kind: "btn-primary" }))) : null,
    browser.enabled ? connection(browser) : null,
    browser.enabled ? setting("Allow actions", browser.changes
      ? "Chat with Work can also open pages, click, type, choose and save files, with your approval in Chat with Work, which you can give for the rest of a chat. Anything that may send, buy, post or delete always asks, with a screenshot."
      : "Reading only. Turn on to let Chat with Work also open pages, click, type, choose and save files, with your approval in Chat with Work.",
    switchInput(browser.changes || view.allowing, (on) => {
      if (on) {
        view.allowing = true;
        app.render();
      } else {
        change(app, { cmd: "browser_changes", changes: false });
      }
    }, { label: "Allow actions in the browser", disabled: busy || view.allowing })) : null,
    view.allowing ? h("div", { class: "confirm" },
      h("p", {}, "Let Chat with Work act in your browser? It can open pages, click, type, choose and save files into a folder that allows changes, on your allowed sites, with your sign-ins there. Each action asks you in Chat with Work first, unless you allow actions for the rest of a chat; anything that may send, buy, post or delete always asks, with a screenshot."),
      h("div", { class: "inline" },
        button("Cancel", () => {
          view.allowing = false;
          app.render();
        }),
        button("Allow Actions", () => {
          view.allowing = false;
          change(app, { cmd: "browser_changes", changes: true });
        }, { kind: "btn-primary" }))) : null,
    browser.enabled ? setting(browser.stopped ? "Stopped" : "Stop using the browser", browser.stopped
      ? "Chat with Work can't use your browser until you resume it."
      : "Stops Chat with Work using your browser at once, until you resume it. The tray menu has the same switch.",
    browser.stopped
      ? button("Resume", () => change(app, { cmd: "browser_stopped", stopped: false }), { kind: "btn-primary", disabled: busy })
      : button("Stop", () => change(app, { cmd: "browser_stopped", stopped: true }), { kind: "btn-subtle-negative", disabled: busy })) : null));

  if (browser.enabled) append(page, sites(app, browser), bots(app, browser));
  append(page,
    view.error ? notice(view.error) : null,
    view.note ? notice(view.note, "positive") : null);
  return page;
}

export default { title, icon, render };
