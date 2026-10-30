// Semantic input binding only. Playwright owns coordinates, hit testing,
// stability and retry. This guard cannot make an obscured target clickable.
import { CLASSIFY_IN_PAGE } from "./classify.js";
function install(this: Element, surface: Element, key: string, action: string, submits: boolean, classify: (facts: any) => { sensitive: boolean; submits: string | null }): void {
  const control = this;
  const alternate = surface !== control;
  const type = control.getAttribute("type");
  let blocked: string | null = null;
  const events = ["pointerdown", "mousedown", "pointerup", "mouseup", "click", "keydown", "beforeinput"];
  const listener = (event: Event) => {
    if (!event.isTrusted) return;
    const attrs = Object.fromEntries(Array.from(control.attributes, a => [a.name, a.value]));
    const form = (control as HTMLInputElement).form ?? control.closest("form");
    const facts = { tag: control.localName, attrs, role: attrs.role ?? "", name: [attrs["aria-label"], ...(Array.from((control as HTMLInputElement).labels ?? [], l => l.textContent)), control.textContent].join(" "), form: form ? { method: form.getAttribute("method") || "get" } : null };
    const classification = classify(facts);
    if (!control.isConnected || !surface.isConnected) blocked = "the requested control was removed";
    if (action === "type" && classification.sensitive) blocked = "the requested field became protected";
    if (action === "click" && !submits && classification.submits) blocked = "the requested control now submits a change";
    if (alternate) {
      if (!(surface instanceof HTMLLabelElement) || surface.control !== control ||
          !(control instanceof HTMLInputElement) || !["radio", "checkbox"].includes(control.type) || control.getAttribute("type") !== type) {
        blocked = "the label no longer activates the requested control";
      }
      for (const target of event.composedPath()) {
        if (target === surface || target === control) break;
        if (target instanceof Element && target.matches("label,a[href],button,input,select,textarea,summary,details,audio[controls],video[controls],img[usemap],object[usemap],[contenteditable]:not([contenteditable=false]),[role=button],[role=link]")) {
          blocked = "another interactive control occupies this label";
        }
      }
    }
    if (blocked) { event.preventDefault(); event.stopImmediatePropagation(); }
  };
  const stop = () => {
    for (const name of events) window.removeEventListener(name, listener, true);
    clearTimeout(timer); delete (globalThis as any)[key]; return blocked;
  };
  for (const name of events) window.addEventListener(name, listener, { capture: true, passive: false });
  const timer = setTimeout(stop, 18_000);
  (globalThis as any)[key] = { stop, result: () => blocked };
}
export const INSTALL_ENGINE_GUARD = `function(surface, key, action, submits) { return (${install.toString()}).call(this, surface, key, action, submits, ${CLASSIFY_IN_PAGE}); }`;
