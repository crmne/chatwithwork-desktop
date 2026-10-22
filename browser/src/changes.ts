// What the extension does to pages, only while the person allowed actions
// on the computer: open, go back, close, click, type, choose, and fetch a
// file for the daemon to save. Each takes `dry_run`, which checks the call
// and describes it (with a screenshot, the target outlined, for the
// approval card) without doing it.

import {
  about,
  capture,
  elementFacts,
  only,
  refId,
  still,
  tabFor,
  withDialog,
  type Method,
  type Params,
  type Policy,
} from "./actions.js";
import { callOn, send } from "./cdp.js";
import { isSensitiveField, submitReason, type ElementFacts } from "./classify.js";
import { ToolError } from "./errors.js";
import { openInGroup, resolveTab } from "./group.js";
import { indicate } from "./page.js";
import { hostOfUrl, siteAllowed } from "./sites.js";

const PREVIEW_MAX_BYTES = 120_000;
const TYPE_MAX_CHARS = 10_000;
const DOWNLOAD_MAX_BYTES = 10 * 1024 * 1024;
const LOAD_TIMEOUT_MS = 10_000;

function allowedChanges(policy: Policy): void {
  if (!policy.changes) {
    throw new ToolError(
      "not_writable",
      "the person allowed Chat with Work only to read in the browser; they can allow actions on their computer",
    );
  }
}

function dryRun(params: Params): boolean {
  if (params.dry_run !== undefined && typeof params.dry_run !== "boolean") {
    throw new ToolError("invalid_argument", "dry_run must be true or false");
  }
  return params.dry_run === true;
}

function flag(params: Params, name: string, fallback: boolean): boolean {
  const value = params[name];
  if (value === undefined) return fallback;
  if (typeof value !== "boolean") throw new ToolError("invalid_argument", `${name} must be true or false`);
  return value;
}

function checkUrl(value: unknown, policy: Policy): string {
  if (typeof value !== "string" || value.length > 4096) {
    throw new ToolError("invalid_argument", "url must be an address of up to 4096 characters");
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new ToolError("invalid_argument", "url must be a full address, like https://github.com/");
  }
  if (!siteAllowed(url.href, policy.sites)) {
    throw new ToolError(
      "site_not_allowed",
      `${url.host || url.protocol} isn't on the sites the person allowed for Chat with Work on this computer`,
    );
  }
  return url.href;
}

// Wait for a navigation the call may have started, then answer with where
// the tab is: on an allowed site, its address and title; elsewhere, only
// the site, and nothing from the page.
async function settle(tabId: number, policy: Policy, started = Date.now()): Promise<object> {
  await new Promise((r) => setTimeout(r, 400));
  while (Date.now() - started < LOAD_TIMEOUT_MS) {
    const tab = await chrome.tabs.get(tabId).catch(() => undefined);
    if (!tab || tab.status === "complete") break;
    await new Promise((r) => setTimeout(r, 150));
  }
  const tab = await chrome.tabs.get(tabId).catch(() => undefined);
  if (!tab) return { tab: tabId, closed: true };
  const url = tab.url ?? tab.pendingUrl ?? "";
  if (!siteAllowed(url, policy.sites)) {
    return {
      tab: tabId,
      allowed: false,
      site: hostOfUrl(url),
      note: "the tab went to a site the person didn't allow; nothing there is read or done",
    };
  }
  return withDialog(tabId, { ...about(tab), loading: tab.status !== "complete" });
}

interface Target {
  tabId: number;
  backendNodeId: number;
  facts: ElementFacts;
}

const FORM_OF = `function () {
  const form = this.form || (this.closest && this.closest("form"));
  return form ? { method: (form.getAttribute("method") || "get").toLowerCase() } : null;
}`;

// The element a call names, with what the extension needs to judge it.
async function target(params: Params, policy: Policy): Promise<Target> {
  const backendNodeId = refId(params.ref);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const ax = await send<{ nodes: { backendDOMNodeId?: number; ignored?: boolean; role?: { value?: unknown }; name?: { value?: unknown } }[] }>(
    tabId,
    "Accessibility.getPartialAXTree",
    { backendNodeId, fetchRelatives: false },
  ).catch(() => ({ nodes: [] }));
  const node = ax.nodes.find((n) => n.backendDOMNodeId === backendNodeId) ?? ax.nodes[0];
  const facts = await elementFacts(tabId, backendNodeId, String(node?.role?.value ?? ""), String(node?.name?.value ?? ""));
  facts.form = await callOn<{ method: string } | null>(tabId, backendNodeId, FORM_OF).catch(() => null);
  return { tabId, backendNodeId, facts };
}

function described(facts: ElementFacts): object {
  return {
    role: facts.role || facts.tag,
    name: facts.name.slice(0, 200),
    tag: facts.tag,
    ...(facts.form ? { in_form: true } : {}),
  };
}

const OUTLINE = `function (on) {
  if (on) {
    this.setAttribute("data-chat-with-work-target", "");
    this.style.setProperty("outline", "3px solid #f97316", "important");
    this.style.setProperty("outline-offset", "2px", "important");
  } else if (this.hasAttribute("data-chat-with-work-target")) {
    this.removeAttribute("data-chat-with-work-target");
    this.style.removeProperty("outline");
    this.style.removeProperty("outline-offset");
  }
}`;

// A screenshot with the target outlined, for the approval card.
async function preview(t: Target): Promise<object> {
  await send(t.tabId, "DOM.scrollIntoViewIfNeeded", { backendNodeId: t.backendNodeId }).catch(() => {});
  await callOn(t.tabId, t.backendNodeId, OUTLINE, [true]).catch(() => {});
  try {
    return await capture(t.tabId, PREVIEW_MAX_BYTES);
  } finally {
    await callOn(t.tabId, t.backendNodeId, OUTLINE, [false]).catch(() => {});
  }
}

const CONTAINS = `function (hit) { return this === hit || this.contains(hit); }`;

// The middle of the element, checked to be the element itself and not
// something drawn over it.
async function point(t: Target): Promise<{ x: number; y: number }> {
  await send(t.tabId, "DOM.scrollIntoViewIfNeeded", { backendNodeId: t.backendNodeId });
  const { quads } = await send<{ quads: number[][] }>(t.tabId, "DOM.getContentQuads", {
    backendNodeId: t.backendNodeId,
  }).catch(() => ({ quads: [] as number[][] }));
  const quad = quads.find((q) => q.length === 8);
  if (!quad) throw new ToolError("page_error", "that element isn't visible on the page");
  const x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4;
  const y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4;
  const hit = await send<{ backendNodeId: number }>(t.tabId, "DOM.getNodeForLocation", {
    x: Math.round(x),
    y: Math.round(y),
    includeUserAgentShadowDOM: false,
  }).catch(() => null);
  const covered =
    !hit || !(await callOn<boolean>(t.tabId, t.backendNodeId, CONTAINS, [], [hit.backendNodeId]).catch(() => false));
  if (covered) {
    throw new ToolError(
      "page_error",
      "something else is drawn over that element (a dialog or a banner?); read the page again",
    );
  }
  return { x, y };
}

const open: Method = async (params, policy) => {
  only(params, ["url", "tab", "dry_run"]);
  allowedChanges(policy);
  const url = checkUrl(params.url, policy);
  const dry = dryRun(params);
  if (params.tab !== undefined) {
    const tab = await resolveTab(params.tab);
    if (dry) return { dry_run: true, url, tab: tab.id, new_tab: false };
    await indicate(tab.id!).catch(() => {});
    const started = Date.now();
    await chrome.tabs.update(tab.id!, { url });
    return { dry_run: false, new_tab: false, ...(await settle(tab.id!, policy, started)) };
  }
  if (dry) return { dry_run: true, url, new_tab: true };
  const started = Date.now();
  const tab = await openInGroup(url);
  return { dry_run: false, new_tab: true, ...(await settle(tab.id!, policy, started)) };
};

// Going back works from a page off the allowed sites too, so a tab that
// wandered off can come back; nothing of that page is read.
const back: Method = async (params, policy) => {
  only(params, ["tab", "dry_run"]);
  allowedChanges(policy);
  const tab = await resolveTab(params.tab);
  const here = siteAllowed(tab.url ?? "", policy.sites) ? about(tab) : { tab: tab.id, site: hostOfUrl(tab.url ?? "") };
  if (dryRun(params)) return { dry_run: true, ...here };
  const started = Date.now();
  await chrome.tabs.goBack(tab.id!).catch(() => {
    throw new ToolError("page_error", "this tab has nothing to go back to");
  });
  return { dry_run: false, ...(await settle(tab.id!, policy, started)) };
};

const close: Method = async (params, policy) => {
  only(params, ["tab", "dry_run"]);
  allowedChanges(policy);
  if (params.tab === undefined) throw new ToolError("invalid_argument", "tab is required");
  const tab = await resolveTab(params.tab);
  const facts = siteAllowed(tab.url ?? "", policy.sites) ? about(tab) : { tab: tab.id, site: hostOfUrl(tab.url ?? "") };
  if (dryRun(params)) return { dry_run: true, ...facts };
  await chrome.tabs.remove(tab.id!);
  return { dry_run: false, closed: true, ...facts };
};

const click: Method = async (params, policy) => {
  only(params, ["tab", "ref", "submits", "dry_run"]);
  allowedChanges(policy);
  const saysSubmits = flag(params, "submits", true);
  const dry = dryRun(params);
  const t = await target(params, policy);
  const reason = submitReason(t.facts);
  if (!saysSubmits && reason) {
    throw new ToolError(
      "would_submit",
      `this click may send, buy, post or delete something (${reason}); call again with submits: true so the person sees it first`,
    );
  }
  const what = { target: described(t.facts), submits: reason !== null || saysSubmits, ...(reason ? { reason } : {}) };
  if (dry) {
    const shot = await preview(t);
    const now = await still(t.tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  const { x, y } = await point(t);
  const started = Date.now();
  const mouse = (type: string, extra: object = {}) =>
    send(t.tabId, "Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra });
  await mouse("mouseMoved", { button: "none", clickCount: 0 });
  await mouse("mousePressed", { buttons: 1 });
  await mouse("mouseReleased", { buttons: 0 });
  return { dry_run: false, ...what, ...(await settle(t.tabId, policy, started)) };
};

const TEXT_TYPES = new Set(["", "text", "search", "email", "url", "tel", "number", "password"]);

function editable(facts: ElementFacts): boolean {
  const type = (facts.attrs.type ?? "").toLowerCase();
  return (
    (facts.tag === "input" && TEXT_TYPES.has(type)) ||
    facts.tag === "textarea" ||
    (facts.attrs.contenteditable !== undefined && facts.attrs.contenteditable !== "false") ||
    ["textbox", "searchbox", "combobox", "spinbutton"].includes(facts.role)
  );
}

const SELECT_CONTENTS = `function (clear) {
  this.focus();
  if (typeof this.select === "function" && clear) { this.select(); return; }
  if (typeof this.setSelectionRange === "function") {
    const end = (this.value || "").length;
    this.setSelectionRange(clear ? 0 : end, end);
    return;
  }
  const range = document.createRange();
  range.selectNodeContents(this);
  if (!clear) range.collapse(false);
  const selection = getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
}`;

const type: Method = async (params, policy) => {
  only(params, ["tab", "ref", "text", "submit", "clear", "dry_run"]);
  allowedChanges(policy);
  const text = params.text;
  if (typeof text !== "string" || Array.from(text).length > TYPE_MAX_CHARS) {
    throw new ToolError("invalid_argument", `text must be a string of up to ${TYPE_MAX_CHARS} characters`);
  }
  const submit = flag(params, "submit", false);
  const clear = flag(params, "clear", true);
  const dry = dryRun(params);
  const t = await target(params, policy);
  if (isSensitiveField(t.facts)) {
    throw new ToolError(
      "sensitive_field",
      "Chat with Work never types into password, payment or one-time code fields; the person fills those in themselves",
    );
  }
  if (!editable(t.facts)) throw new ToolError("invalid_argument", "that element isn't a field to type in");
  const what = { target: described(t.facts), text, submit, clear };
  if (dry) {
    const shot = await preview(t);
    const now = await still(t.tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  await send(t.tabId, "DOM.focus", { backendNodeId: t.backendNodeId }).catch(() => {});
  await callOn(t.tabId, t.backendNodeId, SELECT_CONTENTS, [clear]);
  if (text.length > 0) {
    await send(t.tabId, "Input.insertText", { text });
  } else if (clear) {
    await send(t.tabId, "Input.dispatchKeyEvent", { type: "keyDown", key: "Delete", code: "Delete", windowsVirtualKeyCode: 46 });
    await send(t.tabId, "Input.dispatchKeyEvent", { type: "keyUp", key: "Delete", code: "Delete", windowsVirtualKeyCode: 46 });
  }
  const started = Date.now();
  if (submit) {
    const enter = { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 };
    await send(t.tabId, "Input.dispatchKeyEvent", { type: "keyDown", text: "\r", ...enter });
    await send(t.tabId, "Input.dispatchKeyEvent", { type: "keyUp", ...enter });
  }
  return { dry_run: false, target: what.target, submit, ...(await settle(t.tabId, policy, started)) };
};

const CHOOSE = `function (values, apply) {
  if (!(this instanceof HTMLSelectElement)) return { error: "not_select" };
  const options = Array.from(this.options);
  const chosen = [];
  for (const value of values) {
    const option = options.find((o) => o.value === value || o.label.trim() === value || o.text.trim() === value);
    if (!option) return { missing: value, options: options.slice(0, 50).map((o) => o.label.trim() || o.value) };
    if (option.disabled) return { disabled: value };
    chosen.push(option);
  }
  if (!this.multiple && chosen.length > 1) return { error: "single" };
  if (apply) {
    for (const option of options) option.selected = chosen.includes(option);
    this.dispatchEvent(new Event("input", { bubbles: true }));
    this.dispatchEvent(new Event("change", { bubbles: true }));
  }
  return { chosen: chosen.map((o) => o.label.trim() || o.value) };
}`;

const select: Method = async (params, policy) => {
  only(params, ["tab", "ref", "values", "dry_run"]);
  allowedChanges(policy);
  const values = params.values;
  if (!Array.isArray(values) || values.length === 0 || values.length > 50 || !values.every((v) => typeof v === "string")) {
    throw new ToolError("invalid_argument", "values must be 1 to 50 option values or labels");
  }
  const dry = dryRun(params);
  const t = await target(params, policy);
  const result = await callOn<any>(t.tabId, t.backendNodeId, CHOOSE, [values, !dry]);
  if (result?.error === "not_select") throw new ToolError("invalid_argument", "that element isn't a list to choose from");
  if (result?.error === "single") throw new ToolError("invalid_argument", "that list takes one choice");
  if (result?.missing !== undefined) {
    throw new ToolError(
      "invalid_argument",
      `“${String(result.missing).slice(0, 100)}” isn't one of its options: ${result.options.join(", ").slice(0, 1000)}`,
    );
  }
  if (result?.disabled !== undefined) throw new ToolError("invalid_argument", "that option can't be chosen");
  const what = { target: described(t.facts), chosen: result.chosen };
  if (dry) {
    const shot = await preview(t);
    const now = await still(t.tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  const now = await still(t.tabId, policy);
  return withDialog(t.tabId, { dry_run: false, ...about(now), ...what });
};

const HREF = `function () { return this.href || this.src || this.currentSrc || this.getAttribute("href") || ""; }`;

// The bytes of a file, fetched with the person's sign-in to the site, for
// the daemon to save in a shared folder.
const fetchFile: Method = async (params, policy) => {
  only(params, ["tab", "ref", "url", "dry_run"]);
  allowedChanges(policy);
  if ((params.url === undefined) === (params.ref === undefined)) {
    throw new ToolError("invalid_argument", "pass either url or ref");
  }
  let address: string;
  if (params.ref !== undefined) {
    const backendNodeId = refId(params.ref);
    const tab = await tabFor(params, policy);
    const href = await callOn<string>(tab.id!, backendNodeId, HREF);
    if (!href) throw new ToolError("invalid_argument", "that element doesn't link to a file");
    address = checkUrl(new URL(href, tab.url).href, policy);
  } else {
    address = checkUrl(params.url, policy);
  }
  const name = decodeURIComponent(new URL(address).pathname.split("/").pop() || "download");
  if (dryRun(params)) return { dry_run: true, url: address, name };
  const response = await fetch(address, { credentials: "include", redirect: "follow" }).catch((error) => {
    throw new ToolError("page_error", `couldn't fetch the file: ${String(error?.message ?? error)}`);
  });
  if (!siteAllowed(response.url, policy.sites)) {
    throw new ToolError("site_not_allowed", `the file is on ${hostOfUrl(response.url)}, which isn't allowed`);
  }
  if (!response.ok) throw new ToolError("page_error", `the site answered ${response.status}`);
  const declared = Number(response.headers.get("content-length") ?? "0");
  if (declared > DOWNLOAD_MAX_BYTES) throw new ToolError("too_large", "the file is over 10 MB");
  const reader = response.body?.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (reader) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > DOWNLOAD_MAX_BYTES) {
      await reader.cancel();
      throw new ToolError("too_large", "the file is over 10 MB");
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return {
    dry_run: false,
    url: response.url,
    name: dispositionName(response.headers.get("content-disposition")) ?? name,
    mime_type: (response.headers.get("content-type") ?? "").split(";")[0].trim().slice(0, 100),
    size,
    data: btoa(binary),
  };
};

function dispositionName(header: string | null): string | null {
  if (!header) return null;
  const star = /filename\*\s*=\s*UTF-8''([^;]+)/i.exec(header);
  if (star) {
    try {
      return decodeURIComponent(star[1].trim());
    } catch {
      // fall through
    }
  }
  const plain = /filename\s*=\s*"?([^";]+)"?/i.exec(header);
  return plain ? plain[1].trim() : null;
}

export const changeMethods: Record<string, Method> = {
  open,
  back,
  close,
  click,
  type,
  select,
  fetch: fetchFile,
};
