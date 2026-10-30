import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

let setup: Setup;
let allowed: string;
let blocked: string;
let tab: number;
const servers: { close: () => void }[] = [];

function refOf(text: string, label: RegExp): string {
  const line = text.split("\n").find((item) => label.test(item));
  const ref = line && /\[ref=(e\d+)\]/.exec(line)?.[1];
  if (!ref) throw new Error(`Missing ${label} in ${text}`);
  return ref;
}

// Read-only popup state confirms the policy reached the extension; no sleeps
// stand in for delivery, and no fixture touches the person's browser profile.
async function policy(changes: Partial<Setup["daemon"]["policy"]>): Promise<void> {
  setup.daemon.setPolicy(changes);
  const popup = setup.context.pages().find((p) => p.url().includes("popup.html"))!;
  await expect.poll(() => popup.evaluate(async () => chrome.runtime.sendMessage({ type: "status" })))
    .toMatchObject(changes);
}

test.beforeAll(async () => {
  const b = await serve({ "/private": "<title>Private title</title><h1>Secret body</h1>" });
  blocked = b.origin;
  const a = await serve({
    "/": `<title>Allowed fixture</title><h1>Public body</h1>
      <a href="${blocked}/private">Elsewhere</a><a href="/redirect">Redirect</a>
      <label>Password <input type="password" value="secret-password"></label>
      <button type="button" onclick="document.title='sent'">Send message</button>`,
    "/redirect": `<meta http-equiv="refresh" content="0;url=${blocked}/private">`,
  });
  servers.push(a.server, b.server);
  allowed = a.origin;
  setup = await launch([]);
  await setup.daemon.attached();
  const popup = await setup.context.newPage();
  await popup.goto("chrome-extension://ehjfihpogoacjodjnanclodinjnnpmcp/popup.html");
  await setup.daemon.waitForEvent((event) => event.event === "hello" && event.site_policy_version === 2);
});

test.afterAll(async () => {
  await setup?.cleanup();
  for (const server of servers) server.close();
});

test("empty listed mode denies access; explicit all-sites permits unlisted web pages only", async () => {
  await policy({ changes: true });
  expect((await setup.daemon.call("open", { url: `${allowed}/` })).error?.code).toBe("site_not_allowed");
  await policy({ all_sites: true });
  const opened = await setup.daemon.call("open", { url: `${allowed}/` });
  expect(opened.error).toBeUndefined();
  tab = opened.result.tab;
  expect((await setup.daemon.call("read", { tab })).result.text).toContain("Public body");
  for (const url of ["file:///etc/passwd", "chrome://settings", "data:text/html,private", "https://user:password@example.com/"]) {
    expect((await setup.daemon.call("open", { url })).error?.code).toBe("site_not_allowed");
  }
  const outside = await inWorker(setup.context, async (url) => (await chrome.tabs.create({ url, active: false })).id, `${allowed}/`);
  expect((await setup.daemon.call("read", { tab: outside })).error?.code).toBe("not_in_group");
});

test("blocks win over listed and all-sites modes, including pages already open", async () => {
  const privateTab = (await setup.daemon.call("open", { url: `${blocked}/private` })).result.tab;
  for (const all_sites of [true, false]) {
    await policy({ all_sites, sites: [allowed, blocked], blocked_sites: [blocked] });
    for (const [method, params] of [
      ["open", { url: `${blocked}/private` }], ["read", { tab: privateTab }],
      ["screenshot", { tab: privateTab }], ["fetch", { url: `${blocked}/private` }],
    ] as const) {
      const refused = await setup.daemon.call(method, params);
      expect(refused.error?.code).toBe("site_not_allowed");
      expect(JSON.stringify(refused)).not.toContain("Secret body");
    }
    const listed = (await setup.daemon.call("tabs")).result.tabs.find((t: any) => t.tab === privateTab);
    expect(listed).toMatchObject({ allowed: false });
    expect(listed).not.toHaveProperty("url");
    expect(listed).not.toHaveProperty("title");
  }
  await policy({ all_sites: true });
});

test("clicks and redirects to blocked pages reveal no destination content", async () => {
  for (const label of [/link "Elsewhere"/, /link "Redirect"/]) {
    await setup.daemon.call("open", { tab, url: `${allowed}/` });
    const text = (await setup.daemon.call("read", { tab })).result.text;
    const result = await setup.daemon.call("click", { tab, ref: refOf(text, label), submits: false });
    expect(result.error).toBeUndefined();
    expect(result.result.allowed).toBe(false);
    expect(result.result).not.toHaveProperty("url");
    expect(JSON.stringify(result)).not.toMatch(/Private title|Secret body|\/private/);
    expect((await setup.daemon.call("read", { tab })).error?.code).toBe("site_not_allowed");
    expect((await setup.daemon.call("screenshot", { tab })).error?.code).toBe("site_not_allowed");
  }
});

test("all-sites keeps protected fields, write checks and both Stop controls", async () => {
  await setup.daemon.call("open", { tab, url: `${allowed}/` });
  const text = (await setup.daemon.call("read", { tab })).result.text;
  expect(text).not.toContain("secret-password");
  expect((await setup.daemon.call("type", { tab, ref: refOf(text, /textbox "Password"/), text: "never" })).error?.code).toBe("sensitive_field");
  expect((await setup.daemon.call("click", { tab, ref: refOf(text, /button "Send message"/), submits: false })).error?.code).toBe("would_submit");
  await policy({ changes: false });
  expect((await setup.daemon.call("open", { url: `${allowed}/` })).error?.code).toBe("not_writable");
  setup.daemon.setPolicy({ stopped: true });
  await expect.poll(async () => (await setup.daemon.call("read", { tab })).error?.code).toBe("browser_stopped");
  setup.daemon.setPolicy({ stopped: false });
  await expect.poll(async () => (await setup.daemon.call("read", { tab })).error).toBeUndefined();
  const popup = setup.context.pages().find((p) => p.url().includes("popup.html"))!;
  await popup.evaluate(() => chrome.runtime.sendMessage({ type: "stop" }));
  setup.daemon.setPolicy({ all_sites: false });
  setup.daemon.setPolicy({ all_sites: true });
  expect((await setup.daemon.call("read", { tab })).error?.code).toBe("browser_stopped");
  await popup.evaluate(() => chrome.runtime.sendMessage({ type: "resume" }));
  expect((await setup.daemon.call("read", { tab })).error).toBeUndefined();
});
