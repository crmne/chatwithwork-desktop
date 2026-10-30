import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

const source = readFileSync(new URL("../ui/appearance.js", import.meta.url), "utf8");

test("the embedded theme is applied before a native command can answer", () => {
  for (const theme of ["light", "dark"]) {
    const classes = new Set();
    const root = { classList: { add: value => classes.add(value) }, style: {} };
    runInNewContext(source, {
      window: { CWW_SETTINGS_PANE: { theme } },
      document: { documentElement: root },
    });
    assert.equal(root.style.colorScheme, theme);
    assert.ok(classes.has("embedded"));
  }
});

test("standalone settings retain the system appearance", () => {
  const root = { style: {} };
  runInNewContext(source, { window: {}, document: { documentElement: root } });
  assert.equal(root.style.colorScheme, undefined);
});
