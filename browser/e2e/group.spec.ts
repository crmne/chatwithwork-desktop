import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

let setup: Setup;
let site: Awaited<ReturnType<typeof serve>>;

test.beforeEach(async () => {
  site = await serve({ "/": "<!doctype html><title>Group fixture</title><h1>Group fixture</h1>" });
  setup = await launch([site.origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});

test.afterEach(async () => {
  await setup?.cleanup();
  site?.server.close();
});

test("the first group belongs to the new tab's normal window even while a popup is focused", async () => {
  const windows = await inWorker(setup.context, async () => {
    const normal = await chrome.windows.getLastFocused({ windowTypes: ["normal"] });
    const popup = await chrome.windows.create({ url: "about:blank", type: "popup", focused: true });
    return { normal: normal.id, popup: popup!.id, popupTab: popup!.tabs![0].id };
  });
  const opened = await setup.daemon.call("open", { url: site.origin + "/" });
  expect(opened.error).toBeUndefined();
  expect(opened.result.title).toBe("Group fixture");
  const result = await inWorker(setup.context, async ({ tab, popupTab }) => {
    const owned = await chrome.tabs.get(tab);
    const untouched = await chrome.tabs.get(popupTab);
    const group = await chrome.tabGroups.get(owned.groupId);
    return { windowId: owned.windowId, groupWindowId: group.windowId, title: group.title,
      popupUrl: untouched.url, popupGroup: untouched.groupId };
  }, { tab: opened.result.tab, popupTab: windows.popupTab });
  expect(result).toEqual({ windowId: windows.normal, groupWindowId: windows.normal,
    title: "Chat with Work", popupUrl: "about:blank", popupGroup: -1 });
});

test("grouping failure removes only the new blank tab and never navigates to the requested page", async () => {
  let requests = 0;
  site.server.on("request", () => { requests++; });
  const before = await inWorker(setup.context, async () => {
    const tabs = await chrome.tabs.query({});
    // Fault injection in the disposable fixture only. No live profile uses it.
    chrome.tabs.group = async () => { throw new Error("synthetic grouping failure"); };
    return tabs.map(t => t.id).sort();
  });
  const opened = await setup.daemon.call("open", { url: site.origin + "/" });
  expect(opened.error?.message).toContain("synthetic grouping failure");
  const after = await inWorker(setup.context, async () => (await chrome.tabs.query({})).map(t => t.id).sort());
  expect(after).toEqual(before);
  expect(requests).toBe(0);
});
