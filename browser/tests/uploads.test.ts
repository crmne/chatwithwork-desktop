import assert from "node:assert/strict";
import { test } from "node:test";
import { accepts, fileOrdinal, UPLOAD_WORDS } from "../src/accept.js";
import { snapshotText, type AXNode } from "../src/snapshot.js";
import { uploadMarks, uploadNote, type DomNode } from "../src/upload-marks.js";

test("accept filters as the browser's picker does", () => {
  assert.ok(accepts(undefined, "shot.png", "image/png"));
  assert.ok(accepts("", "anything.bin", "application/octet-stream"));
  assert.ok(accepts(".png,.jpg", "Shot.PNG", "image/png"));
  assert.ok(accepts("image/*", "shot.webp", "image/webp"));
  assert.ok(accepts("application/zip, .crx", "ext.zip", "application/zip"));
  assert.ok(accepts(".aab", "app-release.aab", "application/octet-stream"));
  assert.equal(accepts(".png", "notes.txt", "text/plain"), false);
  assert.equal(accepts("image/*", "notes.txt", "text/plain"), false);
  assert.equal(accepts(".png", ".png", "image/png"), false);
  assert.equal(accepts("application/pdf", "doc.pdf.exe", "application/octet-stream"), false);
});

test("a refusal names a file by its place and kind, never its name", () => {
  assert.equal(fileOrdinal(0, 1, "salary-review.pdf"), "the file (.pdf)");
  assert.equal(fileOrdinal(1, 3, "salary-review.pdf"), "the 2nd file (.pdf)");
  assert.equal(fileOrdinal(10, 12, "README"), "the 11th file");
  assert.ok(!fileOrdinal(0, 1, "salary-review.pdf").includes("salary"));
});

test("upload words in the five languages", () => {
  for (const name of ["Upload screenshot", "Choose file", "Browse…", "Attach", "Datei auswählen", "Carica", "Téléverser", "Subir archivo"]) {
    assert.ok(UPLOAD_WORDS.test(name), name);
  }
  for (const name of ["Send", "Save", "Publish", "Delete"]) assert.equal(UPLOAD_WORDS.test(name), false, name);
});

let next = 1;
const el = (name: string, attributes: Record<string, string> = {}, children: DomNode[] = [], extra: Partial<DomNode> = {}): DomNode => ({
  backendNodeId: next++, nodeType: 1, nodeName: name.toUpperCase(), localName: name,
  attributes: Object.entries(attributes).flat(), children, ...extra,
});
const text = (value: string): DomNode => ({ backendNodeId: next++, nodeType: 3, nodeName: "#text", nodeValue: value });
const doc = (children: DomNode[]): DomNode => ({ backendNodeId: next++, nodeType: 9, nodeName: "#document", children });

test("file fields, their labels and their buttons are marked, hidden ones included", () => {
  const plain = el("input", { type: "file", id: "logo", accept: ".png" });
  const label = el("label", { for: "logo" }, [text("Logo")]);
  const hidden = el("input", { type: "FILE", hidden: "", multiple: "" });
  const button = el("button", {}, [text("Add screenshots")]);
  const submit = el("button", { type: "submit" }, [text("Save")]);
  const lone = el("div", { role: "button" }, [text("Upload bundle")]);
  const unrelated = el("button", {}, [text("Menu")]);
  const marks = uploadMarks(doc([el("html", {}, [el("body", {}, [
    label, plain,
    el("div", {}, [el("div", {}, [button, hidden]), submit]),
    lone,
    el("nav", {}, [unrelated]),
  ])])]));
  assert.deepEqual(marks.get(plain.backendNodeId), { kind: "field", label: "Logo", accept: ".png", multiple: false });
  assert.deepEqual(marks.get(label.backendNodeId), { kind: "button", label: "Logo", field: plain.backendNodeId, accept: ".png", multiple: false });
  assert.equal(marks.get(hidden.backendNodeId)?.multiple, true);
  assert.equal(marks.get(button.backendNodeId)?.field, hidden.backendNodeId);
  assert.equal(marks.has(submit.backendNodeId), false);
  assert.deepEqual(marks.get(lone.backendNodeId), { kind: "button", label: "Upload bundle", multiple: false });
  assert.equal(marks.has(unrelated.backendNodeId), false);
});

test("fields in a frame from another origin are left alone", () => {
  const theirs = el("input", { type: "file" });
  const ours = el("input", { type: "file" });
  const root = { ...doc([el("html", {}, [el("body", {}, [
    ours,
    el("iframe", {}, [], { contentDocument: { ...doc([theirs]), documentURL: "http://127.0.0.1:9000/field" } }),
  ])])]), documentURL: "http://127.0.0.1:8000/page" };
  const marks = uploadMarks(root);
  assert.ok(marks.has(ours.backendNodeId));
  assert.equal(marks.has(theirs.backendNodeId), false);
});

test("fields in shadow roots and frames are found", () => {
  const inShadow = el("input", { type: "file", "aria-label": "Avatar" });
  const host = el("x-uploader", {}, [], { shadowRoots: [{ backendNodeId: next++, nodeType: 11, nodeName: "#document-fragment", children: [inShadow] }] });
  const inFrame = el("input", { type: "file", name: "attachment" });
  const frame = el("iframe", {}, [], { contentDocument: { ...doc([el("html", {}, [el("body", {}, [inFrame])])]), documentURL: "https://example.com/frame" } });
  const marks = uploadMarks({ ...doc([el("html", {}, [el("body", {}, [host, frame])])]), documentURL: "https://example.com/page" });
  assert.equal(marks.get(inShadow.backendNodeId)?.label, "Avatar");
  assert.equal(marks.get(inFrame.backendNodeId)?.label, "attachment");
});

test("the page as text marks uploads, and lists fields the tree leaves out", () => {
  const nodes: AXNode[] = [
    { nodeId: "1", role: { value: "RootWebArea" }, childIds: ["2", "3"] },
    { nodeId: "2", role: { value: "button" }, name: { value: "Logo" }, backendDOMNodeId: 40, parentId: "1" },
    { nodeId: "3", role: { value: "button" }, name: { value: "Add screenshots" }, backendDOMNodeId: 41, parentId: "1" },
  ];
  const uploads = new Map([
    [40, { kind: "field" as const, label: "Logo", accept: ".png", multiple: false }],
    [41, { kind: "button" as const, label: "Add screenshots", field: 42, multiple: true }],
    [42, { kind: "field" as const, label: "Screenshots", accept: "image/*", multiple: true }],
  ]);
  assert.equal(
    snapshotText(nodes, { uploads }),
    [
      '- button "Logo" [ref=e40] [upload accept=".png"]',
      '- button "Add screenshots" [ref=e41] [upload multiple]',
      '- upload field "Screenshots" [ref=e42] [upload accept="image/*" multiple]',
    ].join("\n"),
  );
  assert.equal(uploadNote({ kind: "field", label: "", accept: 'a"b', multiple: false }), ' [upload accept="a\'b"]');
});
