// The service worker: connects to Chat with Work Desktop through native
// messaging, answers the daemon's requests with the methods in actions.ts,
// and keeps the kill switch. An open native port keeps the worker alive;
// an alarm brings it back to reconnect.

import { methods as readMethods, type Policy } from "./actions.js";
import { changeMethods } from "./changes.js";
import { cancelEngine, engineEvents, engineMethods } from "./engine.js";
import { halt } from "./cdp.js";
import { browserStopped, ToolError } from "./errors.js";
import { groupTabs } from "./group.js";

export const HOST = "com.chatwithwork.desktop";
const RECONNECT_ALARM = "reconnect";
const methods = { ...readMethods, ...changeMethods, ...engineMethods };

type DaemonState = "connected" | "not_running" | "refused" | "unknown";

const state = {
  // Native messaging: whether `cww` is running for us.
  native: "disconnected" as "connected" | "disconnected",
  // The daemon behind it.
  daemon: "unknown" as DaemonState,
  error: "",
  policy: { sites: [], all_sites: false, blocked_sites: [], stopped: false, changes: false } as Policy,
  // The person pressed Stop in this extension.
  stopped: false,
  // Calls in progress, for the badge.
  busy: 0,
};

let port: chrome.runtime.Port | null = null;
let connectionGeneration = 0;

async function loadStopped(): Promise<void> {
  const stored = await chrome.storage.local.get("stopped");
  state.stopped = stored.stopped === true;
  await halt(state.stopped || state.policy.stopped);
}

// The kill switch is read before anything connects or answers, so a worker
// that just started never says it isn't stopped when it is.
const loaded = loadStopped();

async function connect(): Promise<void> {
  await loaded;
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
    connectionGeneration++;
    void cancelEngine();
    port = null;
    state.native = "disconnected";
    state.daemon = "unknown";
    void refreshBadge();
    scheduleReconnect();
  });
  announceHello();
  void refreshBadge();
}

function announceHello(): void {
  post({
    event: "hello",
    version: chrome.runtime.getManifest().version,
    site_policy_version: 2,
    engine_protocol: 1,
    browser: navigator.userAgent,
    stopped: state.stopped,
  });
}

function scheduleReconnect(): void {
  void chrome.alarms.create(RECONNECT_ALARM, { delayInMinutes: 0.5 });
}

engineEvents(message => post(message));

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
    connectionGeneration++;
    void cancelEngine();
    state.daemon = (["connected", "not_running", "refused"].includes(message.state) ? message.state : "unknown") as DaemonState;
    state.error = typeof message.message === "string" ? message.message.slice(0, 300) : "";
    // The native host survives a daemon restart. The new daemon needs our
    // identity and current Stop state even though this native port stayed open.
    if (state.daemon === "connected") announceHello();
    void refreshBadge();
    return;
  }
  if (message.method === "policy" && message.id === undefined) {
    const params = message.params ?? {};
    Object.assign(state.policy, {
      sites: Array.isArray(params.sites) ? params.sites.filter((s: unknown) => typeof s === "string") : [],
      all_sites: params.all_sites === true,
      blocked_sites: Array.isArray(params.blocked_sites) ? params.blocked_sites.filter((s: unknown) => typeof s === "string") : [],
      stopped: params.stopped === true,
      changes: params.changes === true,
    });
    if (state.stopped || state.policy.stopped) await halt(true, cancelEngine);
    else { const cancellation = cancelEngine(); await halt(false); await cancellation; }
    void refreshBadge();
    return;
  }
  if (typeof message.id !== "number" || typeof message.method !== "string") return;
  const connection = port;
  const generation = connectionGeneration;
  const reply = await answer(message.method, message.params ?? {});
  if (port === connection && generation === connectionGeneration) post({ id: message.id, ...reply });
}

async function answer(method: string, params: unknown): Promise<{ result: unknown } | { error: { code: string; message: string } }> {
  await loaded;
  try {
    if ((state.stopped || state.policy.stopped) && method !== "engine_cancel") throw browserStopped();
    const run = Object.hasOwn(methods, method) ? methods[method] : undefined;
    if (!run) throw new ToolError("invalid_argument", `unknown method ${method.slice(0, 40)}`);
    if (!params || typeof params !== "object" || Array.isArray(params)) {
      throw new ToolError("invalid_argument", "params must be an object");
    }
    state.busy++;
    void refreshBadge();
    try {
      const result = await run(params as Record<string, unknown>, state.policy);
      // Stop pressed while the call ran: what it found doesn't go out.
      if ((state.stopped || state.policy.stopped) && method !== "engine_cancel") throw browserStopped();
      return { result };
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
  await Promise.all([chrome.storage.local.set({ stopped }), halt(state.stopped || state.policy.stopped, cancelEngine)]);
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
    if (message?.type === "reconnect") await connect();
    const tabs = await groupTabs();
    reply({
      native: state.native,
      daemon: state.daemon,
      error: state.error,
      stopped: state.stopped,
      stoppedOnComputer: state.policy.stopped,
      sites: state.policy.sites,
      all_sites: state.policy.all_sites,
      blocked_sites: state.policy.blocked_sites,
      changes: state.policy.changes,
      tabs: tabs.length,
      version: chrome.runtime.getManifest().version,
    });
  })();
  return true;
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === RECONNECT_ALARM) void connect();
});
chrome.runtime.onStartup.addListener(() => void connect());
chrome.runtime.onInstalled.addListener(() => void connect());

void connect().then(() => refreshBadge());
