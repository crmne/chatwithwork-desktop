import { expect, test } from "@playwright/test";
import { engineAction, launch, serve, type Setup } from "./harness.js";

// The shape from #226: a fixed banner along the bottom of the window over an
// edit form. On /end the submit button is the last thing on the page, so no
// scrolling can lift it above the banner; on /middle it starts under the
// banner but the page can scroll it clear.
const form = (after: string) => `<!doctype html><title>Edit form</title>
<style>
body { margin: 0; font: 16px sans-serif; }
form { padding: 16px; }
label { display: block; margin: 12px 0; }
#banner { position: fixed; left: 0; right: 0; bottom: 0; height: 120px; background: #222; color: #fff; z-index: 50; }
#save { display: block; margin: 0; height: 40px; }
</style>
<form id="form">
  ${Array.from({ length: 30 }, (_, i) => `<label>Field ${i + 1} <input name="f${i}"></label>`).join("")}
  <button type="submit" id="save">Save changes</button>
</form>
${after}
<div id="banner"><span>We use cookies</span> <span class="close" style="cursor:pointer" onclick="banner.remove()">×</span></div>
<p id="saved" style="position:fixed;top:0;right:0;margin:0">Saved 0</p>
<script>let saves = 0; form.addEventListener("submit", e => { e.preventDefault(); saved.textContent = "Saved " + (++saves); });</script>`;

// A fixed dock along the bottom with its own links, and an unnamed handle
// drawn over the middle of them (the shape a theme editor's section bar had):
// a person clicks the visible part of a link. "Hidden" is wholly under the
// handle, so nothing of it shows.
const dock = `<!doctype html><title>Editor</title>
<style>
body { margin: 0; font: 16px sans-serif; }
#dock { position: fixed; left: 0; right: 0; bottom: 0; height: 56px; background: #eee; }
#dock a { position: absolute; bottom: 12px; display: block; width: 120px; height: 32px; line-height: 32px; text-align: center; }
#dock #layout { left: 20px; } #dock #style { left: 160px; } #dock #hidden { left: 420px; width: 60px; }
#handle { position: absolute; left: 0; right: 0; bottom: 20px; height: 16px; background: rgba(0,0,0,0.2); border: 0; }
</style>
<p id="went" style="margin:16px">Went nowhere</p>
<div id="dock">
  <a id="layout" href="#layout">Layout</a><a id="style" href="#style">Style</a><a id="hidden" href="#hidden">Hidden</a>
  <button id="handle" aria-hidden="true" tabindex="-1"></button><button id="cover" style="position:absolute;bottom:0;height:56px;left:400px;width:120px;opacity:0.3;border:0" aria-hidden="true" tabindex="-1"></button>
</div>
<script>addEventListener("hashchange", () => { went.textContent = "Went to " + location.hash.slice(1); });</script>`;

let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async () => {
  ({ origin, server } = await serve({ "/end": form(""), "/middle": form('<div style="height:3000px"></div>'), "/dock": dock }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  for (const page of setup.context.pages()) if (page.url().startsWith(origin)) await page.close();
});

async function fixture(path: string) {
  const opened = await setup.daemon.call("open", { url: origin + path });
  expect(opened.error).toBeUndefined();
  const tab: number = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === origin + path)!;
  const read = await setup.daemon.call("read", { tab });
  const lines: string[] = read.result.text.split("\n");
  const ref = (needle: string) => /\[ref=(e\d+)\]/.exec(lines.find((line) => line.includes(needle)) ?? "")?.[1]!;
  return { tab, page, save: ref('button "Save changes"'), close: ref('clickable "×"') };
}

for (const engine of [false, true]) {
  const label = engine ? "the bundled engine" : "the extension's own engine";
  const run = (method: string, params: Record<string, unknown>) =>
    engine ? engineAction(setup, method, params) : setup.daemon.call(method, params);

  test(`a button under a fixed banner is scrolled clear and clicked with ${label} (#226)`, async () => {
    const { tab, page, save } = await fixture("/middle");
    // It starts under the banner.
    const covered = await page.evaluate(() => {
      const box = document.getElementById("save")!.getBoundingClientRect();
      return box.bottom > innerHeight - 120;
    });
    expect(covered).toBe(true);
    const preview = await run("click", { tab, ref: save, dry_run: true });
    expect(preview.error).toBeUndefined();
    await expect(page.locator("#saved")).toHaveText("Saved 0");
    const clicked = await run("click", { tab, ref: save });
    expect(clicked.error).toBeUndefined();
    await expect(page.locator("#saved")).toHaveText("Saved 1");
  });

  test(`a button the banner still covers fails quickly, certainly, naming the banner, with ${label} (#226)`, async () => {
    const { tab, page, save, close } = await fixture("/end");
    for (const dry_run of [true, false]) {
      const started = Date.now();
      const clicked = await run("click", { tab, ref: save, dry_run });
      expect(Date.now() - started).toBeLessThan(8_000);
      expect(clicked.error?.code).toBe("page_error");
      // Refused before the engine was started, so the daemon passes this on
      // as it is instead of "did not finish ... uncertain".
      if (engine) expect(clicked.methods).toBeUndefined();
      expect(clicked.error?.message).toContain("Nothing was clicked");
      expect(clicked.error?.message).toContain("fixed <div>");
      expect(clicked.error?.message).toContain("along the bottom of the window");
      await expect(page.locator("#saved")).toHaveText("Saved 0");
    }
    // With the banner closed, the same click goes through.
    expect((await run("click", { tab, ref: close })).error).toBeUndefined();
    await expect(page.locator("#banner")).toHaveCount(0);
    expect((await run("click", { tab, ref: save })).error).toBeUndefined();
    await expect(page.locator("#saved")).toHaveText("Saved 1");
  });
}

test("a short engine budget still checks for the banner before any input (#227)", async () => {
  const { tab, page, save } = await fixture("/end");
  const begun = await setup.daemon.call("engine_begin", { action: "click", params: { tab, ref: save }, timeout_ms: 1_500 });
  if (begun.result?.lease) await setup.daemon.call("engine_cancel", { lease: begun.result.lease });
  expect(begun.error?.code).toBe("page_error");
  expect(begun.error?.message).toContain("Nothing was clicked");
  await expect(page.locator("#saved")).toHaveText("Saved 0");
});

for (const engine of [false, true]) {
  const label = engine ? "the bundled engine" : "the extension's own engine";
  const run = (method: string, params: Record<string, unknown>) =>
    engine ? engineAction(setup, method, params) : setup.daemon.call(method, params);

  test(`a link whose middle a dock's handle covers is clicked where it shows, with ${label}`, async () => {
    const opened = await setup.daemon.call("open", { url: origin + "/dock" });
    const tab: number = opened.result.tab;
    const page = setup.context.pages().find((p) => p.url() === origin + "/dock")!;
    const lines: string[] = (await setup.daemon.call("read", { tab })).result.text.split("\n");
    const ref = (needle: string) => /\[ref=(e\d+)\]/.exec(lines.find((line) => line.includes(needle)) ?? "")?.[1]!;
    const clicked = await run("click", { tab, ref: ref('link "Style"'), submits: false });
    expect(clicked.error).toBeUndefined();
    await expect(page.locator("#went")).toHaveText("Went to style");
    // Wholly covered: still refused, certainly and by name, with nothing clicked.
    const hidden = await run("click", { tab, ref: ref('link "Hidden"'), submits: false });
    expect(hidden.error?.code).toBe("page_error");
    expect(hidden.error?.message).toContain("Nothing was clicked");
    await expect(page.locator("#went")).toHaveText("Went to style");
  });
}
