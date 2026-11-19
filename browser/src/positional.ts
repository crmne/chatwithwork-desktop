// Acting where a page gives no usable element: by position on a screenshot
// the model was just given, and by keyboard. A position means nothing on
// its own, so every positional action names the screenshot it was read
// from (`capture`), and is refused if the page has since moved, resized,
// zoomed or navigated: "the page changed, look again", never a click
// somewhere else. What's under the point is classified exactly as the
// ref-based click classifies its target (classify.ts, changes.ts), so a
// position is never a way around a refusal the click would give.

import { about, only, still, tabFor, withDialog, type Method, type Params, type Policy } from "./actions.js";
import { allowedChanges, described, dryRun, flag, pressAt, preview, settle, target, type Target } from "./changes.js";
import { isolatedContext, evaluateInContext, send, cleanUp } from "./cdp.js";
import { isSensitiveField, submitReason, type ElementFacts } from "./classify.js";
import { ToolError } from "./errors.js";
import { capture as captureTab } from "./actions.js";
import { hostOfUrl, siteAllowed } from "./sites.js";

// One screenshot: where the page stood when it was taken, and its picture's
// size, which maps the model's pixels to the page's.
export interface Capture {
  id: string;
  url: string;
  loaderId: string;
  scrollX: number;
  scrollY: number;
  width: number;
  height: number;
  zoom: number;
  imageWidth: number;
  imageHeight: number;
}

const captures = new Map<number, Capture>();
const PREVIEW_BYTES = 120_000;
const LOOK_BYTES = 180_000;
const SETTLE_MS = 400;

// The width and height a JPEG says it has (its SOF segment).
export function jpegSize(base64: string): { width: number; height: number } | null {
  // The header is near the start: decode only that much, whole 4-character groups.
  const length = Math.min(base64.length, 87_380);
  const bytes = Uint8Array.from(atob(base64.slice(0, length - (length % 4))), (c) => c.charCodeAt(0));
  return jpegSizeOf(bytes);
}

export function jpegSizeOf(bytes: Uint8Array): { width: number; height: number } | null {
  let i = 2;
  while (i + 9 < bytes.length) {
    if (bytes[i] !== 0xff) return null;
    const marker = bytes[i + 1];
    const length = (bytes[i + 2] << 8) | bytes[i + 3];
    if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
      return { height: (bytes[i + 5] << 8) | bytes[i + 6], width: (bytes[i + 7] << 8) | bytes[i + 8] };
    }
    i += 2 + length;
  }
  return null;
}

interface Where { url: string; loaderId: string; scrollX: number; scrollY: number; width: number; height: number; zoom: number }

async function where(tabId: number): Promise<Where> {
  const metrics = await send<{ cssVisualViewport: { pageX: number; pageY: number; clientWidth: number; clientHeight: number; zoom?: number } }>(
    tabId, "Page.getLayoutMetrics");
  const { frameTree } = await send<{ frameTree: { frame: { url: string; loaderId: string } } }>(tabId, "Page.getFrameTree");
  const view = metrics.cssVisualViewport;
  // Pixels on screen don't matter: a picture's pixels map to the page's
  // CSS pixels by its size, and a browser zoom changes the CSS viewport's
  // width. Only a pinch zoom shows here.
  return {
    url: frameTree.frame.url, loaderId: frameTree.frame.loaderId,
    scrollX: view.pageX, scrollY: view.pageY, width: view.clientWidth, height: view.clientHeight,
    zoom: Number((view.zoom ?? 1).toFixed(3)),
  };
}

// Remember a screenshot just taken of `tabId`, and answer its id.
export async function remember(tabId: number, before: Where, data: string): Promise<Capture> {
  const size = jpegSize(data) ?? { width: before.width, height: before.height };
  const capture: Capture = { id: crypto.randomUUID().slice(0, 12), ...before, imageWidth: size.width, imageHeight: size.height };
  captures.set(tabId, capture);
  return capture;
}

// A screenshot the model can point at: the picture, its size, and its id.
export async function look(tabId: number, maxBytes = LOOK_BYTES): Promise<{ capture: string; width: number; height: number; image: { mime_type: string; data: string } }> {
  const before = await where(tabId);
  const shot = await captureTab(tabId, maxBytes);
  const after = await where(tabId);
  if (!same(before, after)) throw new ToolError("page_error", "the page moved while it was pictured; look again");
  const capture = await remember(tabId, before, shot.image.data);
  return { capture: capture.id, width: capture.imageWidth, height: capture.imageHeight, image: shot.image };
}

function same(a: Where, b: Where): boolean {
  return a.url === b.url && a.loaderId === b.loaderId && Math.abs(a.scrollX - b.scrollX) < 1 && Math.abs(a.scrollY - b.scrollY) < 1 &&
    Math.abs(a.width - b.width) < 1 && Math.abs(a.height - b.height) < 1 && Math.abs(a.zoom - b.zoom) < 0.01;
}

// The screenshot `id` names, if the page still stands as it did then.
async function current(tabId: number, id: unknown): Promise<Capture> {
  if (typeof id !== "string" || !id) throw new ToolError("invalid_argument", "capture is the id of the screenshot the position comes from");
  const capture = captures.get(tabId);
  if (!capture || capture.id !== id) {
    throw new ToolError("page_error", "the page changed, look again: that isn't this tab's latest screenshot");
  }
  if (!same(capture, await where(tabId))) {
    captures.delete(tabId);
    throw new ToolError("page_error", "the page changed, look again: it moved, resized, zoomed or navigated since that screenshot");
  }
  return capture;
}

function number(value: unknown, name: string, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > max) {
    throw new ToolError("invalid_argument", `${name} must be a pixel of the screenshot, from 0 to ${max}`);
  }
  return value;
}

// A pixel of the screenshot, as a point of the page's viewport in CSS pixels.
export function toViewport(capture: Capture, x: number, y: number): { x: number; y: number } {
  return { x: (x * capture.width) / capture.imageWidth, y: (y * capture.height) / capture.imageHeight };
}

function point(capture: Capture, params: Params, xName = "x", yName = "y"): { x: number; y: number } {
  return toViewport(capture, number(params[xName], xName, capture.imageWidth), number(params[yName], yName, capture.imageHeight));
}

// What a click there would reach: the control around the element under the
// point, as the browser hit-tests it, through shadow roots and frames.
const CONTROL = `function () {
  const ROLES = ["button","link","checkbox","radio","switch","tab","option","menuitem","menuitemcheckbox","menuitemradio","treeitem","combobox","textbox","searchbox","slider","spinbutton","gridcell"];
  const SELECTOR = 'a[href],button,input,select,textarea,label,summary,[onclick],[tabindex],[contenteditable],' + ROLES.map((r) => '[role="' + r + '"]').join(",");
  let el = this.nodeType === 1 ? this : this.parentElement;
  for (let e = el; e; e = e.parentElement || (e.getRootNode && e.getRootNode().host) || null) {
    if (e.matches && e.matches(SELECTOR)) return e;
  }
  return el;
}`;

async function underPoint(tabId: number, location: { x: number; y: number }, policy: Policy): Promise<Target & { drawn: boolean }> {
  const hit = await send<{ backendNodeId?: number; frameId?: string }>(tabId, "DOM.getNodeForLocation", {
    x: Math.round(location.x), y: Math.round(location.y), includeUserAgentShadowDOM: false, ignorePointerEventsNone: false,
  }).catch(() => ({ backendNodeId: undefined }));
  if (hit.backendNodeId === undefined) throw new ToolError("invalid_argument", "nothing of the page is at that point");
  // Only the page's own document: a frame's coordinates and origin aren't
  // the page's, as a click by reference refuses subframes too.
  const { frameTree } = await send<{ frameTree: { frame: { id: string } } }>(tabId, "Page.getFrameTree");
  if (hit.frameId && hit.frameId !== frameTree.frame.id) {
    throw new ToolError("page_error", "that point is inside a frame, which can't be acted on by position; read the frame's page or use its refs");
  }
  // In the node's own frame: an isolated world belongs to the main frame only.
  const { object } = await send<{ object: { objectId: string } }>(tabId, "DOM.resolveNode", { backendNodeId: hit.backendNodeId });
  let control = hit.backendNodeId;
  try {
    const result = await send<{ result: { objectId?: string; subtype?: string } }>(tabId, "Runtime.callFunctionOn", {
      objectId: object.objectId, functionDeclaration: CONTROL, returnByValue: false,
    });
    if (result.result.objectId && result.result.subtype === "node") {
      const { node } = await send<{ node: { backendNodeId: number } }>(tabId, "DOM.describeNode", { objectId: result.result.objectId });
      control = node.backendNodeId;
      await send(tabId, "Runtime.releaseObject", { objectId: result.result.objectId }).catch(() => {});
    }
  } finally {
    await send(tabId, "Runtime.releaseObject", { objectId: object.objectId }).catch(() => {});
  }
  const t = await target({ tab: tabId, ref: `e${control}` }, policy);
  const tag = t.facts.tag;
  const drawn = tag === "canvas" || (tag.includes("-") && !t.facts.role);
  return { ...t, drawn };
}

// Why a press there may send, buy, post or delete something, as the click
// tool judges its target, and more: what the page draws itself (a canvas,
// a custom element with no role) can't be told, so it may.
function reasonFor(t: Target & { drawn: boolean }): string | null {
  return t.drawn ? "it's drawn by the page (a canvas or custom element), so what it does can't be told" : submitReason(t.facts);
}

// The refusals a click on that element would get, and those a position adds.
function refuse(t: Target, policy: Policy): void {
  const facts: ElementFacts = t.facts;
  if (facts.tag === "iframe" || facts.tag === "frame" || facts.tag === "embed" || facts.tag === "object") {
    throw new ToolError("page_error", "that point is inside an embedded frame, which can't be acted on by position");
  }
  if (isSensitiveField(facts)) {
    throw new ToolError("sensitive_field", "that point is a password, payment or one-time code field; the person fills those in themselves");
  }
  if (facts.tag === "input" && (facts.attrs.type ?? "").toLowerCase() === "file") {
    throw new ToolError("invalid_argument", "that point is a file field: use browser_upload, which asks the person with the file");
  }
  if (facts.tag === "a" && facts.attrs.download !== undefined) {
    throw new ToolError("invalid_argument", "that point downloads a file: use browser_download, which saves it into a shared folder");
  }
  const href = facts.tag === "a" ? facts.attrs.href : undefined;
  if (href) {
    let url: URL | null = null;
    try { url = new URL(href, "http://base.invalid/"); } catch { url = null; }
    if (url && url.host !== "base.invalid" && !siteAllowed(url.href, policy.sites, policy.all_sites, policy.blocked_sites)) {
      throw new ToolError("site_not_allowed", `that link goes to ${hostOfUrl(url.href)}, which isn't on the allowed sites`);
    }
  }
}

// Rings at the points an action uses: where it presses and, for a drag,
// a filled one where it lets go, joined by a line.
const MARK = `function (points) {
  const ID = "chat-with-work-point";
  document.getElementById(ID)?.remove();
  if (!points.length) return;
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.id = ID;
  svg.setAttribute("style", "position:fixed;inset:0;width:100vw;height:100vh;z-index:2147483647;pointer-events:none");
  const add = (name, attrs) => { const el = document.createElementNS(NS, name); for (const k in attrs) el.setAttribute(k, attrs[k]); svg.append(el); };
  if (points.length > 1) {
    add("line", { x1: points[0].x, y1: points[0].y, x2: points[1].x, y2: points[1].y, stroke: "#fff", "stroke-width": 6 });
    add("line", { x1: points[0].x, y1: points[0].y, x2: points[1].x, y2: points[1].y, stroke: "#f97316", "stroke-width": 3 });
  }
  points.forEach((p, i) => {
    add("circle", { cx: p.x, cy: p.y, r: 12, fill: "none", stroke: "#fff", "stroke-width": 7 });
    add("circle", { cx: p.x, cy: p.y, r: 12, fill: i ? "#f97316" : "none", stroke: "#f97316", "stroke-width": 3 });
  });
  document.documentElement.append(svg);
}`;

// A picture of the tab with the point marked, for the approval card.
async function previewAt(tabId: number, at: { x: number; y: number }, to?: { x: number; y: number }): Promise<object> {
  const context = await isolatedContext(tabId);
  const mark = (points: { x: number; y: number }[], options = {}) =>
    evaluateInContext(tabId, context, `(${MARK})(${JSON.stringify(points)})`, options).catch(() => {});
  await mark(to ? [at, to] : [at]);
  try {
    return await captureTab(tabId, PREVIEW_BYTES);
  } finally {
    await cleanUp(tabId, async (options) => { await mark([], { ...options }); });
  }
}

function named(t: Target): object {
  return { ...described(t.facts), ref: `e${t.backendNodeId}` };
}

// After an action: where the tab is, and a fresh look to point at next.
async function after(tabId: number, policy: Policy, started: number): Promise<object> {
  const settled = (await settle(tabId, policy, started)) as { allowed?: boolean; closed?: boolean };
  if (settled.allowed === false || settled.closed) return settled;
  await new Promise((r) => setTimeout(r, SETTLE_MS));
  return { ...settled, look: await look(tabId) };
}

const clickAt: Method = async (params, policy) => {
  only(params, ["tab", "capture", "x", "y", "double", "submits", "dry_run"]);
  allowedChanges(policy);
  const saysSubmits = flag(params, "submits", true);
  const double = flag(params, "double", false);
  const dry = dryRun(params);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const capture = await current(tabId, params.capture);
  const location = point(capture, params);
  const t = await underPoint(tabId, location, policy);
  refuse(t, policy);
  const reason = reasonFor(t);
  if (!saysSubmits && reason) {
    throw new ToolError("would_submit", `this click may send, buy, post or delete something (${reason}); call again with submits: true so the person sees it first`);
  }
  const what = { target: named(t), point: location, double, submits: reason !== null || saysSubmits, ...(reason ? { reason } : {}) };
  if (dry) {
    const shot = await previewAt(tabId, location);
    const now = await still(tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  // The page must still stand as the screenshot showed it, and the same
  // control must still be under the point.
  await current(tabId, params.capture);
  const again = await underPoint(tabId, location, policy);
  if (again.backendNodeId !== t.backendNodeId) throw new ToolError("page_error", "the page changed, look again: something else is under that point now");
  const started = Date.now();
  await pressAt(t, policy, t.backendNodeId, location, double ? 2 : 1);
  captures.delete(tabId);
  return { dry_run: false, ...what, ...(await after(tabId, policy, started)) };
};

const hover: Method = async (params, policy) => {
  only(params, ["tab", "capture", "x", "y"]);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const capture = await current(tabId, params.capture);
  const location = point(capture, params);
  const t = await underPoint(tabId, location, policy);
  const started = Date.now();
  await send(tabId, "Input.dispatchMouseEvent", { type: "mouseMoved", ...location, button: "none" });
  captures.delete(tabId);
  return { target: named(t), point: location, ...(await after(tabId, policy, started)) };
};

const wheel: Method = async (params, policy) => {
  only(params, ["tab", "capture", "x", "y", "delta_x", "delta_y"]);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const capture = await current(tabId, params.capture);
  const location = point(capture, params);
  const deltaX = typeof params.delta_x === "number" ? Math.max(-5000, Math.min(5000, params.delta_x)) : 0;
  const deltaY = typeof params.delta_y === "number" ? Math.max(-5000, Math.min(5000, params.delta_y)) : 0;
  if (!deltaX && !deltaY) throw new ToolError("invalid_argument", "say how far to scroll with delta_x or delta_y, in CSS pixels");
  const started = Date.now();
  await send(tabId, "Input.dispatchMouseEvent", { type: "mouseWheel", ...location, deltaX, deltaY, button: "none" });
  captures.delete(tabId);
  return { point: location, ...(await after(tabId, policy, started)) };
};

const drag: Method = async (params, policy) => {
  only(params, ["tab", "capture", "x", "y", "to_x", "to_y", "submits", "dry_run"]);
  allowedChanges(policy);
  const saysSubmits = flag(params, "submits", true);
  const dry = dryRun(params);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const capture = await current(tabId, params.capture);
  const from = point(capture, params);
  const to = point(capture, params, "to_x", "to_y");
  const source = await underPoint(tabId, from, policy);
  const destination = await underPoint(tabId, to, policy);
  refuse(source, policy);
  if (isSensitiveField(destination.facts)) throw new ToolError("sensitive_field", "that drop point is a password, payment or one-time code field");
  // Dropping on something named like an action (a "Delete" bin) may do it.
  const reason = (destination.backendNodeId !== source.backendNodeId ? reasonFor(destination) : null) ?? (source.drawn ? reasonFor(source) : null);
  if (!saysSubmits && reason) {
    throw new ToolError("would_submit", `this drag may send or delete something (${reason}); call again with submits: true so the person sees it first`);
  }
  const what = { target: named(source), drop: named(destination), point: from, to, submits: reason !== null || saysSubmits, ...(reason ? { reason } : {}) };
  if (dry) {
    const shot = await previewAt(tabId, from, to);
    const now = await still(tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  await current(tabId, params.capture);
  const started = Date.now();
  const mouse = (type: string, at: { x: number; y: number }, extra: object = {}) =>
    send(tabId, "Input.dispatchMouseEvent", { type, ...at, button: "left", ...extra });
  await mouse("mouseMoved", from, { button: "none" });
  await mouse("mousePressed", from, { buttons: 1, clickCount: 1 });
  try {
    const steps = 12;
    for (let i = 1; i <= steps; i++) {
      allowedChanges(policy);
      await mouse("mouseMoved", { x: from.x + ((to.x - from.x) * i) / steps, y: from.y + ((to.y - from.y) * i) / steps }, { buttons: 1 });
      await new Promise((r) => setTimeout(r, 16));
    }
  } finally {
    await mouse("mouseReleased", to, { buttons: 0, clickCount: 1 });
  }
  captures.delete(tabId);
  return { dry_run: false, ...what, ...(await after(tabId, policy, started)) };
};

// Keys a person uses to get through a page without a mouse.
const KEYS: Record<string, { key: string; code: string; keyCode: number; text?: string }> = {
  Tab: { key: "Tab", code: "Tab", keyCode: 9 },
  Enter: { key: "Enter", code: "Enter", keyCode: 13, text: "\r" },
  Escape: { key: "Escape", code: "Escape", keyCode: 27 },
  Space: { key: " ", code: "Space", keyCode: 32, text: " " },
  ArrowUp: { key: "ArrowUp", code: "ArrowUp", keyCode: 38 },
  ArrowDown: { key: "ArrowDown", code: "ArrowDown", keyCode: 40 },
  ArrowLeft: { key: "ArrowLeft", code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { key: "ArrowRight", code: "ArrowRight", keyCode: 39 },
  Home: { key: "Home", code: "Home", keyCode: 36 },
  End: { key: "End", code: "End", keyCode: 35 },
  PageUp: { key: "PageUp", code: "PageUp", keyCode: 33 },
  PageDown: { key: "PageDown", code: "PageDown", keyCode: 34 },
  Backspace: { key: "Backspace", code: "Backspace", keyCode: 8 },
  Delete: { key: "Delete", code: "Delete", keyCode: 46 },
};

// The focused element, through shadow roots and same-origin frames.
const FOCUSED = `(() => {
  let el = document.activeElement;
  while (el) {
    if (el.shadowRoot && el.shadowRoot.activeElement) el = el.shadowRoot.activeElement;
    else if (el.tagName === "IFRAME") { try { const inner = el.contentDocument && el.contentDocument.activeElement; if (!inner || inner === el.contentDocument.body) break; el = inner; } catch (e) { break; } }
    else break;
  }
  return el && el !== document.body ? el : null;
})()`;

export async function focused(tabId: number, policy: Policy): Promise<Target | null> {
  // In the extension's own world, so the page's scripts can't say what has focus.
  const contextId = await isolatedContext(tabId);
  const result = await send<{ result: { objectId?: string; subtype?: string } }>(tabId, "Runtime.evaluate", {
    expression: FOCUSED, returnByValue: false, contextId,
  });
  if (!result.result.objectId || result.result.subtype !== "node") return null;
  try {
    const { node } = await send<{ node: { backendNodeId: number } }>(tabId, "DOM.describeNode", { objectId: result.result.objectId });
    return await target({ tab: tabId, ref: `e${node.backendNodeId}` }, policy);
  } finally {
    await send(tabId, "Runtime.releaseObject", { objectId: result.result.objectId }).catch(() => {});
  }
}

const TEXT_TYPES = new Set(["", "text", "search", "email", "url", "tel", "number", "password"]);

// Whether Enter or Space on `facts` may send something: a button's press,
// a field's Enter in a form, or what the page drew itself.
function keyReason(key: string, facts: ElementFacts): string | null {
  if (key === "Enter" && facts.form && (facts.tag === "textarea" ? false : facts.tag === "input" && TEXT_TYPES.has((facts.attrs.type ?? "").toLowerCase()))) {
    return "Enter in this field submits its form";
  }
  return submitReason(facts);
}

const press: Method = async (params, policy) => {
  only(params, ["tab", "key", "shift", "submits", "dry_run"]);
  allowedChanges(policy);
  const name = params.key;
  if (typeof name !== "string" || !Object.hasOwn(KEYS, name)) {
    throw new ToolError("invalid_argument", `key is one of ${Object.keys(KEYS).join(", ")}`);
  }
  const key = KEYS[name];
  const shift = flag(params, "shift", false);
  const saysSubmits = flag(params, "submits", true);
  const dry = dryRun(params);
  const tab = await tabFor(params, policy);
  const tabId = tab.id!;
  const before = await focused(tabId, policy);
  const acts = name === "Enter" || name === "Space";
  const reason = acts ? (before ? keyReason(name, before.facts) : "nothing has focus, so the page decides what it does") : null;
  if (before && isSensitiveField(before.facts) && !["Tab", "Escape"].includes(name)) {
    throw new ToolError("sensitive_field", "a password, payment or one-time code field has focus; press Tab to move on, the person fills those in");
  }
  if (!saysSubmits && reason) {
    throw new ToolError("would_submit", `this key may send, buy, post or delete something (${reason}); call again with submits: true so the person sees it first`);
  }
  const what = { key: name, shift, focused: before ? named(before) : null, submits: reason !== null || saysSubmits, ...(reason ? { reason } : {}) };
  if (dry) {
    const shot = before ? await preview(before) : await captureTab(tabId, PREVIEW_BYTES);
    const now = await still(tabId, policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  const modifiers = shift ? 8 : 0;
  const base = { key: key.key, code: key.code, windowsVirtualKeyCode: key.keyCode, nativeVirtualKeyCode: key.keyCode, modifiers };
  const started = Date.now();
  await send(tabId, "Input.dispatchKeyEvent", { type: "keyDown", ...base, ...(key.text ? { text: key.text } : {}) });
  await send(tabId, "Input.dispatchKeyEvent", { type: "keyUp", ...base });
  captures.delete(tabId);
  const settled = await settle(tabId, policy, started) as { allowed?: boolean };
  const now = settled.allowed === false ? null : await focused(tabId, policy).catch(() => null);
  return withDialog(tabId, { dry_run: false, ...what, ...settled, now_focused: now ? named(now) : null });
};

const screenshot: Method = async (params, policy) => {
  only(params, ["tab"]);
  const tab = await tabFor(params, policy);
  const shot = await look(tab.id!);
  const now = await still(tab.id!, policy);
  return withDialog(tab.id!, { ...about(now), ...shot });
};

export const positionalMethods: Record<string, Method> = { click_at: clickAt, hover, wheel, drag, press, look: screenshot };
