import { expect, test } from "@playwright/test";
import { EXTENSION_ID, inWorker, launch, serve, type Setup } from "./harness.js";

const PLAN = `<!doctype html><html><head><title>Falcon plan</title></head><body>
<h1>Project Falcon</h1>
<p>The quarterly budget is approved.</p>
<a href="/two">Next page</a>
<form method="post" action="/login">
  <label>Email <input name="email" value="ana@example.com"></label>
  <label>Password <input type="password" name="pw" value="hunter2"></label>
  <label>Card <input autocomplete="cc-number" name="card" value="4111111111111111"></label>
  <label for="cn">Card number</label> <input id="cn" value="5500000000000004">
  <span id="sc">Security code</span> <div role="textbox" contenteditable aria-labelledby="sc">737</div>
  <button>Sign in</button>
</form>
<card-form></card-form>
<script>
  customElements.define("card-form", class extends HTMLElement {
    connectedCallback() {
      this.attachShadow({ mode: "open" }).innerHTML = '<label>Card number <input id="shadow-card" value="4000056655665556"></label>';
    }
  });
</script>
<div style="height:3000px"></div>
<p>The end of the plan.</p>
<script>
  setTimeout(() => document.body.insertAdjacentHTML("beforeend", "<p>Loaded later</p>"), 1500);
</script>
</body></html>`;

let setup: Setup;
let allowed: string;
let other: string;
const servers: { close: () => void }[] = [];

// Opens `url` in a tab in the "Chat with Work" group, in the background,
// the way the person would drag a tab in.
async function groupTab(url: string, grouped = true): Promise<number> {
  return inWorker(
    setup.context,
    async ({ url, grouped }) => {
      const tab = await chrome.tabs.create({ url, active: false });
      if (grouped) {
        const groups = (await chrome.tabGroups.query({})).filter((g) => g.title === "Chat with Work");
        if (groups.length) {
          await chrome.tabs.group({ tabIds: [tab.id!], groupId: groups[0].id });
        } else {
          const id = await chrome.tabs.group({ tabIds: [tab.id!] });
          await chrome.tabGroups.update(id, { title: "Chat with Work" });
        }
      }
      for (let i = 0; i < 100; i++) {
        const now = await chrome.tabs.get(tab.id!);
        if (now.status === "complete") break;
        await new Promise((r) => setTimeout(r, 50));
      }
      return tab.id!;
    },
    { url, grouped },
  );
}

test.beforeAll(async () => {
  const a = await serve({ "/": PLAN, "/two": "<title>Two</title><h1>Second page</h1>" });
  const b = await serve({ "/": "<title>Payroll</title><h1>Salaries</h1>" });
  servers.push(a.server, b.server);
  allowed = a.origin;
  other = b.origin;
  setup = await launch([allowed]);
  // A page so the window has an active tab of the person's own.
  const page = setup.context.pages()[0] ?? (await setup.context.newPage());
  await page.goto(`${allowed}/two`);
  await setup.daemon.attached();
});

test.afterAll(async () => {
  await setup?.cleanup();
  for (const server of servers) server.close();
});

test("the native host attaches with the extension's origin and it says hello", async () => {
  expect(setup.daemon.attachedOrigin).toBe(`chrome-extension://${EXTENSION_ID}/`);
  const hello = await setup.daemon.waitForEvent((e) => e.event === "hello");
  expect(hello.version).toBe("0.1.0");
  expect(hello.stopped).toBe(false);
});

test("an empty group says how to start", async () => {
  const reply = await setup.daemon.call("read");
  expect(reply.error?.code).toBe("no_tab");
  expect((await setup.daemon.call("tabs")).result.tabs).toEqual([]);
});

test("reads a background tab in the group as accessible text, never sensitive values", async () => {
  const tab = await groupTab(`${allowed}/`);
  const reply = await setup.daemon.call("read", { tab });
  expect(reply.error).toBeUndefined();
  const text: string = reply.result.text;
  expect(reply.result.title).toBe("Falcon plan");
  expect(text).toContain('heading "Project Falcon" [level=1]');
  expect(text).toContain("The quarterly budget is approved.");
  expect(text).toMatch(/link "Next page" \[ref=e\d+\]/);
  expect(text).toMatch(/textbox "Email" \[ref=e\d+\]: ana@example.com/);
  expect(text).toMatch(/textbox "Password" \[ref=e\d+\] \[sensitive\]: \(hidden\)/);
  expect(text).toMatch(/textbox "Card" \[ref=e\d+\] \[sensitive\]: \(hidden\)/);
  expect(text).not.toContain("hunter2");
  expect(text).not.toContain("4111111111111111");
  expect(text).not.toContain("5500000000000004");
  expect(text).not.toContain("737");
  expect(text).not.toContain("4000056655665556");
  expect(reply.result.next_offset).toBeNull();

  const paged = await setup.daemon.call("read", { tab, offset: 5, max_chars: 10 });
  expect(Array.from(paged.result.text as string)).toHaveLength(10);
  expect(paged.result.next_offset).toBe(15);
});

test("lists the group's tabs and nothing outside it", async () => {
  const outside = await groupTab(`${allowed}/two`, false);
  const reply = await setup.daemon.call("tabs");
  const ids = reply.result.tabs.map((t: { tab: number }) => t.tab);
  expect(ids).not.toContain(outside);
  expect(reply.result.tabs.every((t: { allowed: boolean }) => t.allowed)).toBe(true);
  const refused = await setup.daemon.call("read", { tab: outside });
  expect(refused.error?.code).toBe("not_in_group");
});

test("refuses tabs on sites the person didn't allow", async () => {
  const tab = await groupTab(`${other}/`);
  const reply = await setup.daemon.call("read", { tab });
  expect(reply.error?.code).toBe("site_not_allowed");
  expect(JSON.stringify(reply)).not.toContain("Salaries");
  const shot = await setup.daemon.call("screenshot", { tab });
  expect(shot.error?.code).toBe("site_not_allowed");
  const listed = (await setup.daemon.call("tabs")).result.tabs.find((t: { tab: number }) => t.tab === tab);
  expect(listed.allowed).toBe(false);
  await inWorker(setup.context, (id) => chrome.tabs.remove(id as number), tab);
});

test("screenshots a background tab, as a JPEG that fits a tool result", async () => {
  const tabs = (await setup.daemon.call("tabs")).result.tabs;
  const tab = tabs.find((t: { url: string }) => t.url === `${allowed}/`).tab;
  const reply = await setup.daemon.call("screenshot", { tab });
  expect(reply.error).toBeUndefined();
  expect(reply.result.image.mime_type).toBe("image/jpeg");
  const bytes = Buffer.from(reply.result.image.data, "base64");
  expect(bytes.subarray(0, 3)).toEqual(Buffer.from([0xff, 0xd8, 0xff]));
  expect(reply.result.image.data.length).toBeLessThanOrEqual(180_000);
  expect(reply.result.width).toBeGreaterThan(0);
  // Fields blurred for the picture, a shadow root's too, are clear after it.
  const page = setup.context.pages().find((p) => p.url() === `${allowed}/`)!;
  const filters = await page.evaluate(() => [
    getComputedStyle(document.querySelector("card-form")!.shadowRoot!.querySelector("input")!).filter,
    getComputedStyle(document.querySelector('input[type="password"]')!).filter,
  ]);
  expect(filters).toEqual(["none", "none"]);
  if (process.env.SAVE_SCREENSHOT) (await import("node:fs")).writeFileSync(process.env.SAVE_SCREENSHOT, bytes);
});

test("scrolls and waits for text", async () => {
  const tabs = (await setup.daemon.call("tabs")).result.tabs;
  const tab = tabs.find((t: { url: string }) => t.url === `${allowed}/`).tab;
  const down = await setup.daemon.call("scroll", { tab, direction: "down", pages: 10 });
  expect(down.error).toBeUndefined();
  expect(down.result.scroll_y).toBeGreaterThan(0);
  expect(down.result.at_end).toBe(true);
  const waited = await setup.daemon.call("wait", { tab, text: "Loaded later", seconds: 5 });
  expect(waited.result.found).toBe(true);
  const missing = await setup.daemon.call("wait", { tab, text: "never there", seconds: 1 });
  expect(missing.result.found).toBe(false);
});

test("refuses unknown methods and arguments", async () => {
  expect((await setup.daemon.call("evaluate", { expression: "1" })).error?.code).toBe("invalid_argument");
  expect((await setup.daemon.call("read", { script: "x" })).error?.code).toBe("invalid_argument");
  expect((await setup.daemon.call("scroll", { ref: "body" })).error?.code).toBe("invalid_argument");
});

test("the computer's kill switch refuses every call", async () => {
  setup.daemon.setPolicy({ stopped: true });
  await new Promise((r) => setTimeout(r, 200));
  expect((await setup.daemon.call("tabs")).error?.code).toBe("browser_stopped");
  setup.daemon.setPolicy({ stopped: false });
  await new Promise((r) => setTimeout(r, 200));
  expect((await setup.daemon.call("tabs")).error).toBeUndefined();
});

test("Stop in the extension's menu refuses every call until Resume", async () => {
  const popup = await setup.context.newPage();
  await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
  await expect(popup.locator("#state-text")).toHaveText("Ready");
  await popup.click("#stop");
  await expect(popup.locator("#state-text")).toHaveText("Stopped");
  await setup.daemon.waitForEvent((e) => e.event === "state" && e.stopped === true);
  expect((await setup.daemon.call("tabs")).error?.code).toBe("browser_stopped");
  await popup.click("#resume");
  await expect(popup.locator("#state-text")).toHaveText("Ready");
  expect((await setup.daemon.call("tabs")).error).toBeUndefined();
  await popup.close();
});
