// A page as text: its accessibility tree, the way a screen reader meets it,
// one line per thing on the page. What can be clicked or typed in carries a
// `[ref=e<id>]` the change tools take. Never raw HTML, and never the value
// of a sensitive field.

export interface AXValue {
  type?: string;
  value?: unknown;
}

export interface AXNode {
  nodeId: string;
  ignored?: boolean;
  role?: AXValue;
  name?: AXValue;
  value?: AXValue;
  description?: AXValue;
  properties?: { name: string; value: AXValue }[];
  childIds?: string[];
  parentId?: string;
  backendDOMNodeId?: number;
}

// Roles that only group other things: their children are shown in their
// place.
const TRANSPARENT = new Set([
  "generic", "none", "presentation", "LineBreak", "InlineTextBox", "Section", "group",
  "LayoutTable", "LayoutTableRow", "LayoutTableCell", "Pre", "Div", "Iframe", "IframePresentational",
  "sectionheader", "sectionfooter",
]);

const TEXT = new Set(["StaticText", "text"]);

// Roles named by their content: their text isn't repeated under them.
const NAMED_BY_CONTENT = new Set([
  "link", "button", "heading", "cell", "columnheader", "rowheader", "gridcell", "option",
  "menuitem", "menuitemcheckbox", "menuitemradio", "tab", "treeitem", "checkbox", "radio",
  "switch", "listitem", "label", "legend", "caption", "term", "definition", "tooltip",
]);

// Things a change tool can act on.
const INTERACTIVE = new Set([
  "link", "button", "textbox", "searchbox", "combobox", "listbox", "checkbox", "radio",
  "switch", "slider", "spinbutton", "menuitem", "menuitemcheckbox", "menuitemradio", "option",
  "tab", "treeitem", "textarea", "PopUpButton", "ComboBoxGrouping", "ComboBoxMenuButton",
  "DisclosureTriangle", "summary",
]);

const SHOWN_PROPERTIES = ["level", "checked", "pressed", "selected", "expanded", "disabled", "required", "readonly"];

const NAME_LIMIT = 300;
const INLINE_TEXT_LIMIT = 400;

export interface SnapshotOptions {
  // Backend node IDs of fields whose value is never shown.
  sensitive?: ReadonlySet<number>;
  // Backend node IDs of elements that are evidently clickable (a click
  // handler, `cursor: pointer`, `tabindex`) though the accessibility tree
  // gives them no control role, like a bare "×" that removes a row. They
  // get a ref, shown as `clickable` when they have no role of their own.
  clickable?: ReadonlySet<number>;
}

const MEANINGFUL = /[\p{L}\p{N}].*[\p{L}\p{N}]/u;
const NEAR_LIMIT = 60;

export function snapshotText(nodes: readonly AXNode[], options: SnapshotOptions = {}): string {
  const byId = new Map(nodes.map((n) => [n.nodeId, n]));
  const root = nodes.find((n) => !n.parentId) ?? nodes[0];
  if (!root) return "";
  const lines: string[] = [];
  const start = isRoot(root) ? root.childIds ?? [] : [root.nodeId];
  for (const id of start) visit(id, 0, false, byId, lines, options, new Set());
  return lines.join("\n");
}

function isRoot(node: AXNode): boolean {
  const role = String(node.role?.value ?? "");
  return role === "RootWebArea" || role === "WebArea";
}

function visit(
  id: string,
  depth: number,
  skipText: boolean,
  byId: Map<string, AXNode>,
  lines: string[],
  options: SnapshotOptions,
  seen: Set<string>,
): void {
  if (seen.has(id)) return;
  seen.add(id);
  const node = byId.get(id);
  if (!node) return;
  const role = String(node.role?.value ?? "");
  const children = node.childIds ?? [];
  if (TEXT.has(role)) {
    if (skipText || node.ignored) return;
    const text = clean(String(node.name?.value ?? ""));
    if (!text) return;
    // Runs of text beside each other are one line.
    const prefix = `${indent(depth)}- text: `;
    const last = lines[lines.length - 1];
    if (last?.startsWith(prefix)) {
      lines[lines.length - 1] = `${last} ${text}`;
    } else {
      lines.push(prefix + text);
    }
    return;
  }
  const backend = node.backendDOMNodeId;
  const clickable = backend !== undefined && options.clickable?.has(backend) === true;
  const bare = node.ignored || TRANSPARENT.has(role) || role === "";
  if (bare && !clickable) {
    for (const child of children) visit(child, depth, skipText, byId, lines, options, seen);
    return;
  }
  if (bare) {
    clickableLine(node, depth, skipText, byId, lines, options, seen);
    return;
  }
  const name = clean(String(node.name?.value ?? ""));
  let line = `${indent(depth)}- ${roleName(role)}`;
  if (name) line += ` "${cut(name, NAME_LIMIT).replace(/"/g, "'")}"`;
  for (const prop of node.properties ?? []) {
    if (!SHOWN_PROPERTIES.includes(prop.name)) continue;
    const value = prop.value?.value;
    if (value === false || value === "false" || value === undefined || value === null) continue;
    line += value === true || value === "true" ? ` [${prop.name}]` : ` [${prop.name}=${String(value)}]`;
  }
  const sensitive = backend !== undefined && options.sensitive?.has(backend) === true;
  const focusable = (node.properties ?? []).some((p) => p.name === "focusable" && p.value?.value === true);
  if (backend !== undefined && (INTERACTIVE.has(role) || focusable || clickable)) {
    line += ` [ref=e${backend}]`;
  }
  if (sensitive) line += " [sensitive]";
  const value = node.value?.value;
  if (sensitive) {
    line += ": (hidden)";
  } else if (value !== undefined && value !== null && value !== "" && String(value) !== name) {
    line += `: ${cut(clean(String(value)), NAME_LIMIT)}`;
  }
  lines.push(line);
  const mark = lines.length;
  const childSkip = skipText || (NAMED_BY_CONTENT.has(role) && name !== "") || sensitive;
  for (const child of children) visit(child, depth + 1, childSkip, byId, lines, options, seen);
  // A node whose only content is text gets it on its own line.
  if (lines.length === mark + 1 && !line.includes(": ")) {
    const only = lines[mark];
    const prefix = `${indent(depth + 1)}- text: `;
    if (only.startsWith(prefix) && only.length - prefix.length <= INLINE_TEXT_LIMIT) {
      lines.splice(mark, 1);
      lines[mark - 1] = `${line}: ${only.slice(prefix.length)}`;
    }
  }
}

// An element with no role of its own that can be clicked: one line, named
// by its text, or, when that is only a glyph or nothing (an icon), also by
// the text just before it, so "×" beside "Row 3" can be told apart.
function clickableLine(
  node: AXNode,
  depth: number,
  skipText: boolean,
  byId: Map<string, AXNode>,
  lines: string[],
  options: SnapshotOptions,
  seen: Set<string>,
): void {
  const before = lines.length;
  lines.push("");
  for (const child of node.childIds ?? []) visit(child, depth + 1, skipText, byId, lines, options, seen);
  let name = clean(String(node.name?.value ?? ""));
  const textPrefix = `${indent(depth + 1)}- text: `;
  let inline = "";
  if (lines.length === before + 2 && lines[before + 1].startsWith(textPrefix)) {
    const only = lines[before + 1].slice(textPrefix.length);
    if (!name) name = only;
    else if (only !== name && only.length <= INLINE_TEXT_LIMIT) inline = `: ${only}`;
    if (only === name || inline) lines.splice(before + 1, 1);
  }
  let line = `${indent(depth)}- clickable`;
  if (name) line += ` "${cut(name, NAME_LIMIT).replace(/"/g, "'")}"`;
  line += ` [ref=e${node.backendDOMNodeId}]`;
  if (!MEANINGFUL.test(name)) {
    const near = textBefore(lines, before);
    if (near) line += ` [near="${cut(near, NEAR_LIMIT).replace(/"/g, "'")}"]`;
  }
  lines[before] = line + inline;
}

// The nearest text shown before line `index`: a quoted name or a line's
// text, whichever comes last.
function textBefore(lines: string[], index: number): string {
  for (let i = index - 1; i >= 0 && i >= index - 12; i--) {
    const line = lines[i];
    const value = /^\s*- [^":\[]*(?: "[^"]*")?(?: \[[^\]]*\])*: (.+)$/.exec(line)?.[1];
    const quoted = /- [^"\[]*"([^"]*)"/.exec(line)?.[1];
    const text = value && !value.startsWith("(hidden)") && !line.includes("[sensitive]") ? value : quoted;
    if (text && MEANINGFUL.test(text)) return text;
  }
  return "";
}

function roleName(role: string): string {
  switch (role) {
    case "image":
    case "img":
      return "img";
    case "paragraph":
    case "Paragraph":
      return "paragraph";
    default:
      return role;
  }
}

function indent(depth: number): string {
  return "  ".repeat(Math.min(depth, 20));
}

function clean(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f​-‍⁠﻿]/g, " ").replace(/\s+/g, " ").trim();
}

function cut(text: string, max: number): string {
  const chars = Array.from(text);
  return chars.length > max ? `${chars.slice(0, max - 1).join("")}…` : text;
}

// One page of `text`, by Unicode scalar values (what Ruby counts), from
// `offset`.
export function page(
  text: string,
  offset: number,
  maxChars: number,
): { text: string; offset: number; next_offset: number | null; total_chars: number } {
  const chars = Array.from(text);
  const start = Math.min(Math.max(0, offset), chars.length);
  const end = Math.min(chars.length, start + Math.max(1, maxChars));
  return {
    text: chars.slice(start, end).join(""),
    offset: start,
    next_offset: end < chars.length ? end : null,
    total_chars: chars.length,
  };
}
