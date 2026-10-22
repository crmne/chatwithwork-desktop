// What the daemon can ask of the extension. Every method checks, itself,
// that the tab is in the Chat with Work group and on a site the person
// allowed, whatever the daemon decided.

import { evaluate, send, takeDialog } from "./cdp.js";
import { isSensitiveField, SENSITIVE_PATTERNS, type ElementFacts } from "./classify.js";
import { ToolError } from "./errors.js";
import { groupTabs, resolveTab } from "./group.js";
import { afterCapture, indicate, prepareForCapture } from "./page.js";
import { hostOfUrl, siteAllowed } from "./sites.js";
import { page, snapshotText, type AXNode } from "./snapshot.js";

export interface Policy {
  sites: string[];
  // Stopped from the tray or `cww browser stop`.
  stopped: boolean;
}

type Params = Record<string, unknown>;
type Method = (params: Params, policy: Policy) => Promise<unknown>;

const READ_MAX_CHARS = 32_000;
const SCREENSHOT_MAX_BYTES = 180_000;
const SCREENSHOT_MAX_WIDTH = 1280;
const WAIT_MAX_SECONDS = 15;

const EDITABLE_ROLES = new Set(["textbox", "searchbox", "combobox", "spinbutton", "textarea"]);

function only(params: Params, allowed: string[]): void {
  for (const key of Object.keys(params)) {
    if (!allowed.includes(key)) throw new ToolError("invalid_argument", `unknown argument ${key}`);
  }
}

function integer(value: unknown, name: string, min: number, max: number, fallback: number): number {
  if (value === undefined || value === null) return fallback;
  if (typeof value !== "number" || !Number.isInteger(value) || value < min || value > max) {
    throw new ToolError("invalid_argument", `${name} must be a whole number from ${min} to ${max}`);
  }
  return value;
}

function allowedTab(tab: chrome.tabs.Tab, policy: Policy): void {
  const url = tab.url ?? tab.pendingUrl ?? "";
  if (!siteAllowed(url, policy.sites)) {
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
async function tabFor(params: Params, policy: Policy): Promise<chrome.tabs.Tab> {
  const tab = await resolveTab(params.tab);
  allowedTab(tab, policy);
  await indicate(tab.id!);
  return tab;
}

// After reading: the tab must still be in the group and on an allowed site,
// or nothing read goes out.
async function still(tabId: number, policy: Policy): Promise<chrome.tabs.Tab> {
  const tab = await resolveTab(tabId);
  allowedTab(tab, policy);
  return tab;
}

function about(tab: chrome.tabs.Tab): { tab: number; url: string; title: string } {
  return { tab: tab.id!, url: tab.url ?? "", title: tab.title ?? "" };
}

function withDialog<T extends object>(tabId: number, result: T): T & { dialog?: string } {
  const dialog = takeDialog(tabId);
  return dialog ? { ...result, dialog: `dismissed ${dialog}` } : result;
}

async function elementFacts(tabId: number, backendNodeId: number, role = "", name = ""): Promise<ElementFacts> {
  const { node } = await send<{ node: { nodeName: string; attributes?: string[] } }>(tabId, "DOM.describeNode", {
    backendNodeId,
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
    tabs: list.map((t) => ({
      ...about(t),
      allowed: siteAllowed(t.url ?? "", policy.sites),
      active: t.active,
      loading: t.status === "loading",
    })),
  };
};

const read: Method = async (params, policy) => {
  only(params, ["tab", "offset", "max_chars"]);
  const offset = integer(params.offset, "offset", 0, Number.MAX_SAFE_INTEGER, 0);
  const maxChars = integer(params.max_chars, "max_chars", 1, READ_MAX_CHARS, READ_MAX_CHARS);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
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
  const text = snapshotText(nodes, { sensitive });
  const now = await still(tabId, policy);
  return withDialog(tabId, { ...about(now), ...page(text, offset, maxChars) });
};

const screenshot: Method = async (params, policy) => {
  only(params, ["tab", "max_bytes"]);
  const maxBytes = integer(params.max_bytes, "max_bytes", 20_000, SCREENSHOT_MAX_BYTES, SCREENSHOT_MAX_BYTES);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const metrics = await send<{
    cssVisualViewport: { pageX: number; pageY: number; clientWidth: number; clientHeight: number };
  }>(tabId, "Page.getLayoutMetrics");
  const view = metrics.cssVisualViewport;
  await prepareForCapture(tabId, SENSITIVE_PATTERNS);
  let data = "";
  let scale = Math.min(1, SCREENSHOT_MAX_WIDTH / Math.max(1, view.clientWidth));
  let quality = 70;
  try {
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
    await afterCapture(tabId, SENSITIVE_PATTERNS);
  }
  if (data.length > maxBytes) {
    throw new ToolError("too_large", "the screenshot doesn't fit in a tool result");
  }
  const now = await still(tabId, policy);
  return withDialog(tabId, {
    ...about(now),
    width: Math.round(view.clientWidth * scale),
    height: Math.round(view.clientHeight * scale),
    image: { mime_type: "image/jpeg", data },
  });
};

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
    ).catch(() => ({ ready: "loading", found: false }));
    if (text ? state.found : state.ready === "complete" && Date.now() - started >= 500) {
      found = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  const now = await still(tabId, policy);
  return withDialog(tabId, { ...about(now), found, waited_ms: Date.now() - started });
};

export const methods: Record<string, Method> = { tabs, read, screenshot, scroll, wait };
