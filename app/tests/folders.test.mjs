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

function page(platform, commands, roots = [root], context = {}) {
  const views = {};
  const app = { context, info: { platform, home: "C:\\Users\\a" }, status: { roots, commands }, view: (n) => (views[n] ||= {}), render() {} };
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


test("read-only folders still explain how to enable commands, and private exclusions live below folders", () => {
  const tree = page("linux", null, [{...root, writable:false, bots:[{id:"42",name:"Issue fixer"}]}]);
  assert.ok(byText(tree, "h3", "Run commands"));
  assert.ok(byText(tree, "p", "Allow changes"));
  assert.ok(byText(tree, "h2", "Excluded files and folders"));
  assert.ok(tree.textContent.includes("Issue fixer"));
  assert.ok(!tree.textContent.includes("Lend"));
  assert.ok(!switches(tree).some(name => /bot|Issue fixer/.test(name)));
  const footer = [...walk(tree)].find(node => node.className === "group__footer");
  assert.ok(byText(footer, "button", "Add folder"));
});


test("only an authorized organization administrator gets the policy link", () => {
  const member = page("linux", null, [root], {account:"1000001", commands_allowed:false, can_manage_policy:false});
  assert.ok(member.textContent.includes("Ask an organization owner or admin"));
  assert.equal(byText(member,"button","Organization policy"), null);
  const admin = page("linux", null, [root], {account:"1000001", commands_allowed:false, can_manage_policy:true});
  assert.ok(byText(admin,"button","Organization policy"));
});
