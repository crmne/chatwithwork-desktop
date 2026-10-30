import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, byKey, byText, walk } from "./dom.mjs";

install();
await import("../ui/app.js");
const { render, statusChanged } = await import("../ui/pages/privacy.js");
const flush = () => new Promise(resolve => setImmediate(resolve));

function fixture() {
  const view = { deny: { builtin: [".ssh", ".env*"], extra: ["Clients/Private"], removed: [] } };
  const app = { page: "folders", status: {}, view: () => view, render() {} };
  return { app, view };
}

test("custom edits and removal use the native control request; built-ins have no edit actions", async () => {
  const { app, view } = fixture();
  let tree = render(app, true);
  assert.equal(byKey(tree, "remove-exclusion-.ssh"), null);
  assert.ok(byText(tree, "summary", "Built-in protections"));
  byKey(tree, "edit-exclusion-Clients/Private").fire("click");
  tree = render(app, true);
  const input = byKey(tree, "exclusion-pattern");
  input.value = "Clients/Confidential";
  input.fire("input");
  [...walk(tree)].find(el => el.tagName === "FORM").fire("submit");
  await flush();
  assert.deepEqual(calls.at(-1), { command: "control", args: { request: {cmd: "deny_extra", previous: "Clients/Private", pattern: "Clients/Confidential"} } });
  view.deny = { extra: ["Clients/Private"] };
  byKey(render(app), "remove-exclusion-Clients/Private").fire("click");
  await flush();
  assert.equal(calls.at(-1).args.request.pattern, null);
  assert.equal(calls.at(-1).args.request.previous, "Clients/Private");
});

test("a status refresh preserves a typed draft and its edit target", async () => {
  const { app, view } = fixture();
  byKey(render(app), "edit-exclusion-Clients/Private").fire("click");
  const input = byKey(render(app), "exclusion-pattern");
  input.value = "Clients/Unfinished";
  input.fire("input");
  statusChanged(app);
  await flush();
  assert.equal(view.draft, "Clients/Unfinished");
  assert.equal(view.editing, "Clients/Private");
  assert.equal(byKey(render(app), "exclusion-pattern").getAttribute("value"), "Clients/Unfinished");
});

test("declining native confirmation preserves the existing exclusion and typed edit", async () => {
  const { app, view } = fixture();
  byKey(render(app), "edit-exclusion-Clients/Private").fire("click");
  const input = byKey(render(app), "exclusion-pattern");
  input.value = "Clients/Unfinished";
  input.fire("input");
  const invoke = globalThis.__TAURI_INTERNALS__.invoke;
  globalThis.__TAURI_INTERNALS__.invoke = async () => { throw {code: "declined", message: "No changes made."}; };
  try {
    [...walk(render(app))].find(el => el.tagName === "FORM").fire("submit");
    await flush();
    assert.deepEqual(view.deny.extra, ["Clients/Private"]);
    assert.equal(view.draft, "Clients/Unfinished");
    assert.equal(view.editing, "Clients/Private");
    assert.equal(view.error, null);
    assert.equal(view.busy, false);
  } finally {
    globalThis.__TAURI_INTERNALS__.invoke = invoke;
  }
});
