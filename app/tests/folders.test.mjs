// The shared folders page on Windows, in a stand-in document (dom.mjs):
// the Run commands switch where macOS and Linux have it, the reason where
// this Windows can't sandbox commands, and the drive step Node.js needs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { install, byText, walk, calls } from "./dom.mjs";

install();
await import("../ui/app.js");
const { render, leave } = await import("../ui/pages/folders.js");

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

test("Windows that cannot sandbox commands keeps a disabled switch and explains why", () => {
  const tree = page("windows", { available: false, detail: "commands need Windows 11 24H2 with the August 2026 update or later" });
  const command = [...walk(tree)].find(e => e.attributes?.["aria-label"] === "Run commands in work");
  assert.ok(command);
  assert.equal(command.disabled, true);
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
  const command = [...walk(tree)].find(e => e.attributes?.["aria-label"] === "Run commands in work");
  assert.equal(command.disabled, true);
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

test("an existing command grant remains switchable off when the platform loses support", () => {
  const tree = page("windows", {available:false}, [{...root,allows_commands:true}]);
  const command = [...walk(tree)].find(e => e.attributes?.["aria-label"] === "Run commands in work");
  assert.equal(command.checked, true);
  assert.equal(command.disabled, false);
});

test("a local widening needs one inline Allow and passes its one-use native confirmation", async () => {
  const invoke=globalThis.__TAURI_INTERNALS__.invoke;
  globalThis.__TAURI_INTERNALS__.invoke=async(command,args)=>{
    calls.push({command,args});
    return command==="prepare_control"?{confirmation:"one-use",message:"Let Chat with Work change files in Work?"}:{};
  };
  const views={};const app={page:"folders",context:{},info:{platform:"linux"},status:{roots:[{...root,writable:false}]},view:n=>views[n]||=( {}),render(){}};
  try {
    const control=[...walk(render(app))].find(e=>e.attributes?.["aria-label"]==="Allow changes in work");
    const before=calls.length;control.checked=true;control.fire("change");
    assert.equal(byText(render(app),"button","Allow Changes").disabled,true);
    await new Promise(resolve=>setImmediate(resolve));
    assert.deepEqual(calls.slice(before).map(c=>c.command),["prepare_control"]);
    assert.ok(byText(render(app),"p","Let Chat with Work change files in Work?"));
    byText(render(app),"button","Allow Changes").fire("click");
    await new Promise(resolve=>setImmediate(resolve));
    assert.deepEqual(calls.at(-1),{command:"control",args:{request:{cmd:"roots_writable",root:"work",writable:true},confirmation:"one-use"}});
    assert.equal(byText(render(app),"button","Allow Changes"),null);
  } finally {globalThis.__TAURI_INTERNALS__.invoke=invoke;}
});

test("leaving before inline preparation returns cancels that token without changing the folder", async () => {
  const invoke=globalThis.__TAURI_INTERNALS__.invoke;let resolve;
  globalThis.__TAURI_INTERNALS__.invoke=(command,args)=>{
    calls.push({command,args});return command==="prepare_control"?new Promise(done=>resolve=done):Promise.resolve({});
  };
  const views={};const app={page:"folders",context:{},info:{platform:"linux"},status:{roots:[{...root,writable:false}]},view:n=>views[n]||=({}),render(){}};
  try {
    const before=calls.length;
    const control=[...walk(render(app))].find(e=>e.attributes?.["aria-label"]==="Allow changes in work");
    control.checked=true;control.fire("change");leave(app);resolve({confirmation:"late"});
    await new Promise(done=>setImmediate(done));
    assert.deepEqual(calls.slice(before).map(c=>c.command),["prepare_control","cancel_control"]);
    assert.equal(byText(render(app),"button","Allow Changes"),null);
  } finally {globalThis.__TAURI_INTERNALS__.invoke=invoke;}
});
