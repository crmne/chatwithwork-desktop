// The debugger, attached only to tabs in the Chat with Work group and only
// while they're being used: it detaches after a minute without a call, so
// Chrome's "started debugging this browser" bar doesn't stay.

import { pageStartedDownload } from "./downloads.js";
import { browserStopped, ToolError } from "./errors.js";
import { NOT_RESPONDING, PROBE_TIMEOUT_MS, Responsiveness } from "./responsive.js";

const IDLE_DETACH_MS = 60_000;
const COMMAND_TIMEOUT_MS = 15_000;
// A page stuck in a script can keep the debugger from attaching at all.
const ATTACH_TIMEOUT_MS = 10_000;

const responsiveness = new Responsiveness();

// `work`, or a timeout error after `ms`. The work itself isn't cancelled.
async function within<T>(work: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new ToolError("timeout", message)), ms);
  });
  try {
    return await Promise.race([work, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

interface Session {
  idle?: ReturnType<typeof setTimeout>;
  // The last JavaScript dialog the extension dismissed, for the next result.
  dialog?: string;
}

// Calls that put back what a tool changed on the page (a screenshot's
// blur) still run after Stop; nothing else does.
export interface SendOptions {
  cleanup?: boolean;
  // Engine cleanup must never recreate an attachment after Stop.
  existingOnly?: boolean;
}

const sessions = new Map<number, Session>();
const attaching = new Map<number, Promise<void>>();

// Set while the person has stopped Chat with Work from using the browser:
// nothing attaches or sends, so a call still running (a wait polling, a
// scroll after a screenshot) can't take the debugger back after Stop.
let halted = false;

export function isHalted(): boolean {
  return halted;
}

// Stop or resume. Halting is set before detaching, so nothing reattaches
// in between.
export async function halt(on: boolean, beforeDetach?: () => Promise<void>): Promise<void> {
  halted = on;
  if (on) {
    try { await beforeDetach?.(); } finally { await detachAll(); }
  }
}

// Run `work` with cleanup calls allowed, and, if Stop was pressed
// meanwhile, detach again once it's done.
// If the tab isn't answering, the cleanup can't reach it now, so it runs
// again once the tab answers (cleanups only take back what a tool put on
// the page, such as a screenshot's blur, so running one twice is harmless).
// `existingOnly` says the cleanup needs the attachment it was made in; once
// that attachment ends it's dropped, and nothing attaches again for it.
export async function cleanUp(
  tabId: number,
  work: (options: SendOptions) => Promise<void>,
  policy: { existingOnly?: boolean } = {},
): Promise<void> {
  try {
    await work({ cleanup: true });
  } finally {
    if (responsiveness.verdict(tabId) !== "send") deferCleanup(tabId, { work, existingOnly: policy.existingOnly === true });
    if (halted) await detach(tabId);
  }
}

const RETRY_EVERY_MS = 2_000;
const RETRY_FOR_MS = 120_000;
interface Deferred { work: (options: SendOptions) => Promise<void>; existingOnly: boolean }
const deferred = new Map<number, { works: Deferred[]; timer?: ReturnType<typeof setTimeout>; until: number }>();

function deferCleanup(tabId: number, item: Deferred): void {
  const entry = deferred.get(tabId) ?? { works: [], until: 0 };
  if (!entry.works.some((queued) => queued.work === item.work) && entry.works.length < 20) entry.works.push(item);
  entry.until = Date.now() + RETRY_FOR_MS;
  deferred.set(tabId, entry);
  if (!entry.timer) entry.timer = setTimeout(() => void retryCleanup(tabId), RETRY_EVERY_MS);
}

// Check, briefly, whether the tab answers; when it does, run what it owes.
async function retryCleanup(tabId: number): Promise<void> {
  const entry = deferred.get(tabId);
  if (!entry) return;
  entry.timer = undefined;
  // Cleanups tied to an attachment that has ended (Stop, or the tab let go)
  // have nothing left to clean, and must not attach again.
  if (!sessions.has(tabId)) entry.works = entry.works.filter((item) => !item.existingOnly);
  if (!entry.works.length) {
    deferred.delete(tabId);
    return;
  }
  // The check itself attaches only if some cleanup may.
  const existingOnly = entry.works.every((item) => item.existingOnly);
  let answered = false;
  try {
    await send(tabId, "Runtime.evaluate", { expression: "0", returnByValue: true }, { cleanup: true, existingOnly });
    answered = true;
  } catch {
    // Still not answering, or gone.
  }
  if (answered) {
    deferred.delete(tabId);
    for (const item of entry.works) {
      await cleanUp(tabId, item.work, { existingOnly: item.existingOnly }).catch(() => {});
    }
    return;
  }
  if (halted) await detach(tabId);
  if (Date.now() < entry.until && !closedTabs.has(tabId)) entry.timer = setTimeout(() => void retryCleanup(tabId), RETRY_EVERY_MS);
  else deferred.delete(tabId);
}

const closedTabs = new Set<number>();

export function isAttached(tabId: number): boolean {
  return sessions.has(tabId);
}

async function attach(tabId: number, options: SendOptions = {}): Promise<void> {
  if (options.existingOnly && !sessions.has(tabId)) throw new ToolError("unavailable", "the browser attachment ended");
  if (halted && !options.cleanup) throw browserStopped();
  if (sessions.has(tabId)) return;
  // A tab that just stalled fails at once, even while its attachment is
  // still pending, instead of being waited on again.
  if (responsiveness.verdict(tabId) === "fail") throw new ToolError("timeout", NOT_RESPONDING);
  const pending = attaching.get(tabId);
  if (pending) return within(pending, ATTACH_TIMEOUT_MS, NOT_RESPONDING);
  // Whether every caller stopped waiting, so nobody will check Stop after it.
  let abandoned = false;
  const work = (async () => {
    try {
      await chrome.debugger.attach({ tabId }, "1.3");
    } catch (error) {
      const message = String((error as Error)?.message ?? error);
      if (/already attached/i.test(message)) {
        throw new ToolError(
          "busy",
          "another debugger (such as DevTools or another extension) is attached to this tab",
        );
      }
      throw new ToolError("unavailable", `can't use this tab: ${message}`);
    }
    // Stop wins over an attachment that completed after it, whether or not
    // anyone still waits for it. Only a cleanup still waiting may use it.
    if (halted && (abandoned || !options.cleanup)) {
      await chrome.debugger.detach({ tabId }).catch(() => {});
      throw browserStopped();
    }
    sessions.set(tabId, {});
    // An attachment nobody waits for any more still detaches when idle.
    touch(tabId);
    const target = { tabId };
    // Answer dialogs so a page can't hang the agent, and behave as if
    // focused while in the background.
    await chrome.debugger.sendCommand(target, "Page.enable").catch(() => {});
    await chrome.debugger
      .sendCommand(target, "Emulation.setFocusEmulationEnabled", { enabled: true })
      .catch(() => {});
    if (halted && (abandoned || !options.cleanup)) await detach(tabId);
  })();
  attaching.set(tabId, work);
  void work.catch(() => {}).finally(() => attaching.delete(tabId));
  try {
    await within(work, ATTACH_TIMEOUT_MS, NOT_RESPONDING);
  } catch (error) {
    if (error instanceof ToolError && error.code === "timeout") {
      abandoned = true;
      responsiveness.stalled(tabId);
    }
    throw error;
  }
  // Stopped while attaching: let go at once.
  if (halted && !options.cleanup) {
    await detach(tabId);
    throw browserStopped();
  }
}

function touch(tabId: number): void {
  const session = sessions.get(tabId);
  if (!session) return;
  if (session.idle) clearTimeout(session.idle);
  session.idle = setTimeout(() => void detach(tabId), IDLE_DETACH_MS);
}

export async function detach(tabId: number): Promise<void> {
  const session = sessions.get(tabId);
  if (!session) return;
  if (session.idle) clearTimeout(session.idle);
  sessions.delete(tabId);
  await chrome.debugger.detach({ tabId }).catch(() => {});
}

export async function detachAll(): Promise<void> {
  await Promise.all([...sessions.keys()].map((tabId) => detach(tabId)));
  // Also any left from before the service worker restarted.
  const targets = await chrome.debugger.getTargets().catch(() => []);
  await Promise.all(
    targets
      .filter((t) => t.attached && t.tabId !== undefined && t.extensionId === undefined)
      .map((t) => chrome.debugger.detach({ tabId: t.tabId! }).catch(() => {})),
  );
}

export async function send<T = any>(
  tabId: number,
  method: string,
  params: Record<string, unknown> = {},
  options: SendOptions = {},
): Promise<T> {
  if (halted && !options.cleanup) throw browserStopped();
  if (responsiveness.verdict(tabId) === "fail") throw new ToolError("timeout", NOT_RESPONDING);
  await attach(tabId, options);
  touch(tabId);
  await responding(tabId);
  try {
    return (await within(
      chrome.debugger.sendCommand({ tabId }, method, params) as Promise<T>,
      COMMAND_TIMEOUT_MS,
      `the page didn't answer ${method} in time: ${NOT_RESPONDING}`,
    ));
  } catch (error) {
    if (error instanceof ToolError) {
      if (error.code === "timeout") responsiveness.stalled(tabId);
      throw error;
    }
    throw new ToolError("page_error", `${method}: ${String((error as Error)?.message ?? error)}`);
  }
}

// Before a command to a tab that stopped answering: fail at once right after
// it stopped, and later check, briefly, whether it answers again. Without
// this, every later call to a hung tab waited out the full timeout of each
// of its commands and answered after Chat with Work had given up.
async function responding(tabId: number): Promise<void> {
  const verdict = responsiveness.verdict(tabId);
  if (verdict === "send") return;
  if (verdict === "fail") throw new ToolError("timeout", NOT_RESPONDING);
  try {
    await within(
      chrome.debugger.sendCommand({ tabId }, "Runtime.evaluate", { expression: "0", returnByValue: true }),
      PROBE_TIMEOUT_MS,
      NOT_RESPONDING,
    );
  } catch (error) {
    if (error instanceof ToolError) {
      responsiveness.stalled(tabId);
      throw error;
    }
    // An answer, even an error, means the page is responding again.
  }
  responsiveness.answered(tabId);
}

chrome.tabs.onRemoved.addListener((tabId) => {
  responsiveness.forget(tabId);
  const entry = deferred.get(tabId);
  if (entry?.timer) clearTimeout(entry.timer);
  deferred.delete(tabId);
  closedTabs.add(tabId);
  if (closedTabs.size > 1_000) closedTabs.clear();
});

// The dialog dismissed since the last call, once.
export function takeDialog(tabId: number): string | undefined {
  const session = sessions.get(tabId);
  const dialog = session?.dialog;
  if (session) session.dialog = undefined;
  return dialog;
}

chrome.debugger.onEvent.addListener((source, method, params: any) => {
  const tabId = source.tabId;
  if (tabId === undefined || !sessions.has(tabId)) return;
  if (method === "Page.downloadWillBegin" && typeof params?.url === "string") {
    pageStartedDownload(params.url);
    return;
  }
  if (method === "Page.javascriptDialogOpening") {
    const session = sessions.get(tabId)!;
    session.dialog = `${params?.type ?? "dialog"}: ${String(params?.message ?? "").slice(0, 300)}`;
    // Dismissed: an alert is closed, a confirm or prompt is cancelled,
    // a "leave this page?" keeps the page.
    void chrome.debugger
      .sendCommand({ tabId }, "Page.handleJavaScriptDialog", { accept: false })
      .catch(() => {});
  }
});

chrome.debugger.onDetach.addListener((source) => {
  if (source.tabId !== undefined) {
    const session = sessions.get(source.tabId);
    if (session?.idle) clearTimeout(session.idle);
    sessions.delete(source.tabId);
  }
});

// The extension's own world in the tab's main frame: it sees the page's
// DOM, but not the page's scripts, nor they it.
export async function isolatedContext(tabId: number, options: SendOptions = {}): Promise<number> {
  const tree = await send<{ frameTree: { frame: { id: string } } }>(tabId, "Page.getFrameTree", {}, options);
  const world = await send<{ executionContextId: number }>(
    tabId,
    "Page.createIsolatedWorld",
    { frameId: tree.frameTree.frame.id, worldName: "Chat with Work", grantUniveralAccess: false },
    options,
  );
  return world.executionContextId;
}

// Run `expression` in the extension's world.
export async function evaluate<T = unknown>(tabId: number, expression: string, options: SendOptions = {}): Promise<T> {
  const contextId = await isolatedContext(tabId, options);
  return evaluateInContext(tabId, contextId, expression, options);
}

// A check installed before input must stay in that document's world, even
// when the input navigates. Never create a new world to read its old guard.
export async function evaluateInContext<T = unknown>(tabId: number, contextId: number, expression: string, options: SendOptions = {}): Promise<T> {
  const result = await send<{ result: { value?: T }; exceptionDetails?: { text?: string } }>(
    tabId,
    "Runtime.evaluate",
    { expression, contextId, returnByValue: true, awaitPromise: true },
    options,
  );
  if (result.exceptionDetails) {
    throw new ToolError("page_error", `the page refused a check: ${result.exceptionDetails.text ?? "error"}`);
  }
  return result.result.value as T;
}

// Call `fn` (a function's source) with `this` the element `backendNodeId`
// and `others` (more elements) then `args` as its arguments, in the
// extension's world.
export async function callOn<T = unknown>(
  tabId: number,
  backendNodeId: number,
  fn: string,
  args: unknown[] = [],
  others: number[] = [],
  options: SendOptions = {},
): Promise<T> {
  const contextId = await isolatedContext(tabId, options);
  const resolve = async (id: number) => {
    const { object } = await send<{ object: { objectId: string } }>(
      tabId,
      "DOM.resolveNode",
      { backendNodeId: id, executionContextId: contextId },
      options,
    ).catch(() => {
      throw new ToolError("not_found", `e${id} isn't on the page any more; read the page again`);
    });
    return object.objectId;
  };
  const objectId = await resolve(backendNodeId);
  const extra = await Promise.all(others.map(resolve));
  const result = await send<{ result: { value?: T }; exceptionDetails?: { text?: string } }>(
    tabId,
    "Runtime.callFunctionOn",
    {
      objectId,
      functionDeclaration: fn,
      arguments: [...extra.map((id) => ({ objectId: id })), ...args.map((value) => ({ value }))],
      returnByValue: true,
      awaitPromise: true,
    },
    options,
  );
  if (result.exceptionDetails) {
    throw new ToolError("page_error", `the page refused: ${result.exceptionDetails.text ?? "error"}`);
  }
  return result.result.value as T;
}

// Resolve a related DOM element returned by an extension-owned function.
// This is internal, not a protocol operation or a way to run supplied code.
// A backend id lets the normal CDP geometry and hit guards use that element.
export async function relatedNode(tabId: number, backendNodeId: number, fn: string, args: unknown[] = []): Promise<number | undefined> {
  const contextId = await isolatedContext(tabId);
  const { object } = await send<{ object: { objectId: string } }>(tabId, "DOM.resolveNode", { backendNodeId, executionContextId: contextId });
  let related: string | undefined;
  try {
    const result = await send<{ result: { objectId?: string; subtype?: string }; exceptionDetails?: unknown }>(tabId, "Runtime.callFunctionOn", {
      objectId: object.objectId, functionDeclaration: fn,
      arguments: args.map((value) => ({ value })), returnByValue: false,
    });
    related = result.result.objectId;
    if (result.exceptionDetails) throw new ToolError("page_error", "the page refused to resolve that control's label");
    if (!related || result.result.subtype !== "node") return undefined;
    const { node } = await send<{ node: { backendNodeId: number } }>(tabId, "DOM.describeNode", { objectId: related });
    return node.backendNodeId;
  } finally {
    for (const objectId of [related, object.objectId]) {
      if (objectId) await send(tabId, "Runtime.releaseObject", { objectId }).catch(() => {});
    }
  }
}
