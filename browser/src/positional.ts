// Acting where a page gives no usable element: by position on a screenshot
// the model was just given, and by keyboard. A position means nothing on
// its own, so every positional action names the screenshot it was read
// from (`capture`), and is refused if the page has since moved, resized,
// zoomed or navigated: "the page changed, look again", never a click
// somewhere else. What's under the point is classified exactly as the
// ref-based click classifies its target (classify.ts, changes.ts), so a
// position is never a way around a refusal the click would give.

import { about, only, still, tabFor, withDialog, type Method, type Params, type Policy } from "./actions.js";
import { resolveTab } from "./group.js";
import { allowedChanges, described, dryRun, editable, flag, pressAt, preview, settle, target, type Target } from "./changes.js";
import { callOn, isolatedContext, evaluateInContext, send, cleanUp } from "./cdp.js";
import { guardStatus, INSTALL_POINTER_GUARD } from "./pointer.js";
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

// What each dry run showed the person, by the call it checked, so the
// approved call acts on those same elements and not on whatever the page
// put there while the person looked at the card. A call nobody was shown
// (an automatic one) has nothing to match.
// Kept until a new dry run of the same call replaces it, however long the
// person takes; only the oldest go once there are many.
const shown = new Map<string, number[]>();
const SHOWN_KEPT = 500;

function shownKey(tabId: number, method: string, params: Params): string {
  const rest = Object.entries(params).filter(([k]) => !["dry_run", "tab", "preflight"].includes(k)).sort(([a], [b]) => a.localeCompare(b));
  return `${tabId}:${method}:${JSON.stringify(rest)}`;
}

// A dry run the daemon makes itself right before acting (a local client's
// approved call, or an automatic action) checks the page again but never
// replaces what the person was shown.
export function showed(tabId: number, method: string, params: Params, ids: number[]): void {
  if (params.preflight === true) return;
  const key = shownKey(tabId, method, params);
  shown.delete(key);
  shown.set(key, ids);
  while (shown.size > SHOWN_KEPT) shown.delete(shown.keys().next().value!);
}

export function sameAsShown(tabId: number, method: string, params: Params, ids: number[]): void {
  // A refused call keeps its record, so the same call again is refused too
  // until a new dry run (a new card) shows what is there now.
  const key = shownKey(tabId, method, params);
  const entry = shown.get(key);
  if (entry && (entry.length !== ids.length || entry.some((id, i) => id !== ids[i]))) {
    throw new ToolError("page_error", "the page changed since the person saw it: something else is there now, so nothing was done; look again");
  }
  shown.delete(key);
}

// What the page draws itself, whose purpose nothing on the page says.
function drawnFacts(facts: ElementFacts): boolean {
  return facts.tag === "canvas" || (facts.tag.includes("-") && !facts.role);
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

// A link's destination as the browser resolves it (the page's base and
// scheme), for the allowed-sites check.
const LINK = `function () {
  if (this instanceof HTMLAnchorElement || this instanceof HTMLAreaElement) return this.href;
  if (this instanceof SVGAElement) { const raw = this.href.baseVal || this.getAttribute("xlink:href") || ""; try { return raw ? new URL(raw, document.baseURI).href : null; } catch (e) { return null; } }
  return null;
}`;

async function underPoint(tabId: number, location: { x: number; y: number }, policy: Policy): Promise<Target & { drawn: boolean; href: string | null }> {
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
  let href: string | null = null;
  try {
    const result = await send<{ result: { objectId?: string; subtype?: string } }>(tabId, "Runtime.callFunctionOn", {
      objectId: object.objectId, functionDeclaration: CONTROL, returnByValue: false,
    });
    if (result.result.objectId && result.result.subtype === "node") {
      const { node } = await send<{ node: { backendNodeId: number } }>(tabId, "DOM.describeNode", { objectId: result.result.objectId });
      control = node.backendNodeId;
      const link = await send<{ result: { value?: unknown } }>(tabId, "Runtime.callFunctionOn", {
        objectId: result.result.objectId, functionDeclaration: LINK, returnByValue: true,
      }).catch(() => ({ result: {} as { value?: unknown } }));
      href = typeof link.result.value === "string" ? link.result.value : null;
      await send(tabId, "Runtime.releaseObject", { objectId: result.result.objectId }).catch(() => {});
    }
  } finally {
    await send(tabId, "Runtime.releaseObject", { objectId: object.objectId }).catch(() => {});
  }
  const t = await target({ tab: tabId, ref: `e${control}` }, policy);
  return { ...t, drawn: drawnFacts(t.facts), href };
}

// Why a press there may send, buy, post or delete something, as the click
// tool judges its target, and more: what the page draws itself (a canvas,
// a custom element with no role) can't be told, so it may.
function reasonFor(t: Target & { drawn: boolean }): string | null {
  return t.drawn ? "it's drawn by the page (a canvas or custom element), so what it does can't be told" : submitReason(t.facts);
}

// The refusals a click on that element would get, and those a position adds.
function refuse(t: Target & { href?: string | null }, policy: Policy): void {
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
  // Where a link goes, as the browser resolved it; a link that runs script
  // or opens another app stays on this page as far as sites go.
  if (t.href && /^https?:/i.test(t.href) && !siteAllowed(t.href, policy.sites, policy.all_sites, policy.blocked_sites)) {
    throw new ToolError("site_not_allowed", `that link goes to ${hostOfUrl(t.href)}, which isn't on the allowed sites`);
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
export async function after(tabId: number, policy: Policy, started: number): Promise<object> {
  const settled = (await settle(tabId, policy, started)) as { allowed?: boolean; closed?: boolean };
  if (settled.allowed === false || settled.closed) return settled;
  await new Promise((r) => setTimeout(r, SETTLE_MS));
  const fresh = await look(tabId).catch(() => null);
  // The picture is of the page the tab shows now: it goes back only if the
  // tab, checked once more (in the group, on an allowed site), shows the
  // very page that picture recorded, and the answer describes that tab.
  const pictured = fresh ? captures.get(tabId) : undefined;
  const tab = await still(tabId, policy).catch(() => null);
  if (!tab) {
    const gone = await resolveTab(tabId).catch(() => null);
    if (!gone) return { tab: tabId, closed: true };
    return { tab: tabId, allowed: false, site: hostOfUrl(gone.url ?? ""), note: "the tab went on to a site that isn't allowed; nothing from it is shown" };
  }
  const bare = (u: string) => u.split("#")[0];
  // The same document too: a reload of the same address is another page.
  const document = await send<{ frameTree: { frame: { loaderId: string } } }>(tabId, "Page.getFrameTree").catch(() => null);
  if (!fresh || !pictured || pictured.id !== fresh.capture || bare(pictured.url) !== bare(tab.url ?? "") ||
      document?.frameTree.frame.loaderId !== pictured.loaderId) {
    return { ...settled, ...about(tab), note: "the page was still changing; look again" };
  }
  return { ...settled, ...about(tab), look: fresh };
}

const clickAt: Method = async (params, policy) => {
  only(params, ["tab", "capture", "x", "y", "double", "submits", "dry_run", "preflight"]);
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
    showed(tabId, "click_at", params, [t.backendNodeId]);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  // The page must still stand as the screenshot showed it, and the same
  // control the card showed must still be under the point.
  sameAsShown(tabId, "click_at", params, [t.backendNodeId]);
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
  only(params, ["tab", "capture", "x", "y", "to_x", "to_y", "submits", "dry_run", "preflight"]);
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
  // Pressing the source may do what clicking it does (a drag that barely
  // moves is a click), and dropping on something named like an action (a
  // "Delete" bin) may do that.
  const reason = reasonFor(source) ?? (destination.backendNodeId !== source.backendNodeId ? reasonFor(destination) : null);
  if (!saysSubmits && reason) {
    throw new ToolError("would_submit", `this drag may send or delete something (${reason}); call again with submits: true so the person sees it first`);
  }
  const what = { target: named(source), drop: named(destination), point: from, to, submits: reason !== null || saysSubmits, ...(reason ? { reason } : {}) };
  if (dry) {
    const shot = await previewAt(tabId, from, to);
    const now = await still(tabId, policy);
    showed(tabId, "drag", params, [source.backendNodeId, destination.backendNodeId]);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  sameAsShown(tabId, "drag", params, [source.backendNodeId, destination.backendNodeId]);
  await current(tabId, params.capture);
  const changed = "the page changed, look again: something else is under that point now";
  if ((await underPoint(tabId, from, policy)).backendNodeId !== source.backendNodeId) throw new ToolError("page_error", changed);
  const started = Date.now();
  const mouse = (type: string, at: { x: number; y: number }, extra: object = {}) =>
    send(tabId, "Input.dispatchMouseEvent", { type, ...at, button: "left", ...extra });
  await mouse("mouseMoved", from, { button: "none" });
  // Hovering may make the page put something else there: press only what
  // was checked, and keep the press from reaching anything else even if
  // the page moves something under the point as it arrives.
  if ((await underPoint(tabId, from, policy).catch(() => null))?.backendNodeId !== source.backendNodeId) {
    throw new ToolError("page_error", `${changed}; nothing was pressed`);
  }
  const key = `__cwwPointer_${crypto.randomUUID()}`;
  const guarded = await isolatedContext(tabId);
  const obstruction = await callOn<string | null>(tabId, source.backendNodeId, INSTALL_POINTER_GUARD,
    [key, from, ["pointerdown", "mousedown"]], [source.backendNodeId]);
  if (obstruction) throw new ToolError("page_error", `${obstruction}; nothing was pressed`);
  try {
    await mouse("mousePressed", from, { buttons: 1, clickCount: 1 });
  } finally {
    const blocked = await evaluateInContext<string | null>(tabId, guarded, guardStatus(key, true)).catch(() => null);
    if (blocked) {
      await mouse("mouseReleased", from, { buttons: 0, clickCount: 1 });
      captures.delete(tabId);
      throw new ToolError("page_error", `${blocked}; the press was held back and nothing was dragged`);
    }
  }
  let release = from;
  try {
    const steps = 12;
    for (let i = 1; i <= steps; i++) {
      allowedChanges(policy);
      await mouse("mouseMoved", { x: from.x + ((to.x - from.x) * i) / steps, y: from.y + ((to.y - from.y) * i) / steps }, { buttons: 1 });
      await new Promise((r) => setTimeout(r, 16));
    }
    // Let go there only over what was checked: the drop target, or the
    // dragged element itself, which many pages move under the pointer.
    const there = await underPoint(tabId, to, policy).catch(() => null);
    if (there && (there.backendNodeId === destination.backendNodeId || there.backendNodeId === source.backendNodeId)) release = to;
  } finally {
    await mouse("mouseReleased", release, { buttons: 0, clickCount: 1 });
  }
  if (release !== to) {
    captures.delete(tabId);
    throw new ToolError("page_error", "the drop point changed during the drag, so it was let go where it started; look again");
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
    const { node } = await send<{ node: { backendNodeId: number; shadowRoots?: { shadowRootType?: string }[] } }>(tabId, "DOM.describeNode", {
      objectId: result.result.objectId, depth: 1, pierce: true,
    });
    // Focus inside a closed shadow root can't be seen, so what gets the
    // key can't be checked.
    if (node.shadowRoots?.some((root) => root.shadowRootType === "closed")) {
      throw new ToolError("page_error", "focus is inside a closed part of the page that can't be checked, so keys and typing don't go there; click or Tab elsewhere first");
    }
    const t = await target({ tab: tabId, ref: `e${node.backendNodeId}` }, policy);
    // Focus inside another site's frame can't be inspected, so keys and
    // typing don't go there.
    if (["iframe", "frame", "embed", "object"].includes(t.facts.tag)) {
      throw new ToolError("page_error", "focus is inside an embedded frame, which keys and typing can't reach; click or Tab elsewhere first");
    }
    return t;
  } finally {
    await send(tabId, "Runtime.releaseObject", { objectId: result.result.objectId }).catch(() => {});
  }
}

const TEXT_TYPES = new Set(["", "text", "search", "email", "url", "tel", "number", "password"]);

// Whether a key may send, buy, post or delete something, given what has
// focus: any key but Tab when nothing has (the page decides what it does),
// Enter or Space on a button, in a form's field, or on what the page drew
// itself, and Delete or Backspace anywhere but a text field.
function keyReason(key: string, focus: ElementFacts | null): string | null {
  if (!focus) return key === "Tab" ? null : "nothing has focus, so the page decides what it does";
  const acts = key === "Enter" || key === "Space";
  const deletes = key === "Delete" || key === "Backspace";
  if ((acts || deletes) && drawnFacts(focus)) return "it's drawn by the page (a canvas or custom element), so what the key does can't be told";
  if (deletes) return editable(focus) ? null : "outside a text field, Delete and Backspace may delete what's selected";
  if (!acts) return null;
  if (key === "Enter" && focus.form && focus.tag === "input" && TEXT_TYPES.has((focus.attrs.type ?? "").toLowerCase())) {
    return "Enter in this field submits its form";
  }
  return submitReason(focus);
}

const press: Method = async (params, policy) => {
  only(params, ["tab", "key", "shift", "submits", "dry_run", "preflight"]);
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
  const reason = keyReason(name, before?.facts ?? null);
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
    showed(tabId, "press", params, before ? [before.backendNodeId] : []);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  sameAsShown(tabId, "press", params, before ? [before.backendNodeId] : []);
  // Focus must not have moved since it was checked: read it again right
  // before the key goes.
  const last = await focused(tabId, policy);
  if ((last?.backendNodeId ?? null) !== (before?.backendNodeId ?? null)) {
    throw new ToolError("page_error", "focus moved since it was checked, so the key wasn't sent; look again");
  }
  const modifiers = shift ? 8 : 0;
  const base = { key: key.key, code: key.code, windowsVirtualKeyCode: key.keyCode, nativeVirtualKeyCode: key.keyCode, modifiers };
  const started = Date.now();
  await send(tabId, "Input.dispatchKeyEvent", { type: "keyDown", ...base, ...(key.text ? { text: key.text } : {}) });
  await send(tabId, "Input.dispatchKeyEvent", { type: "keyUp", ...base });
  captures.delete(tabId);
  const settled = await after(tabId, policy, started) as { allowed?: boolean; closed?: boolean };
  const now = settled.allowed === false || settled.closed ? null : await focused(tabId, policy).catch(() => null);
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
