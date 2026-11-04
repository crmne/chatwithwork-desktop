// The update banner in the Chat with Work window (banner.rs): Restart to
// Update for a copy that installs updates itself, or the command for a copy
// a package manager updates, with Copy. Only the banner's own commands.

import { ICONS } from "./icons.js";

const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
const root = document.getElementById("banner");
let copied = false;
let shown = null;

function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") element.className = value;
    else if (key.startsWith("on")) element.addEventListener(key.slice(2).toLowerCase(), value);
    else element.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    element.append(child instanceof Node ? child : String(child));
  }
  return element;
}

function glyph(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 256 256");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  // Phosphor's markup, bundled in icons.js.
  svg.innerHTML = ICONS[name] || "";
  return svg;
}

const button = (label, onclick, kind = "") => h("button", { type: "button", class: `btn ${kind}`.trim(), onclick }, label);

function fit() {
  invoke("banner_fit", { height: Math.ceil(root.getBoundingClientRect().height) }).catch(() => {});
}

export function render(view) {
  shown = view;
  if (!view) {
    root.replaceChildren();
    return;
  }
  const version = view.release?.version;
  const dismiss = h("button", { type: "button", class: "btn btn-ghost btn-icon", "aria-label": "Dismiss", title: "Dismiss",
    onclick: () => invoke("banner_dismiss") }, glyph("x"));
  if (view.kind === "restart") {
    root.replaceChildren(
      h("div", { class: "banner__row" },
        h("div", { class: "banner__body" },
          h("h1", {}, `Version ${version} is ready`),
          h("p", {}, view.installs_next_start
            ? "It installs the next time Chat with Work starts."
            : "Restart to use it. Your windows come back.")),
        button("Restart to Update", () => invoke("banner_restart"), "btn-primary"),
        dismiss));
  } else {
    const copy = async () => {
      try {
        await navigator.clipboard.writeText(view.command);
        copied = true;
        render(shown);
        setTimeout(() => { copied = false; render(shown); }, 1500);
      } catch {}
    };
    root.replaceChildren(
      h("div", { class: "banner__row" },
        h("div", { class: "banner__body" },
          h("h1", {}, `Version ${version} is available`),
          h("p", {}, view.advice)),
        dismiss),
      view.command ? h("div", { class: "command" },
        h("code", { "aria-label": "Command" }, view.command),
        button([glyph(copied ? "check" : "copy"), copied ? "Copied" : "Copy"], copy)) : null,
      h("div", { class: "inline" },
        view.download ? button([glyph("download-simple"), "Download"], () => invoke("banner_open", { which: "download" })) : null,
        button("Release Notes", () => invoke("banner_open", { which: "notes" }))));
  }
  // Measured now: a window on another workspace draws no frames, so
  // requestAnimationFrame would wait until it shows.
  fit();
}

window.cwwEvent = (event, payload) => {
  if (event === "update") invoke("banner_state").then(render).catch(() => {});
};

invoke("banner_state").then(render).catch(() => {});
