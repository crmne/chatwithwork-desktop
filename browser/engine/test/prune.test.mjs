// What staging leaves out of the npm tree (scripts/package.py, PRUNED and
// KEPT): declarations, TypeScript sources, source maps and Markdown go;
// JavaScript, package.json files and every licence or notice stay.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const script = join(dirname(fileURLToPath(import.meta.url)), '..', 'scripts', 'package.py');
const python = process.platform === 'win32' ? 'python' : 'python3';

const pruned = [
  'a/index.d.ts', 'a/index.d.mts', 'a/index.d.cts', 'a/lib/source.ts', 'a/lib/source.mts',
  'a/index.js.map', 'a/README.md', 'a/CHANGELOG.markdown', 'a/docs/guide.MD',
];
const kept = [
  'a/index.js', 'a/index.mjs', 'a/index.cjs', 'a/package.json', 'a/data.json',
  'a/LICENSE', 'a/LICENSE.md', 'a/license.md', 'a/LICENCE.txt', 'a/NOTICE.md', 'a/COPYING.md',
  'a/AUTHORS.md', 'a/ThirdPartyNotices.md', 'a/third-party-notices.md', 'a/PATENTS.md', 'a/COPYRIGHT.md',
];

test('staging leaves out what nothing loads and keeps code, manifests and licences', () => {
  const root = mkdtempSync(join(tmpdir(), 'engine-prune-'));
  try {
    for (const file of [...pruned, ...kept]) {
      const path = join(root, 'node_modules', file);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, 'x');
    }
    // The engine's own files beside node_modules are never touched.
    writeFileSync(join(root, 'README.md'), 'x');
    const removed = execFileSync(python, [script, 'prune', '--directory', root], { encoding: 'utf8' }).trim();
    assert.equal(Number(removed), pruned.length);
    for (const file of pruned) assert.equal(existsSync(join(root, 'node_modules', file)), false, `${file} should go`);
    for (const file of kept) assert.equal(existsSync(join(root, 'node_modules', file)), true, `${file} should stay`);
    assert.equal(existsSync(join(root, 'README.md')), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
