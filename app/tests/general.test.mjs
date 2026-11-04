// General's update notice for a copy a package manager updates, in a
// stand-in document (dom.mjs). Run with `node --test app/tests/`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, byKey, byText, walk } from "./dom.mjs";

install();
await import("../ui/app.js");
const { updatesSection } = await import("../ui/pages/general.js");

const AUR = {
  phase: "unsupported",
  version: "0.11.1",
  release: { version: "0.12.0", url: "https://github.com/crmne/chatwithwork-desktop/releases/tag/v0.12.0" },
  advice: "Installed from the AUR. Update it with your AUR helper.",
  command: "yay -Syu chatwithwork-desktop-bin",
  download: null,
  dismissed: false,
  releases: "https://github.com/crmne/chatwithwork-desktop/releases/latest",
};

function settings(update) {
  const views = { general: { update } };
  const app = {
    tree: null,
    view: (name) => (views[name] ||= {}),
    render() { app.tree = updatesSection(app); },
  };
  app.render();
  return app;
}

const text = (root) => [...walk(root)].map((e) => (typeof e === "string" ? e : "")).join("") || root.textContent;

test("a package manager's release shows the version, how to update and the command", () => {
  const app = settings(AUR);
  const notice = byKey(app.tree, "update-notice");
  assert.ok(notice);
  assert.ok(notice.textContent.includes("Version 0.12.0 is available"));
  assert.ok(notice.textContent.includes("You have 0.11.1. Installed from the AUR."));
  assert.ok(byText(notice, "code", "yay -Syu chatwithwork-desktop-bin"));
  assert.equal(byKey(notice, "update-download"), null);
  assert.ok(!text(app.tree).includes("—"));
  // Never Restart to Update, and nothing that runs the command.
  assert.equal(byText(app.tree, "button", "Restart to Update"), null);
});

test("Copy puts the command on the clipboard, and nothing runs it", async () => {
  let copied = null;
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async (t) => { copied = t; } } } });
  const app = settings(AUR);
  calls.length = 0;
  byKey(app.tree, "update-copy").fire("click");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(copied, "yay -Syu chatwithwork-desktop-bin");
  assert.deepEqual(calls, []);
  assert.ok(byKey(app.tree, "update-copy").textContent.includes("Copied"));
});

test("Release Notes and Download open the release's pages", () => {
  const app = settings({ ...AUR, command: "sudo apt install ~/Downloads/chatwithwork-desktop_0.12.0_amd64.deb",
    download: "https://github.com/crmne/chatwithwork-desktop/releases/download/v0.12.0/chatwithwork-desktop_0.12.0_amd64.deb" });
  calls.length = 0;
  byKey(app.tree, "update-download").fire("click");
  byKey(app.tree, "update-notes").fire("click");
  assert.deepEqual(calls.map((c) => c.args.url), [
    "https://github.com/crmne/chatwithwork-desktop/releases/download/v0.12.0/chatwithwork-desktop_0.12.0_amd64.deb",
    "https://github.com/crmne/chatwithwork-desktop/releases/tag/v0.12.0",
  ]);
});

test("dismissing asks the app, and a dismissed release is one quiet row", async () => {
  const app = settings(AUR);
  calls.length = 0;
  byKey(app.tree, "update-dismiss").fire("click");
  assert.equal(calls[0].command, "update_dismiss");
  const quiet = settings({ ...AUR, dismissed: true });
  assert.equal(byKey(quiet.tree, "update-notice"), null);
  assert.ok(quiet.tree.textContent.includes("Version 0.12.0 is out."));
  assert.ok(byText(quiet.tree, "button", "Release Notes"));
});

test("a downloaded update still offers Restart to Update", () => {
  const app = settings({ phase: "ready", version: "0.11.1", release: { version: "0.12.0" } });
  assert.equal(byKey(app.tree, "update-notice"), null);
  assert.ok(byText(app.tree, "button", "Restart to Update"));
});
