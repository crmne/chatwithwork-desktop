// Private transport for the bundled, pinned engine. These are daemon methods,
// never model tools. All browser authority still originates on this computer.
import { about, integer, only, still, type Method, type Params, type Policy } from "./actions.js";
import { allowedChanges, CHOOSE, described, dryRun, editable, flag, preview, settle, target, type Target } from "./changes.js";
import { callOn, cleanUp, evaluate, isHalted, relatedNode, send } from "./cdp.js";
import { isSensitiveField, submitReason } from "./classify.js";
import { documentIdentity, requireRef, sameDocument, type DocumentIdentity } from "./engine-refs.js";
import { INSTALL_ENGINE_GUARD } from "./engine-guard.js";
import { guardStatus, LABEL_AT, LABEL_COUNT, LABEL_DESCENDANT_AT_POINT } from "./pointer.js";
import { ToolError } from "./errors.js";

interface Command { id: number; method: string; params?: Record<string, any>; sessionId?: string }
interface Operation {
  lease: string; tab: number; session: string; action: string; params: Params; policy: Policy;
  target: Target; document: DocumentIdentity; targetInfo: any; labels: number[];
  timer: ReturnType<typeof setTimeout>; started: number; surface: number; phase: boolean; trial: boolean;
  dispatched: boolean; pressed: boolean; navigated: boolean; cancelled: boolean; guard: string;
  inputQueue: Promise<unknown>; deadline: number; interceptors: Set<string>; commands: number; bytes: number;
}
let active: Operation | undefined;
let acquiring = false;
let generation = 0;
let emit: (event: unknown) => void = () => {};
export function engineEvents(callback: (event: unknown) => void): void { emit = callback; }
const fail = (code: string, message = "The browser operation stopped. Read the page again before another action."): never => { throw new ToolError(code, message); };
const errorEnvelope = (error: unknown) => ({ error: { code: error instanceof ToolError ? error.code : "page_error", message: "The browser operation did not complete." } });
const SESSION_METHODS = new Set([
  "DOM.enable", "DOM.getDocument", "DOM.getFlattenedDocument", "DOM.describeNode", "DOM.resolveNode", "DOM.getContentQuads", "DOM.getBoxModel", "DOM.scrollIntoViewIfNeeded",
  "Runtime.enable", "Runtime.evaluate", "Runtime.callFunctionOn", "Runtime.releaseObject", "Runtime.releaseObjectGroup", "Runtime.runIfWaitingForDebugger",
  "Page.enable", "Page.getFrameTree", "Page.getLayoutMetrics", "Page.createIsolatedWorld", "Page.addScriptToEvaluateOnNewDocument", "Page.removeScriptToEvaluateOnNewDocument", "Page.setLifecycleEventsEnabled",
  "Network.enable", "Log.enable", "Emulation.setFocusEmulationEnabled",
  "Input.dispatchMouseEvent", "Input.dispatchKeyEvent", "Input.insertText",
]);
const NOOP = new Set(["Page.bringToFront", "Page.setFontFamilies", "Emulation.setEmulatedMedia", "Target.setAutoAttach"]);

async function semantic(op: Operation): Promise<Target> {
  const current = await target(op.params, op.policy);
  if (current.tabId !== op.tab || current.backendNodeId !== op.target.backendNodeId) fail("not_found");
  if (op.action === "click" && !flag(op.params, "submits", true) && submitReason(current.facts)) fail("would_submit");
  if (op.action === "type" && (isSensitiveField(current.facts) || !editable(current.facts))) fail("sensitive_field");
  if (op.action === "select") {
    const result = await callOn<any>(op.tab, current.backendNodeId, CHOOSE, [op.params.values, false]);
    if (result.error || result.missing !== undefined || result.disabled !== undefined) fail("invalid_argument");
  }
  return current;
}
async function current(op: Operation): Promise<void> {
  if (active !== op || op.cancelled || isHalted() || op.policy.stopped || Date.now() >= op.deadline) fail("browser_stopped");
  allowedChanges(op.policy);
  if (op.navigated) fail(op.dispatched ? "navigation_started" : "not_found");
  try { await still(op.tab, op.policy); }
  catch (error) {
    const tab = await chrome.tabs.get(op.tab).catch(() => undefined);
    if (op.dispatched && (!tab || (tab.url && tab.url !== op.targetInfo.url))) {
      navigation(op); fail("navigation_started");
    }
    throw error;
  }
  if (!sameDocument(op.document, await documentIdentity(op.tab))) {
    navigation(op); fail(op.dispatched ? "navigation_started" : "not_found");
  }
}
function navigation(op: Operation): void {
  if (active !== op || op.navigated) return;
  op.navigated = true;
  emit({ event: "engine", lease: op.lease, message: { method: op.dispatched ? "Cww.navigationStarted" : "Cww.operationCancelled", params: {} } });
}
async function cleanup(op: Operation): Promise<void> {
  clearTimeout(op.timer);
  await cleanUp(op.tab, async options => {
    // Stop the upstream event interceptors through their own stop method.
    // Never allow arbitrary queued engine commands after revocation.
    for (const objectId of op.interceptors) {
      await send(op.tab, "Runtime.callFunctionOn", { objectId, functionDeclaration: "function(){ return this.stop(); }", returnByValue: true }, options).catch(() => {});
    }
    await evaluate(op.tab, guardStatus(op.guard, true), options).catch(() => {});
  });
}
export async function cancelEngine(): Promise<void> {
  generation++;
  const op = active;
  if (!op) return;
  active = undefined; op.cancelled = true;
  emit({ event: "engine", lease: op.lease, message: { method: "Cww.operationCancelled", params: {} } });
  await cleanup(op);
}
function operation(params: Params): Operation {
  if (!active || typeof params.lease !== "string" || params.lease !== active.lease) fail("not_found");
  return active!;
}
const begin: Method = async (params, policy) => {
  only(params, ["action", "params", "timeout_ms"]);
  if (active || acquiring) fail("busy", "Another browser operation is still running.");
  acquiring = true;
  const admitted = generation;
  try {
    const timeout = integer(params.timeout_ms, "timeout_ms", 1, 14_000, 14_000);
    const action = params.action;
    if (typeof action !== "string" || !["click", "type", "select"].includes(action)) fail("invalid_argument");
    const args = params.params;
    if (!args || typeof args !== "object" || Array.isArray(args)) fail("invalid_argument");
    const input = args as Params;
    only(input, action === "click" ? ["tab", "ref", "dry_run", "submits"] : action === "type" ? ["tab", "ref", "dry_run", "text", "clear", "submit"] : ["tab", "ref", "dry_run", "values"]);
    allowedChanges(policy); dryRun(input);
    if (action === "type") {
      flag(input, "clear", true); flag(input, "submit", false);
      if (typeof input.text !== "string" || [...input.text].length > 10_000) fail("invalid_argument");
    }
    if (action === "select" && (!Array.isArray(input.values) || !input.values.length || input.values.length > 50 || input.values.some(v => typeof v !== "string"))) fail("invalid_argument");
    const t = await target(input, policy);
    const document = await requireRef(t.tabId, t.backendNodeId);
    const labels: number[] = [];
    if (action === "click") {
      const count = await callOn<number>(t.tabId, t.backendNodeId, LABEL_COUNT);
      for (let i = 0; i < count; i++) { const id = await relatedNode(t.tabId, t.backendNodeId, LABEL_AT, [i]); if (id) labels.push(id); }
    }
    const { targetInfo } = await send(t.tabId, "Target.getTargetInfo");
    const lease = crypto.randomUUID();
    const op: Operation = { lease, tab: t.tabId, session: `cww-engine-${lease}`, action: action as string, params: input, policy, target: t, document, targetInfo, labels,
      timer: setTimeout(() => { if (active?.lease === lease) void cancelEngine(); }, timeout), inputQueue: Promise.resolve(), deadline: Date.now() + timeout, started: Date.now(), surface: t.backendNodeId, phase: false, trial: false,
      dispatched: false, pressed: false, navigated: false, cancelled: false, guard: `__cwwEngine_${lease}`, interceptors: new Set(), commands: 0, bytes: 0 };
    if (generation !== admitted || isHalted() || policy.stopped) { clearTimeout(op.timer); fail("browser_stopped"); }
    active = op;
    try { await current(op); await semantic(op); }
    catch (error) { await cancelEngine(); throw error; }
    return { lease, timeout_ms: Math.max(1, op.deadline - Date.now()), tab: op.tab, session: op.session, action, params: input, document, targetInfo, backendNodeId: t.backendNodeId, labelBackendNodeIds: labels, facts: t.facts };
  } finally { acquiring = false; }
};

async function surfaceSafe(op: Operation, x?: number, y?: number): Promise<void> {
  if (op.surface === op.target.backendNodeId) return;
  const valid = await callOn<boolean>(op.tab, op.target.backendNodeId, `function(label) {
    return this instanceof HTMLInputElement && ['radio','checkbox'].includes(this.type) && label instanceof HTMLLabelElement && label.control === this && label.isConnected;
  }`, [], [op.surface]);
  if (!valid) fail("not_found");
  if (x === undefined || y === undefined) return;
  const hit = await relatedNode(op.tab, op.surface, LABEL_DESCENDANT_AT_POINT, [{ x, y }]);
  if (hit) {
    const { node } = await send(op.tab, "DOM.describeNode", { backendNodeId: hit, depth: 0, pierce: true });
    const independent = await callOn<boolean>(op.tab, hit, `function(label) {
      for (let node=this; node && node !== label; node=node.assignedSlot ?? node.parentElement ?? node.getRootNode().host ?? null) {
        if(node.matches('label,a[href],button,input,select,textarea,summary,details,audio[controls],video[controls],img[usemap],object[usemap],[contenteditable]:not([contenteditable=false]),[role=button],[role=link]')) return true;
      }
      return false;
    }`, [], [op.surface]);
    if (independent) fail("page_error", "Another interactive control occupies this label.");
    if (node.shadowRoots?.length) fail("page_error", "Another component occupies this label. Read the page again.");
  }
}
async function commandResult(op: Operation, command: Command): Promise<unknown> {
  await current(op);
  const { method, params = {}, sessionId } = command;
  if (!sessionId) {
    switch (method) {
      case "Browser.getVersion": return { protocolVersion: "1.3", product: `Chrome/${navigator.userAgent.match(/(?:Headless)?Chrome\/([\d.]+)/)?.[1] || "0"}`, revision: "", userAgent: navigator.userAgent, jsVersion: "" };
      case "Target.setAutoAttach":
        emit({ event: "engine", lease: op.lease, message: { method: "Target.attachedToTarget", params: { sessionId: op.session, targetInfo: { ...op.targetInfo, attached: true }, waitingForDebugger: false } } });
        return {};
      case "Target.getTargetInfo":
        if (params.targetId && params.targetId !== op.targetInfo.targetId) fail("not_found");
        return { targetInfo: op.targetInfo };
      case "Target.getTargets": return { targetInfos: [op.targetInfo] };
      case "Target.getBrowserContexts": return { browserContextIds: [] };
      case "Target.setDiscoverTargets": case "Browser.setDownloadBehavior": return {};
      default: return fail("invalid_argument");
    }
  }
  if (sessionId !== op.session) fail("not_found");
  if (method === "Cww.actionPhase" || method === "Cww.trialPhase") {
    const surface = params.surfaceBackendNodeId;
    if (!Number.isSafeInteger(surface) || (surface !== op.target.backendNodeId && !op.labels.includes(surface))) fail("not_found");
    op.surface = surface;
    await semantic(op); await surfaceSafe(op);
    await evaluate(op.tab, guardStatus(op.guard, true));
    await callOn(op.tab, op.target.backendNodeId, INSTALL_ENGINE_GUARD, [op.guard, op.action, flag(op.params, "submits", true)], [surface]);
    op.phase = true; op.trial = method === "Cww.trialPhase";
    return {};
  }
  // This lease never follows navigation. Do not leave engine code that can
  // initialize automatically in a future, possibly disallowed document.
  if (method === "Page.addScriptToEvaluateOnNewDocument") return { identifier: "cww-current-document-only" };
  if (method === "Page.removeScriptToEvaluateOnNewDocument") return {};
  if (NOOP.has(method)) return {};
  if (!SESSION_METHODS.has(method)) fail("invalid_argument");
  // Never create an all-origin/OOPIF world: subframe actions remain outside
  // the released fixed-tool contract. Main-frame shadow DOM stays supported.
  if (method === "Page.createIsolatedWorld" && params.frameId !== op.document.frame) fail("not_found");
  const expression = method === "Runtime.callFunctionOn" ? params.arguments?.[3]?.value : undefined;
  const interceptor = typeof expression === "string" && expression.includes(".setupHitTargetInterceptor(");
  const scriptInput = typeof expression === "string" && /\.fill\(|\.selectOptions\(/.test(expression);
  const input = method.startsWith("Input.");
  // Upstream trial normally dispatches events behind its own interceptor.
  // A page listener installed earlier can still observe those events. Keep
  // upstream geometry/actionability, but never deliver trial input or hover.
  if (input && op.trial) {
    await semantic(op);
    await surfaceSafe(op, typeof params.x === "number" ? params.x : undefined, typeof params.y === "number" ? params.y : undefined);
    return {};
  }
  if (input || scriptInput) {
    await semantic(op);
    if (op.phase) await surfaceSafe(op, typeof params.x === "number" ? params.x : undefined, typeof params.y === "number" ? params.y : undefined);
  }
  const dispatch = scriptInput || (input && !(method === "Input.dispatchMouseEvent" && params.type === "mouseMoved"));
  if (dispatch && !op.phase) fail("invalid_argument");
  if (dispatch && !op.trial && dryRun(op.params)) fail("invalid_argument");
  if (op.action === "type" && (method === "Input.insertText" || method === "Input.dispatchKeyEvent")) {
    const focused = await callOn<boolean>(op.tab, op.target.backendNodeId, "function(){return this.getRootNode().activeElement === this;}");
    if (!focused) fail("sensitive_field");
  }
  // A policy update while the asynchronous checks ran wins before input.
  if (active !== op || op.cancelled || isHalted() || op.policy.stopped || Date.now() >= op.deadline) fail("browser_stopped");
  allowedChanges(op.policy);
  if (method === "Input.dispatchMouseEvent" && params.type === "mousePressed") {
    if (op.pressed) fail("action_already_dispatched", "A press was already sent. Read the page before another action.");
    op.pressed = true;
  }
  if (method === "Input.dispatchMouseEvent" && params.type === "mouseReleased" && !op.pressed) fail("invalid_argument");
  if (dispatch && !op.trial) op.dispatched = true;
  const result = await send(op.tab, method, params);
  if (method === "Runtime.releaseObject") op.interceptors.delete(params.objectId);
  if (interceptor && typeof result?.result?.objectId === "string") op.interceptors.add(result.result.objectId);
  if (op.cancelled) { await cleanup(op); fail("browser_stopped"); }
  if (op.navigated) fail(op.dispatched ? "navigation_started" : "not_found");
  if (method === "Page.getFrameTree") return { frameTree: { frame: result.frameTree.frame } };
  return result;
}
const command: Method = async (params) => {
  only(params, ["lease", "command"]);
  try {
    const op = operation(params);
    const value = params.command as Command;
    if (!value || typeof value !== "object" || !Number.isSafeInteger(value.id) || typeof value.method !== "string") fail("invalid_argument");
    only(value as unknown as Params, ["id", "method", "params", "sessionId"]);
    if (value.params !== undefined && (!value.params || typeof value.params !== "object" || Array.isArray(value.params))) fail("invalid_argument");
    const bytes = new TextEncoder().encode(JSON.stringify(value)).byteLength;
    op.bytes += bytes;
    if (++op.commands > 512 || bytes > 2 * 1024 * 1024 || op.bytes > 16 * 1024 * 1024) fail("too_large");
    if (value.method.startsWith("Input.")) {
      // Playwright sends a mouse sequence concurrently. Preserve wire order
      // through our asynchronous policy checks, while events/Stop stay live.
      const result = op.inputQueue.then(() => commandResult(op, value));
      op.inputQueue = result.catch(() => {});
      return { result: await result };
    }
    return { result: await commandResult(op, value) };
  } catch (error) { return errorEnvelope(error); }
};
const finish: Method = async (params) => {
  only(params, ["lease", "surfaceBackendNodeId"]);
  const op = operation(params);
  try {
    if (params.surfaceBackendNodeId !== op.surface || !op.phase) fail("invalid_argument");
    if (!op.navigated) { await current(op); await semantic(op); await surfaceSafe(op); }
    else if (!op.dispatched) fail("not_found");
    const blocked = op.navigated ? null : await evaluate<string | null>(op.tab, guardStatus(op.guard, false));
    if (blocked) fail("page_error", blocked);
    const t = op.target;
    const what = { target: described(t.facts), ...(op.surface !== t.backendNodeId ? { activation_surface: "associated label" } : {}),
      ...(op.action === "click" ? { submits: flag(op.params, "submits", true) || submitReason(t.facts) !== null } : {}),
      ...(op.action === "type" ? { submit: flag(op.params, "submit", false) } : {}) };
    if (dryRun(op.params)) {
      const shot = await preview({ ...t, backendNodeId: op.surface }, false);
      await current(op);
      return { dry_run: true, ...about(await still(op.tab, op.policy)), ...what, ...shot,
        ...(op.action === "type" ? { text: op.params.text, clear: flag(op.params, "clear", true) } : {}),
        ...(op.action === "select" ? { chosen: op.params.values } : {}) };
    }
    return { dry_run: false, ...what, ...(await settle(op.tab, op.policy, op.started)) };
  } finally { if (active === op) active = undefined; await cleanup(op); }
};
const cancel: Method = async (params) => {
  only(params, ["lease"]);
  if (active?.lease === params.lease) await cancelEngine();
  return {};
};
export const engineMethods: Record<string, Method> = { engine_begin: begin, engine_command: command, engine_finish: finish, engine_cancel: cancel };

const EVENTS = new Set(["Runtime.executionContextCreated", "Runtime.executionContextDestroyed", "Runtime.executionContextsCleared", "Page.frameAttached", "Page.frameDetached", "Page.frameNavigated", "Page.frameStartedLoading", "Page.frameStoppedLoading", "Page.lifecycleEvent", "Page.navigatedWithinDocument", "DOM.documentUpdated", "Inspector.detached"]);
chrome.debugger.onEvent.addListener((source, method, params: any) => {
  const op = active;
  if (!op || source.tabId !== op.tab || source.sessionId || !EVENTS.has(method)) return;
  // Navigation is a terminal boundary. No destination DOM, execution context,
  // network payload or event is passed to the old operation, even if allowed.
  if (method === "Page.frameNavigated" && !params.frame?.parentId && params.frame?.loaderId !== op.document.loader) { navigation(op); return; }
  if (op.navigated || op.cancelled || isHalted() || op.policy.stopped || !op.policy.changes) return;
  if (method === "Runtime.executionContextCreated" && params.context?.auxData?.frameId !== op.document.frame) return;
  if (method.startsWith("Page.frame") && (params.frame?.parentId || (params.frameId && params.frameId !== op.document.frame))) return;
  void still(op.tab, op.policy).then(() => {
    if (active === op && !op.navigated && !op.cancelled && !isHalted() && !op.policy.stopped && op.policy.changes) {
      emit({ event: "engine", lease: op.lease, message: { method, params, sessionId: op.session } });
    }
  }).catch(async () => {
    if (active !== op || op.navigated) return;
    const tab = await chrome.tabs.get(op.tab).catch(() => undefined);
    if (op.dispatched && (!tab || (tab.url && tab.url !== op.targetInfo.url))) navigation(op);
    else if (active === op && !op.navigated) void cancelEngine();
  });
});
chrome.tabs.onRemoved.addListener(tab => { if (active?.tab === tab) navigation(active); });
