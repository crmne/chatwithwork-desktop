import { test } from "node:test";
import assert from "node:assert/strict";
import { install, calls, walk, byText } from "./dom.mjs";
install();
await import("../ui/app.js");
const { render } = await import("../ui/pages/bot_sharing.js");

test("the bot pane uses only its authenticated bot context for folder and browser grants", async () => {
  const views = {};
  const app = { context:{bot:{id:42,name:"Issue fixer"}}, status:{roots:[{id:"work",label:"Work",writable:true,allows_commands:true,bots:[]}],browser:{enabled:true,bots:[]}}, view:name=>(views[name] ||= {}), render() {} };
  const tree = render(app);
  const switches = [...walk(tree)].filter(node => node.attributes.role === "switch");
  assert.deepEqual(switches.map(node => node.attributes["aria-label"]), ["Share Work with Issue fixer", "Share browser with Issue fixer"]);
  calls.length = 0;
  switches[0].checked = true;
  switches[0].fire("change");
  await Promise.resolve();
  assert.deepEqual(calls[0], {command:"control",args:{request:{cmd:"roots_bot",root:"work",bot:"42",name:"Issue fixer",lent:true,unattended:false}}});
  app.context = {};
  const unscoped = render(app);
  assert.equal([...walk(unscoped)].filter(node => node.attributes.role === "switch").length, 0);
  assert.ok(byText(unscoped, "p", "Open this from a bot"));
});
