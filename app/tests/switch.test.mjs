import { beforeEach, test } from "node:test";
import assert from "node:assert/strict";
import { install } from "./dom.mjs";
install();
const { forgetSaving, switchInput } = await import("../ui/app.js");
beforeEach(forgetSaving);
const flush = () => new Promise(resolve => setImmediate(resolve));
const turn = async (control, on) => { control.checked = on; control.fire("change"); await flush(); };

test("a switch moves at once and waits with a spinner until the change arrives", async () => {
  let asked = null;
  const draw = (on) => switchInput(on, async (want) => { asked = want; }, { label: "One", key: "one" });
  await turn(draw(false), true);
  assert.equal(asked, true);
  const waiting = draw(false);
  assert.equal(waiting.checked, true);
  assert.equal(waiting.disabled, true);
  assert.match(waiting.className || waiting.attributes.class, /switch--saving/);
  assert.equal(waiting.attributes["aria-busy"], "true");

  const arrived = draw(true);
  assert.equal(arrived.checked, true);
  assert.equal(arrived.disabled, false);
  assert.doesNotMatch(arrived.className || arrived.attributes.class, /switch--saving/);
  assert.equal(draw(false).checked, false, "nothing is left waiting");
});

test("a switch goes back when the change never arrives", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const draw = (on) => switchInput(on, async () => {}, { label: "Two", key: "two" });
  await turn(draw(false), true);
  assert.equal(draw(false).checked, true);
  t.mock.timers.tick(3000);
  assert.equal(draw(false).checked, false);
  assert.equal(draw(false).disabled, false);
});

test("a change that fails also goes back", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const draw = (on) => switchInput(on, () => Promise.reject(new Error("no")), { label: "Three", key: "three" });
  await turn(draw(false), true);
  t.mock.timers.tick(3000);
  assert.equal(draw(false).checked, false);
});

test("two switches wait apart", async () => {
  const one = (on) => switchInput(on, async () => {}, { label: "Four" });
  const other = (on) => switchInput(on, async () => {}, { label: "Five" });
  await turn(one(false), true);
  assert.equal(other(false).checked, false);
  assert.equal(other(false).disabled, false);
});

test("a switch that asks first has nothing to wait for", async () => {
  const draw = (on) => switchInput(on, () => {}, { label: "Six" });
  await turn(draw(false), true);
  assert.equal(draw(false).checked, false);
  assert.equal(draw(false).disabled, false);
});

test("a change that never answers goes back too", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const draw = (on) => switchInput(on, () => new Promise(() => {}), { label: "Seven" });
  await turn(draw(false), true);
  t.mock.timers.tick(19999);
  assert.equal(draw(false).disabled, true);
  t.mock.timers.tick(1);
  assert.equal(draw(false).checked, false);
});

test("a switch keeps waiting while its change runs, even if the page already shows it", async () => {
  let finish;
  const draw = (on) => switchInput(on, () => new Promise((resolve) => { finish = resolve; }), { label: "Eight" });
  await turn(draw(false), true);
  assert.equal(draw(true).disabled, true);
  finish();
  await flush();
  assert.equal(draw(true).disabled, false);
});
