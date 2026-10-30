// Opt-in packaged path: real confined Rust daemon, pinned bundled Node,
// unchanged registry engine, real native messaging host and extension.
import { expect, test, type BrowserContext } from "@playwright/test";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { cwwBinary, inWorker, launchBrowser, serve } from "./harness.js";

test.skip(process.env.CWW_PACKAGED_ENGINE !== "1", "requires an immutable staged package beside CWW_BROWSER_HOST_BINARY");

let home: string;
let daemon: ChildProcess;
let context: BrowserContext;
let diagnostics = "";

function cww(...args: string[]): string {
  return execFileSync(cwwBinary(), args, {
    env: { ...process.env, CWW_HOME: home }, encoding: "utf8",
  });
}

async function browserCall(tool: string, args: object, approved = false): Promise<any> {
  return new Promise((resolve, reject) => {
    const command = ["browser", "call", tool, "--input", "-"];
    if (approved) command.push("--approve");
    const child = spawn(cwwBinary(), command, {
      env: { ...process.env, CWW_HOME: home }, stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "", error = "";
    child.stdout.on("data", chunk => { output += chunk; });
    child.stderr.on("data", chunk => { error += chunk; });
    child.once("error", reject);
    child.once("close", code => {
      try { resolve({ ...JSON.parse(output), exitCode: code }); }
      catch { reject(new Error(error || "Invalid CLI response")); }
    });
    child.stdin.end(JSON.stringify(args));
  });
}

test.beforeAll(async () => {
  home = mkdtempSync(join(tmpdir(), "cww-packaged-engine-"));
  mkdirSync(join(home, "config"), { recursive: true });
  writeFileSync(join(home, "config/config.toml"), "[browser]\nenabled = true\nchanges = true\n");
  daemon = spawn(cwwBinary(), ["daemon", "run"], {
    env: { ...process.env, CWW_HOME: home, CWW_LOG: "warn,cww::browsing::engine=debug" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  daemon.stdout?.on("data", chunk => { diagnostics += chunk; });
  daemon.stderr?.on("data", chunk => { diagnostics += chunk; });
  await expect.poll(() => diagnostics, { timeout: 50_000,
    message: "the real confined bundled engine must report ready, not silently select legacy" })
    .toContain("bundled browser engine is ready");
  const status = JSON.parse(cww("status", "--json"));
  expect(status.sandbox.state).toBe("enforced");
  context = await launchBrowser(home);
  await expect.poll(() => JSON.parse(cww("browser", "status", "--json")).connected).toBe(true);
});

test.afterAll(async () => {
  await context?.close();
  if (daemon && daemon.exitCode === null && daemon.signalCode === null) {
    const exited = new Promise<void>(resolve => daemon.once("exit", () => resolve()));
    try { cww("daemon", "stop"); } catch { daemon.kill("SIGTERM"); }
    await exited;
  }
  if (home) rmSync(home, { recursive: true, force: true });
});

test("packaged click, type and select use the confined full engine without focusing another tab", async () => {
  const fixture = await serve({ "/": `<!doctype html><title>Packaged engine fixture</title>
    <button type="button" id="button" onclick="document.querySelector('#state').textContent='Updated'">Show update</button>
    <input id="message" aria-label="Message" value="Original">
    <select id="colour" aria-label="Colour"><option value="red">Red</option><option value="blue">Blue</option></select>
    <input id="password" aria-label="Password" type="password" value="fixture-secret">
    <p id="state">Unchanged</p>` });
  try {
    cww("browser", "allow", fixture.origin);
    const outside = await context.newPage();
    await outside.goto(fixture.origin);
    const opened = await browserCall("browser_open", { url: fixture.origin }, true);
    expect(opened.exitCode).toBe(0);
    const tab = opened.structuredContent.tab;
    const read = await browserCall("browser_read", { tab });
    expect(read.structuredContent.text).not.toContain("fixture-secret");
    const ref = (name: string) => {
      const line = read.structuredContent.text.split("\n").find((line: string) => line.includes(`"${name}"`));
      const value = /\[ref=(e\d+)\]/.exec(line ?? "")?.[1];
      expect(value, name).toBeTruthy(); return value;
    };
    const active = await inWorker(context, async () => (await chrome.tabs.query({ active: true })).map(tab => tab.id));
    const before = (diagnostics.match(/using bundled browser engine/g) ?? []).length;
    const preview = await browserCall("browser_click", { tab, ref: ref("Show update"), submits: false, dry_run: true });
    expect(preview.exitCode).toBe(0);
    expect(preview.content.some((item: any) => item.type === "image")).toBe(true);
    expect((await browserCall("browser_read", { tab })).structuredContent.text).toContain("Unchanged");
    // Default selection still resolves only the owned group, even though an
    // unrelated tab is active. The Rust wrapper must accept that resolved ID.
    const defaultPreview = await browserCall("browser_click", { ref: ref("Show update"), submits: false, dry_run: true });
    expect(defaultPreview.structuredContent.error, diagnostics).toBeUndefined();
    expect((await browserCall("browser_click", { tab, ref: ref("Show update"), submits: false }, true)).exitCode).toBe(0);
    const typed = await browserCall("browser_type", { tab, ref: ref("Message"), text: "Packaged value" }, true);
    expect(typed.structuredContent.error, diagnostics).toBeUndefined(); expect(typed.exitCode).toBe(0);
    const selected = await browserCall("browser_select", { tab, ref: ref("Colour"), values: ["Blue"] }, true);
    expect(selected.structuredContent.error).toBeUndefined(); expect(selected.exitCode).toBe(0);
    const after = await browserCall("browser_read", { tab });
    expect(after.structuredContent.text).toContain("Updated");
    expect(after.structuredContent.text).toContain("Packaged value");
    expect(after.structuredContent.text).toContain("Blue");
    expect((diagnostics.match(/using bundled browser engine/g) ?? []).length - before).toBeGreaterThanOrEqual(4);
    expect(await inWorker(context, async () => (await chrome.tabs.query({ active: true })).map(tab => tab.id))).toEqual(active);
    await expect(outside.locator("#state")).toHaveText("Unchanged");
    const refused = await browserCall("browser_type", { tab, ref: ref("Password"), text: "blocked" }, true);
    expect(refused.structuredContent.error.code).toBe("sensitive_field");
  } finally { fixture.server.close(); }
});
