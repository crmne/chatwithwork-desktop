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

let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async () => {
  ({ origin, server } = await serve({ "/end": form(""), "/middle": form('<div style="height:3000px"></div>') }));
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
