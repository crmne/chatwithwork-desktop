// Build the extension into build/: compile the TypeScript with tsc and copy
// the static files. `--store` also writes chatwithwork-browser.zip for
// the Chrome Web Store, without the manifest's development `key` (the store
// keeps the ID from the key.pem uploaded with the first version). `--unpacked`
// packages the public key too, so manual installs keep the native host's ID.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "build");
const tsc = join(root, "node_modules", ".bin", "tsc");
const store = process.argv.includes("--store");
const unpacked = process.argv.includes("--unpacked");
if (store && unpacked) throw new Error("Choose either the store or unpacked package");
const manifest = JSON.parse(readFileSync(join(root, "static", "manifest.json"), "utf8"));
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
if (manifest.version !== pkg.version) throw new Error("Manifest and package versions must match");

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
execFileSync(tsc, ["-p", root], { stdio: "inherit" });
cpSync(join(root, "static"), out, { recursive: true });

// The unit tests, compiled apart from the extension.
if (existsSync(join(root, "tests", "tsconfig.json"))) {
  execFileSync(tsc, ["-p", join(root, "tests")], { stdio: "inherit" });
}

if (store || unpacked) {
  if (store) {
    delete manifest.key;
    writeFileSync(join(out, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  }
  const zip = join(root, store ? "chatwithwork-browser.zip" : `chatwithwork-browser-${manifest.version}-unpacked.zip`);
  rmSync(zip, { force: true });
  execFileSync("zip", ["-qr", "-X", zip, "."], { cwd: out, stdio: "inherit" });
  console.log(`wrote ${zip}`);
}
