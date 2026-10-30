import { expect, test } from "@playwright/test";
import { inWorker, launch, serve, type Setup } from "./harness.js";

let setup: Setup;
let origin: string;
let blocked: string;
const servers: { close: () => void }[] = [];

test.beforeAll(async () => {
  const privateSite = await serve({ "/private": "<title>Private title</title><h1>Secret body</h1>" });
  blocked = privateSite.origin;
  const site = await serve({
    "/": `<a href="/done">Same site</a><a href="${blocked}/private">Other site</a><a href="/redirect">Redirect</a>
      <button type="button" onclick="document.querySelector('p').textContent='Clicked'">Stay here</button><p>Unchanged</p>`,
    "/done": "<title>Destination</title><h1>Arrived</h1>",
    "/redirect": `<meta http-equiv="refresh" content="0;url=${blocked}/private">`,
  });
  origin = site.origin;
  servers.push(site.server, privateSite.server);
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});

test.afterEach(async () => {
  await inWorker(setup.context, () => {
    const state = globalThis as any;
    if (state.originalSend) chrome.debugger.sendCommand = state.originalSend;
    delete state.originalSend;
  });
});

test.afterAll(async () => {
  await setup?.cleanup();
  for (const server of servers) server.close();
});

async function fixture(label: string) {
  const opened = await setup.daemon.call("open", { url: `${origin}/` });
  expect(opened.error).toBeUndefined();
  const tab = opened.result.tab;
  const read = await setup.daemon.call("read", { tab });
  expect(read.error).toBeUndefined();
  const line = read.result.text.split("\n").find((line: string) => line.includes(`"${label}"`));
  const ref = /\[ref=(e\d+)\]/.exec(line ?? "")?.[1];
  expect(ref).toBeTruthy();
  return { tab, ref };
}

// Hold only the post-input guard read until the real navigation commits.
// The production code must handle the actual destroyed CDP context, without
// sending another press or evaluating anything in the destination document.
async function guardRead(tab: number, destination: string, failure?: string) {
  await inWorker(setup.context, ({ tab, destination, failure }) => {
    const state = globalThis as any;
    const original = chrome.debugger.sendCommand.bind(chrome.debugger);
    state.originalSend = original;
    state.presses = 0;
    state.guardReads = 0;
    state.missingContexts = 0;
    state.destinationWorlds = 0;
    chrome.debugger.sendCommand = (async (source: any, method: string, params: any) => {
      if (source.tabId !== tab) return original(source, method, params);
      if (method === "Input.dispatchMouseEvent" && params.type === "mousePressed") state.presses++;
      if (method === "Page.createIsolatedWorld" && (await chrome.tabs.get(tab)).url === destination) state.destinationWorlds++;
      if (method === "Runtime.evaluate" && params.expression?.includes("__cwwPointer_") && params.expression.includes("?.result()")) {
        state.guardReads++;
        if (destination) {
          const until = Date.now() + 5_000;
          while (true) {
            const current = await chrome.tabs.get(tab);
            if (current.url === destination && current.status === "complete") break;
            if (Date.now() >= until) throw new Error("fixture navigation did not commit");
            await new Promise((resolve) => setTimeout(resolve, 10));
          }
        }
        if (failure) throw new Error(failure);
        try {
          return await original(source, method, params);
        } catch (error) {
          if (String(error).includes("Cannot find context with specified id")) state.missingContexts++;
          throw error;
        }
      }
      return original(source, method, params);
    }) as any;
  }, { tab, destination, failure });
}

for (const [label, destination, allowed] of [
  ["Same site", "same", true], ["Other site", "blocked", false], ["Redirect", "blocked", false],
] as const) {
  test(`a ${label} navigation completes once after its old guard context disappears`, async () => {
    const { tab, ref } = await fixture(label);
    await guardRead(tab, destination === "same" ? `${origin}/done` : `${blocked}/private`);
    const result = await setup.daemon.call("click", { tab, ref, submits: false });
    expect(result.error).toBeUndefined();
    if (allowed) {
      expect(result.result.url).toBe(`${origin}/done`);
      expect((await setup.daemon.call("read", { tab })).result.text).toContain("Arrived");
    } else {
      expect(result.result.allowed).toBe(false);
      expect(result.result).not.toHaveProperty("url");
      expect(JSON.stringify(result)).not.toMatch(/Private title|Secret body|\/private/);
      expect((await setup.daemon.call("read", { tab })).error?.code).toBe("site_not_allowed");
    }
    const counts = await inWorker(setup.context, () => {
      const state = globalThis as any;
      return { presses: state.presses, reads: state.guardReads, worlds: state.destinationWorlds, missing: state.missingContexts };
    });
    expect(counts.presses).toBe(1);
    expect(counts.reads).toBe(1);
    // The allowed read above intentionally creates a destination world.
    if (!allowed) expect(counts.worlds).toBe(0);
    expect(counts.missing).toBe(1);
  });
}

test("a missing context without document replacement remains an error", async () => {
  const { tab, ref } = await fixture("Stay here");
  await guardRead(tab, "", '{"code":-32000,"message":"Cannot find context with specified id"}');
  const result = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(result.error?.code).toBe("page_error");
  expect(await inWorker(setup.context, () => (globalThis as any).presses)).toBe(1);
});

test("an unrelated runtime failure is not hidden by navigation", async () => {
  const { tab, ref } = await fixture("Same site");
  await guardRead(tab, `${origin}/done`, '{"code":-32000,"message":"Unrelated check failure"}');
  const result = await setup.daemon.call("click", { tab, ref, submits: false });
  expect(result.error?.code).toBe("page_error");
  expect(result.error?.message).toContain("Unrelated check failure");
  expect(await inWorker(setup.context, () => (globalThis as any).presses)).toBe(1);
});
