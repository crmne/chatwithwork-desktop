import assert from "node:assert/strict";
import { test } from "node:test";
import { FAIL_FAST_MS, Responsiveness } from "../src/responsive.js";

test("a tab is used normally until a command to it stalls", () => {
  const tabs = new Responsiveness(() => 0);
  assert.equal(tabs.verdict(1), "send");
});

test("right after a stall, calls to that tab fail at once and other tabs are untouched (#218)", () => {
  let now = 1_000;
  const tabs = new Responsiveness(() => now);
  tabs.stalled(1);
  assert.equal(tabs.verdict(1), "fail");
  assert.equal(tabs.verdict(2), "send");
  now += FAIL_FAST_MS - 1;
  assert.equal(tabs.verdict(1), "fail");
});

test("later, one short check decides, and an answer clears the tab", () => {
  let now = 1_000;
  const tabs = new Responsiveness(() => now);
  tabs.stalled(1);
  now += FAIL_FAST_MS;
  assert.equal(tabs.verdict(1), "probe");
  tabs.stalled(1);
  assert.equal(tabs.verdict(1), "fail");
  now += FAIL_FAST_MS;
  tabs.answered(1);
  assert.equal(tabs.verdict(1), "send");
});

test("a closed tab is forgotten", () => {
  const tabs = new Responsiveness(() => 0);
  tabs.stalled(3);
  tabs.forget(3);
  assert.equal(tabs.verdict(3), "send");
});
