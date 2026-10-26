import assert from "node:assert/strict";
import { test } from "node:test";
import { page, snapshotText, type AXNode } from "../src/snapshot.js";

const nodes: AXNode[] = [
  { nodeId: "1", role: { value: "RootWebArea" }, name: { value: "Inbox" }, childIds: ["2", "3", "6", "8", "10"] },
  { nodeId: "2", parentId: "1", role: { value: "heading" }, name: { value: "Inbox" },
    properties: [{ name: "level", value: { value: 1 } }], childIds: ["2t"], backendDOMNodeId: 20 },
  { nodeId: "2t", parentId: "2", role: { value: "StaticText" }, name: { value: "Inbox" } },
  { nodeId: "3", parentId: "1", role: { value: "generic" }, childIds: ["4"] },
  { nodeId: "4", parentId: "3", role: { value: "link" }, name: { value: "Compose" }, childIds: ["5"],
    backendDOMNodeId: 41, properties: [{ name: "focusable", value: { value: true } }] },
  { nodeId: "5", parentId: "4", role: { value: "StaticText" }, name: { value: "Compose" } },
  { nodeId: "6", parentId: "1", role: { value: "textbox" }, name: { value: "Password" }, value: { value: "hunter2" },
    backendDOMNodeId: 60 },
  { nodeId: "8", parentId: "1", role: { value: "paragraph" }, childIds: ["9", "9b"] },
  { nodeId: "9", parentId: "8", role: { value: "StaticText" }, name: { value: "Three  new\nmessages" } },
  { nodeId: "9b", parentId: "8", role: { value: "StaticText" }, name: { value: "today." } },
  { nodeId: "10", parentId: "1", ignored: true, role: { value: "generic" }, childIds: ["11"] },
  { nodeId: "11", parentId: "10", role: { value: "textbox" }, name: { value: "Search" }, value: { value: "budget" },
    backendDOMNodeId: 110 },
];

test("reads a page as accessible text with refs", () => {
  const text = snapshotText(nodes, { sensitive: new Set([60]) });
  assert.equal(
    text,
    [
      '- heading "Inbox" [level=1]',
      '- link "Compose" [ref=e41]',
      '- textbox "Password" [ref=e60] [sensitive]: (hidden)',
      "- paragraph: Three new messages today.",
      '- textbox "Search" [ref=e110]: budget',
    ].join("\n"),
  );
  assert.ok(!text.includes("hunter2"));
});

test("pages by Unicode scalar values", () => {
  const result = page("a😀bcd", 1, 2);
  assert.deepEqual(result, { text: "😀b", offset: 1, next_offset: 3, total_chars: 5 });
  assert.equal(page("abc", 0, 10).next_offset, null);
});
