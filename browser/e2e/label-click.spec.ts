import { expect, test } from "@playwright/test";
import { launch, serve, type Setup } from "./harness.js";

const form = (type: string) => `<!doctype html><title>Label activation fixtures</title>
<style>
body { margin:0; font:16px sans-serif; }
#choice { position:absolute; left:40px; top:40px; width:26px; height:26px; opacity:0; }
#caption { position:absolute; left:40px; top:40px; width:260px; height:64px; display:flex; align-items:center; justify-content:center; background:#eee; z-index:2; }
#other { position:absolute; left:400px; top:40px; }
#result { margin-top:150px; }
.overlay { position:absolute; inset:0; background:white; z-index:10; }
</style>
<input id="choice" type="${type}" name="choice"><label id="caption" for="choice"><span>Use this option</span></label>
<input id="other" type="${type}" name="other" aria-label="Other option">
<p id="result">No action</p>
<script>
window.changes=0;choice.addEventListener('change',()=>result.textContent='Changes: '+(++window.changes));
other.addEventListener('change',()=>result.textContent='Wrong control');
</script>`;

let setup: Setup;
let server: { close: () => void };
let origin: string;

test.beforeAll(async () => {
  ({ server, origin } = await serve({ "/radio": form("radio"), "/checkbox": form("checkbox") }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  for (const page of setup.context.pages()) if (page.url().startsWith(origin)) await page.close();
});

async function fixture(type = "radio") {
  const opened = await setup.daemon.call("open", { url: `${origin}/${type}` });
  expect(opened.error).toBeUndefined();
  const tab: number = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === `${origin}/${type}`)!;
  const read = await setup.daemon.call("read", { tab });
  const line = read.result.text.split("\n").find((line: string) => line.includes(`${type} "Use this option"`));
  const ref = /\[ref=(e\d+)\]/.exec(line ?? "")?.[1];
  expect(ref).toBeTruthy();
  return { tab, ref, page };
}

for (const type of ["radio", "checkbox"]) {
  for (const style of ["sibling", "wrapped", "pseudo-element", "display none", "visibility hidden"]) {
    test(`${type}: activates only the intended control through its ${style} label`, async () => {
      const { tab, ref, page } = await fixture(type);
      await page.evaluate((style) => {
        const input = document.getElementById("choice")!;
        const label = document.getElementById("caption")!;
        if (style === "wrapped") {
          label.removeAttribute("for");
          label.prepend(input);
          input.style.cssText = "position:absolute;inset:0;width:100%;height:100%;opacity:0;pointer-events:none";
        }
        if (style === "display none") input.style.display = "none";
        if (style === "visibility hidden") input.style.visibility = "hidden";
        if (style === "pseudo-element") {
          const css = document.createElement("style");
          css.textContent = '#caption::after { content:""; position:absolute; inset:0; background:rgba(0,0,0,.01); }';
          document.head.append(css);
        }
      }, style);
      const preview = await setup.daemon.call("click", { tab, ref, submits: false, dry_run: true });
      expect(preview.error).toBeUndefined();
      expect(preview.result.activation_surface).toBe("associated label");
      await expect(page.locator("#choice")).not.toBeChecked();
      await expect(page.locator("#result")).toHaveText("No action");
      const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
      expect(clicked.error).toBeUndefined();
      await expect(page.locator("#choice")).toBeChecked();
      await expect(page.locator("#other")).not.toBeChecked();
      await expect(page.locator("#result")).toHaveText("Changes: 1");
    });
  }
}

for (const cover of ["another control's label", "real overlay", "nested button", "nested link"]) {
  test(`refuses ${cover} over a radio's activation surface`, async () => {
    const { tab, ref, page } = await fixture();
    await page.evaluate((cover) => {
      const caption = document.getElementById("caption")!;
      if (cover === "another control's label") {
        caption.setAttribute("for", "other");
      } else {
        const overlay = document.createElement(cover === "nested button" ? "button" : cover === "nested link" ? "a" : "div");
        overlay.className = "overlay";
        if (overlay instanceof HTMLAnchorElement) overlay.href = "#wrong";
        overlay.textContent = "Different action";
        overlay.addEventListener("click", (event) => {
          event.preventDefault();
          document.getElementById("result")!.textContent = "Wrong action";
        });
        (cover === "real overlay" ? document.body : caption).append(overlay);
      }
    }, cover);
    const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
    expect(clicked.error?.code).toBe("page_error");
    expect(clicked.error?.message).toContain("drawn over");
    await expect(page.locator("#choice")).not.toBeChecked();
    await expect(page.locator("#other")).not.toBeChecked();
    await expect(page.locator("#result")).toHaveText("No action");
  });
}

test("a visible label never enables its disabled control", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => { (document.getElementById("choice") as HTMLInputElement).disabled = true; });
  const preview = await setup.daemon.call("click", { tab, ref, submits: false, dry_run: true });
  expect(preview.error?.message).toContain("disabled");
  await expect(page.locator("#choice")).not.toBeChecked();
  await expect(page.locator("#result")).toHaveText("No action");
});

test("a label reassigned during hover cannot activate a different control", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const caption = document.getElementById("caption")!;
    caption.addEventListener("pointermove", () => caption.setAttribute("for", "other"), { once: true });
  });
  const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(clicked.error?.message).toContain("no longer belongs");
  await expect(page.locator("#choice")).not.toBeChecked();
  await expect(page.locator("#other")).not.toBeChecked();
  await expect(page.locator("#result")).toHaveText("No action");
});

test("the trusted-event guard refuses a label rebound after mousedown", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const caption = document.getElementById("caption")!;
    caption.addEventListener("mousedown", () => caption.setAttribute("for", "other"), { once: true });
  });
  const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(clicked.error?.message).toContain("no longer belongs");
  await expect(page.locator("#choice")).not.toBeChecked();
  await expect(page.locator("#other")).not.toBeChecked();
  await expect(page.locator("#result")).toHaveText("No action");
});

test("label retargeting never turns a protected field into a selectable control", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const input = document.getElementById("choice") as HTMLInputElement;
    input.type = "password";
    input.value = "synthetic-secret-not-for-output";
  });
  const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(clicked.error?.code).toBe("page_error");
  expect(clicked.error?.message).toContain("drawn over");
  expect(JSON.stringify(clicked)).not.toContain("synthetic-secret-not-for-output");
  await expect(page.locator("#result")).toHaveText("No action");
});
