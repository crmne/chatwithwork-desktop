// The real daemon: the extension's host attaches to `cww daemon run` (in
// its sandbox), the daemon reports the browser, and its kill switch
// reaches the extension.
import { expect, test } from "@playwright/test";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EXTENSION_ID, cwwBinary, launchBrowser } from "./harness.js";

let home: string;
let daemon: ChildProcess;

function cww(...args: string[]): string {
  return execFileSync(cwwBinary(), args, { env: { ...process.env, CWW_HOME: home }, encoding: "utf8" });
}

async function until<T>(check: () => T | undefined, what: string): Promise<T> {
  for (let i = 0; i < 100; i++) {
    const value = check();
    if (value !== undefined) return value;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`never: ${what}`);
}

test.beforeAll(async () => {
  home = mkdtempSync(join(tmpdir(), "cww-e2e-"));
  mkdirSync(join(home, "config"), { recursive: true });
  daemon = spawn(cwwBinary(), ["daemon", "run"], {
    env: { ...process.env, CWW_HOME: home, RUST_LOG: "warn" },
    stdio: "ignore",
  });
  await until(() => {
    try {
      return cww("status", "--json") ? true : undefined;
    } catch {
      return undefined;
    }
  }, "the daemon answers");
});

test.afterAll(async () => {
  daemon?.kill();
  rmSync(home, { recursive: true, force: true });
});

test("the daemon serves the extension, and its kill switch reaches it", async () => {
  cww("browser", "allow", "github.com");
  const context = await launchBrowser(home);
  try {
    const status = await until(() => {
      const value = JSON.parse(cww("browser", "status", "--json"));
      return value.connected ? value : undefined;
    }, "the browser connects");
    expect(status.extension.origin).toBe(`chrome-extension://${EXTENSION_ID}/`);
    expect(status.extension.browser).toMatch(/^Chrome \d+/);
    expect(status.sites).toEqual(["github.com"]);

    const popup = await context.newPage();
    await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
    await expect(popup.locator("#state-text")).toHaveText("Ready");
    await expect(popup.locator("#sites li")).toHaveText(["github.com"]);

    cww("browser", "stop");
    await popup.reload();
    await expect(popup.locator("#state-text")).toHaveText("Stopped on this computer");
    cww("browser", "resume");
    await popup.reload();
    await expect(popup.locator("#state-text")).toHaveText("Ready");

    // Stop in the extension shows in the daemon's status.
    await popup.click("#stop");
    await until(() => (JSON.parse(cww("browser", "status", "--json")).extension?.stopped ? true : undefined), "stopped shows");
    await popup.click("#resume");
  } finally {
    await context.close();
  }
});
