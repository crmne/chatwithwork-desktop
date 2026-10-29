// A small stand-in for the browser's document, enough to load the bundled
// settings pages in Node and drive what they draw: elements with
// attributes, properties, children and listeners, found by `data-key`.
// Commands the page invokes are recorded; `app_info` never answers, so the
// page's own start never runs.

class Element {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.attributes = {};
    this.children = [];
    this.listeners = {};
    this.value = "";
    this.checked = false;
    this.disabled = false;
    this.className = "";
    this.style = {};
  }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; }
  get textContent() {
    return this.children.map((c) => (typeof c === "string" ? c : c.textContent)).join("");
  }
  set innerHTML(_) {}
  // Fires `type` with `target` set to this element, as a browser would.
  fire(type) {
    for (const listener of this.listeners[type] || []) listener({ target: this, preventDefault() {} });
  }
}

export const calls = [];

export function install() {
  globalThis.Node = Element;
  globalThis.document = {
    createElement: (tag) => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
    getElementById: () => new Element("div"),
    addEventListener() {},
    documentElement: new Element("html"),
  };
  globalThis.location = { hash: "" };
  globalThis.window = globalThis;
  globalThis.__TAURI_INTERNALS__ = {
    invoke: (command, args) => {
      calls.push({ command, args });
      return command === "app_info" ? new Promise(() => {}) : Promise.resolve({});
    },
  };
}

// Every element under `root` (depth first).
export function* walk(root) {
  if (!root || typeof root === "string") return;
  yield root;
  for (const child of root.children || []) yield* walk(child);
}

export function byKey(root, key) {
  for (const element of walk(root)) if (element.attributes?.["data-key"] === key) return element;
  return null;
}

export function byText(root, tag, text) {
  for (const element of walk(root)) {
    if (element.tagName === tag.toUpperCase() && element.textContent.includes(text)) return element;
  }
  return null;
}
