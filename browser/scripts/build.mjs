// Build the extension into build/: compile the TypeScript with tsc and copy
// the static files. `--store` also writes build/chatwithwork-browser.zip for
// the Chrome Web Store, without the manifest's development `key` (the store
// keeps the ID from the key.pem uploaded with the first version).
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "build");
const tsc = join(root, "node_modules", ".bin", "tsc");

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
execFileSync(tsc, ["-p", root], { stdio: "inherit" });
cpSync(join(root, "static"), out, { recursive: true });

// The unit tests, compiled apart from the extension.
if (existsSync(join(root, "tests", "tsconfig.json"))) {
  execFileSync(tsc, ["-p", join(root, "tests")], { stdio: "inherit" });
}

if (process.argv.includes("--store")) {
  const manifest = JSON.parse(readFileSync(join(out, "manifest.json"), "utf8"));
  delete manifest.key;
  writeFileSync(join(out, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  const zip = join(root, "chatwithwork-browser.zip");
  rmSync(zip, { force: true });
  execFileSync("zip", ["-qr", "-X", zip, "."], { cwd: out, stdio: "inherit" });
  console.log(`wrote ${zip}`);
}
