import assert from "node:assert/strict";
import { test } from "node:test";
import { candidatePoints, visiblePoint } from "../src/pointer.js";

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

test("every visible fragment's centre is tried before any grid point, so a wrapped link's later lines get a turn", () => {
  const line = (top: number) => [0, top, 120, top, 120, top + 20, 0, top + 20];
  const points = candidatePoints([line(0), line(20), line(40), line(-100)], 1000, 700);
  assert.deepEqual(points.slice(0, 3), [{ x: 60, y: 10 }, { x: 60, y: 30 }, { x: 60, y: 50 }]);
  // The offscreen fragment adds nothing, and every point stays inside a fragment.
  assert.ok(points.every((p) => p.x >= 0 && p.x <= 120 && p.y >= 0 && p.y <= 60));
  assert.ok(points.length > 3 && points.length <= 75);
});
