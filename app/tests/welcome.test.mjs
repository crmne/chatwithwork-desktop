import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, byKey, byText } from "./dom.mjs";

install();
await import("../ui/app.js");
const { render } = await import("../ui/pages/welcome.js");

function settings(status = null) {
  const views = { welcome: { autostart: true }, general: { autostart: true } };
  const app = {
    status,
    info: { onboarded: false, agent_installed: true, tray_place: "system tray" },
    known: true,
    view: (name) => (views[name] ||= {}),
    render() { app.tree = render(app); },
    go(page) { app.destination = page; },
  };
  app.render();
  calls.length = 0;
  globalThis.__TAURI_INTERNALS__.invoke = async (command, args) => {
    calls.push({ command, args });
    return {};
  };
  return app;
}

const settled = () => new Promise((resolve) => setImmediate(resolve));

test("chat opens without pairing, a running agent, or shared folders", async () => {
  const app = settings();
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.deepEqual(calls.map(({command}) => command), ["show_main", "set_onboarded"]);
  assert.equal(app.info.onboarded, true);
  assert.equal(app.destination, "folders");
});

test("a paired computer needs no shared folder to finish welcome", async () => {
  const app = settings({ server: "https://chatwithwork.com", roots: [] });
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.equal(calls[0].command, "show_main");
  assert.equal(app.info.onboarded, true);
});

test("an opening failure stays visible and can be retried", async () => {
  const app = settings();
  globalThis.__TAURI_INTERNALS__.invoke = async (command, args) => {
    calls.push({ command, args });
    if (calls.length === 1) throw { message: "The chat window could not open. Try again." };
    return {};
  };
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.equal(app.info.onboarded, false);
  assert.equal(app.destination, undefined);
  assert.ok(byText(app.tree, "div", "The chat window could not open. Try again."));
  assert.equal(byKey(app.tree, "welcome-open").disabled, false);
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.equal(app.info.onboarded, true);
});

test("repeated clicks cannot start overlapping completion requests", async () => {
  const app = settings();
  let opened;
  globalThis.__TAURI_INTERNALS__.invoke = (command, args) => {
    calls.push({ command, args });
    return command === "show_main" ? new Promise((resolve) => { opened = resolve; }) : Promise.resolve({});
  };
  const button = byKey(app.tree, "welcome-open");
  button.fire("click");
  button.fire("click");
  assert.equal(calls.filter(({command}) => command === "show_main").length, 1);
  assert.equal(byKey(app.tree, "welcome-open").disabled, true);
  opened();
  await settled();
  assert.equal(app.info.onboarded, true);
});

test("a failed start-at-login change is reported before completion", async () => {
  const app = settings();
  app.view("welcome").autostart = false;
  globalThis.__TAURI_INTERNALS__.invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === "set_autostart") throw { message: "Could not change start at login." };
    if (command === "autostart") return { enabled: true };
    return {};
  };
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.equal(app.info.onboarded, false);
  assert.ok(byText(app.tree, "div", "Could not change start at login."));
  assert.equal(calls.some(({command}) => command === "show_main"), false);
});

test("the start-at-login choice is saved even before its current value arrives", async () => {
  const app = settings();
  delete app.view("general").autostart;
  byKey(app.tree, "welcome-open").fire("click");
  await settled();
  assert.deepEqual(calls.find(({command}) => command === "set_autostart")?.args, { enabled: true });
  assert.equal(app.info.onboarded, true);
});
