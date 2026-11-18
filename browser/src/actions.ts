// What the daemon can ask of the extension. Every method checks, itself,
// that the tab is in the Chat with Work group and on a site the person
// allowed, whatever the daemon decided.

import { callOn, cleanUp, evaluate, send, takeDialog } from "./cdp.js";
import { isSensitiveField, SENSITIVE_PATTERNS, type ElementFacts } from "./classify.js";
import { ToolError } from "./errors.js";
import { groupTabs, resolveTab } from "./group.js";
import { afterCapture, indicate, prepareForCapture } from "./page.js";
import { hostOfUrl, siteAllowed } from "./sites.js";
import { documentIdentity, rememberRefs, sameDocument } from "./engine-refs.js";
import { page, snapshotText, type AXNode } from "./snapshot.js";

export interface Policy {
  sites: string[];
  all_sites: boolean;
  blocked_sites: string[];
  // Stopped from the tray or `cww browser stop`.
  stopped: boolean;
  // The person allowed actions (clicking, typing, opening pages), not
  // only reading.
  changes: boolean;
}

export type Params = Record<string, unknown>;
export type Method = (params: Params, policy: Policy) => Promise<unknown>;

const READ_MAX_CHARS = 32_000;
const SCREENSHOT_MAX_BYTES = 180_000;
const SCREENSHOT_MAX_WIDTH = 1280;
const WAIT_MAX_SECONDS = 15;

const EDITABLE_ROLES = new Set(["textbox", "searchbox", "combobox", "spinbutton", "textarea"]);

export function only(params: Params, allowed: string[]): void {
  for (const key of Object.keys(params)) {
    if (!allowed.includes(key)) throw new ToolError("invalid_argument", `unknown argument ${key}`);
  }
}

export function integer(value: unknown, name: string, min: number, max: number, fallback: number): number {
  if (value === undefined || value === null) return fallback;
  if (typeof value !== "number" || !Number.isInteger(value) || value < min || value > max) {
    throw new ToolError("invalid_argument", `${name} must be a whole number from ${min} to ${max}`);
  }
  return value;
}

export function allowedTab(tab: chrome.tabs.Tab, policy: Policy): void {
  const url = tab.url ?? tab.pendingUrl ?? "";
  if (!siteAllowed(url, policy.sites, policy.all_sites, policy.blocked_sites)) {
    const host = hostOfUrl(url);
    throw new ToolError(
      "site_not_allowed",
      host
        ? `${host} isn't on the sites the person allowed for Chat with Work on this computer`
        : "this tab isn't on a site the person allowed for Chat with Work on this computer",
    );
  }
}

// The tab a call acts on, checked, with the indicator showing.
export async function tabFor(params: Params, policy: Policy): Promise<chrome.tabs.Tab> {
  const tab = await resolveTab(params.tab);
  allowedTab(tab, policy);
  await indicate(tab.id!);
  return tab;
}

// After reading: the tab must still be in the group and on an allowed site,
// or nothing read goes out.
export async function still(tabId: number, policy: Policy): Promise<chrome.tabs.Tab> {
  const tab = await resolveTab(tabId);
  allowedTab(tab, policy);
  return tab;
}

export function about(tab: chrome.tabs.Tab): { tab: number; url: string; title: string } {
  return { tab: tab.id!, url: tab.url ?? "", title: tab.title ?? "" };
}

export function withDialog<T extends object>(tabId: number, result: T): T & { dialog?: string } {
  const dialog = takeDialog(tabId);
  return dialog ? { ...result, dialog: `dismissed ${dialog}` } : result;
}

export async function elementFacts(tabId: number, backendNodeId: number, role = "", name = ""): Promise<ElementFacts> {
  const { node } = await send<{ node: { nodeName: string; attributes?: string[] } }>(tabId, "DOM.describeNode", {
    backendNodeId,
  }).catch(() => {
    throw new ToolError("not_found", `e${backendNodeId} isn't on the page any more; read the page again`);
  });
  const attrs: Record<string, string> = {};
  const list = node.attributes ?? [];
  for (let i = 0; i + 1 < list.length; i += 2) attrs[list[i].toLowerCase()] = list[i + 1];
  return { tag: node.nodeName.toLowerCase(), attrs, role, name };
}

const tabs: Method = async (params, policy) => {
  only(params, []);
  const list = await groupTabs();
  return {
    tabs: list.map((t) => siteAllowed(t.url ?? "", policy.sites, policy.all_sites, policy.blocked_sites)
      ? { ...about(t), allowed: true, active: t.active, loading: t.status === "loading" }
      : { tab: t.id, site: hostOfUrl(t.url ?? ""), allowed: false }),
  };
};

const read: Method = async (params, policy) => {
  only(params, ["tab", "offset", "max_chars"]);
  const offset = integer(params.offset, "offset", 0, Number.MAX_SAFE_INTEGER, 0);
  const maxChars = integer(params.max_chars, "max_chars", 1, READ_MAX_CHARS, READ_MAX_CHARS);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const document = await documentIdentity(tabId);
  const { nodes, sensitive } = await sensitiveFields(tabId);
  const clickable = await clickableElements(tabId);
  const text = snapshotText(nodes, { sensitive, clickable });
  const now = await still(tabId, policy);
  const current = await documentIdentity(tabId);
  if (!sameDocument(document, current)) throw new ToolError("page_error", "the page changed while reading; read it again");
  const slice = page(text, offset, maxChars);
  rememberRefs(tabId, document, slice.text);
  return withDialog(tabId, { ...about(now), ...slice });
};

const screenshot: Method = async (params, policy) => {
  only(params, ["tab", "max_bytes"]);
  const maxBytes = integer(params.max_bytes, "max_bytes", 20_000, SCREENSHOT_MAX_BYTES, SCREENSHOT_MAX_BYTES);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const shot = await capture(tabId, maxBytes);
  const now = await still(tabId, policy);
  return withDialog(tabId, { ...about(now), ...shot });
};

// A JPEG of what the tab shows, at most `maxBytes` of base64, with
// sensitive fields blurred and the indicator hidden.
export async function capture(
  tabId: number,
  maxBytes: number,
): Promise<{ width: number; height: number; image: { mime_type: string; data: string } }> {
  const metrics = await send<{
    cssVisualViewport: { pageX: number; pageY: number; clientWidth: number; clientHeight: number };
  }>(tabId, "Page.getLayoutMetrics");
  const view = metrics.cssVisualViewport;
  let data = "";
  let scale = Math.min(1, SCREENSHOT_MAX_WIDTH / Math.max(1, view.clientWidth));
  let quality = 70;
  // What this screenshot blurs carries its own token, so a stalled earlier
  // screenshot's late cleanup leaves this one's blur alone.
  const token = crypto.randomUUID();
  // Each field blurred by its node, so those in shadow roots, which a
  // document query doesn't reach, are unblurred after too.
  const blurred: number[] = [];
  try {
    // Nothing is captured unless every field browser_read hides is blurred:
    // a field that can't be found or blurred refuses the screenshot.
    try {
      await prepareForCapture(tabId, SENSITIVE_PATTERNS, token);
      // Also every field browser_read hides, found the same way, including
      // those named only by a label and those in shadow roots.
      const { sensitive } = await sensitiveFields(tabId);
      for (const backendNodeId of sensitive) {
        await callOn(tabId, backendNodeId, BLUR, [true, token]);
        blurred.push(backendNodeId);
      }
    } catch (error) {
      if (error instanceof ToolError) throw error;
      throw new ToolError(
        "page_error",
        "the page changed while hiding its password and payment fields; take the screenshot again",
      );
    }
    for (let attempt = 0; attempt < 5; attempt++) {
      const shot = await send<{ data: string }>(tabId, "Page.captureScreenshot", {
        format: "jpeg",
        quality,
        clip: { x: view.pageX, y: view.pageY, width: view.clientWidth, height: view.clientHeight, scale },
        captureBeyondViewport: false,
      });
      data = shot.data;
      if (data.length <= maxBytes) break;
      quality = Math.max(30, quality - 15);
      scale *= 0.75;
    }
  } finally {
    // Put the fields back even if Stop was pressed meanwhile.
    await cleanUp(tabId, async (options) => {
      for (const backendNodeId of blurred) {
        await callOn(tabId, backendNodeId, BLUR, [false, token], [], options).catch(() => {});
      }
      await afterCapture(tabId, SENSITIVE_PATTERNS, token, options);
    });
  }
  if (data.length > maxBytes) {
    throw new ToolError("too_large", "the screenshot doesn't fit in a tool result");
  }
  return {
    width: Math.round(view.clientWidth * scale),
    height: Math.round(view.clientHeight * scale),
    image: { mime_type: "image/jpeg", data },
  };
}

function scrollPage(direction: string, pages: number): { y: number; height: number; view: number } {
  const amount = Math.round(window.innerHeight * 0.85 * pages) * (direction === "up" ? -1 : 1);
  window.scrollBy({ top: amount, behavior: "instant" as ScrollBehavior });
  const root = document.scrollingElement ?? document.documentElement;
  return { y: Math.round(window.scrollY), height: root.scrollHeight, view: window.innerHeight };
}

const scroll: Method = async (params, policy) => {
  only(params, ["tab", "direction", "pages", "ref"]);
  const backendNodeId = params.ref === undefined ? undefined : refId(params.ref);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  if (backendNodeId !== undefined) {
    await send(tabId, "DOM.scrollIntoViewIfNeeded", { backendNodeId });
  } else {
    const direction = params.direction ?? "down";
    if (direction !== "up" && direction !== "down") {
      throw new ToolError("invalid_argument", "direction must be up or down");
    }
    const pages = integer(params.pages, "pages", 1, 10, 1);
    await evaluate(tabId, `(${scrollPage.toString()})(${JSON.stringify(direction)}, ${pages})`);
  }
  const position = await evaluate<{ y: number; height: number; view: number }>(
    tabId,
    `({ y: Math.round(window.scrollY), height: (document.scrollingElement ?? document.documentElement).scrollHeight, view: window.innerHeight })`,
  );
  const now = await still(tabId, policy);
  return withDialog(tabId, {
    ...about(now),
    scroll_y: position.y,
    page_height: position.height,
    view_height: position.view,
    at_end: position.y + position.view >= position.height - 2,
  });
};

// The page's accessibility tree, and the fields in it the extension never
// reads or shows: passwords, payment cards, one-time codes, judged by their
// attributes and accessible names (a label "Card number" included).
async function sensitiveFields(tabId: number): Promise<{ nodes: AXNode[]; sensitive: Set<number> }> {
  const { nodes } = await send<{ nodes: AXNode[] }>(tabId, "Accessibility.getFullAXTree");
  const sensitive = new Set<number>();
  for (const node of nodes) {
    const role = String(node.role?.value ?? "");
    if (node.ignored || node.backendDOMNodeId === undefined || !EDITABLE_ROLES.has(role)) continue;
    const facts = await elementFacts(tabId, node.backendDOMNodeId, role, String(node.name?.value ?? "")).catch(
      () => null,
    );
    // A field the extension can't look at is treated as sensitive.
    if (!facts || isSensitiveField(facts)) sensitive.add(node.backendDOMNodeId);
  }
  return { nodes, sensitive };
}

interface DomSnapshot {
  strings: string[];
  documents: {
    nodes: {
      parentIndex?: number[];
      nodeType?: number[];
      nodeName?: number[];
      backendNodeId?: number[];
      attributes?: number[][];
      isClickable?: { index: number[] };
    };
    layout: { nodeIndex: number[]; styles: number[][]; bounds: number[][] };
  }[];
}

// Interactive by what they are, so the accessibility tree already offers them.
const CONTROL_TAGS = new Set([
  "button", "input", "select", "textarea", "option", "label", "summary", "details", "iframe", "frame",
  "video", "audio", "embed", "object",
]);
const CONTROL_ROLES = new Set([
  "button", "link", "menuitem", "menuitemcheckbox", "menuitemradio", "checkbox", "radio", "switch", "tab",
  "option", "treeitem", "textbox", "searchbox", "combobox", "listbox", "slider", "spinbutton", "scrollbar",
]);
const NEVER_CLICKABLE = new Set(["html", "body", "head", "script", "style", "template", "noscript"]);
const CLICKABLE_LIMIT = 2_000;

// Elements in the page's own document that are evidently clickable though
// unlabelled: the browser knows of a click handler on them (or `onclick`),
// they take focus (`tabindex`), or they start a `cursor: pointer` region.
// Controls, what's inside one, and wrappers around one are left to the
// accessibility tree. Read from a DOM snapshot: no page script runs and
// nothing on the page changes. A page this can't read gets no extra refs.
async function clickableElements(tabId: number): Promise<Set<number>> {
  const found = new Set<number>();
  const snapshot = await send<DomSnapshot>(tabId, "DOMSnapshot.captureSnapshot", {
    computedStyles: ["cursor", "visibility"],
  }).catch((error) => {
    if (error instanceof ToolError && error.code === "browser_stopped") throw error;
    return undefined;
  });
  const doc = snapshot?.documents[0];
  if (!snapshot || !doc) return found;
  const { strings } = snapshot;
  const nodes = doc.nodes;
  const parents = nodes.parentIndex ?? [];
  const count = parents.length;
  const tag = (i: number) => (strings[nodes.nodeName?.[i] ?? -1] ?? "").toLowerCase();
  const attrs = (i: number) => {
    const list = nodes.attributes?.[i] ?? [];
    const out: Record<string, string> = {};
    for (let a = 0; a + 1 < list.length; a += 2) out[(strings[list[a]] ?? "").toLowerCase()] = strings[list[a + 1]] ?? "";
    return out;
  };
  const layout = new Map<number, number>();
  doc.layout.nodeIndex.forEach((node, index) => layout.set(node, index));
  const style = (i: number, which: number) => {
    const index = layout.get(i);
    return index === undefined ? "" : strings[doc.layout.styles[index]?.[which] ?? -1] ?? "";
  };
  // The nearest element above, through shadow roots to their hosts.
  const parentElement = (i: number) => {
    let p = parents[i] ?? -1;
    while (p >= 0 && nodes.nodeType?.[p] !== 1) p = parents[p] ?? -1;
    return p;
  };
  const control = new Array<boolean>(count).fill(false);
  for (let i = 0; i < count; i++) {
    if (nodes.nodeType?.[i] !== 1) continue;
    const name = tag(i);
    const a = attrs(i);
    const role = (a.role ?? "").trim().toLowerCase();
    // A hidden input is no control anyone clicks; it doesn't hide its clickable wrapper.
    if (name === "input" && (a.type ?? "").trim().toLowerCase() === "hidden") continue;
    control[i] = CONTROL_TAGS.has(name) || (name === "a" && a.href !== undefined) || CONTROL_ROLES.has(role) ||
      (a.contenteditable !== undefined && a.contenteditable !== "false");
  }
  // Inside a control, or around one.
  const insideControl = new Array<boolean>(count).fill(false);
  const aroundControl = new Array<boolean>(count).fill(false);
  for (let i = 0; i < count; i++) {
    if (nodes.nodeType?.[i] !== 1) continue;
    for (let p = parentElement(i); p >= 0; p = parentElement(p)) {
      if (control[p]) { insideControl[i] = true; break; }
    }
    if (control[i]) for (let p = parentElement(i); p >= 0 && !aroundControl[p]; p = parentElement(p)) aroundControl[p] = true;
  }
  const clickHandler = new Set(nodes.isClickable?.index ?? []);
  for (let i = 0; i < count && found.size < CLICKABLE_LIMIT; i++) {
    if (nodes.nodeType?.[i] !== 1 || control[i] || insideControl[i] || aroundControl[i]) continue;
    const name = tag(i);
    if (NEVER_CLICKABLE.has(name)) continue;
    const index = layout.get(i);
    const bounds = index === undefined ? undefined : doc.layout.bounds[index];
    if (!bounds || !(bounds[2] > 0 && bounds[3] > 0) || style(i, 1) !== "visible") continue;
    const a = attrs(i);
    const pointer = style(i, 0) === "pointer";
    const parent = parentElement(i);
    const startsPointer = pointer && (parent < 0 || style(parent, 0) !== "pointer");
    if (clickHandler.has(i) || a.onclick !== undefined || a.tabindex !== undefined || startsPointer) {
      const backend = nodes.backendNodeId?.[i];
      if (backend !== undefined) found.add(backend);
    }
  }
  return found;
}

// One field's blur, marked with the capture's token like blurSensitive's.
const BLUR = `function (on, token) {
  const ATTR = "data-chat-with-work-blur";
  const others = (this.getAttribute(ATTR) ?? "").split(/\\s+/).filter((t) => t && t !== token);
  if (on) {
    this.setAttribute(ATTR, [...others, token].join(" "));
    this.style.setProperty("filter", "blur(8px)", "important");
  } else if (this.hasAttribute(ATTR)) {
    if (others.length) {
      this.setAttribute(ATTR, others.join(" "));
    } else {
      this.removeAttribute(ATTR);
      this.style.removeProperty("filter");
    }
  }
}`;

export function refId(ref: unknown): number {
  const match = typeof ref === "string" ? /^e(\d{1,12})$/.exec(ref) : null;
  if (!match) throw new ToolError("invalid_argument", "ref must look like e123, from browser_read");
  return Number(match[1]);
}

const wait: Method = async (params, policy) => {
  only(params, ["tab", "text", "seconds"]);
  const seconds = integer(params.seconds, "seconds", 1, WAIT_MAX_SECONDS, params.text ? 10 : 2);
  const text = params.text;
  if (text !== undefined && (typeof text !== "string" || text.length === 0 || text.length > 500)) {
    throw new ToolError("invalid_argument", "text must be 1 to 500 characters");
  }
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const started = Date.now();
  const deadline = started + seconds * 1000;
  let found = false;
  while (Date.now() < deadline) {
    const state = await evaluate<{ ready: string; found: boolean }>(
      tabId,
      `({ ready: document.readyState, found: ${
        text ? `(document.body?.innerText ?? "").toLowerCase().includes(${JSON.stringify(text.toLowerCase())})` : "false"
      } })`,
    ).catch((error) => {
      if (error instanceof ToolError && error.code === "browser_stopped") throw error;
      return { ready: "loading", found: false };
    });
    // The page's raw text is only a quick first look: a match counts only
    // if it's in what browser_read shows, so a wait can't find text in a
    // password, card or one-time code field.
    if (text ? state.found && (await readableIncludes(tabId, text)) : state.ready === "complete" && Date.now() - started >= 500) {
      found = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  const now = await still(tabId, policy);
  return withDialog(tabId, { ...about(now), found, waited_ms: Date.now() - started });
};

// Whether the text browser_read would show for the page includes `text`.
async function readableIncludes(tabId: number, text: string): Promise<boolean> {
  const { nodes, sensitive } = await sensitiveFields(tabId).catch((error) => {
    if (error instanceof ToolError && error.code === "browser_stopped") throw error;
    return { nodes: [] as AXNode[], sensitive: new Set<number>() };
  });
  return snapshotText(nodes, { sensitive }).toLowerCase().includes(text.toLowerCase());
}

export const methods: Record<string, Method> = { tabs, read, screenshot, scroll, wait };
