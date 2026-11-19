// Which elements of a page take files: every `<input type=file>`, in shadow
// roots and same-process frames too, hidden ones included (a page's own
// button usually opens a hidden one), with the labels and buttons that go
// with it, and buttons named for uploading. Read from the debugger's DOM
// tree, so no page script runs. Pure, so it can be tested without a browser.

import { DROP_WORDS, UPLOAD_WORDS } from "./accept.js";

export interface DomNode {
  backendNodeId: number;
  nodeType: number;
  nodeName: string;
  localName?: string;
  nodeValue?: string;
  attributes?: string[];
  children?: DomNode[];
  shadowRoots?: DomNode[];
  contentDocument?: DomNode;
}

export interface UploadMark {
  // A file field itself, a label or button that goes with one, or an
  // element that says to drop files on it.
  kind: "field" | "button" | "drop";
  label: string;
  accept?: string;
  multiple: boolean;
  // The file field a label or button goes with, when known.
  field?: number;
}

const LABEL_LIMIT = 100;
const BUTTONS_PER_FIELD = 3;
const MARK_LIMIT = 200;

interface Seen {
  node: DomNode;
  parent: Seen | null;
  scope: DomNode;
  attrs: Record<string, string>;
}

function attributes(node: DomNode): Record<string, string> {
  const out: Record<string, string> = {};
  const list = node.attributes ?? [];
  for (let i = 0; i + 1 < list.length; i += 2) out[list[i].toLowerCase()] = list[i + 1];
  return out;
}

function tag(node: DomNode): string {
  return (node.localName || node.nodeName || "").toLowerCase();
}

// The text inside an element, squished and cut, frames and shadow roots left out.
export function textOf(node: DomNode, limit = LABEL_LIMIT): string {
  const parts: string[] = [];
  let length = 0;
  const walk = (n: DomNode) => {
    if (length > limit * 2) return;
    if (n.nodeType === 3 && n.nodeValue) {
      parts.push(n.nodeValue);
      length += n.nodeValue.length;
    }
    const name = tag(n);
    if (name === "script" || name === "style" || name === "template") return;
    for (const child of n.children ?? []) walk(child);
  };
  walk(node);
  return parts.join(" ").replace(/\s+/g, " ").trim().slice(0, limit);
}

function isButton(seen: Seen): boolean {
  const name = tag(seen.node);
  const role = (seen.attrs.role ?? "").trim().toLowerCase();
  const type = (seen.attrs.type ?? "").trim().toLowerCase();
  return (
    (name === "button" && type !== "submit" && type !== "reset") ||
    (name === "input" && type === "button") ||
    role === "button"
  );
}

function isFileInput(seen: Seen): boolean {
  return tag(seen.node) === "input" && (seen.attrs.type ?? "").trim().toLowerCase() === "file";
}

export function uploadMarks(root: DomNode): Map<number, UploadMark> {
  const all: Seen[] = [];
  const visit = (node: DomNode, parent: Seen | null, scope: DomNode) => {
    let here = parent;
    if (node.nodeType === 1) {
      here = { node, parent, scope, attrs: attributes(node) };
      all.push(here);
    }
    for (const child of node.children ?? []) visit(child, here, scope);
    for (const shadow of node.shadowRoots ?? []) visit(shadow, here, shadow);
    if (node.contentDocument) visit(node.contentDocument, null, node.contentDocument);
  };
  visit(root, null, root);

  const marks = new Map<number, UploadMark>();
  const descendants = (ancestor: Seen) => all.filter((s) => {
    for (let p = s.parent; p; p = p.parent) if (p === ancestor) return true;
    return false;
  });
  for (const input of all.filter(isFileInput)) {
    if (marks.size >= MARK_LIMIT) break;
    const facts = { accept: input.attrs.accept?.slice(0, 200) || undefined, multiple: input.attrs.multiple !== undefined };
    const id = input.attrs.id;
    const labels = all.filter((s) => tag(s.node) === "label" && s.scope === input.scope &&
      ((id && s.attrs.for === id) || descendants(s).includes(input)));
    const label =
      input.attrs["aria-label"]?.trim() ||
      labels.map((l) => textOf(l.node)).find(Boolean) ||
      input.attrs.title?.trim() ||
      input.attrs.name?.trim() ||
      "";
    marks.set(input.node.backendNodeId, { kind: "field", label: label.slice(0, LABEL_LIMIT), ...facts });
    for (const l of labels) {
      marks.set(l.node.backendNodeId, { kind: "button", label: textOf(l.node) || label, field: input.node.backendNodeId, ...facts });
    }
    // The page's own button beside it, within three levels up.
    let ancestor = input.parent;
    // Not past a container that holds another file field, or the page's body.
    for (let level = 0; ancestor && level < 3; level++, ancestor = ancestor.parent) {
      if (["body", "html", "form"].includes(tag(ancestor.node))) break;
      const inside = descendants(ancestor);
      if (inside.some((s) => s !== input && isFileInput(s))) break;
      const buttons = inside.filter((s) => s !== input && isButton(s));
      if (buttons.length === 0) continue;
      for (const button of buttons.slice(0, BUTTONS_PER_FIELD)) {
        if (!marks.has(button.node.backendNodeId)) {
          const text = button.attrs["aria-label"]?.trim() || textOf(button.node);
          marks.set(button.node.backendNodeId, { kind: "button", label: text || label, field: input.node.backendNodeId, ...facts });
        }
      }
      break;
    }
  }
  // Buttons named for uploading, which may make their field only when pressed.
  for (const button of all.filter(isButton)) {
    if (marks.size >= MARK_LIMIT) break;
    if (marks.has(button.node.backendNodeId)) continue;
    const text = button.attrs["aria-label"]?.trim() || textOf(button.node) || button.attrs.title?.trim() || "";
    if (UPLOAD_WORDS.test(text)) marks.set(button.node.backendNodeId, { kind: "button", label: text, multiple: false });
  }
  // Zones that say to drop files on them: the innermost element saying so,
  // or one the page gave a drop handler or a dropzone name.
  for (const zone of all) {
    if (marks.size >= MARK_LIMIT) break;
    if (marks.has(zone.node.backendNodeId) || isFileInput(zone)) continue;
    const name = tag(zone.node);
    if (["html", "body", "head", "script", "style", "template", "label", "button", "input"].includes(name)) continue;
    const said = zone.attrs.ondrop !== undefined || zone.attrs.dropzone !== undefined ||
      /(^|[\s_-])drop-?zone([\s_-]|$)/i.test(zone.attrs.class ?? "");
    const own = DROP_WORDS.test(textOf(zone.node, 300)) &&
      !(zone.node.children ?? []).some((child) => child.nodeType === 1 && DROP_WORDS.test(textOf(child, 300)));
    if (said || own) {
      marks.set(zone.node.backendNodeId, { kind: "drop", label: textOf(zone.node) || zone.attrs["aria-label"]?.trim() || "", multiple: true });
    }
  }
  return marks;
}

// ` [upload accept=".png,image/*" multiple]`, for a line of the page as text.
export function uploadNote(mark: UploadMark): string {
  if (mark.kind === "drop") return " [upload drop]";
  const accept = mark.accept ? ` accept="${mark.accept.replace(/"/g, "'")}"` : "";
  return ` [upload${accept}${mark.multiple ? " multiple" : ""}]`;
}
