import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

let setup: Setup;
let allowed: string;
let other: string;
const servers: { close: () => void }[] = [];

const FORM = (other: string) => `<!doctype html><html><head><title>Inbox</title></head><body>
<h1>Inbox</h1>
<a href="/two">Next page</a>
<a href="${other}/">Elsewhere</a>
<a href="/report.csv">Q3 report</a>
<form action="/search" method="get">
  <label>Query <input name="q"></label>
  <label>Password <input type="password" name="pw"></label>
  <button>Search</button>
</form>
<button type="button" onclick="document.getElementById('more').textContent='More text here'">Show more</button>
<p id="more"></p>
<div role="button" tabindex="0" onclick="document.title='sent'">Send message</div>
<label>Size <select name="size"><option value="s">Small</option><option value="m">Medium</option><option value="l">Large</option></select></label>
<div style="position:relative">
  <button type="button" onclick="document.title='hidden clicked'">Hidden</button>
  <div style="position:absolute;inset:-20px;background:white">Cookie banner</div>
</div>
</body></html>`;

function refOf(text: string, line: RegExp): string {
  const found = text.split("\n").find((l) => line.test(l));
  const ref = found && /\[ref=(e\d+)\]/.exec(found)?.[1];
  if (!ref) throw new Error(`no ref for ${line} in\n${text}`);
  return ref;
}

async function readTab(tab: number): Promise<string> {
  const reply = await setup.daemon.call("read", { tab });
  expect(reply.error).toBeUndefined();
  return reply.result.text;
}

test.beforeAll(async () => {
  const b = await serve({ "/": "<title>Elsewhere</title><h1>Private</h1>" });
  other = b.origin;
  const a = await serve({
    "/": FORM(other),
    "/two": "<title>Two</title><h1>Second page</h1>",
    "/search?q=falcon&pw=": "<title>Results</title><h1>Results for falcon</h1>",
    "/report.csv": "quarter,revenue\nQ3,100\n",
  });
  servers.push(a.server, b.server);
  allowed = a.origin;
  setup = await launch([allowed]);
  const page = setup.context.pages()[0] ?? (await setup.context.newPage());
  await page.goto(`${allowed}/two`);
  await setup.daemon.attached();
});

test.afterAll(async () => {
  await setup?.cleanup();
  for (const server of servers) server.close();
});

test("actions are refused until the person allows them", async () => {
  const reply = await setup.daemon.call("open", { url: `${allowed}/` });
  expect(reply.error?.code).toBe("not_writable");
  setup.daemon.setPolicy({ changes: true });
  await new Promise((r) => setTimeout(r, 200));
});

test("opens allowed pages in a background tab in its own group, and nothing else", async () => {
  const refused = await setup.daemon.call("open", { url: `${other}/` });
  expect(refused.error?.code).toBe("site_not_allowed");
  const dry = await setup.daemon.call("open", { url: `${allowed}/`, dry_run: true });
  expect(dry.result).toEqual({ dry_run: true, url: `${allowed}/`, new_tab: true });
  expect((await setup.daemon.call("tabs")).result.tabs).toEqual([]);

  const opened = await setup.daemon.call("open", { url: `${allowed}/` });
  expect(opened.error).toBeUndefined();
  expect(opened.result.title).toBe("Inbox");
  const group = await inWorker(setup.context, async (tab) => {
    const t = await chrome.tabs.get(tab as number);
    const g = await chrome.tabGroups.get(t.groupId);
    return { title: g.title, active: t.active, discardable: t.autoDiscardable };
  }, opened.result.tab);
  expect(group).toEqual({ title: "Chat with Work", active: false, discardable: false });
});

test("clicks in a background tab, and refuses a click said not to submit that would", async () => {
  const tab = (await setup.daemon.call("tabs")).result.tabs[0].tab;
  const text = await readTab(tab);

  const more = refOf(text, /button "Show more"/);
  const clicked = await setup.daemon.call("click", { tab, ref: more, submits: false });
  expect(clicked.error).toBeUndefined();
  expect(await readTab(tab)).toContain("More text here");

  const send = refOf(text, /button "Send message"/);
  const refused = await setup.daemon.call("click", { tab, ref: send, submits: false });
  expect(refused.error?.code).toBe("would_submit");
  const search = refOf(text, /button "Search"/);
  expect((await setup.daemon.call("click", { tab, ref: search, submits: false })).error?.code).toBe("would_submit");

  const preview = await setup.daemon.call("click", { tab, ref: send, dry_run: true });
  expect(preview.result.dry_run).toBe(true);
  expect(preview.result.submits).toBe(true);
  expect(preview.result.reason).toContain("Send message");
  expect(preview.result.target).toMatchObject({ role: "button", name: "Send message" });
  expect(preview.result.image.mime_type).toBe("image/jpeg");
  if (process.env.SAVE_PREVIEW) {
    (await import("node:fs")).writeFileSync(process.env.SAVE_PREVIEW, Buffer.from(preview.result.image.data, "base64"));
  }
  // The dry run changed nothing.
  const title = await inWorker(setup.context, async (id) => (await chrome.tabs.get(id as number)).title, tab);
  expect(title).toBe("Inbox");

  const hidden = refOf(text, /button "Hidden"/);
  const covered = await setup.daemon.call("click", { tab, ref: hidden, submits: false });
  expect(covered.error?.code).toBe("page_error");
  expect(covered.error?.message).toContain("drawn over");
});

test("chooses options and types, never into passwords", async () => {
  const tab = (await setup.daemon.call("tabs")).result.tabs[0].tab;
  const text = await readTab(tab);

  const size = refOf(text, /combobox "Size"/);
  const missing = await setup.daemon.call("select", { tab, ref: size, values: ["Huge"] });
  expect(missing.error?.message).toContain("Small, Medium, Large");
  const chose = await setup.daemon.call("select", { tab, ref: size, values: ["Large"] });
  expect(chose.result.chosen).toEqual(["Large"]);
  expect(await readTab(tab)).toMatch(/combobox "Size".*: Large/);

  const password = refOf(text, /textbox "Password"/);
  const secret = await setup.daemon.call("type", { tab, ref: password, text: "hunter2" });
  expect(secret.error?.code).toBe("sensitive_field");

  const query = refOf(text, /textbox "Query"/);
  const typed = await setup.daemon.call("type", { tab, ref: query, text: "falcon", submit: true });
  expect(typed.error).toBeUndefined();
  expect(typed.result.url).toBe(`${allowed}/search?q=falcon&pw=`);
  expect(typed.result.title).toBe("Results");

  const back = await setup.daemon.call("back", { tab });
  expect(back.result.title).toBe("Inbox");
});

test("a click that leaves the allowed sites says only where it went", async () => {
  const tab = (await setup.daemon.call("tabs")).result.tabs[0].tab;
  const away = refOf(await readTab(tab), /link "Elsewhere"/);
  const reply = await setup.daemon.call("click", { tab, ref: away, submits: false });
  expect(reply.error).toBeUndefined();
  expect(reply.result.allowed).toBe(false);
  expect(reply.result.site).toBe(new URL(other).host);
  expect(JSON.stringify(reply)).not.toContain("Private");
  expect((await setup.daemon.call("read", { tab })).error?.code).toBe("site_not_allowed");
  await setup.daemon.call("back", { tab });
});

test("fetches files from allowed sites for the daemon to save", async () => {
  const tab = (await setup.daemon.call("tabs")).result.tabs[0].tab;
  const link = refOf(await readTab(tab), /link "Q3 report"/);
  const dry = await setup.daemon.call("fetch", { tab, ref: link, dry_run: true });
  expect(dry.result).toEqual({ dry_run: true, url: `${allowed}/report.csv`, name: "report.csv" });
  const file = await setup.daemon.call("fetch", { tab, ref: link });
  expect(Buffer.from(file.result.data, "base64").toString()).toBe("quarter,revenue\nQ3,100\n");
  expect(file.result.size).toBe(23);
  expect((await setup.daemon.call("fetch", { url: `${other}/` })).error?.code).toBe("site_not_allowed");
});

test("closes its tabs", async () => {
  const tab = (await setup.daemon.call("tabs")).result.tabs[0].tab;
  expect((await setup.daemon.call("close", { tab })).result.closed).toBe(true);
  expect((await setup.daemon.call("tabs")).result.tabs).toEqual([]);
});
