import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

// Review findings on #227: a tab that stops answering must not outlive Stop,
// must fail fast even while its attachment is still pending, and must not
// keep a screenshot's blur when the screenshot timed out.
const PAGE = `<!doctype html><title>Sign in</title>
<label>Email <input id="email" value="person@example.com"></label>
<label>Password <input id="pw" type="password" value="hunter2"></label>`;

let setup: Setup, origin: string, server: { close(): void };
test.beforeAll(async () => {
  ({ origin, server } = await serve({ "/": PAGE }));
  setup = await launch([origin]);
  await setup.daemon.attached();
});
test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  await inWorker(setup.context, () => {
    const spy = (globalThis as any).stallSpy;
    if (spy) { chrome.debugger.attach = spy.attach; chrome.debugger.sendCommand = spy.sendCommand; delete (globalThis as any).stallSpy; }
  });
  setup.daemon.setPolicy({ stopped: false, changes: false, sites: [origin] });
  for (const page of setup.context.pages()) if (page.url().startsWith(origin)) await page.close();
});

async function openTab(): Promise<number> {
  setup.daemon.setPolicy({ stopped: false, changes: true, sites: [origin] });
  // Opening uses tabs only, so the debugger isn't attached yet.
  const opened = await setup.daemon.call("open", { url: origin });
  expect(opened.error).toBeUndefined();
  return opened.result.tab;
}

// Whether the extension still holds the debugger on the tab: attaching again
// from the extension fails only then (Playwright's own connection doesn't count).
const attachedTo = (tab: number) =>
  inWorker(setup.context, async (id) => {
    const attach = (globalThis as any).stallSpy?.attach ?? chrome.debugger.attach.bind(chrome.debugger);
    try {
      await attach({ tabId: id }, "1.3");
    } catch (error) {
      return /already attached/i.test(String((error as Error)?.message ?? error));
    }
    await chrome.debugger.detach({ tabId: id }).catch(() => {});
    return false;
  }, tab);

test("a stalled attachment fails fast, and one that completes after Stop detaches itself", async () => {
  test.setTimeout(90_000);
  const tab = await openTab();
  // The browser takes 12 seconds to attach, longer than the extension waits.
  await inWorker(setup.context, (id) => {
    const spy = { attach: chrome.debugger.attach.bind(chrome.debugger), sendCommand: chrome.debugger.sendCommand.bind(chrome.debugger) };
    (globalThis as any).stallSpy = spy;
    chrome.debugger.attach = (async (target: any, version: string) => {
      if (target.tabId === id) await new Promise((done) => setTimeout(done, 12_000));
      return spy.attach(target, version);
    }) as typeof chrome.debugger.attach;
  }, tab);
  let started = Date.now();
  const first = await setup.daemon.call("read", { tab });
  expect(first.error?.message).toContain("isn't responding");
  // One attachment wait, not one per command.
  expect(Date.now() - started).toBeLessThan(12_000);
  started = Date.now();
  const second = await setup.daemon.call("read", { tab });
  expect(second.error?.message).toContain("isn't responding");
  expect(Date.now() - started).toBeLessThan(1_500);
  // Stop before the late attachment completes; it must not stay attached.
  setup.daemon.setPolicy({ stopped: true });
  await new Promise((done) => setTimeout(done, 5_000));
  expect(await attachedTo(tab)).toBe(false);
});

test("a screenshot that times out still takes its blur off once the page answers", async () => {
  test.setTimeout(90_000);
  const tab = await openTab();
  const page = setup.context.pages().find((p) => p.url() === `${origin}/`)!;
  expect((await setup.daemon.call("read", { tab })).error).toBeUndefined();
  // The capture never answers, as on a page that hangs mid-screenshot.
  await inWorker(setup.context, (id) => {
    const spy = { attach: chrome.debugger.attach.bind(chrome.debugger), sendCommand: chrome.debugger.sendCommand.bind(chrome.debugger) };
    (globalThis as any).stallSpy = spy;
    chrome.debugger.sendCommand = ((target: any, method: string, params?: any) => {
      if (target.tabId === id && method === "Page.captureScreenshot") return new Promise(() => {});
      return spy.sendCommand(target, method, params);
    }) as typeof chrome.debugger.sendCommand;
  }, tab);
  const shot = await setup.daemon.call("screenshot", { tab });
  expect(shot.error?.message).toContain("isn't responding");
  await inWorker(setup.context, () => {
    const spy = (globalThis as any).stallSpy;
    chrome.debugger.sendCommand = spy.sendCommand;
  });
  // The page answers again: the fields it blurred are put back by themselves.
  await expect.poll(() => page.locator("#pw").evaluate((el) => (el as HTMLElement).style.filter), { timeout: 20_000 }).toBe("");
  expect(await page.locator("[data-chat-with-work-blur]").count()).toBe(0);
});

test("a click guard's cleanup that waits for a stalled tab doesn't attach again after Stop (#227)", async () => {
  test.setTimeout(90_000);
  const tab = await openTab();
  const read = await setup.daemon.call("read", { tab });
  expect(read.error).toBeUndefined();
  const ref = /\[ref=(e\d+)\]/.exec(read.result.text.split("\n").find((line: string) => line.includes('textbox "Email"')) ?? "")?.[1];
  expect(ref).toBeTruthy();
  // The click's mouse events never answer, so its guard's cleanup waits for
  // the tab; every attachment after Stop is counted.
  await inWorker(setup.context, (id) => {
    const spy = { attach: chrome.debugger.attach.bind(chrome.debugger), sendCommand: chrome.debugger.sendCommand.bind(chrome.debugger), attachesAfterStop: 0, stopped: false };
    (globalThis as any).stallSpy = spy;
    chrome.debugger.attach = (async (target: any, version: string) => {
      if (target.tabId === id && spy.stopped) spy.attachesAfterStop++;
      return spy.attach(target, version);
    }) as typeof chrome.debugger.attach;
    chrome.debugger.sendCommand = ((target: any, method: string, params?: any) => {
      if (target.tabId === id && method === "Input.dispatchMouseEvent") return new Promise(() => {});
      return spy.sendCommand(target, method, params);
    }) as typeof chrome.debugger.sendCommand;
  }, tab);
  const click = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(click.error?.message).toContain("isn't responding");
  await inWorker(setup.context, () => {
    const spy = (globalThis as any).stallSpy;
    spy.stopped = true;
    chrome.debugger.sendCommand = spy.sendCommand;
  });
  setup.daemon.setPolicy({ stopped: true });
  // Long enough for the tab to be checked again several times.
  await new Promise((done) => setTimeout(done, 10_000));
  expect(await inWorker(setup.context, () => (globalThis as any).stallSpy.attachesAfterStop)).toBe(0);
  expect(await attachedTo(tab)).toBe(false);
});

test("a stalled screenshot's late cleanup leaves a later screenshot's blur on (#227)", async () => {
  test.setTimeout(90_000);
  const tab = await openTab();
  expect((await setup.daemon.call("read", { tab })).error).toBeUndefined();
  // The first capture never answers. The tab's "are you there" check is held
  // back until the second capture is under way, so the first screenshot's
  // cleanup runs in the middle of the second.
  await inWorker(setup.context, (id) => {
    const spy: any = { attach: chrome.debugger.attach.bind(chrome.debugger), sendCommand: chrome.debugger.sendCommand.bind(chrome.debugger), captures: 0, release: false };
    (globalThis as any).stallSpy = spy;
    chrome.debugger.sendCommand = (async (target: any, method: string, params?: any) => {
      if (target.tabId === id && method === "Runtime.evaluate" && params?.expression === "0" && !spy.release) throw new Error("held");
      if (target.tabId === id && method === "Page.captureScreenshot") {
        if (++spy.captures === 1) return new Promise(() => {});
        if (spy.captures === 2) {
          spy.release = true;
          await new Promise((done) => setTimeout(done, 5_000));
          const seen = await spy.sendCommand(target, "Runtime.evaluate", { expression: "document.getElementById('pw').style.filter", returnByValue: true });
          spy.filterAtCapture = seen.result.value;
        }
      }
      return spy.sendCommand(target, method, params);
    }) as typeof chrome.debugger.sendCommand;
  }, tab);
  const first = await setup.daemon.call("screenshot", { tab });
  expect(first.error?.message).toContain("isn't responding");
  // Past the few seconds a stalled tab fails at once.
  await new Promise((done) => setTimeout(done, 6_000));
  const second = await setup.daemon.call("screenshot", { tab });
  expect(second.error).toBeUndefined();
  expect(await inWorker(setup.context, () => (globalThis as any).stallSpy.filterAtCapture)).toContain("blur");
  // And both cleanups together leave nothing behind.
  const page = setup.context.pages().find((p) => p.url() === `${origin}/`)!;
  await expect.poll(() => page.locator("#pw").evaluate((el) => (el as HTMLElement).style.filter)).toBe("");
  expect(await page.locator("[data-chat-with-work-blur], [data-chat-with-work-capture]").count()).toBe(0);
});

test("a screenshot's cleanup shows the indicator only when no other capture holds the page (#227)", async () => {
  const tab = await openTab();
  const page = setup.context.pages().find((p) => p.url() === `${origin}/`)!;
  expect((await setup.daemon.call("read", { tab })).error).toBeUndefined();
  // Another capture holds the page while this screenshot is taken.
  await page.evaluate(() => {
    document.documentElement.setAttribute("data-chat-with-work-capture", "other");
  });
  expect((await setup.daemon.call("screenshot", { tab })).error).toBeUndefined();
  expect(await page.locator("#chat-with-work-indicator").evaluate((el) => (el as HTMLElement).style.display)).toBe("none");
  expect(await page.locator("html").getAttribute("data-chat-with-work-capture")).toBe("other");
  await page.evaluate(() => document.documentElement.removeAttribute("data-chat-with-work-capture"));
  expect((await setup.daemon.call("screenshot", { tab })).error).toBeUndefined();
  expect(await page.locator("#chat-with-work-indicator").evaluate((el) => (el as HTMLElement).style.display)).toBe("");
});
