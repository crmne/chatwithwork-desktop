import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

// Like component-library consent buttons: the accessible button lives in
// a shadow tree, with its label supplied as slotted text from the host.
const PAGE = `<!doctype html><title>Consent controls</title>
<style>consent-button { display: block; margin: 24px; width: 240px; }</style>
<consent-button>Essential cookies</consent-button>
<consent-button closed>Closed component</consent-button>
<consent-button covered>Covered component</consent-button>
<div style="position:relative"><consent-button>Outside overlay</consent-button><div style="position:absolute;inset:0;background:white">Cover</div></div>
<p id="result">No action</p>
<script>
customElements.define('consent-button',class extends HTMLElement {
  connectedCallback() {
    const root=this.attachShadow({mode:this.hasAttribute('closed')?'closed':'open'});
    root.innerHTML='<style>button{padding:18px;width:240px} div{position:relative}.cover{position:absolute;inset:0;background:white}</style><div><button type="button"><slot></slot></button>'+(this.hasAttribute('covered')?'<div class="cover">Cover</div>':'')+'</div>';
    root.querySelector('button').addEventListener('click',()=>document.querySelector('#result').textContent=this.textContent);
  }
});
</script>`;

const ACTIONABILITY = `<!doctype html><title>Pointer fixtures</title>
<style>
body { margin:0; font:16px sans-serif; }
#target { display:block; width:240px; padding:24px; margin:24px; }
#cover { position:fixed; inset:0; background:white; z-index:50; }
</style>
<a id="target" href="#done"><span>Continue</span></a>
<p id="result">No action</p>
<script>
window.count=0;
target.addEventListener('click', e => { e.preventDefault(); result.textContent='Actions: '+(++window.count); });
</script>`;

let setup: Setup;
let server: { close: () => void };
let origin: string;

test.beforeAll(async () => {
  ({ server, origin } = await serve({ "/": PAGE, "/pointer": ACTIONABILITY }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});

test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  for (const page of setup.context.pages()) {
    if (page.url() === `${origin}/pointer`) await page.close();
  }
});

for (const [label, covered] of [["Essential cookies", false], ["Closed component", false], ["Covered component", true], ["Outside overlay", true]] as const) {
  test(`${covered ? "refuses" : "clicks"} ${label} in a background tab`, async () => {
    const opened = await setup.daemon.call("open", { url: origin });
    expect(opened.error).toBeUndefined();
    const tab = opened.result.tab;
    const before = await setup.daemon.call("read", { tab });
    const line = before.result.text.split("\n").find((line: string) => line.includes(`button "${label}"`));
    const ref = /\[ref=(e\d+)\]/.exec(line || "")?.[1];
    expect(ref).toBeTruthy();
    const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
    const after = await setup.daemon.call("read", { tab });
    if (covered) {
      expect(clicked.error?.code).toBe("page_error");
      expect(after.result.text).toContain("No action");
    } else {
      expect(clicked.error).toBeUndefined();
      expect(after.result.text).not.toContain("No action");
      expect(after.result.text).toContain(`paragraph: ${label}`);
    }
  });
}

async function fixture() {
  const opened = await setup.daemon.call("open", { url: `${origin}/pointer` });
  expect(opened.error).toBeUndefined();
  const tab: number = opened.result.tab;
  const page = setup.context.pages().find((p) => p.url() === `${origin}/pointer`)!;
  expect(page).toBeTruthy();
  const read = await setup.daemon.call("read", { tab });
  const line = read.result.text.split("\n").find((line: string) => line.includes('link "Continue"'));
  const ref = /\[ref=(e\d+)\]/.exec(line ?? "")?.[1];
  expect(ref).toBeTruthy();
  return { tab, ref, page };
}

for (const scenario of ["contents", "clipped", "scrolled", "nested closed shadow", "slotted link"] as const) {
  test(`clicks a ${scenario} link once, using its visible painted content`, async () => {
    const { tab, ref, page } = await fixture();
    await page.evaluate((scenario) => {
      const target = document.getElementById("target")!;
      if (scenario === "contents") target.style.display = "contents";
      if (scenario === "clipped") Object.assign(target.style, { position: "fixed", width: "2000px", left: "-1600px", top: "80px", margin: "0" });
      if (scenario === "scrolled") Object.assign(target.style, { marginTop: "2200px", transform: "translate(3.25px, 2.5px)" });
      if (scenario === "nested closed shadow") {
        const host = document.createElement("div");
        document.body.append(host);
        const outer = host.attachShadow({ mode: "closed" });
        const inner = document.createElement("div");
        outer.append(inner);
        inner.attachShadow({ mode: "open" }).append(target);
      }
      if (scenario === "slotted link") {
        const host = document.createElement("div");
        document.body.append(host);
        host.attachShadow({ mode: "open" }).innerHTML = '<div><slot></slot></div>';
        host.append(target);
      }
    }, scenario);
    const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
    expect(clicked.error).toBeUndefined();
    await expect(page.locator("#result")).toHaveText("Actions: 1");
    await page.close();
  });
}

test("previews refuse a real overlay, naming its safe accessible label without field values", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const cover = document.createElement("div");
    cover.id = "cover";
    cover.setAttribute("role", "dialog");
    cover.setAttribute("aria-label", "Cookie settings");
    cover.innerHTML = '<input type="password" value="never-return-this-secret">';
    document.body.append(cover);
  });
  const preview = await setup.daemon.call("click", { tab, ref, submits: false, dry_run: true });
  expect(preview.error?.code).toBe("page_error");
  expect(preview.error?.message).toContain('role="dialog" name="Cookie settings"');
  expect(JSON.stringify(preview)).not.toContain("never-return-this-secret");
  await expect(page.locator("#result")).toHaveText("No action");
  await page.close();
});

test("waits for a temporary overlay and disabled control, then delivers one click", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const target = document.getElementById("target")!;
    target.setAttribute("aria-disabled", "true");
    const cover = document.createElement("div");
    cover.id = "cover";
    document.body.append(cover);
  });
  const clicked = setup.daemon.call("click", { tab, ref, submits: false });
  // The page, not an extension force-click, makes the control ready.
  await page.waitForTimeout(250);
  await page.evaluate(() => {
    document.getElementById("cover")!.remove();
    document.getElementById("target")!.removeAttribute("aria-disabled");
  });
  expect((await clicked).error).toBeUndefined();
  await expect(page.locator("#result")).toHaveText("Actions: 1");
  await page.close();
});

test("refuses a hover-created overlay before pressing it", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    document.getElementById("target")!.addEventListener("pointermove", () => {
      const cover = document.createElement("div");
      cover.id = "cover";
      cover.setAttribute("aria-label", "New overlay");
      cover.addEventListener("click", () => { document.getElementById("result")!.textContent = "Wrong action"; });
      document.body.append(cover);
    }, { once: true });
  });
  const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(clicked.error?.message).toContain("New overlay");
  await expect(page.locator("#result")).toHaveText("No action");
  await page.close();
});

test("waits for motion to stop and scrolls past a sticky header without forcing through it", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => {
    const target = document.getElementById("target")!;
    target.style.marginTop = "2200px";
    const header = document.createElement("div");
    header.style.cssText = "position:fixed;inset:0 0 auto;height:500px;background:white;z-index:10";
    header.setAttribute("aria-label", "Sticky header");
    document.body.append(header);
    const style = document.createElement("style");
    style.textContent = "@keyframes move { from { transform:translateX(0) } to { transform:translateX(300px) } } #target { animation:move 2s linear infinite alternate; }";
    style.id = "motion";
    document.head.append(style);
    document.body.style.paddingBottom = "1000px";
  });
  const clicked = setup.daemon.call("click", { tab, ref, submits: false });
  await page.waitForTimeout(250);
  await expect(page.locator("#result")).toHaveText("No action");
  await page.evaluate(() => document.getElementById("motion")!.remove());
  expect((await clicked).error).toBeUndefined();
  await expect(page.locator("#result")).toHaveText("Actions: 1");
});

test("a permanently disabled control fails within a bounded wait", async () => {
  const { tab, ref, page } = await fixture();
  await page.evaluate(() => document.getElementById("target")!.setAttribute("aria-disabled", "true"));
  const started = Date.now();
  const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(clicked.error?.message).toContain("disabled");
  expect(Date.now() - started).toBeLessThan(8_000);
  await expect(page.locator("#result")).toHaveText("No action");
});

test("trusted-event guard blocks a race after preflight and never retries a press", async () => {
  const { tab, ref, page } = await fixture();
  // Test-only CDP interposition: install an overlay at the exact boundary
  // between the final DOM check and dispatch. No such operation is exposed
  // by the extension's fixed tool protocol.
  await inWorker(setup.context, async (tabId) => {
    const original = chrome.debugger.sendCommand.bind(chrome.debugger);
    (globalThis as any).__restorePointerTest = () => { chrome.debugger.sendCommand = original; };
    (globalThis as any).__pointerPresses = 0;
    chrome.debugger.sendCommand = (async (target: any, method: string, params: any) => {
      if (target.tabId === tabId && method === "Input.dispatchMouseEvent" && params.type === "mousePressed") {
        (globalThis as any).__pointerPresses++;
        await original(target, "Runtime.evaluate", { expression: `
          const cover=document.createElement('div');
          cover.id='cover';cover.setAttribute('aria-label','Racing overlay');
          cover.addEventListener('click',()=>document.getElementById('result').textContent='Wrong action');
          document.body.append(cover);
        ` });
      }
      return original(target, method, params);
    }) as typeof chrome.debugger.sendCommand;
  }, tab);
  try {
    const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
    expect(clicked.error?.message).toContain("Racing overlay");
    expect(await inWorker(setup.context, () => (globalThis as any).__pointerPresses)).toBe(1);
    await expect(page.locator("#result")).toHaveText("No action");
  } finally {
    await inWorker(setup.context, () => (globalThis as any).__restorePointerTest());
  }
  await page.evaluate(() => document.getElementById("cover")!.remove());
  expect((await setup.daemon.call("click", { tab, ref, submits: false })).error).toBeUndefined();
  await expect(page.locator("#result")).toHaveText("Actions: 1");
});

for (const revocation of ["actions", "site", "Stop"] as const) {
  test(`respects ${revocation} revoked while waiting for actionability`, async () => {
    const { tab, ref, page } = await fixture();
    await page.evaluate(() => document.getElementById("target")!.setAttribute("aria-disabled", "true"));
    const clicked = setup.daemon.call("click", { tab, ref, submits: false });
    await page.waitForTimeout(150);
    setup.daemon.setPolicy(revocation === "actions" ? { changes: false } : revocation === "site" ? { sites: [] } : { stopped: true });
    const result = await clicked;
    expect(result.error?.code).toBe(revocation === "actions" ? "not_writable" : revocation === "site" ? "site_not_allowed" : "browser_stopped");
    await expect(page.locator("#result")).toHaveText("No action");
    setup.daemon.setPolicy({ changes: true, sites: [origin], stopped: false });
    await inWorker(setup.context, async () => {});
    await page.close();
  });
}
