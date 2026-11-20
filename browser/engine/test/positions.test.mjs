// Where the engine may press when a target's centre is covered: positions the
// extension found by its own hit test, only for the target and its labels,
// and only as plain numbers inside the element.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validate } from '../engine.mjs';

const operation = (positions) => ({
  action: 'click', backendNodeId: 41, labelBackendNodeIds: [42], session: 's', params: { ref: 'e41' },
  ...(positions === undefined ? {} : { positions }),
});

test('a position for the target or its label is accepted', () => {
  assert.doesNotThrow(() => validate(operation()));
  assert.doesNotThrow(() => validate(operation({})));
  assert.doesNotThrow(() => validate(operation({ 41: { x: 12.5, y: 4 }, 42: { x: 0, y: 0 } })));
});

test('a position for anything else, or not a plain point inside the element, is refused', () => {
  for (const positions of [
    { 43: { x: 1, y: 1 } },
    { 41: { x: -1, y: 1 } },
    { 41: { x: 1, y: 20_000 } },
    { 41: { x: Number.NaN, y: 1 } },
    { 41: { x: 1, y: 1, force: true } },
    { 41: '1,1' },
    [{ x: 1, y: 1 }],
    null,
  ]) assert.throws(() => validate(operation(positions)), /invalid_operation/, JSON.stringify(positions));
});
