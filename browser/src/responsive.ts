// Tabs whose page stopped answering the debugger, usually because a script
// is stuck in a loop. A hung page must not make every later call wait out
// the full command timeout one command at a time: right after a command
// times out, calls to that tab fail at once; later, one short check decides
// whether the page answers again. Pure, so it can be tested without Chrome.

// How long calls to a tab fail at once after it stopped answering.
export const FAIL_FAST_MS = 5_000;
// How long a check of a tab that stopped answering may take.
export const PROBE_TIMEOUT_MS = 2_000;

export type Verdict = "send" | "probe" | "fail";

export class Responsiveness {
  private readonly hung = new Map<number, number>();

  constructor(private readonly now: () => number = Date.now) {}

  // A command to the tab didn't answer in time.
  stalled(tabId: number): void {
    this.hung.set(tabId, this.now());
  }

  // The tab answered.
  answered(tabId: number): void {
    this.hung.delete(tabId);
  }

  forget(tabId: number): void {
    this.hung.delete(tabId);
  }

  // What to do before the next command to the tab.
  verdict(tabId: number): Verdict {
    const since = this.hung.get(tabId);
    if (since === undefined) return "send";
    return this.now() - since < FAIL_FAST_MS ? "fail" : "probe";
  }
}

export const NOT_RESPONDING =
  "the page in this tab isn't responding (a script on it may be stuck), so nothing more was done in it; " +
  "what was already sent may or may not have taken effect. Other tabs still work: wait and read the page " +
  "again, or close the tab with browser_close";
