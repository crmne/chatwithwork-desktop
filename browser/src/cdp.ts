// The debugger, attached only to tabs in the Chat with Work group and only
// while they're being used: it detaches after a minute without a call, so
// Chrome's "started debugging this browser" bar doesn't stay.

import { pageStartedDownload } from "./downloads.js";
import { browserStopped, ToolError } from "./errors.js";

const IDLE_DETACH_MS = 60_000;
const COMMAND_TIMEOUT_MS = 15_000;

interface Session {
  idle?: ReturnType<typeof setTimeout>;
  // The last JavaScript dialog the extension dismissed, for the next result.
  dialog?: string;
}

// Calls that put back what a tool changed on the page (a screenshot's
// blur) still run after Stop; nothing else does.
export interface SendOptions {
  cleanup?: boolean;
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
export async function halt(on: boolean): Promise<void> {
  halted = on;
  if (on) await detachAll();
}

// Run `work` with cleanup calls allowed, and, if Stop was pressed
// meanwhile, detach again once it's done.
export async function cleanUp(tabId: number, work: (options: SendOptions) => Promise<void>): Promise<void> {
  try {
    await work({ cleanup: true });
  } finally {
    if (halted) await detach(tabId);
  }
}

export function isAttached(tabId: number): boolean {
  return sessions.has(tabId);
}

async function attach(tabId: number, options: SendOptions = {}): Promise<void> {
  if (halted && !options.cleanup) throw browserStopped();
  if (sessions.has(tabId)) return;
  const pending = attaching.get(tabId);
  if (pending) return pending;
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
    sessions.set(tabId, {});
    const target = { tabId };
    // Answer dialogs so a page can't hang the agent, and behave as if
    // focused while in the background.
    await chrome.debugger.sendCommand(target, "Page.enable").catch(() => {});
    await chrome.debugger
      .sendCommand(target, "Emulation.setFocusEmulationEnabled", { enabled: true })
      .catch(() => {});
  })();
  attaching.set(tabId, work);
  try {
    await work;
  } finally {
    attaching.delete(tabId);
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
  await attach(tabId, options);
  touch(tabId);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new ToolError("timeout", `the page didn't answer ${method} in time`)),
      COMMAND_TIMEOUT_MS,
    );
  });
  try {
    return (await Promise.race([
      chrome.debugger.sendCommand({ tabId }, method, params),
      timeout,
    ])) as T;
  } catch (error) {
    if (error instanceof ToolError) throw error;
    throw new ToolError("page_error", `${method}: ${String((error as Error)?.message ?? error)}`);
  } finally {
    clearTimeout(timer);
  }
}

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
