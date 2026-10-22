// The service worker: connects to Chat with Work Desktop through native
// messaging, answers the daemon's requests with the methods in actions.ts,
// and keeps the kill switch. An open native port keeps the worker alive;
// an alarm brings it back to reconnect.

import { methods, type Policy } from "./actions.js";
import { detachAll } from "./cdp.js";
import { ToolError } from "./errors.js";
import { groupTabs } from "./group.js";

export const HOST = "com.chatwithwork.desktop";
const RECONNECT_ALARM = "reconnect";

type DaemonState = "connected" | "not_running" | "refused" | "unknown";

const state = {
  // Native messaging: whether `cww` is running for us.
  native: "disconnected" as "connected" | "disconnected",
  // The daemon behind it.
  daemon: "unknown" as DaemonState,
  error: "",
  policy: { sites: [], stopped: false } as Policy,
  // The person pressed Stop in this extension.
  stopped: false,
  // Calls in progress, for the badge.
  busy: 0,
};

let port: chrome.runtime.Port | null = null;

async function loadStopped(): Promise<void> {
  const stored = await chrome.storage.local.get("stopped");
  state.stopped = stored.stopped === true;
}

function connect(): void {
  if (port) return;
  let next: chrome.runtime.Port;
  try {
    next = chrome.runtime.connectNative(HOST);
  } catch (error) {
    state.error = String((error as Error)?.message ?? error);
    scheduleReconnect();
    return;
  }
  port = next;
  state.native = "connected";
  state.error = "";
  next.onMessage.addListener((message) => void onMessage(message));
  next.onDisconnect.addListener(() => {
    state.error = chrome.runtime.lastError?.message ?? "";
    port = null;
    state.native = "disconnected";
    state.daemon = "unknown";
    void refreshBadge();
    scheduleReconnect();
  });
  post({
    event: "hello",
    version: chrome.runtime.getManifest().version,
    browser: navigator.userAgent,
    stopped: state.stopped,
  });
  void refreshBadge();
}

function scheduleReconnect(): void {
  void chrome.alarms.create(RECONNECT_ALARM, { delayInMinutes: 0.5 });
}

function post(message: unknown): void {
  try {
    port?.postMessage(message);
  } catch {
    // The port closed meanwhile; onDisconnect reconnects.
  }
}

async function onMessage(message: any): Promise<void> {
  if (!message || typeof message !== "object") return;
  if (message.event === "daemon") {
    state.daemon = (["connected", "not_running", "refused"].includes(message.state) ? message.state : "unknown") as DaemonState;
    state.error = typeof message.message === "string" ? message.message.slice(0, 300) : "";
    void refreshBadge();
    return;
  }
  if (message.method === "policy" && message.id === undefined) {
    const params = message.params ?? {};
    state.policy = {
      sites: Array.isArray(params.sites) ? params.sites.filter((s: unknown) => typeof s === "string") : [],
      stopped: params.stopped === true,
    };
    if (state.policy.stopped) await detachAll();
    void refreshBadge();
    return;
  }
  if (typeof message.id !== "number" || typeof message.method !== "string") return;
  const reply = await answer(message.method, message.params ?? {});
  post({ id: message.id, ...reply });
}

async function answer(method: string, params: unknown): Promise<{ result: unknown } | { error: { code: string; message: string } }> {
  try {
    if (state.stopped || state.policy.stopped) {
      throw new ToolError(
        "browser_stopped",
        "the person stopped Chat with Work from using the browser; they can resume it on their computer",
      );
    }
    const run = Object.hasOwn(methods, method) ? methods[method] : undefined;
    if (!run) throw new ToolError("invalid_argument", `unknown method ${method.slice(0, 40)}`);
    if (!params || typeof params !== "object" || Array.isArray(params)) {
      throw new ToolError("invalid_argument", "params must be an object");
    }
    state.busy++;
    void refreshBadge();
    try {
      return { result: await run(params as Record<string, unknown>, state.policy) };
    } finally {
      state.busy--;
      setTimeout(() => void refreshBadge(), 1500);
    }
  } catch (error) {
    if (error instanceof ToolError) return { error: { code: error.code, message: error.message } };
    return { error: { code: "internal", message: String((error as Error)?.message ?? error).slice(0, 300) } };
  }
}

async function refreshBadge(): Promise<void> {
  const off = state.stopped || state.policy.stopped;
  const ready = state.native === "connected" && state.daemon === "connected";
  let text = "";
  let color = "#71717a";
  let title = "Chat with Work";
  if (off) {
    text = "OFF";
    title = "Chat with Work: stopped";
  } else if (!ready) {
    text = "!";
    color = "#d97706";
    title = "Chat with Work: Chat with Work Desktop isn't connected";
  } else if (state.busy > 0) {
    text = "ON";
    color = "#16a34a";
    title = "Chat with Work is using a tab";
  }
  await chrome.action.setBadgeText({ text }).catch(() => {});
  await chrome.action.setBadgeBackgroundColor({ color }).catch(() => {});
  await chrome.action.setTitle({ title }).catch(() => {});
}

async function setStopped(stopped: boolean): Promise<void> {
  state.stopped = stopped;
  await chrome.storage.local.set({ stopped });
  if (stopped) await detachAll();
  post({ event: "state", stopped });
  await refreshBadge();
}

// The popup's questions.
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  // Only the extension's own menu, in its popup or opened as a page.
  if (sender.id !== chrome.runtime.id || !sender.url?.startsWith(chrome.runtime.getURL("popup.html"))) {
    return false;
  }
  void (async () => {
    if (message?.type === "stop") await setStopped(true);
    if (message?.type === "resume") await setStopped(false);
    if (message?.type === "reconnect") connect();
    const tabs = await groupTabs();
    reply({
      native: state.native,
      daemon: state.daemon,
      error: state.error,
      stopped: state.stopped,
      stoppedOnComputer: state.policy.stopped,
      sites: state.policy.sites,
      tabs: tabs.length,
      version: chrome.runtime.getManifest().version,
    });
  })();
  return true;
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === RECONNECT_ALARM) connect();
});
chrome.runtime.onStartup.addListener(() => connect());
chrome.runtime.onInstalled.addListener(() => connect());

void loadStopped().then(() => {
  connect();
  void refreshBadge();
});
