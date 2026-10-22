// The debugger, attached only to tabs in the Chat with Work group and only
// while they're being used: it detaches after a minute without a call, so
// Chrome's "started debugging this browser" bar doesn't stay.

import { ToolError } from "./errors.js";

const IDLE_DETACH_MS = 60_000;
const COMMAND_TIMEOUT_MS = 15_000;

interface Session {
  idle?: ReturnType<typeof setTimeout>;
  // The last JavaScript dialog the extension dismissed, for the next result.
  dialog?: string;
}

const sessions = new Map<number, Session>();
const attaching = new Map<number, Promise<void>>();

export function isAttached(tabId: number): boolean {
  return sessions.has(tabId);
}

async function attach(tabId: number): Promise<void> {
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
    // Downloads only go where `browser_download` puts them.
    await chrome.debugger
      .sendCommand(target, "Page.setDownloadBehavior", { behavior: "deny" })
      .catch(() => {});
  })();
  attaching.set(tabId, work);
  try {
    await work;
  } finally {
    attaching.delete(tabId);
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
): Promise<T> {
  await attach(tabId);
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

// Run `expression` in a world of the extension's own in the tab's main
// frame: it sees the page's DOM, but not the page's scripts, nor they it.
export async function evaluate<T = unknown>(tabId: number, expression: string): Promise<T> {
  const tree = await send<{ frameTree: { frame: { id: string } } }>(tabId, "Page.getFrameTree");
  const world = await send<{ executionContextId: number }>(tabId, "Page.createIsolatedWorld", {
    frameId: tree.frameTree.frame.id,
    worldName: "Chat with Work",
    grantUniveralAccess: false,
  });
  const result = await send<{ result: { value?: T }; exceptionDetails?: { text?: string } }>(
    tabId,
    "Runtime.evaluate",
    {
      expression,
      contextId: world.executionContextId,
      returnByValue: true,
      awaitPromise: true,
    },
  );
  if (result.exceptionDetails) {
    throw new ToolError("page_error", `the page refused a check: ${result.exceptionDetails.text ?? "error"}`);
  }
  return result.result.value as T;
}
