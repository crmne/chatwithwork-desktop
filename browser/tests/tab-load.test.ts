import assert from "node:assert/strict";
import { mock, test } from "node:test";
import { loadEnded, type TabLoads } from "../src/tab-load.js";

type Listener = (tabId: number, change: { status?: string }) => void;

function fakeTabs(status: string | Promise<string>) {
  const listeners = new Set<Listener>();
  const tabs: TabLoads = {
    onUpdated: { addListener: (f) => void listeners.add(f), removeListener: (f) => void listeners.delete(f) },
    get: async () => ({ status: await status }),
  };
  const update = (tabId: number, change: { status?: string }) => { for (const f of [...listeners]) f(tabId, change); };
  return { tabs, listeners, update };
}

async function settled(promise: Promise<void>): Promise<boolean> {
  let done = false;
  void promise.then(() => { done = true; });
  for (let i = 0; i < 10; i++) await Promise.resolve();
  return done;
}

test("waits while the new tab's blank page is loading, and ends when its load does", async () => {
  const { tabs, listeners, update } = fakeTabs("loading");
  const wait = loadEnded(tabs, 7, 60_000);
  assert.equal(await settled(wait), false);
  update(8, { status: "complete" });
  update(7, { status: "loading" });
  update(7, {});
  assert.equal(await settled(wait), false);
  update(7, { status: "complete" });
  assert.equal(await settled(wait), true);
  assert.equal(listeners.size, 0);
});

test("an unloaded tab isn't taken for a loaded one", async () => {
  const { tabs, update } = fakeTabs("unloaded");
  const wait = loadEnded(tabs, 7, 60_000);
  update(7, { status: "unloaded" });
  assert.equal(await settled(wait), false);
  update(7, { status: "complete" });
  assert.equal(await settled(wait), true);
});

test("a tab that already finished, even before the listener was added, doesn't wait", async () => {
  const { tabs, listeners } = fakeTabs("complete");
  assert.equal(await settled(loadEnded(tabs, 7, 60_000)), true);
  assert.equal(listeners.size, 0);
});

test("a tab that's gone doesn't wait", async () => {
  const { tabs, listeners } = fakeTabs(Promise.reject(new Error("No tab with id: 7")));
  assert.equal(await settled(loadEnded(tabs, 7, 60_000)), true);
  assert.equal(listeners.size, 0);
});

test("a load that never ends is waited for no longer than the limit", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { tabs, listeners } = fakeTabs("loading");
    const wait = loadEnded(tabs, 7, 2_000);
    mock.timers.tick(1_999);
    assert.equal(await settled(wait), false);
    mock.timers.tick(1);
    assert.equal(await settled(wait), true);
    assert.equal(listeners.size, 0);
  } finally {
    mock.timers.reset();
  }
});
