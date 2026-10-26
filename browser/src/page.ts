// Small scripts the extension runs in its own isolated world in a tab
// (cdp.evaluate). Each is a self-contained function, turned into an
// expression with its arguments as JSON: they close over nothing.

import { evaluate, isHalted, type SendOptions } from "./cdp.js";

const INDICATOR_ID = "chat-with-work-indicator";

function showIndicator(id: string, label: string): void {
  let host = document.getElementById(id);
  if (!host) {
    host = document.createElement("div");
    host.id = id;
    host.setAttribute("aria-hidden", "true");
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        :host { all: initial; }
        .edge { position: fixed; inset: 0; pointer-events: none; z-index: 2147483647;
          border: 3px solid transparent; border-radius: 4px;
          border-image: linear-gradient(90deg, #ff5f6d, #ffc371, #47e891, #4facfe, #a77bff) 1; }
        .pill { position: fixed; top: 8px; left: 50%; transform: translateX(-50%);
          pointer-events: none; z-index: 2147483647; padding: 4px 12px; border-radius: 999px;
          background: #0b0b0c; color: #f4f4f5; font: 500 12px/1.4 system-ui, sans-serif;
          box-shadow: 0 4px 16px rgba(0,0,0,.3); white-space: nowrap; }
      </style>
      <div class="edge"></div><div class="pill"></div>`;
    (document.documentElement || document.body).appendChild(host);
    (host as any).__cwwRoot = root;
  }
  const root = (host as any).__cwwRoot as ShadowRoot | undefined;
  const pill = root?.querySelector(".pill");
  if (pill) pill.textContent = label;
  host.style.display = "";
}

function hideIndicator(id: string, hidden: boolean): void {
  const host = document.getElementById(id);
  if (!host) return;
  if (hidden) host.style.display = "none";
  else host.remove();
}

// Blur fields the extension never shows, before a screenshot (`on`), and
// take the blur off after (`!on`).
function blurSensitive(on: boolean, autocomplete: string, words: string): number {
  const ATTR = "data-chat-with-work-blur";
  if (!on) {
    document.querySelectorAll(`[${ATTR}]`).forEach((el) => {
      (el as HTMLElement).style.removeProperty("filter");
      el.removeAttribute(ATTR);
    });
    return 0;
  }
  const autocompleteRe = new RegExp(autocomplete);
  const wordsRe = new RegExp(words, "i");
  let count = 0;
  document.querySelectorAll('input, textarea, [contenteditable], [role="textbox"], [role="spinbutton"]').forEach((el) => {
    const type = (el.getAttribute("type") ?? "").toLowerCase();
    const labels = Array.from((el as HTMLInputElement).labels ?? []).map((l) => l.textContent ?? "");
    const labelledBy = (el.getAttribute("aria-labelledby") ?? "")
      .split(/\s+/)
      .map((id) => (id ? document.getElementById(id)?.textContent ?? "" : ""));
    const text = ["name", "id", "aria-label", "placeholder", "data-testid"]
      .map((a) => el.getAttribute(a) ?? "")
      .concat(labels, labelledBy)
      .join(" ");
    const sensitive =
      (el.tagName === "INPUT" && type === "password") ||
      autocompleteRe.test((el.getAttribute("autocomplete") ?? "").toLowerCase()) ||
      wordsRe.test(text);
    if (sensitive) {
      (el as HTMLElement).style.setProperty("filter", "blur(8px)", "important");
      el.setAttribute(ATTR, "");
      count++;
    }
  });
  return count;
}

function call(fn: (...args: any[]) => unknown, ...args: unknown[]): string {
  return `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(",")})`;
}

const hideTimers = new Map<number, ReturnType<typeof setTimeout>>();

// Show "Chat with Work is using this tab" while the agent acts, and for a
// few seconds after its last call.
export async function indicate(tabId: number): Promise<void> {
  const timer = hideTimers.get(tabId);
  if (timer) clearTimeout(timer);
  await evaluate(tabId, call(showIndicator, INDICATOR_ID, "Chat with Work is using this tab")).catch(() => {});
  hideTimers.set(
    tabId,
    setTimeout(() => {
      hideTimers.delete(tabId);
      void evaluate(tabId, call(hideIndicator, INDICATOR_ID, false)).catch(() => {});
    }, 4000),
  );
}

// Around a screenshot: the indicator and sensitive fields don't show in it.
export async function prepareForCapture(tabId: number, patterns: { autocomplete: string; words: string }): Promise<void> {
  await evaluate(tabId, call(hideIndicator, INDICATOR_ID, true)).catch(() => {});
  await evaluate(tabId, call(blurSensitive, true, patterns.autocomplete, patterns.words));
}

export async function afterCapture(
  tabId: number,
  patterns: { autocomplete: string; words: string },
  options: SendOptions = {},
): Promise<void> {
  await evaluate(tabId, call(blurSensitive, false, patterns.autocomplete, patterns.words), options).catch(() => {});
  // After Stop the tab isn't being used any more, so it doesn't say so.
  if (isHalted()) return;
  await evaluate(tabId, call(showIndicator, INDICATOR_ID, "Chat with Work is using this tab"), options).catch(() => {});
}

export const pageScripts = { call };
