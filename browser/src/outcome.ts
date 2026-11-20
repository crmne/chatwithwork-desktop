// What an engine operation that ended without success sent to the page, from
// the flags the extension sets itself before each input event goes out
// (engine.ts). Shared by the outcome check and its fallback when the check
// runs out of time, so a pointer move or partial input is never lost.

export interface SentFlags {
  action: string;
  dispatched: boolean;
  moved: boolean;
  released: boolean;
  written: boolean;
}

export interface Sent {
  dispatched: boolean;
  moved?: boolean;
  complete?: boolean;
}

export function sentFacts(op: SentFlags): Sent {
  if (!op.dispatched) return { dispatched: false, ...(op.moved ? { moved: true } : {}) };
  return { dispatched: true, complete: op.action === "click" ? op.released : op.action === "type" ? op.written : true };
}
