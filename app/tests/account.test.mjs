// The account page's pairing card in a stand-in document (dom.mjs). Run
// with `node --test app/tests/`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, byKey, byText } from "./dom.mjs";

install();
// app.js first: it imports every page, as the settings window loads them.
await import("../ui/app.js");
const { pairingSection } = await import("../ui/pages/account.js");

// A settings window whose redraw draws the pairing card again, as app.js's does.
function settings() {
  const views = {};
  const app = {
    tree: null,
    view: (name) => (views[name] ||= {}),
    render() { app.tree = { children: pairingSection(app, { server: null }) }; },
  };
  app.render();
  return app;
}

test("pairing uses the whole address typed, not the one from the last redraw", async () => {
  const app = settings();
  const box = byKey(app.tree, "custom-server");
  box.checked = true;
  box.fire("change");
  const field = byKey(app.tree, "server");
  // The field redraws the page once, when it stops being blank.
  for (const typed of ["s", "staging.chatwithwork.com"]) {
    field.value = typed;
    field.fire("input");
  }
  calls.length = 0;
  byText(app.tree, "button", "Pair with Chat with Work").fire("click");
  await Promise.resolve();
  assert.deepEqual(calls.find((c) => c.command === "pair")?.args, { server: "staging.chatwithwork.com" });
});
