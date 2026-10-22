// The tab group the extension works in. A tab is in reach only while it's
// in a group named "Chat with Work"; the person can drag tabs in to lend
// them, and out to take them back.

import { ToolError } from "./errors.js";

export const GROUP_TITLE = "Chat with Work";

export function isOurGroup(group: chrome.tabGroups.TabGroup): boolean {
  return (group.title ?? "").trim() === GROUP_TITLE;
}

export async function ourGroupIds(): Promise<Set<number>> {
  const groups = await chrome.tabGroups.query({});
  return new Set(groups.filter(isOurGroup).map((g) => g.id));
}

export async function groupTabs(): Promise<chrome.tabs.Tab[]> {
  const groups = await ourGroupIds();
  if (groups.size === 0) return [];
  const tabs = await chrome.tabs.query({});
  return tabs.filter((t) => t.id !== undefined && groups.has(t.groupId));
}

// The tab a call names, which must be in the group; or, without one, the
// group's tab used last.
export async function resolveTab(tab: unknown): Promise<chrome.tabs.Tab> {
  const tabs = await groupTabs();
  if (tab !== undefined && tab !== null) {
    const id = Number(tab);
    const found = tabs.find((t) => t.id === id);
    if (!found) {
      throw new ToolError(
        "not_in_group",
        `tab ${String(tab)} isn't in the Chat with Work tab group; browser_tabs lists the tabs there`,
      );
    }
    return found;
  }
  if (tabs.length === 0) {
    throw new ToolError(
      "no_tab",
      "there's no tab in the Chat with Work tab group yet; open one with browser_open if it's offered, or ask the person to drag a tab into a group named “Chat with Work”",
    );
  }
  return tabs.reduce((a, b) => ((b.lastAccessed ?? 0) > (a.lastAccessed ?? 0) ? b : a));
}

// Open `url` in a new background tab in the group, making the group in the
// window used last if there isn't one.
export async function openInGroup(url: string): Promise<chrome.tabs.Tab> {
  const groups = (await chrome.tabGroups.query({})).filter(isOurGroup);
  let windowId: number | undefined = groups[0]?.windowId;
  if (windowId === undefined) {
    const window = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => undefined);
    windowId = window?.id;
  }
  const tab = await chrome.tabs.create({ url, active: false, ...(windowId !== undefined ? { windowId } : {}) });
  await addToGroup(tab.id!, groups[0]?.id);
  return chrome.tabs.get(tab.id!);
}

export async function addToGroup(tabId: number, groupId?: number): Promise<void> {
  if (groupId !== undefined) {
    await chrome.tabs.group({ tabIds: [tabId], groupId });
  } else {
    const created = await chrome.tabs.group({ tabIds: [tabId] });
    await chrome.tabGroups.update(created, { title: GROUP_TITLE, color: "blue", collapsed: false });
  }
  // Memory Saver doesn't discard the agent's tabs.
  await chrome.tabs.update(tabId, { autoDiscardable: false }).catch(() => {});
}

// Tabs a group tab opens (`target=_blank`, `window.open`) join the group.
// A popup window of its own can't be grouped and stays out of reach.
chrome.tabs.onCreated.addListener((tab) => {
  void (async () => {
    if (tab.id === undefined || tab.openerTabId === undefined) return;
    const opener = await chrome.tabs.get(tab.openerTabId).catch(() => undefined);
    if (!opener || opener.groupId === chrome.tabGroups.TAB_GROUP_ID_NONE) return;
    const groups = await ourGroupIds();
    if (!groups.has(opener.groupId)) return;
    // Chrome usually puts it there itself.
    if (tab.groupId === opener.groupId) return;
    const window = await chrome.windows.get(tab.windowId).catch(() => undefined);
    if (window?.type !== "normal") return;
    await chrome.tabs.group({ tabIds: [tab.id], groupId: opener.groupId }).catch(() => {});
  })();
});
