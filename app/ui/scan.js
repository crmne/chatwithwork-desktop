// The scan window: settings on the left, the pages on the right, and what
// to do with them at the foot. Everything reaches the app through the scan
// commands in scan.rs; events arrive through window.cwwEvent("scan", ...).
// Nothing here talks to the network: the pages stay on this computer until
// Send hands the PDF to the background agent.

import { ICONS } from "./icons.js";

const internals = window.__TAURI_INTERNALS__;

async function call(command, args = {}) {
  try {
    return await internals.invoke(command, args);
  } catch (error) {
    if (error && typeof error === "object" && "message" in error) throw error;
    throw { code: "failed", message: String(error) };
  }
}

// h("p", { class: "muted" }, "text"): elements without innerHTML, so no
// scanner's or bot's name is ever read as markup.
function h(tag, props = {}, ...children) {
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

// A message with `commands` in backticks, those as code people can copy.
function withCode(text) {
  return String(text ?? "").split("`").map((part, i) => (i % 2 ? h("code", { class: "command" }, part) : part));
}

function icon(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 256 256");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  // The markup is Phosphor's, bundled in icons.js.
  svg.innerHTML = ICONS[name] || "";
  return svg;
}

const iconButton = (name, label, onclick, { disabled = false, key } = {}) =>
  h("button", { type: "button", class: "btn btn-ghost btn-icon", "aria-label": label, title: label, onclick, disabled, "data-key": key }, icon(name));

const COLORS = [
  ["black_and_white", "B&W", "Black and white"],
  ["grayscale", "Gray", "Grayscale"],
  ["color", "Color", "Color"],
];

const state = {
  info: null,
  settings: null,
  languages: [],
  ocr: { available: false },
  devices: null,
  looking: false,
  device: null,
  caps: null,
  pages: [],
  scanning: false,
  arriving: new Set(),
  error: null,
  notice: null,
  destinations: null,
  destinationsError: null,
  // A composer in Chat with Work waits for this scan (its attach menu).
  attaching: false,
  sending: false,
  progress: null,
  dragging: null,
  saving: false,
  menuOpen: false,
};

// ---- Talking to the app ---------------------------------------------------

async function start() {
  const info = await call("scan_info");
  state.info = info;
  state.settings = info.settings;
  state.languages = info.languages;
  state.ocr = info.ocr;
  state.pages = info.pages;
  state.scanning = info.busy;
  state.attaching = Boolean(info.attaching);
  if (!state.ocr.available) state.settings.ocr = null;
  render();
  await lookForScanners(info.device);
  loadDestinations();
}

async function lookForScanners(preferred) {
  state.looking = true;
  state.error = null;
  render();
  try {
    const { devices } = await call("scan_devices");
    state.devices = devices;
    const keep = state.device && devices.find((d) => d.id === state.device.id);
    state.device = keep || devices.find((d) => d.id === preferred) || devices[0] || null;
    if (state.device) await readCapabilities();
  } catch (error) {
    state.devices = [];
    state.device = null;
    state.error = error.message;
  }
  state.looking = false;
  render();
}

async function readCapabilities() {
  state.caps = null;
  try {
    const { capabilities } = await call("scan_capabilities", { device: state.device.id });
    state.caps = capabilities;
    const s = state.settings;
    if (capabilities.resolutions.length && !capabilities.resolutions.includes(s.resolution)) {
      s.resolution = capabilities.resolutions.reduce((a, b) => (Math.abs(b - s.resolution) < Math.abs(a - s.resolution) ? b : a));
    }
    if (!capabilities.color && s.color === "color") s.color = "grayscale";
  } catch (error) {
    state.error = error.message;
  }
}

async function loadDestinations() {
  try {
    const answer = await call("scan_destinations");
    state.destinations = answer.destinations || [];
    state.destinationsError = null;
  } catch (error) {
    state.destinations = null;
    state.destinationsError = error.message;
  }
  render();
}

function saveSettings() {
  call("scan_save_settings", { settings: state.settings, device: state.device?.id ?? null }).catch(() => {});
}

async function scan(append) {
  if (!state.device || state.scanning) return;
  state.error = null;
  state.notice = null;
  state.scanning = true;
  if (!append) state.pages = [];
  render();
  try {
    await call("scan_start", { device: state.device, settings: effectiveSettings(), append });
  } catch (error) {
    state.scanning = false;
    state.error = error.message;
    render();
  }
}

// Duplex only where the scanner has it; OCR only where the computer can.
function effectiveSettings() {
  const s = { ...state.settings };
  if (!state.caps?.duplex) s.duplex = false;
  if (!state.ocr.available) s.ocr = null;
  return s;
}

window.cwwEvent = (event, payload) => {
  if (event !== "scan") return;
  switch (payload.kind) {
    case "page":
      state.pages.push(payload.page);
      state.arriving.add(payload.page.id);
      break;
    case "end": {
      state.scanning = false;
      if (payload.message) state.notice = { tone: "attention", text: payload.message };
      break;
    }
    case "error":
      if (payload.code !== undefined) state.scanning = false;
      state.error = payload.message;
      break;
    case "progress":
      state.progress = payload.text;
      break;
    case "attaching":
      state.attaching = Boolean(payload.attaching);
      break;
  }
  render();
};

async function edit(page, change) {
  try {
    const { page: changed } = await call("scan_edit", { id: page.id, ...change });
    state.pages = state.pages.map((p) => (p.id === changed.id ? changed : p));
  } catch (error) {
    state.error = error.message;
  }
  render();
}

async function arrange(page, change) {
  try {
    const { pages } = await call("scan_arrange", { id: page.id, ...change });
    state.pages = pages;
  } catch (error) {
    state.error = error.message;
  }
  render();
}

// The chosen OCR language when this computer can't read it (scan_info).
function missingLanguage() {
  return state.settings.ocr && state.ocr.available && state.ocr.missing?.code === state.settings.ocr ? state.ocr.missing : null;
}

// Reads the languages again, after the person added one.
async function checkLanguages() {
  try {
    const info = await call("scan_info");
    state.languages = info.languages;
    state.ocr = info.ocr;
    state.notice = missingLanguage() ? null : { tone: "positive", text: "The language is available now." };
  } catch (error) {
    state.error = error.message;
  }
  render();
}

// Refuses to finish with searchable text in a language this computer can't
// read, rather than reading in another one.
function languageBlocks() {
  const missing = missingLanguage();
  if (!missing) return false;
  state.error = `This computer can't read ${missing.name} yet, so the text can't be made searchable. ${missing.how} Or turn off Make Text Searchable.`;
  render();
  return true;
}

// Hands the PDF to Chat with Work's own composer: the one that asked for
// it, or a new chat's. The message is written there, with everything the
// composer offers, and the window closes.
async function attach() {
  const kept = state.pages.filter((p) => !p.blank);
  if (!kept.length || state.sending || languageBlocks()) return;
  state.sending = true;
  state.error = null;
  state.notice = null;
  state.progress = "Preparing…";
  render();
  try {
    await call("scan_attach", {
      language: state.settings.ocr && state.ocr.available ? state.settings.ocr : null,
    });
  } catch (error) {
    state.error = error.message;
  }
  state.sending = false;
  state.progress = null;
  render();
}

// Sends the PDF to a bot that takes scans, through the background agent.
async function sendToBot(destination) {
  const kept = state.pages.filter((p) => !p.blank);
  if (!kept.length || state.sending || languageBlocks()) return;
  state.sending = true;
  state.error = null;
  state.notice = null;
  state.progress = "Preparing…";
  render();
  try {
    await call("scan_send", {
      destination: { kind: "trigger", id: destination.id },
      message: null,
      language: state.settings.ocr && state.ocr.available ? state.settings.ocr : null,
    });
    state.notice = { tone: "positive", text: `Sent to ${destination.name}.` };
  } catch (error) {
    state.error = error.message;
  }
  state.sending = false;
  state.progress = null;
  render();
}

// Saves the PDF on this computer: no Chat with Work, no sign-in.
async function save() {
  const kept = state.pages.filter((p) => !p.blank);
  if (!kept.length || state.sending || languageBlocks()) return;
  state.sending = true;
  state.saving = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    const answer = await call("scan_save", {
      language: state.settings.ocr && state.ocr.available ? state.settings.ocr : null,
    });
    if (answer.saved) {
      state.notice = { tone: "positive", text: `Saved as ${answer.name}.`, action: ["Show in Folder", () => call("scan_reveal", { path: answer.saved })] };
    }
  } catch (error) {
    state.error = error.message;
  }
  state.sending = false;
  state.saving = false;
  state.progress = null;
  render();
}

// ---- Drawing ----------------------------------------------------------------

function render() {
  if (!state.settings) return;
  const active = document.activeElement?.dataset?.key;
  const present = (nodes) => nodes.filter((node) => node !== null && node !== undefined && node !== false);
  document.getElementById("settings").replaceChildren(...present(renderSettings()));
  document.getElementById("main").replaceChildren(...present(renderMain()));
  if (active) document.querySelector(`[data-key="${CSS.escape(active)}"]`)?.focus({ preventScroll: true });
  state.arriving.clear();
}

function field(label, control, id) {
  return h("div", { class: "field" }, h("label", { for: id }, label), control);
}

function renderSettings() {
  const s = state.settings;
  const busy = state.scanning || state.sending;
  const devices = state.devices || [];
  const caps = state.caps;

  const scanner = h("div", { class: "field__row" },
    h("select", {
      class: "select", id: "device", "data-key": "device", disabled: busy || state.looking || !devices.length,
      onchange: async (event) => {
        state.device = devices.find((d) => d.id === event.target.value) || null;
        render();
        if (state.device) await readCapabilities();
        saveSettings();
        render();
      },
    },
    state.looking && !devices.length ? h("option", {}, "Looking for scanners…") : null,
    !state.looking && !devices.length ? h("option", {}, "No scanner found") : null,
    devices.map((d) => h("option", { value: d.id, selected: d.id === state.device?.id }, d.name))),
    iconButton("arrow-counter-clockwise", "Look for scanners again", () => lookForScanners(state.device?.id), { disabled: busy || state.looking, key: "refresh" }));

  const colors = h("div", { class: "segmented", role: "radiogroup", "aria-label": "Color mode" },
    COLORS.map(([value, short, long]) => h("label", { title: long },
      h("input", {
        type: "radio", name: "color", value, checked: s.color === value, disabled: busy || (value === "color" && caps && !caps.color),
        "data-key": `color-${value}`,
        onchange: () => { s.color = value; saveSettings(); render(); },
      }), short, h("span", { class: "sr-only" }, ` (${long})`))));

  const resolutions = caps?.resolutions?.length ? caps.resolutions : [150, 200, 300, 600];
  const resolution = h("select", {
    class: "select", id: "resolution", "data-key": "resolution", disabled: busy,
    onchange: (event) => { s.resolution = Number(event.target.value); saveSettings(); },
  }, resolutions.map((r) => h("option", { value: r, selected: r === s.resolution }, `${r} dpi${r === 300 ? " (recommended)" : ""}`)));

  const toggle = (key, title, note, { disabled = false, checked = s[key] } = {}) =>
    h("label", { class: "toggle" },
      h("span", {}, title, note ? h("small", {}, note) : null),
      h("input", { type: "checkbox", role: "switch", class: "switch", checked: checked && !disabled, disabled: busy || disabled, "data-key": key,
        onchange: (event) => { s[key] = event.target.checked; saveSettings(); render(); } }));

  const ocrOn = Boolean(s.ocr) && state.ocr.available;
  const ocr = h("label", { class: "toggle" },
    h("span", {}, "Make text searchable",
      h("small", {}, state.ocr.available ? "Read on this computer, as a text layer you can select" : withCode(state.ocr.reason))),
    h("input", { type: "checkbox", role: "switch", class: "switch", checked: ocrOn, disabled: busy || !state.ocr.available, "data-key": "ocr",
      onchange: (event) => {
        if (!event.target.checked && s.ocr) state.lastLanguage = s.ocr;
        s.ocr = event.target.checked ? (state.lastLanguage || state.languages[0]?.code || null) : null;
        saveSettings();
        render();
      } }));
  // A chosen language this computer can't read stays chosen, said so, with
  // the way to add it; nothing silently reads in another language.
  const missing = missingLanguage();
  const language = ocrOn ? field("Language", h("select", {
    class: "select", id: "language", "data-key": "language", disabled: busy,
    onchange: (event) => { s.ocr = event.target.value; state.lastLanguage = s.ocr; state.ocr.missing = null; saveSettings(); render(); },
  },
  missing ? h("option", { value: missing.code, selected: true }, `${missing.name} (not available)`) : null,
  state.languages.map((l) => h("option", { value: l.code, selected: l.code === s.ocr }, l.name))), "language") : null;
  const missingNote = ocrOn && missing ? h("div", { class: "notice notice--attention scan__missing", role: "status" },
    h("p", {}, `This computer can't read ${missing.name} yet. `, withCode(missing.how)),
    h("div", { class: "inline" },
      state.ocr.can_add ? h("button", { type: "button", class: "btn", "data-key": "add-language", onclick: () => call("scan_add_language") }, "Open Language Settings") : null,
      h("button", { type: "button", class: "btn", "data-key": "check-languages", onclick: checkLanguages }, "Check Again"))) : null;

  return [
    h("div", { class: "nav__brand" }, h("img", { src: "mark.svg", alt: "" }), "Scan"),
    field("Scanner", scanner, "device"),
    h("div", { class: "field" }, h("span", { class: "field__label" }, "Color mode"), colors),
    field("Resolution", resolution, "resolution"),
    toggle("duplex", "Both sides", caps && !caps.duplex ? "This scanner scans one side" : null, { disabled: Boolean(caps) && !caps.duplex }),
    toggle("remove_blank_pages", "Remove blank pages", "Leaves out empty backs; you can bring them back"),
    ocr,
    language,
    missingNote,
    h("button", {
      type: "button", class: "btn btn-primary scan__go", "data-key": "scan",
      disabled: !state.device || busy || state.looking,
      onclick: () => scan(state.pages.length > 0),
    }, icon("scan"), state.scanning ? "Scanning…" : state.pages.length ? "Scan More Pages" : "Scan"),
    state.scanning ? h("button", { type: "button", class: "btn", onclick: () => call("scan_cancel"), "data-key": "stop" }, "Stop") : null,
    h("p", { class: "scan__privacy" }, icon("lock-simple"),
      "Scans stay on this computer until you send them. Text is read here, not on Chat with Work's servers."),
  ];
}

function renderMain() {
  const kept = state.pages.filter((p) => !p.blank);
  const blank = state.pages.filter((p) => p.blank);
  const body = h("div", { class: "scan__body" },
    h("header", { class: "scan__header" },
      h("h1", {}, "Pages"),
      state.scanning
        ? h("span", { class: "state state--live", role: "status" }, `Scanning… ${kept.length} ${kept.length === 1 ? "page" : "pages"}`)
        : kept.length ? h("span", { class: "state" }, `${kept.length} ${kept.length === 1 ? "page" : "pages"}`) : null),
    state.error ? h("div", { class: "notice notice--negative", role: "alert" }, withCode(state.error)) : null,
    state.notice ? h("div", { class: `notice notice--${state.notice.tone}`, role: "status" }, state.notice.text,
      state.notice.action ? [" ", h("button", { type: "button", class: "link", onclick: state.notice.action[1] }, state.notice.action[0])] : null) : null,
    kept.length || state.scanning ? sheets(kept, true) : emptyState(),
    blank.length ? h("details", { class: "left-out" },
      h("summary", {}, `${blank.length} blank ${blank.length === 1 ? "page" : "pages"} left out`),
      sheets(blank, false)) : null);
  return [body, bar(kept)];
}

function emptyState() {
  if (!state.looking && state.devices && !state.devices.length) {
    return h("div", { class: "scan__empty" }, icon("scan"),
      h("strong", {}, "No scanner found"),
      h("p", {}, "Connect a scanner with USB and turn it on, then look for scanners again. On Linux, scanners work through SANE."),
      h("button", { type: "button", class: "btn", onclick: () => lookForScanners() }, "Look Again"));
  }
  return h("div", { class: "scan__empty" }, icon("scan"),
    h("strong", {}, "Put the pages in the scanner"),
    h("p", {}, "Choose how to scan on the left, then Scan. You can check, crop, turn and order the pages before anything is sent."));
}

function sheets(pages, kept) {
  const all = state.pages;
  return h("ol", { class: "sheets" },
    pages.map((page, n) => {
      const index = all.indexOf(page);
      return h("li", {
        class: ["sheet", page.blank ? "sheet--blank" : "", state.arriving.has(page.id) ? "sheet--arriving" : ""].join(" ").trim(),
        draggable: kept && !state.sending ? "true" : false,
        ondragstart: (event) => { state.dragging = page.id; event.dataTransfer.effectAllowed = "move"; event.currentTarget.classList.add("sheet--dragging"); },
        ondragend: (event) => { state.dragging = null; event.currentTarget.classList.remove("sheet--dragging"); },
        ondragover: (event) => { if (state.dragging !== null && state.dragging !== page.id) { event.preventDefault(); event.currentTarget.classList.add("sheet--target"); } },
        ondragleave: (event) => event.currentTarget.classList.remove("sheet--target"),
        ondrop: (event) => {
          event.preventDefault();
          const moving = all.find((p) => p.id === state.dragging);
          if (moving) arrange(moving, { index });
        },
      },
      h("div", { class: "sheet__paper" }, h("img", { src: page.thumb, alt: `Page ${n + 1}` }),
        page.crop ? h("span", { class: "tag sheet__badge" }, "Cropped") : null),
      h("div", { class: "sheet__foot" },
        h("span", { class: "sheet__number" }, kept ? String(n + 1).padStart(2, "0") : "blank"),
        kept ? [
          iconButton("caret-left", "Move earlier", () => arrange(page, { index: all.indexOf(pages[n - 1]) }), { disabled: n === 0 || state.sending, key: `left-${page.id}` }),
          iconButton("caret-right", "Move later", () => arrange(page, { index: all.indexOf(pages[n + 1]) }), { disabled: n === pages.length - 1 || state.sending, key: `right-${page.id}` }),
          iconButton("arrow-clockwise", "Turn clockwise", () => edit(page, { turn: true }), { disabled: state.sending, key: `turn-${page.id}` }),
          iconButton("crop", "Crop", () => openCrop(page), { disabled: state.sending, key: `crop-${page.id}` }),
          iconButton("trash", "Delete page", () => arrange(page, { delete: true }), { disabled: state.sending, key: `delete-${page.id}` }),
        ] : h("button", { type: "button", class: "btn", onclick: () => edit(page, { keep: true }), "data-key": `keep-${page.id}` }, "Keep")));
    }),
    state.scanning && kept ? h("li", { class: "sheet sheet--placeholder", "aria-hidden": "true" }, h("div", { class: "sheet__paper" }, h("span", { class: "state state--live" }, "Scanning"))) : null);
}

// Whether this computer can send to Chat with Work now: paired, and allowed
// to use its owner's chats (the server answered where scans can go).
// Until the server answers, the new chat stays the main action.
const canReachChatWithWork = () => !state.destinationsError;

// What the bar says next to the finish button.
function hint(signedIn) {
  if (state.attaching) return "It goes into your message in Chat with Work.";
  if (signedIn) return "You write your message in Chat with Work, as in any new chat.";
  return null;
}

function bar(kept) {
  const busy = state.scanning || state.sending;
  const signedIn = canReachChatWithWork();
  const words = hint(signedIn);
  return h("footer", { class: "scan__bar" },
    h("button", { type: "button", class: "btn", disabled: busy || !state.device || !state.pages.length, "data-key": "rescan",
      onclick: () => scan(false) }, "Rescan"),
    h("span", { class: "spacer" }),
    state.progress
      ? h("span", { class: "scan__progress", role: "status" }, state.progress)
      : words ? h("span", { class: "scan__hint" }, words) : null,
    finishButton(kept, signedIn, busy));
}

// One way to finish, and the others in its menu. The main part puts the
// scan in Chat with Work's composer: the one that asked for it, or a new
// chat's. The menu sends it to a bot that takes scans, or saves it on this
// computer. A computer that can't reach Chat with Work saves, and says in
// the menu what sending needs.
function finishButton(kept, signedIn, busy) {
  const disabled = busy || !kept.length;
  const working = state.sending && !state.saving;
  const saveLabel = state.saving ? "Saving…" : "Save to Computer";
  let main;
  if (state.attaching) {
    main = h("button", { type: "button", class: "btn btn-primary split__main", disabled, "data-key": "attach", onclick: attach },
      icon("paperclip"), working ? "Attaching…" : "Attach to Message");
  } else if (signedIn) {
    main = h("button", { type: "button", class: "btn btn-primary split__main", disabled, "data-key": "attach", onclick: attach },
      icon("chat-circle-text"), working ? "Opening…" : "Continue in a New Chat");
  } else {
    main = h("button", { type: "button", class: "btn btn-primary split__main", disabled, "data-key": "save", onclick: save },
      icon("download-simple"), saveLabel);
  }
  const items = [];
  if (signedIn && !state.attaching) {
    for (const destination of state.destinations || []) {
      items.push(h("button", { type: "button", role: "menuitem", class: "split__item", "data-key": `menu-bot-${destination.id}`,
        onclick: () => { closeMenu(); sendToBot(destination); } },
        icon("robot"), h("span", {}, `Send to ${destination.name}`, destination.project ? h("small", {}, destination.project) : null)));
    }
  }
  if (signedIn || state.attaching) {
    items.push(h("button", { type: "button", role: "menuitem", class: "split__item", "data-key": "menu-save",
      onclick: () => { closeMenu(); save(); } }, icon("download-simple"), h("span", {}, "Save to Computer")));
  } else {
    items.push(h("button", { type: "button", role: "menuitem", class: "split__item", "data-key": "menu-send",
      onclick: () => { closeMenu(); call("scan_open_account"); } },
      icon("paper-plane-tilt"),
      h("span", {}, "Send to Chat with Work",
        h("small", {}, `${state.destinationsError || "This computer can't reach Chat with Work."} Set it up in Settings.`))));
  }
  return h("div", { class: "split", onkeydown: menuKeys },
    main,
    h("button", {
      type: "button", class: "btn btn-primary split__toggle", disabled, "data-key": "finish-menu",
      "aria-haspopup": "menu", "aria-expanded": state.menuOpen ? "true" : "false", "aria-controls": "finish-menu",
      "aria-label": "More ways to finish", title: "More ways to finish",
      onclick: () => (state.menuOpen ? closeMenu() : openMenu()),
    }, icon("caret-down")),
    state.menuOpen ? h("div", { class: "split__menu", id: "finish-menu", role: "menu", "aria-label": "More ways to finish" }, items) : null);
}

function openMenu() {
  state.menuOpen = true;
  render();
  document.querySelector(".split__menu [role=menuitem]")?.focus();
}

function closeMenu({ refocus = false } = {}) {
  if (!state.menuOpen) return;
  state.menuOpen = false;
  render();
  if (refocus) document.querySelector('[data-key="finish-menu"]')?.focus();
}

// The menu's keys: arrows move between items, Escape and Tab close it.
function menuKeys(event) {
  const items = [...document.querySelectorAll(".split__menu [role=menuitem]")];
  if (event.key === "ArrowDown" && !state.menuOpen && event.target.dataset.key === "finish-menu") {
    event.preventDefault();
    openMenu();
  } else if (state.menuOpen && event.key === "Escape") {
    event.preventDefault();
    closeMenu({ refocus: true });
  } else if (state.menuOpen && event.key === "Tab") {
    closeMenu();
  } else if (state.menuOpen && (event.key === "ArrowDown" || event.key === "ArrowUp") && items.length) {
    event.preventDefault();
    const at = items.indexOf(document.activeElement);
    const next = (at + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length;
    items[next].focus();
  }
}

// A click anywhere else closes the menu.
document.addEventListener("pointerdown", (event) => {
  if (state.menuOpen && !event.target.closest?.(".split")) closeMenu();
});

// ---- The crop view --------------------------------------------------------

async function openCrop(page) {
  const dialog = document.getElementById("crop");
  let picture;
  try {
    picture = await call("scan_page", { id: page.id });
  } catch (error) {
    state.error = error.message;
    render();
    return;
  }
  let box = picture.crop ? { ...picture.crop } : { x: 0, y: 0, width: 1, height: 1 };
  const img = h("img", { src: picture.image, alt: "The page as scanned" });
  const frame = h("div", { class: "crop__frame" }, img);
  const rect = h("div", { class: "crop__box", tabindex: "0", "aria-label": "Crop area; use the arrow keys to move it, with Shift to resize" });
  const handles = ["nw", "n", "ne", "e", "se", "s", "sw", "w"].map((at) => h("span", { class: "crop__handle", "data-at": at }));
  frame.append(rect, ...handles);

  const place = () => {
    rect.style.left = `${box.x * 100}%`;
    rect.style.top = `${box.y * 100}%`;
    rect.style.width = `${box.width * 100}%`;
    rect.style.height = `${box.height * 100}%`;
    for (const handle of handles) {
      const at = handle.dataset.at;
      const x = at.includes("w") ? box.x : at.includes("e") ? box.x + box.width : box.x + box.width / 2;
      const y = at.includes("n") ? box.y : at.includes("s") ? box.y + box.height : box.y + box.height / 2;
      handle.style.left = `${x * 100}%`;
      handle.style.top = `${y * 100}%`;
      handle.style.cursor = `${at}-resize`;
    }
  };
  const MIN = 0.05;
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

  frame.addEventListener("pointerdown", (event) => {
    const bounds = img.getBoundingClientRect();
    const at = event.target.dataset?.at || (event.target === rect ? "move" : "draw");
    const startX = (event.clientX - bounds.left) / bounds.width;
    const startY = (event.clientY - bounds.top) / bounds.height;
    const from = { ...box };
    frame.setPointerCapture(event.pointerId);
    const onMove = (move) => {
      const px = clamp((move.clientX - bounds.left) / bounds.width, 0, 1);
      const py = clamp((move.clientY - bounds.top) / bounds.height, 0, 1);
      if (at === "move") {
        box.x = clamp(from.x + px - startX, 0, 1 - from.width);
        box.y = clamp(from.y + py - startY, 0, 1 - from.height);
      } else if (at === "draw") {
        box = { x: Math.min(px, startX), y: Math.min(py, startY), width: Math.max(MIN, Math.abs(px - startX)), height: Math.max(MIN, Math.abs(py - startY)) };
        box.width = Math.min(box.width, 1 - box.x);
        box.height = Math.min(box.height, 1 - box.y);
      } else {
        let { x, y, width, height } = from;
        const right = from.x + from.width;
        const bottom = from.y + from.height;
        if (at.includes("w")) { x = clamp(px, 0, right - MIN); width = right - x; }
        if (at.includes("e")) { width = clamp(px - x, MIN, 1 - x); }
        if (at.includes("n")) { y = clamp(py, 0, bottom - MIN); height = bottom - y; }
        if (at.includes("s")) { height = clamp(py - y, MIN, 1 - y); }
        box = { x, y, width, height };
      }
      place();
    };
    const onUp = () => {
      frame.removeEventListener("pointermove", onMove);
      frame.removeEventListener("pointerup", onUp);
    };
    frame.addEventListener("pointermove", onMove);
    frame.addEventListener("pointerup", onUp);
  });

  rect.addEventListener("keydown", (event) => {
    const step = event.altKey ? 0.002 : 0.01;
    const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[event.key];
    if (!d) return;
    event.preventDefault();
    if (event.shiftKey) {
      box.width = clamp(box.width + d[0], MIN, 1 - box.x);
      box.height = clamp(box.height + d[1], MIN, 1 - box.y);
    } else {
      box.x = clamp(box.x + d[0], 0, 1 - box.width);
      box.y = clamp(box.y + d[1], 0, 1 - box.height);
    }
    place();
  });

  const close = () => dialog.close();
  dialog.replaceChildren(h("div", { class: "crop__layout" },
    h("header", { class: "crop__header" },
      h("h2", { id: "crop-title" }, "Crop"),
      iconButton("x", "Close", close)),
    h("div", { class: "crop__stage" }, frame),
    h("footer", { class: "crop__footer" },
      h("p", { class: "note" }, "Drag the corners, or draw a new area. Only the area inside is sent."),
      h("button", { type: "button", class: "btn", onclick: () => { box = { x: 0, y: 0, width: 1, height: 1 }; place(); } }, "Whole Page"),
      h("button", { type: "button", class: "btn", onclick: close }, "Cancel"),
      h("button", { type: "button", class: "btn btn-primary", onclick: async () => {
        close();
        const whole = box.x < 0.001 && box.y < 0.001 && box.width > 0.998 && box.height > 0.998;
        await edit(page, whole ? { whole: true } : { crop: box });
      } }, "Crop"))));
  place();
  dialog.showModal();
  rect.focus();
}

start().catch((error) => {
  document.getElementById("main").replaceChildren(h("div", { class: "scan__body" }, h("div", { class: "notice notice--negative" }, error.message || String(error))));
});
