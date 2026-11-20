// Waiting for a tab's load to end, through the parts of chrome.tabs it
// needs, so it can be tested without a browser.

export interface TabLoads {
  onUpdated: {
    addListener(callback: (tabId: number, change: { status?: string }) => void): void;
    removeListener(callback: (tabId: number, change: { status?: string }) => void): void;
  };
  get(tabId: number): Promise<{ status?: string }>;
}

// Resolves once `tabId` isn't loading any more (complete, or unloaded), or
// after `ms` at most, and stops listening either way. Never rejects: a tab
// that's gone is left to whatever is done with it next.
export function loadEnded(tabs: TabLoads, tabId: number, ms: number): Promise<void> {
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      tabs.onUpdated.removeListener(changed);
      resolve();
    };
    const changed = (id: number, change: { status?: string }) => {
      if (id === tabId && change.status !== undefined && change.status !== "loading") done();
    };
    const timer = setTimeout(done, ms);
    tabs.onUpdated.addListener(changed);
    // Asked after listening, so a load that ended meanwhile isn't missed.
    void tabs.get(tabId).then((now) => { if (now.status !== "loading") done(); }, done);
  });
}
