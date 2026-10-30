// Portions adapted from Playwright, Copyright (c) Microsoft Corporation,
// licensed under Apache-2.0. See static/THIRD-PARTY-NOTICES.txt and
// static/LICENSE-APACHE for provenance and the full license.
// Modifications: fixed isolated-world scripts, bounded background-tab waits,
// privacy-limited diagnostics and no force, selector, or arbitrary-code API.

export interface Point { x: number; y: number }

// Playwright's _clickablePoint: clip content quads to the viewport before
// choosing a center, and skip offscreen/zero-area fragments of inline links.
export function visiblePoint(quads: number[][], width: number, height: number): Point | undefined {
  for (const quad of quads) {
    if (quad.length !== 8 || !quad.every(Number.isFinite)) continue;
    const points = [0, 2, 4, 6].map((i) => ({
      x: Math.min(Math.max(quad[i], 0), width),
      y: Math.min(Math.max(quad[i + 1], 0), height),
    }));
    const area = Math.abs(points.reduce((sum, p, i) => {
      const next = points[(i + 1) % points.length];
      return sum + (p.x * next.y - next.x * p.y) / 2;
    }, 0));
    if (area > 0.99) return points.reduce((sum, p) => ({ x: sum.x + p.x / 4, y: sum.y + p.y / 4 }), { x: 0, y: 0 });
  }
  return undefined;
}

// Self-contained: this function is serialized into the extension's isolated
// world. Its arguments and source are extension-owned, never model-supplied.
// Adapted from Playwright's expectHitTarget. Walk roots outside-in, including
// closed roots reached from the target; CDP's hit node alone is retargeted and
// is not evidence that a shadow/slotted/contents target is covered.
function hitTarget(target: Element, point: Point): string | null {
  const parent = (element: Element): Element | null =>
    element.parentElement ?? (element.getRootNode() as ShadowRoot).host ?? null;
  const roots: (Document | ShadowRoot)[] = [];
  for (let element: Element | null = target; element;) {
    const root = element.getRootNode() as Document | ShadowRoot;
    if (root.nodeType !== 9 && !(root as ShadowRoot).host) return "the target is detached";
    roots.push(root);
    element = root.nodeType === 9 ? null : (root as ShadowRoot).host;
  }
  let hit: Element | undefined;
  for (let index = roots.length - 1; index >= 0; index--) {
    const root = roots[index];
    const elements = root.elementsFromPoint(point.x, point.y);
    const single = root.elementFromPoint(point.x, point.y);
    if (single && elements[0] && parent(single) === elements[0] && getComputedStyle(single).display === "contents") {
      // Chromium 1342092: elementsFromPoint can omit display:contents nodes.
      elements.unshift(single);
    }
    if (elements[0]?.shadowRoot === root && elements[1] === single) elements.shift();
    if (!elements[0]) break;
    hit = elements[0];
    if (index && hit !== (roots[index - 1] as ShadowRoot).host) break;
  }
  for (let element: Element | null | undefined = hit; element; element = element.assignedSlot ?? parent(element)) {
    if (element === target) return null;
  }
  // No outerHTML, textContent, URLs, field values or arbitrary attributes.
  // A short accessible label helps distinguish a consent dialog from a bug.
  const tag = hit?.localName ?? "viewport";
  const role = hit?.getAttribute("role")?.slice(0, 40);
  const label = hit?.getAttribute("aria-label")?.replace(/\s+/g, " ").slice(0, 120);
  return `something else is drawn over that element: <${tag}>${role ? ` role=${JSON.stringify(role)}` : ""}${label ? ` name=${JSON.stringify(label)}` : ""}; read the page again`;
}

export interface PointerState {
  reason: string | null;
  width: number;
  height: number;
}

function state(this: Element): PointerState {
  let reason: string | null = null;
  if (!this.isConnected) reason = "the target is detached; read the page again";
  // CDP content quads are in the main viewport. Do not mix them with a
  // different document's hit-test coordinates or bypass a covering iframe.
  else if (this.ownerDocument !== document) reason = "clicks inside this frame are not supported; open its page in a separate allowed tab";
  else if (getComputedStyle(this).visibility !== "visible") reason = "that element isn't visible on the page";
  else {
    for (let element: Element | null = this; element; element = element.assignedSlot ?? element.parentElement ?? (element.getRootNode() as ShadowRoot).host ?? null) {
      if (element.matches(":disabled") || element.getAttribute("aria-disabled") === "true" || element.hasAttribute("inert")) {
        reason = "that element is disabled";
        break;
      }
    }
  }
  return { reason, width: innerWidth, height: innerHeight };
}

export const POINTER_STATE = state.toString();
export const HIT_TARGET = `function (point) { return (${hitTarget.toString()})(this, point); }`;
export const SCROLL_TARGET = `function (alignment) {
  this.scrollIntoView({block: alignment, inline: alignment, behavior: "instant"});
}`;

// The guard lives only in our isolated world's global object. Page scripts
// cannot replace it. Capture trusted pointer events so a layout shift after
// the last CDP check cannot deliver the click to an unrelated element. Like
// Playwright, check the first press, then keep its result for the release:
// handlers are allowed to intentionally change layout after mousedown.
function installGuard(this: Element, key: string, point: Point, hit: typeof hitTarget): string | null {
  const initial = hit(this, point);
  if (initial) return initial;
  const target = this;
  let result: string | null | undefined;
  const events = ["pointerdown", "mousedown", "pointerup", "mouseup", "click", "auxclick", "dblclick"];
  const listener = (event: Event) => {
    if (!event.isTrusted) return;
    if (result === undefined) {
      const mouse = event as MouseEvent;
      result = hit(target, { x: mouse.clientX, y: mouse.clientY });
    }
    if (result) {
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
    }
  };
  const stop = () => {
    for (const name of events) window.removeEventListener(name, listener, true);
    clearTimeout(timer);
    delete (globalThis as any)[key];
    return result ?? null;
  };
  for (const name of events) window.addEventListener(name, listener, { capture: true, passive: false });
  // Cleanup survives a lost control connection; no permanent event blocker.
  const timer = setTimeout(stop, 15_000);
  (globalThis as any)[key] = { stop, result: () => result ?? null };
  return null;
}

export const INSTALL_POINTER_GUARD = `function (key, point) {
  return (${installGuard.toString()}).call(this, key, point, ${hitTarget.toString()});
}`;

export function guardStatus(key: string, stop: boolean): string {
  // key is generated by the extension, not supplied by the page/model.
  return `globalThis[${JSON.stringify(key)}]?.${stop ? "stop" : "result"}() ?? null`;
}
