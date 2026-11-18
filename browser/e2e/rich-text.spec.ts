import { expect, test } from "@playwright/test";
import { engineAction, launch, serve, type Setup } from "./harness.js";

// A small editor that behaves like Trix (Basecamp's editor) where #218 went
// wrong: it keeps its own model, handles every edit in `beforeinput`, gives
// typed-over text the formatting of the selection's start, forgets that
// formatting once everything is deleted, and puts its saved caret back
// whenever it gains focus.
const EDITOR = `<!doctype html><title>Card</title>
<style>#editor{border:1px solid #999;min-height:80px;padding:8px}</style>
<div id="editor" contenteditable="true" role="textbox" aria-label="Notes" aria-multiline="true"></div>
<button id="bold" type="button" title="Bold">Bold</button>
<script>
const el = document.getElementById("editor");
let runs = [], saved = 0, current = false;
const text = () => runs.map(r => r.text).join("");
const boldAt = i => { let at = 0; for (const r of runs) { if (i < at + r.text.length) return r.bold; at += r.text.length; } return runs.length ? runs[runs.length - 1].bold : false; };
function offset(node, off) { const r = document.createRange(); r.selectNodeContents(el); r.setEnd(node, off); return r.toString().length + r.cloneContents().querySelectorAll("br").length; }
function range() { const s = getSelection(); if (!s.rangeCount || !el.contains(s.anchorNode)) return [saved, saved]; const r = s.getRangeAt(0); const a = offset(r.startContainer, r.startOffset), b = offset(r.endContainer, r.endOffset); return [Math.min(a, b), Math.max(a, b)]; }
function splice(start, end, insert, bold) {
  const chars = [];
  for (const r of runs) for (const c of r.text) chars.push({ c, bold: r.bold });
  chars.splice(start, end - start, ...[...insert].map(c => ({ c, bold })));
  runs = [];
  for (const { c, bold } of chars) { const last = runs[runs.length - 1]; if (last && last.bold === bold) last.text += c; else runs.push({ text: c, bold }); }
  if (!chars.length) current = false;
  render(start + insert.length);
}
function render(caret) {
  el.innerHTML = runs.map(r => { const html = r.text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/\\n/g, "<br>"); return r.bold ? "<strong>" + html + "</strong>" : html; }).join("");
  saved = caret; place(caret);
}
function place(caret) {
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT); let at = 0, node;
  while ((node = walker.nextNode())) { if (caret <= at + node.length) { getSelection().collapse(node, caret - at); return; } at += node.length; }
  getSelection().collapse(el, el.childNodes.length);
}
el.addEventListener("focus", () => place(saved));
document.addEventListener("selectionchange", () => { if (el.contains(getSelection().anchorNode)) saved = range()[0]; });
el.addEventListener("beforeinput", e => {
  e.preventDefault();
  const [s, end] = range();
  if (e.inputType.startsWith("delete")) {
    if (s < end) splice(s, end, "", false);
    else if (e.inputType.endsWith("Backward") && s > 0) splice(s - 1, s, "", false);
    else if (s < text().length) splice(s, s + 1, "", false);
    return;
  }
  const data = e.inputType === "insertText" ? e.data : "\\n";
  splice(s, end, data, s < end || s === 0 ? boldAt(s) : (current || boldAt(s - 1)));
});
document.getElementById("bold").addEventListener("mousedown", e => e.preventDefault());
document.getElementById("bold").addEventListener("click", () => { current = !current; });
window.load = html => { runs = html; saved = 0; current = false; render(0); };
window.load([{ text: "Bold heading", bold: true }]);
</script>`;

const HANG = `<!doctype html><title>Stuck page</title>
<button id="stuck" type="button">Bold</button><p id="done">Waiting</p>
<script>stuck.onclick = () => { const end = Date.now() + 45000; while (Date.now() < end) {} done.textContent = "Done"; };</script>`;

let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async () => {
  ({ origin, server } = await serve({ "/": EDITOR, "/hang": HANG, "/other": "<h1>Another page</h1>" }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });

async function open(path: string) {
  const opened = await setup.daemon.call("open", { url: origin + path });
  expect(opened.error).toBeUndefined();
  const tab: number = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === origin + path)!;
  const read = await setup.daemon.call("read", { tab });
  const ref = (name: string) => {
    const value = /\[ref=(e\d+)\]/.exec(read.result.text.split("\n").find((line: string) => line.includes(`"${name}"`)) ?? "")?.[1];
    expect(value, name).toBeTruthy();
    return value!;
  };
  return { tab, page, ref };
}

for (const engine of [false, true]) {
  const run = (method: string, params: Record<string, unknown>) =>
    engine ? engineAction(setup, method, params) : setup.daemon.call(method, params);
  const label = engine ? "the bundled engine" : "the extension's own engine";

  test(`replacing an editor's bold text types plain text in place with ${label} (#218)`, async () => {
    const { tab, page, ref } = await open("/");
    const typed = await run("type", { tab, ref: ref("Notes"), text: "Line one\nLine two" });
    expect(typed.error).toBeUndefined();
    const html = await page.locator("#editor").innerHTML();
    expect(html).not.toContain("Bold heading");
    expect(html).not.toContain("<strong>");
    expect(await page.locator("#editor").innerText()).toBe("Line one\nLine two");
    await page.close();
  });

  test(`appending keeps the formatting a person would get at the end with ${label}`, async () => {
    const { tab, page, ref } = await open("/");
    await page.evaluate(() => (window as any).load([{ text: "Bold", bold: true }, { text: " plain", bold: false }]));
    const typed = await run("type", { tab, ref: ref("Notes"), text: " more", clear: false });
    expect(typed.error).toBeUndefined();
    expect(await page.locator("#editor").innerHTML()).toBe("<strong>Bold</strong> plain more");
    await page.close();
  });
}

test("clicking an editor's Bold button answers and the page stays usable (#218)", async () => {
  const { tab, page, ref } = await open("/");
  const clicked = await setup.daemon.call("click", { tab, ref: ref("Bold"), submits: false });
  expect(clicked.error).toBeUndefined();
  const read = await setup.daemon.call("read", { tab });
  expect(read.error).toBeUndefined();
  await page.close();
});

test("a page stuck after a click fails that tab quickly and leaves other tabs working (#218)", async () => {
  test.setTimeout(120_000);
  const { tab, page, ref } = await open("/hang");
  let started = Date.now();
  const clicked = await setup.daemon.call("click", { tab, ref: ref("Bold"), submits: false });
  // Before, this call and every later one on the tab got no answer within
  // the 20 seconds Chat with Work waits.
  expect(Date.now() - started).toBeLessThan(18_000);
  expect(clicked.error?.message).toContain("isn't responding");
  started = Date.now();
  const read = await setup.daemon.call("read", { tab });
  expect(read.error?.message).toContain("isn't responding");
  expect(Date.now() - started).toBeLessThan(3_000);
  const other = await setup.daemon.call("open", { url: `${origin}/other` });
  expect(other.error).toBeUndefined();
  const otherRead = await setup.daemon.call("read", { tab: other.result.tab });
  expect(otherRead.result.text).toContain("Another page");
  const closed = await setup.daemon.call("close", { tab });
  expect(closed.error).toBeUndefined();
  expect(page.isClosed() || (await page.waitForEvent("close", { timeout: 5_000 }).then(() => true))).toBe(true);
  await setup.context.pages().find((p) => p.url() === `${origin}/other`)?.close();
});
