import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

// Acting where a page gives no usable element: by a position on a screenshot
// the model was just given, or by keyboard. The test reads where things are
// from the page itself and turns them into the screenshot's pixels, as a
// model reads them off the picture.
const STATE = `<pre id="out"></pre><script>function said(t){ out.textContent += t + "\\n"; }</script>`;
const PAGES: Record<string, string> = {
  // A "Choose a starting theme" dialog: cards with no role, no tabindex and
  // no handler of their own (the click is delegated to the grid).
  "/themes": `<!doctype html><title>Themes</title><style>body{margin:0;font:16px sans-serif}
    #grid{display:grid;grid-template-columns:repeat(3,180px);gap:20px;padding:40px} .card{height:120px;border:1px solid #999;display:flex;align-items:center;justify-content:center}</style>
    <div role="dialog" aria-label="Choose a starting theme"><div id="grid"><div class="card">Classic</div><div class="card">Ocean</div><div class="card">Forest</div></div></div>${STATE}
    <script>grid.addEventListener("click", (e) => { const c = e.target.closest(".card"); if (c) said("chose " + c.textContent); });</script>`,
  "/keyboard": `<!doctype html><title>Keys</title><div role="dialog" aria-label="Choose">
    <div class="card" tabindex="0">Classic</div><div class="card" tabindex="0">Ocean</div></div>
    <form onsubmit="event.preventDefault(); said('submitted')"><label>Note <input id="note"></label></form>
    <label>Password <input type="password" id="pw"></label>${STATE}
    <script>document.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target.classList.contains("card")) said("chose " + e.target.textContent); });</script>`,
  "/canvas": `<!doctype html><title>Canvas</title><canvas id="c" width="300" height="120" style="margin:40px"></canvas>${STATE}<script>
    const g = c.getContext("2d"); g.fillStyle = "#0a7"; g.fillRect(50, 30, 200, 60); g.fillStyle = "#fff"; g.font = "20px sans-serif"; g.fillText("Start", 125, 67);
    c.addEventListener("click", (e) => { if (e.offsetX > 50 && e.offsetX < 250 && e.offsetY > 30 && e.offsetY < 90) said("pressed start"); });</script>`,
  "/slider": `<!doctype html><title>Slider</title><input id="r" type="range" min="0" max="100" value="0" style="width:300px;margin:40px" oninput="said('value ' + this.value)">
    <ul id="list" style="list-style:none;width:200px;margin:40px;padding:0">${["One", "Two", "Three"].map((t) => `<li style="height:40px;border:1px solid #999;user-select:none">${t}</li>`).join("")}</ul>${STATE}
    <script>let held = null; list.addEventListener("mousedown", (e) => { held = e.target.closest("li"); });
      document.addEventListener("mouseup", (e) => { if (!held) return; const over = document.elementsFromPoint(e.clientX, e.clientY).find((el) => el.tagName === "LI" && el !== held);
        if (over) { list.insertBefore(held, over.nextSibling); said("order " + [...list.children].map((li) => li.textContent).join(",")); } held = null; });</script>`,
  "/long": `<!doctype html><title>Long</title><button id="b" style="margin:40px" onclick="said('pressed')">Choose</button><div style="height:3000px"></div>${STATE}`,
  "/form": `<!doctype html><title>Form</title><form onsubmit="event.preventDefault(); said('sent')"><input id="note" value="x"><button id="send" style="margin:40px">Send</button></form>${STATE}`,
  "/swap": `<!doctype html><title>Swap</title><div id="src" style="width:120px;height:60px;margin:40px;background:#ccc">Card</div>${STATE}
    <script>src.addEventListener("mouseover", () => { const pw = document.createElement("input"); pw.type = "password"; pw.style.cssText = "width:120px;height:60px;margin:40px;display:block";
      pw.addEventListener("mousedown", () => said("pressed password")); src.replaceWith(pw); }, { once: true });</script>`,
  "/password": `<!doctype html><title>Sign in</title><label>Password <input type="password" id="pw" style="width:300px;margin:40px"></label>`,
  "/covered": `<!doctype html><title>Covered</title><button id="b" style="margin:40px" onclick="said('pressed under')">Save</button>
    <div id="banner" style="position:fixed;inset:0 0 auto 0;height:200px;background:#eee" onclick="said('banner')">Cookie choices</div>${STATE}`,
};

let setup: Setup;
let server: { close: () => void };
let origin: string;

test.beforeAll(async () => {
  ({ server, origin } = await serve(PAGES));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => { for (const p of setup.context.pages()) if (p.url().startsWith(origin)) await p.close(); });

async function open(path: string): Promise<number> {
  const opened = await setup.daemon.call("open", { url: `${origin}${path}` });
  expect(opened.error).toBeUndefined();
  return opened.result.tab;
}

function page(path: string) {
  return setup.context.pages().find((p) => p.url() === `${origin}${path}`)!;
}

// The center of `selector`, in the pixels of the look `shot` answered.
async function at(path: string, selector: string, shot: any): Promise<{ x: number; y: number }> {
  const box = await page(path).evaluate((s) => { const r = document.querySelector(s)!.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2, w: innerWidth, h: innerHeight }; }, selector);
  return { x: (box.x * shot.width) / box.w, y: (box.y * shot.height) / box.h };
}

async function said(path: string): Promise<string> {
  return page(path).evaluate(() => document.querySelector("#out")?.textContent ?? "");
}

test("a card grid with no references: the second card by position", async () => {
  const tab = await open("/themes");
  const shot = (await setup.daemon.call("look", { tab })).result;
  expect(shot.capture).toBeTruthy();
  const point = await at("/themes", ".card:nth-child(2)", shot);
  const dry = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false, dry_run: true });
  expect(dry.error).toBeUndefined();
  expect(dry.result.target.name).toContain("Ocean");
  expect(dry.result.target.ref).toMatch(/^e\d+$/);
  expect(dry.result.image.data).toBeTruthy();
  expect(await said("/themes")).toBe("");
  const done = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(done.error).toBeUndefined();
  expect(await said("/themes")).toBe("chose Ocean\n");
  expect(done.result.look.capture).not.toBe(shot.capture);
  // The old screenshot can't be used again.
  const stale = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(stale.error?.message).toContain("look again");
});

test("the same dialog by keyboard: Tab to the second card, Enter, and focus reported after each key", async () => {
  const tab = await open("/keyboard");
  const first = await setup.daemon.call("press", { tab, key: "Tab", submits: false });
  expect(first.result.now_focused.name).toContain("Classic");
  expect(first.result.look.capture).toBeTruthy();
  const second = await setup.daemon.call("press", { tab, key: "Tab", submits: false });
  expect(second.result.now_focused.name).toContain("Ocean");
  const back = await setup.daemon.call("press", { tab, key: "Tab", shift: true, submits: false });
  expect(back.result.now_focused.name).toContain("Classic");
  await setup.daemon.call("press", { tab, key: "Tab", submits: false });
  const enter = await setup.daemon.call("press", { tab, key: "Enter" });
  expect(enter.error).toBeUndefined();
  expect(await said("/keyboard")).toBe("chose Ocean\n");
});

test("Enter in a form's field may submit, typing goes into focus, never into a password field", async () => {
  const tab = await open("/keyboard");
  await page("/keyboard").evaluate(() => (document.querySelector("#note") as HTMLInputElement).focus());
  const typed = await setup.daemon.call("type", { tab, text: "hello" });
  expect(typed.error).toBeUndefined();
  expect(await page("/keyboard").evaluate(() => (document.querySelector("#note") as HTMLInputElement).value)).toBe("hello");
  const quiet = await setup.daemon.call("press", { tab, key: "Enter", submits: false });
  expect(quiet.error?.code).toBe("would_submit");
  expect(await said("/keyboard")).toBe("");
  await page("/keyboard").evaluate(() => (document.querySelector("#pw") as HTMLInputElement).focus());
  expect((await setup.daemon.call("type", { tab, text: "secret" })).error?.code).toBe("sensitive_field");
  expect((await setup.daemon.call("press", { tab, key: "Backspace", submits: false })).error?.code).toBe("sensitive_field");
  expect((await setup.daemon.call("press", { tab, key: "Tab", submits: false })).error).toBeUndefined();
});

test("a canvas button can be pressed, but only as a click that may send something", async () => {
  const tab = await open("/canvas");
  const read = await setup.daemon.call("read", { tab });
  expect(read.result.text).toContain("no element references");
  const shot = (await setup.daemon.call("look", { tab })).result;
  const box = await page("/canvas").evaluate(() => { const r = document.querySelector("canvas")!.getBoundingClientRect(); return { x: r.x + 150, y: r.y + 60, w: innerWidth, h: innerHeight }; });
  const point = { x: (box.x * shot.width) / box.w, y: (box.y * shot.height) / box.h };
  const quiet = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(quiet.error?.code).toBe("would_submit");
  expect(quiet.error?.message).toContain("drawn by the page");
  const done = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point });
  expect(done.error).toBeUndefined();
  expect(await said("/canvas")).toBe("pressed start\n");
});

test("drag a slider, and a sortable item to another place", async () => {
  const tab = await open("/slider");
  let shot = (await setup.daemon.call("look", { tab })).result;
  const left = await page("/slider").evaluate(() => { const r = document.querySelector("#r")!.getBoundingClientRect(); return { x: r.x + 4, y: r.y + r.height / 2, to: r.x + r.width * 0.75, w: innerWidth, h: innerHeight }; });
  const scale = (v: number, of: number, by: number) => (v * by) / of;
  const slid = await setup.daemon.call("drag", { tab, capture: shot.capture, x: scale(left.x, left.w, shot.width), y: scale(left.y, left.h, shot.height),
    to_x: scale(left.to, left.w, shot.width), to_y: scale(left.y, left.h, shot.height), submits: false });
  expect(slid.error).toBeUndefined();
  const value = Number(await page("/slider").evaluate(() => (document.querySelector("#r") as HTMLInputElement).value));
  expect(value).toBeGreaterThan(60);
  expect(value).toBeLessThan(90);
  shot = slid.result.look;
  const from = await at("/slider", "li:nth-child(1)", shot);
  const to = await at("/slider", "li:nth-child(3)", shot);
  const moved = await setup.daemon.call("drag", { tab, capture: shot.capture, ...from, to_x: to.x, to_y: to.y, submits: false });
  expect(moved.error).toBeUndefined();
  expect(await said("/slider")).toContain("order Two,Three,One");
});

test("an approved click acts only on what the card showed, never on a replacement", async () => {
  const tab = await open("/themes");
  const shot = (await setup.daemon.call("look", { tab })).result;
  const point = await at("/themes", ".card:nth-child(2)", shot);
  const checked = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false, dry_run: true });
  expect(checked.result.target.name).toContain("Ocean");
  await page("/themes").evaluate(() => { const old = document.querySelectorAll(".card")[1]; old.replaceWith(old.cloneNode(true)); });
  const done = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(done.error?.code).toBe("page_error");
  expect(done.error?.message).toContain("since the person saw it");
  expect(await said("/themes")).toBe("");
});

test("a drag that starts on a send button may send, and Delete with nothing focused may delete", async () => {
  const tab = await open("/form");
  const shot = (await setup.daemon.call("look", { tab })).result;
  const send = await at("/form", "#send", shot);
  const dragged = await setup.daemon.call("drag", { tab, capture: shot.capture, ...send, to_x: send.x, to_y: send.y, submits: false });
  expect(dragged.error?.code).toBe("would_submit");
  expect(await said("/form")).toBe("");
  await page("/form").evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  expect((await setup.daemon.call("press", { tab, key: "Delete", submits: false })).error?.code).toBe("would_submit");
  expect((await setup.daemon.call("press", { tab, key: "Escape", submits: false })).error?.code).toBe("would_submit");
  await page("/form").evaluate(() => { const note = document.querySelector("#note") as HTMLInputElement; note.focus(); note.setSelectionRange(1, 1); });
  const deleted = await setup.daemon.call("press", { tab, key: "Backspace", submits: false });
  expect(deleted.error).toBeUndefined();
  expect(deleted.result.look.capture).toBeTruthy();
  expect(await page("/form").evaluate(() => (document.querySelector("#note") as HTMLInputElement).value)).toBe("");
});

test("a drag never presses what hovering put under the point", async () => {
  const tab = await open("/swap");
  const shot = (await setup.daemon.call("look", { tab })).result;
  const from = await at("/swap", "#src", shot);
  const dragged = await setup.daemon.call("drag", { tab, capture: shot.capture, ...from, to_x: from.x + 200, to_y: from.y, submits: false });
  expect(dragged.error?.code).toBe("page_error");
  expect(dragged.error?.message).toContain("nothing was pressed");
  expect(await said("/swap")).toBe("");
});

test("a page that scrolled between the look and the click is refused", async () => {
  const tab = await open("/long");
  const shot = (await setup.daemon.call("look", { tab })).result;
  const point = await at("/long", "#b", shot);
  await page("/long").evaluate(() => window.scrollTo(0, 200));
  const refused = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(refused.error?.message).toContain("look again");
  expect(await said("/long")).toBe("");
});

test("a password field under the point is refused, and a covered point acts on what covers it, by name", async () => {
  let tab = await open("/password");
  let shot = (await setup.daemon.call("look", { tab })).result;
  const field = await at("/password", "#pw", shot);
  expect((await setup.daemon.call("click_at", { tab, capture: shot.capture, ...field, submits: false })).error?.code).toBe("sensitive_field");
  tab = await open("/covered");
  shot = (await setup.daemon.call("look", { tab })).result;
  const button = await at("/covered", "#b", shot);
  const dry = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...button, dry_run: true });
  expect(dry.result.target.name).toContain("Cookie choices");
});

test("at a zoom of 110%, a click lands where the picture showed", async () => {
  const tab = await open("/themes");
  await inWorker(setup.context, async (id: number) => { await chrome.tabs.setZoom(id, 1.1); }, tab);
  await new Promise((r) => setTimeout(r, 500));
  const shot = (await setup.daemon.call("look", { tab })).result;
  const point = await at("/themes", ".card:nth-child(3)", shot);
  const done = await setup.daemon.call("click_at", { tab, capture: shot.capture, ...point, submits: false });
  expect(done.error).toBeUndefined();
  expect(await said("/themes")).toBe("chose Forest\n");
  await inWorker(setup.context, async (id: number) => { await chrome.tabs.setZoom(id, 1); }, tab);
});

test("at a device pixel ratio of 2, a click lands where the picture showed", async () => {
  const dense = await launch([origin], { deviceScaleFactor: 2 });
  try {
    await dense.daemon.attached();
    dense.daemon.setPolicy({ changes: true });
    const opened = await dense.daemon.call("open", { url: `${origin}/themes` });
    expect(opened.error).toBeUndefined();
    const tab = opened.result.tab;
    const looked = await dense.daemon.call("look", { tab });
    expect(looked.error).toBeUndefined();
    const shot = looked.result;
    const view = dense.context.pages().find((p) => p.url() === `${origin}/themes`)!;
    expect(await view.evaluate(() => devicePixelRatio)).toBe(2);
    const box = await view.evaluate(() => { const r = document.querySelector(".card:nth-child(2)")!.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2, w: innerWidth, h: innerHeight }; });
    const done = await dense.daemon.call("click_at", { tab, capture: shot.capture, x: (box.x * shot.width) / box.w, y: (box.y * shot.height) / box.h, submits: false });
    expect(done.error).toBeUndefined();
    expect(await view.evaluate(() => document.querySelector("#out")!.textContent)).toBe("chose Ocean\n");
  } finally {
    await dense.cleanup();
  }
});
