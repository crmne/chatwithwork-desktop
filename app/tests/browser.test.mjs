import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, byKey, byText, walk } from "./dom.mjs";
install();
await import("../ui/app.js");
const { render } = await import("../ui/pages/browser.js");
const flush = () => new Promise(resolve => setImmediate(resolve));
function fixture(overrides = {}) {
  const view = {};
  const browser = { enabled:true, changes:false, stopped:false, sites:["example.com"], blocked_sites:["private.example.com"], all_sites:false, bots:[], ...overrides };
  const app = { status:{browser}, view:()=>view, render(){} };
  return { app, view, browser };
}
const toggle = tree => [...walk(tree)].find(e => e.attributes?.["aria-label"] === "Use your browser");

test("the single browser toggle stops and resumes without losing registration or site lists", async () => {
  const {app,browser} = fixture();
  let control = toggle(render(app));
  assert.equal(control.checked,true);
  control.checked=false;control.fire("change");await flush();
  assert.deepEqual(calls.at(-1),{command:"control",args:{request:{cmd:"browser_stopped",stopped:true}}});
  browser.stopped=true;
  control=toggle(render(app));assert.equal(control.checked,false);
  control.checked=true;control.fire("change");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_stopped",stopped:false});
  assert.deepEqual(browser.sites,["example.com"]);
});

test("extension Stop is shown as off and is never silently resumed", () => {
  const {app} = fixture({extension:{browser:"Chrome",stopped:true}});
  const tree=render(app);const control=toggle(tree);
  assert.equal(control.checked,false);assert.equal(control.disabled,true);
  assert.ok(tree.textContent.includes("Resume in the browser extension"));
  const before=calls.length;control.checked=true;control.fire("change");
  assert.equal(calls.length,before);
});

test("enabling an unregistered browser uses native registration and confirmation", async () => {
  const {app}=fixture({enabled:false});const control=toggle(render(app));
  control.checked=true;control.fire("change");await flush();
  assert.deepEqual(calls.at(-1),{command:"browser_enable",args:{enabled:true}});
});

test("mode changes use the guarded native request and preserve both site lists", async () => {
  const {app,browser}=fixture();byText(render(app),"button","All sites except").fire("click");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_all_sites",all_sites:true});
  assert.deepEqual(browser.sites,["example.com"]);assert.deepEqual(browser.blocked_sites,["private.example.com"]);
  browser.all_sites=true;
  assert.equal(byKey(render(app),"allowedSite"),null);
  assert.ok(byKey(render(app),"blockedSite"));
});

test("blocked-site edits use their own policy and cannot accidentally add an allowed site", async () => {
  const {app,view}=fixture();let tree=render(app);const input=byKey(tree,"blockedSite");
  input.value="bank.example";input.fire("input");
  tree=render(app);byKey(tree,"blockedSite-form").fire("submit");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_blocked_site",site:"bank.example",blocked:true});
  assert.equal(view.blockedSite,"");
  byKey(render(app),"blockedSite-remove-private.example.com").fire("click");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_blocked_site",site:"private.example.com",blocked:false});
});


test("submitting after the last valid character uses the complete domain", async () => {
  const {app}=fixture();let tree=render(app);
  let input=byKey(tree,"allowedSite");input.value="elster.d";input.fire("input");
  tree=render(app);input=byKey(tree,"allowedSite");input.value="elster.de";input.fire("input");
  // The input remained valid, so there was no redraw between the last character and submit.
  byKey(tree,"allowedSite-form").fire("submit");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_site",site:"elster.de",allowed:true});
});

test("advanced policy with an older extension explains why access is unavailable", () => {
  const {app,browser}=fixture({extension:{browser:"Chromium",site_policy_version:1}});
  assert.ok(render(app).textContent.includes("Update the browser extension"));
  browser.extension.site_policy_version=2;
  assert.ok(!render(app).textContent.includes("Update the browser extension"));
});

test("browser approval defaults to asking and can only change when actions are allowed", async () => {
  const {app,browser}=fixture();
  let control=byKey(render(app),"browser-approvals");
  assert.equal(control.checked,true);assert.equal(control.disabled,true);
  browser.changes=true;control=byKey(render(app),"browser-approvals");
  assert.equal(control.disabled,false);
  control.checked=false;control.fire("change");await flush();
  assert.deepEqual(calls.at(-1).args.request,{cmd:"browser_ask_before_actions",ask:false});
  browser.ask_before_actions=false;
  assert.ok(render(app).textContent.includes("including sending, buying and deleting"));
});
