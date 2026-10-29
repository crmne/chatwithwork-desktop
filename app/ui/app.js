// The settings window: a side nav and one page at a time, drawn from the
// app's state and the daemon's status. Everything reaches the app through
// the commands in local.rs; the daemon only through its allow-list.

import { ICONS } from "./icons.js";
import welcome from "./pages/welcome.js";
import folders from "./pages/folders.js";
import browser from "./pages/browser.js";
import privacy from "./pages/privacy.js";
import activity from "./pages/activity.js";
import account from "./pages/account.js";
import notifications from "./pages/notifications.js";
import general from "./pages/general.js";

const PAGES = { welcome, folders, browser, privacy, activity, account, notifications, general };
const NAV = ["folders", "browser", "privacy", "activity", "account", "notifications", "general"];

// The app's commands. A refusal comes back as { code, message }.
const internals = window.__TAURI_INTERNALS__;

export async function call(command, args = {}) {
  try {
    return await internals.invoke(command, args);
  } catch (error) {
    if (error && typeof error === "object" && "message" in error) throw error;
    throw { code: "failed", message: String(error) };
  }
}

export const control = (request) => call("control", { request });

// Events from the app: it calls window.cwwEvent(name, payload) in this page
// (windows.rs, tell_settings), so the pages need none of Tauri's own APIs.
const handlers = {};

window.cwwEvent = (event, payload) => {
  for (const handler of handlers[event] || []) handler(payload);
};

function on(event, handler) {
  (handlers[event] ||= []).push(handler);
  return Promise.resolve();
}

// h("p", { class: "muted" }, "text", child): elements without innerHTML, so
// nothing from the daemon or a file name is ever read as markup.
export function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") element.className = value;
    else if (key.startsWith("on")) element.addEventListener(key.slice(2).toLowerCase(), value);
    else if (key in element && typeof value !== "string") element[key] = value;
    else element.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    element.append(child instanceof Node ? child : String(child));
  }
  return element;
}

// Element.append, without the nulls and falses conditional parts leave.
export function append(element, ...children) {
  element.append(...children.flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false));
  return element;
}

export function icon(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 256 256");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  // The markup is Phosphor's, bundled in icons.js.
  svg.innerHTML = ICONS[name] || "";
  return svg;
}

export const section = (title, description, ...children) =>
  h("section", { class: "section" },
    h("header", { class: "section__header" }, h("h2", {}, title), description ? h("p", {}, description) : null),
    ...children);

export const group = (...rows) => h("div", { class: "group" }, ...rows);

export const row = ({ title, text, micro, actions, attention, extra } = {}) =>
  h("div", { class: attention ? "row row--attention" : "row" },
    h("div", { class: "row__body" },
      title ? h("h3", {}, title) : null,
      text ? h("p", {}, text) : null,
      micro ? h("span", { class: "micro truncate" }, micro) : null,
      extra || null),
    actions ? h("div", { class: "row__actions" }, actions) : null);

export const button = (label, onclick, { kind = "", disabled = false, key } = {}) =>
  h("button", { type: "button", class: `btn ${kind}`.trim(), onclick, disabled, "data-key": key }, label);

export const switchInput = (checked, onchange, { label, disabled = false, key } = {}) =>
  h("input", { type: "checkbox", role: "switch", class: "switch", checked, disabled, "aria-label": label, "data-key": key,
    onchange: (event) => onchange(event.target.checked) });

export const notice = (text, tone = "negative") => h("div", { class: `notice notice--${tone}`, role: tone === "negative" ? "alert" : "status" }, text);

export const pageHeader = (title, description) =>
  h("header", { class: "page__header" }, h("h1", {}, title), description ? h("p", {}, description) : null);

export const host = (origin) => {
  try {
    return new URL(origin).host;
  } catch {
    return origin || "";
  }
};

export function thousands(n) {
  return Number(n || 0).toLocaleString("en-US");
}

// Shown on pages that need the daemon while it isn't running.
export function agentMissing(app) {
  if (!app.known) return h("p", { class: "muted" }, "Checking the background agent…");
  const view = app.view("agent");
  return group(h("div", { class: "row row--stack" },
    h("div", { class: "row__body" },
      h("h3", {}, "The background agent isn't running"),
      h("p", {}, "It runs in the background and answers Chat with Work. Start it to share folders and see its activity.")),
    h("div", { class: "inline" },
      button(view.starting ? "Starting…" : "Start Background Agent", async () => {
        view.starting = true;
        view.error = null;
        app.render();
        try {
          await call("start_agent");
        } catch (error) {
          view.error = error.message;
        }
        view.starting = false;
        app.render();
      }, { kind: "btn-primary", disabled: view.starting || !app.info.agent_installed })),
    !app.info.agent_installed ? h("p", { class: "note" }, "Couldn't find the cww command. Install the background agent, then try again.") : null,
    view.error ? notice(view.error) : null));
}

const app = {
  info: {},
  // The daemon's status (null while it isn't running), whether the app has
  // heard from it yet, and its health as the tray words it.
  status: null,
  known: false,
  health: { label: "Checking…", tone: "", serving: false },
  page: null,
  views: {},
  call,
  control,
  // A page's own state, kept across redraws.
  view(name) {
    return (this.views[name] ||= {});
  },
  go(page) {
    if (!PAGES[page]) page = "folders";
    if (this.page !== page) {
      this.page = page;
      history.replaceState(null, "", `#${page}`);
      PAGES[page].enter?.(this);
      document.getElementById("main").scrollTop = 0;
    }
    this.render();
  },
  render() {
    renderNav();
    const main = document.getElementById("main");
    // Keep focus and the caret on the control that had them.
    const active = document.activeElement;
    const key = active?.dataset?.key;
    const selection = key && "selectionStart" in active ? [active.selectionStart, active.selectionEnd] : null;
    main.replaceChildren(PAGES[this.page].render(this));
    if (key) {
      const again = main.querySelector(`[data-key="${CSS.escape(key)}"]`);
      if (again) {
        again.focus({ preventScroll: true });
        if (selection && "setSelectionRange" in again) {
          try {
            again.setSelectionRange(...selection);
          } catch {}
        }
      }
    }
  },
};

function renderNav() {
  const nav = document.getElementById("nav");
  nav.replaceChildren(
    h("div", { class: "nav__brand" }, h("img", { src: "mark.svg", alt: "" }), "Chat with Work"),
    ...(app.info.onboarded ? NAV : ["welcome", ...NAV]).map((page) =>
      h("button", { type: "button", class: "nav__link", "aria-current": app.page === page ? "page" : false,
        onclick: () => app.go(page) }, icon(PAGES[page].icon), PAGES[page].title)),
    h("div", { class: "nav__foot" },
      h("span", { class: `nav__status state state--${app.health.tone}` }, app.known ? app.health.label : "Checking…"),
      button("Open Chat with Work", () => call("show_main"), { kind: "btn-primary" })),
  );
}

async function start() {
  app.info = await call("app_info");
  const take = (payload) => {
    app.status = payload.status;
    app.known = payload.known;
    app.health = payload.health;
  };
  take(app.info.status);
  // Redraw only when the status really changed: compare each payload with
  // the one before, as the app serialized them.
  let previous = JSON.stringify(app.info.status);
  await on("status", (payload) => {
    const serialized = JSON.stringify(payload);
    const changed = serialized !== previous;
    previous = serialized;
    take(payload);
    if (changed) {
      for (const page of Object.values(PAGES)) page.statusChanged?.(app);
      app.render();
    }
  });
  await on("navigate", (page) => app.go(page));
  for (const page of Object.values(PAGES)) await page.listen?.(app, on);
  const wanted = location.hash.slice(1);
  app.go(PAGES[wanted] ? wanted : app.info.onboarded ? "folders" : "welcome");
}

start().catch((error) => {
  document.getElementById("main").replaceChildren(notice(error.message || String(error)));
});
