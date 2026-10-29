// The shared folders page on Windows, in a stand-in document (dom.mjs):
// the Run commands switch where macOS and Linux have it, the reason where
// this Windows can't sandbox commands, and the drive step Node.js needs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { install, byText, walk } from "./dom.mjs";

install();
await import("../ui/app.js");
const { render } = await import("../ui/pages/folders.js");

const root = { id: "work", label: "work", local_path: "C:\\Users\\a\\work", writable: true, allows_commands: false, network: false, available: true, index: { state: "ready" }, bots: [] };

function page(platform, commands, roots = [root]) {
  const views = {};
  const app = { info: { platform, home: "C:\\Users\\a" }, status: { roots, commands }, view: (n) => (views[n] ||= {}), render() {} };
  return render(app);
}

const switches = (tree) => [...walk(tree)].filter((e) => e.attributes?.role === "switch").map((e) => e.attributes["aria-label"]);

test("Windows that can sandbox commands gets the Run commands switch", () => {
  const tree = page("windows", { available: true, drives: [] });
  assert.ok(switches(tree).includes("Run commands in work"), switches(tree).join(", "));
});

test("Windows that can't says why where the switch would be", () => {
  const tree = page("windows", { available: false, detail: "commands need Windows 11 24H2 with the August 2026 update or later" });
  assert.ok(!switches(tree).includes("Run commands in work"));
  assert.ok(byText(tree, "p", "Commands need Windows 11 24H2 with the August 2026 update"));
});

test("a drive Node.js can't see is offered to an administrator", () => {
  const tree = page("windows", { available: true, drives: [{ root: "C:\\", prepared: false }] }, [{ ...root, allows_commands: true }]);
  assert.ok(byText(tree, "h3", "Node.js and cmd in commands"));
  assert.ok(byText(tree, "p", "Windows asks for an administrator"));
});
