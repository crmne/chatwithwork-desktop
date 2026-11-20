import assert from "node:assert/strict";
import { test } from "node:test";
import { sentFacts } from "../src/outcome.js";

const base = { action: "click", dispatched: false, moved: false, released: false, written: false };

test("nothing sent stays nothing sent, and a pointer move is kept", () => {
  assert.deepEqual(sentFacts(base), { dispatched: false });
  // The outcome check's fallback when its time runs out uses the same facts,
  // so the move isn't reported as nothing reaching the page.
  assert.deepEqual(sentFacts({ ...base, moved: true }), { dispatched: false, moved: true });
});

test("input that left says whether all of it did", () => {
  assert.deepEqual(sentFacts({ ...base, dispatched: true }), { dispatched: true, complete: false });
  assert.deepEqual(sentFacts({ ...base, dispatched: true, released: true }), { dispatched: true, complete: true });
  assert.deepEqual(sentFacts({ ...base, action: "type", dispatched: true }), { dispatched: true, complete: false });
  assert.deepEqual(sentFacts({ ...base, action: "type", dispatched: true, written: true }), { dispatched: true, complete: true });
  assert.deepEqual(sentFacts({ ...base, action: "select", dispatched: true }), { dispatched: true, complete: true });
});
