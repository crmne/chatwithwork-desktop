import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const build = (...args) => execFileSync(process.execPath, [join(root, "scripts/build.mjs"), ...args]);
const contents = (zip, file) => execFileSync("unzip", ["-p", join(root, zip), file], { encoding: "utf8" });
const names = (zip) => execFileSync("unzip", ["-Z1", join(root, zip)], { encoding: "utf8" }).split("\n");

test("manual ZIP keeps the registered identity and excludes stale output; store ZIP omits the public key", () => {
  build();
  // A key added for an earlier store upload must never leak into a later archive.
  writeFileSync(join(root, "build/key.pem"), "synthetic stale key fixture");
  writeFileSync(join(root, "build/stale.txt"), "old output");
  build("--unpacked");
  const manual = `chatwithwork-browser-${pkg.version}-unpacked.zip`;
  const manifest = JSON.parse(contents(manual, "manifest.json"));
  assert.equal(manifest.version, pkg.version);
  const digest = createHash("sha256").update(Buffer.from(manifest.key, "base64")).digest("hex").slice(0, 32);
  const id = [...digest].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join("");
  assert.equal(id, "ehjfihpogoacjodjnanclodinjnnpmcp");
  assert.ok(names(manual).includes("js/background.js"));
  assert.ok(!names(manual).includes("key.pem"));
  assert.ok(!names(manual).includes("stale.txt"));

  build("--store");
  const store = JSON.parse(contents("chatwithwork-browser.zip", "manifest.json"));
  assert.equal(store.version, pkg.version);
  assert.ok(!("key" in store));
  assert.ok(!names("chatwithwork-browser.zip").includes("key.pem"));
  // Leave build/ usable for development and the Chromium tests.
  build();
});
