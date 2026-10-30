import assert from "node:assert/strict";
import { test } from "node:test";
import { visiblePoint } from "../src/pointer.js";

test("click point skips offscreen inline fragments and clips partial content to the viewport", () => {
  assert.deepEqual(visiblePoint([
    [-100, -30, 80, -30, 80, -10, -100, -10],
    [-100, 10, 80, 10, 80, 30, -100, 30],
  ], 100, 100), { x: 40, y: 20 });
  assert.deepEqual(visiblePoint([[-1600, 20, 400, 20, 400, 70, -1600, 70]], 1000, 700), { x: 200, y: 45 });
});

test("click point preserves subpixel coordinates and rejects malformed or zero-area quads", () => {
  assert.deepEqual(visiblePoint([[8, 8, 9, 8, 9, 9, 8, 9]], 100, 100), { x: 8.5, y: 8.5 });
  assert.equal(visiblePoint([[0, 0, 0, 0, 0, 0, 0, 0], [0, 1], [NaN, 0, 5, 0, 5, 5, 0, 5]], 100, 100), undefined);
});
