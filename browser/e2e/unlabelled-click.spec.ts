import { expect, test } from "@playwright/test";
import { engineAction, launch, serve, type Setup } from "./harness.js";

// The shape from #225: an edit form whose rows each end in a bare "×" with a
// click handler and no role or name, and a banner whose close "X" is a
// `<div onclick>`. Also an icon drawn only by CSS, a `tabindex` element, and
// controls the accessibility tree already offers.
const FORM = `<!doctype html><title>Edit listing</title>
<style>
body { font: 16px sans-serif; margin: 24px; }
.row { display: flex; gap: 8px; align-items: center; margin: 8px 0; }
.remove { cursor: pointer; padding: 4px 8px; }
.icon { display: inline-block; width: 16px; height: 16px; background: #c00; cursor: pointer; }
#banner { position: fixed; left: 0; right: 0; top: 0; padding: 8px; background: #ffd; }
#banner .close { float: right; cursor: pointer; }
.wrapper { cursor: pointer; padding: 4px; }
</style>
<div id="banner">Try our newsletter <div class="close" onclick="this.parentNode.remove()">X</div></div>
<h1 style="margin-top:60px">Edit listing</h1>
<form id="form" onsubmit="return false">
  <div class="row" id="row1"><input aria-label="Feature 1" value="Alpha"><span class="remove">×</span></div>
  <div class="row" id="row2"><input aria-label="Feature 2" value="Beta"><span class="remove">×</span></div>
  <div class="row" id="row3"><input aria-label="Feature 3" value="Gamma"><span class="remove">×</span></div>
  <p>Logo <i class="icon" id="clear-logo"></i></p>
  <div class="wrapper"><button type="button" id="add">Add feature</button></div>
  <div tabindex="0" id="more">More options</div>
</form>
<p id="log">Nothing yet</p>
<script>
for (const x of document.querySelectorAll(".remove")) x.addEventListener("click", () => x.parentNode.remove());
document.getElementById("clear-logo").addEventListener("click", () => log.textContent = "Logo cleared");
document.getElementById("add").addEventListener("click", () => log.textContent = "Added");
document.getElementById("more").addEventListener("click", () => log.textContent = "More");
</script>`;

// Review findings on #227: a clickable that only wraps a hidden input, and a
// long unnamed control whose action word comes after its first 200 characters.
const EXTRA = `<!doctype html><title>More rows</title>
<div class="row" id="hidden-row"><span id="hidden-remove" style="cursor:pointer" onclick="this.parentNode.remove()">×<input type="hidden" name="row" value="4"></span></div>
<label for="details">Details</label><button type="button" id="details" aria-label="Details" onclick="log.textContent='Deleted'">Delete</button>
<span id="long" style="cursor:pointer" onclick="log.textContent='Long clicked'">${"More ".repeat(41)}Delete</span>
<p id="log">Nothing yet</p>`;

let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async () => {
  ({ origin, server } = await serve({ "/": FORM, "/extra": EXTRA }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  for (const page of setup.context.pages()) if (page.url().startsWith(origin)) await page.close();
});

async function fixture() {
  const opened = await setup.daemon.call("open", { url: origin });
  expect(opened.error).toBeUndefined();
  const tab: number = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === `${origin}/`)!;
  const read = await setup.daemon.call("read", { tab });
  expect(read.error).toBeUndefined();
  const lines: string[] = read.result.text.split("\n");
  const refOf = (line: string | undefined) => /\[ref=(e\d+)\]/.exec(line ?? "")?.[1];
  return { tab, page, text: read.result.text as string, lines, refOf };
}

test("the reader gives unlabelled clickable controls a ref, named by text or position (#225)", async () => {
  const { text, lines, refOf } = await fixture();
  const removes = lines.filter((line) => line.includes('clickable "×"'));
  expect(removes).toHaveLength(3);
  for (const [index, value] of ["Alpha", "Beta", "Gamma"].entries()) {
    expect(removes[index]).toContain(`[near="${value}"]`);
    expect(refOf(removes[index])).toBeTruthy();
  }
  expect(refOf(lines.find((line) => line.includes('clickable "X"')))).toBeTruthy();
  // An icon with no text at all is named by what's before it.
  expect(lines.find((line) => /- clickable \[ref=e\d+\] \[near="Logo"\]/.test(line))).toBeTruthy();
  expect(refOf(lines.find((line) => line.includes('"More options"')))).toBeTruthy();
  // A pointer wrapper around a real button adds nothing: the button is the control.
  expect(text.match(/Add feature/g)).toHaveLength(1);
  expect(refOf(lines.find((line) => line.includes('button "Add feature"')))).toBeTruthy();
  // Plain text stays text.
  expect(lines.find((line) => line.includes("Edit listing"))).not.toContain("[ref=");
});

for (const engine of [false, true]) {
  const label = engine ? "the bundled engine" : "the extension's own engine";
  const run = (method: string, params: Record<string, unknown>) =>
    engine ? engineAction(setup, method, params) : setup.daemon.call(method, params);

  test(`a glyph-only control can't be clicked as harmless, but asks and then works, with ${label} (#225)`, async () => {
    const { tab, page, lines, refOf } = await fixture();
    const ref = refOf(lines.filter((line) => line.includes('clickable "×"'))[1])!;
    const plain = await run("click", { tab, ref, submits: false });
    expect(plain.error?.code).toBe("would_submit");
    expect(plain.error?.message ?? "").not.toBe("");
    await expect(page.locator(".row")).toHaveCount(3);

    const preview = await run("click", { tab, ref, dry_run: true });
    expect(preview.error).toBeUndefined();
    expect(preview.result.submits).toBe(true);
    expect(preview.result.target.name).toBe("×");
    expect(preview.result.image.data.length).toBeGreaterThan(100);
    await expect(page.locator(".row")).toHaveCount(3);

    const clicked = await run("click", { tab, ref });
    expect(clicked.error).toBeUndefined();
    await expect(page.locator("#row2")).toHaveCount(0);
    await expect(page.locator("#row1")).toHaveCount(1);
    await expect(page.locator("#row3")).toHaveCount(1);
  });

  test(`a banner's close control and a named tabindex control click with ${label}`, async () => {
    const { tab, page, lines, refOf } = await fixture();
    const more = refOf(lines.find((line) => line.includes('"More options"')))!;
    // Named, and the name doesn't commit to anything: a plain write.
    expect((await run("click", { tab, ref: more, submits: false })).error).toBeUndefined();
    await expect(page.locator("#log")).toHaveText("More");
    const close = refOf(lines.find((line) => line.includes('clickable "X"')))!;
    expect((await run("click", { tab, ref: close })).error).toBeUndefined();
    await expect(page.locator("#banner")).toHaveCount(0);
  });
}

test("a clickable that only wraps a hidden input still gets a ref (#227)", async () => {
  const opened = await setup.daemon.call("open", { url: `${origin}/extra` });
  const read = await setup.daemon.call("read", { tab: opened.result.tab });
  expect(read.result.text).toMatch(/- clickable "×" \[ref=e\d+\]/);
});

for (const engine of [false, true]) {
  test(`a long unnamed control is judged by all of its text with ${engine ? "the bundled engine" : "the extension's own engine"} (#227)`, async () => {
    const opened = await setup.daemon.call("open", { url: `${origin}/extra` });
    const tab = opened.result.tab;
    const page = setup.context.pages().find((p) => p.url() === `${origin}/extra`)!;
    const read = await setup.daemon.call("read", { tab });
    const ref = /\[ref=(e\d+)\]/.exec(read.result.text.split("\n").find((line: string) => line.includes("More More")) ?? "")?.[1];
    expect(ref).toBeTruthy();
    const params = { tab, ref, submits: false };
    const clicked = engine ? await engineAction(setup, "click", params) : await setup.daemon.call("click", params);
    expect(clicked.error?.code).toBe("would_submit");
    // Refused by the check before the click, not left to the page guard.
    if (engine) expect(clicked.methods).toBeUndefined();
    await expect(page.locator("#log")).toHaveText("Nothing yet");
  });
}

test("the page guard still judges a control's own text when a label names it otherwise (#227)", async () => {
  const opened = await setup.daemon.call("open", { url: `${origin}/extra` });
  const tab = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === `${origin}/extra`)!;
  const read = await setup.daemon.call("read", { tab });
  const ref = /\[ref=(e\d+)\]/.exec(read.result.text.split("\n").find((line: string) => line.includes('button "Details"')) ?? "")?.[1];
  expect(ref).toBeTruthy();
  // The check before the click sees only the accessible name, "Details";
  // the guard at the click sees "Delete" and stops it.
  const clicked = await engineAction(setup, "click", { tab, ref, submits: false });
  expect(clicked.error?.message ?? "").toContain("submits a change");
  await expect(page.locator("#log")).toHaveText("Nothing yet");
});
