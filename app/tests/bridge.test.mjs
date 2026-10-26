// The bridge script (app/src/bridge.js) in a stand-in page: what it asks
// the app for when the page confirms, alerts, or runs on another site.
// Run with `node --test app/tests/`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const ORIGIN = "https://work.example";
const template = readFileSync(new URL("../src/bridge.js", import.meta.url), "utf8")
  .replace("__ORIGIN__", JSON.stringify(ORIGIN))
  .replace("__COMPONENTS__", JSON.stringify(["external", "computer"]))
  .replace("__VERSION__", JSON.stringify("0.0.0"));

// A page at `origin` whose app answers each invoke with `answer(command, args)`.
function page({ origin = ORIGIN, answer = () => true } = {}) {
  const calls = [];
  const listeners = {};
  const on = (target) => (type, listener) => (listeners[`${target}:${type}`] ||= []).push(listener);
  const window = {
    location: { origin },
    navigator: { platform: "MacIntel" },
    __TAURI_INTERNALS__: {
      invoke: async (command, args) => {
        // As JSON, as the app gets it (and out of the page's realm).
        calls.push(JSON.parse(JSON.stringify({ command, args })));
        return answer(command, args);
      },
    },
    addEventListener: on("window"),
  };
  // The dialog plugin's, which these pages may not call.
  window.alert = () => assert.fail("the plugin's alert ran");
  window.confirm = () => assert.fail("the plugin's confirm ran");
  const document = { addEventListener: on("document") };
  vm.runInNewContext(template, { window, document, URL, URLSearchParams, Element: class {}, HTMLFormElement: class {} });
  const fire = (key) => (listeners[key] || []).forEach((listener) => listener({}));
  return { window, calls, fire };
}

// A form like the ones Rails draws, with a hidden _method field.
function form({ method = "post", hidden, attributes = {} } = {}) {
  return {
    getAttribute: (name) => (name === "method" ? method : attributes[name] ?? null),
    querySelector: (selector) => (selector === "input[name=_method]" && hidden ? { value: hidden } : null),
  };
}

const turbo8 = () => ({ config: { forms: { confirm: () => assert.fail("Turbo's own confirm ran") } } });

test("Turbo's confirmation asks the app, once Turbo has loaded", async () => {
  const { window, calls, fire } = page();
  window.Turbo = turbo8();
  fire("document:turbo:load");

  const button = { getAttribute: (name) => ({ "data-bridge-confirm": "Disconnect" })[name] ?? null };
  const answer = await window.Turbo.config.forms.confirm("Disconnect Google Drive?", form({ hidden: "delete" }), button);

  assert.equal(answer, true);
  assert.deepEqual(calls, [{
    command: "page_dialog",
    args: {
      dialog: { kind: "confirm", message: "Disconnect Google Drive?", detail: null, confirm: "Disconnect", destructive: true },
    },
  }]);
});

test("a DELETE without a label says Delete, and anything else OK", async () => {
  const { window, calls, fire } = page();
  window.Turbo = turbo8();
  fire("window:DOMContentLoaded");

  await window.Turbo.config.forms.confirm("Remove it?", form({ hidden: "delete" }), null);
  await window.Turbo.config.forms.confirm("Send it?", form({ attributes: { "data-bridge-description": "It goes to everyone." } }), null);

  assert.deepEqual(calls.map(({ args }) => args.dialog), [
    { kind: "confirm", message: "Remove it?", detail: null, confirm: "Delete", destructive: true },
    { kind: "confirm", message: "Send it?", detail: "It goes to everyone.", confirm: "OK", destructive: false },
  ]);
});

test("Cancel, or an app that can't ask, leaves the submission unstarted", async () => {
  for (const answer of [() => false, () => { throw new Error("busy"); }]) {
    const { window, fire } = page({ answer });
    window.Turbo = turbo8();
    fire("document:turbo:load");
    assert.equal(await window.Turbo.config.forms.confirm("Sure?", form(), null), false);
  }
});

test("an older Turbo gets the same confirmation through setConfirmMethod", async () => {
  const { window, calls, fire } = page();
  let method;
  window.Turbo = { setConfirmMethod: (given) => { method = given; } };
  fire("window:load");
  assert.equal(await method("Sure?", form(), null), true);
  assert.equal(calls[0].args.dialog.kind, "confirm");
});

test("a confirmation the page set after the app's is left alone", () => {
  const { window, fire } = page();
  window.Turbo = turbo8();
  fire("document:turbo:load");
  const own = () => true;
  window.Turbo.config.forms.confirm = own;
  fire("document:turbo:load");
  assert.equal(window.Turbo.config.forms.confirm, own);
});

test("alert() shows the app's message box", () => {
  const { window, calls } = page();
  window.alert("Saved.");
  assert.deepEqual(calls.map(({ command, args }) => [command, args.dialog]), [["page_dialog", { kind: "alert", message: "Saved." }]]);
});

test("confirm() asks the app and answers a promise of the person's answer", async () => {
  for (const [answer, expected] of [[() => true, true], [() => false, false], [() => { throw new Error("busy"); }, false]]) {
    const { window, calls } = page({ answer });
    assert.equal(await window.confirm("Sure?"), expected);
    assert.deepEqual(calls.map(({ args }) => args.dialog), [{ kind: "confirm", message: "Sure?" }]);
  }
});

test("pages of other sites get nothing", () => {
  const { window, calls, fire } = page({ origin: "https://elsewhere.example" });
  window.Turbo = turbo8();
  fire("document:turbo:load");
  assert.equal(window.ChatWithWorkDesktop, undefined);
  assert.throws(() => window.alert("hi"), /the plugin's alert ran/);
  assert.equal(calls.length, 0);
});
