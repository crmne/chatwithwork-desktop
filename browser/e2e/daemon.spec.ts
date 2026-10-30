// The real daemon: the extension's host attaches to `cww daemon run` (in
// its sandbox), the daemon reports the browser, and its kill switch
// reaches the extension.
import { expect, test } from "@playwright/test";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EXTENSION_ID, cwwBinary, launchBrowser, serve } from "./harness.js";

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

async function startDaemon(): Promise<void> {
  daemon = spawn(cwwBinary(), ["daemon", "run"], {
    env: { ...process.env, CWW_HOME: home, RUST_LOG: "warn" },
    stdio: "ignore",
  });
  await until(() => {
    try {
      // `status` answers even with no daemon; wait until one does.
      return JSON.parse(cww("status", "--json")).running === false ? undefined : true;
    } catch {
      return undefined;
    }
  }, "the daemon answers");
}

async function stopDaemon(): Promise<void> {
  if (daemon.exitCode !== null || daemon.signalCode !== null) return;
  const exited = new Promise<void>((resolve) => daemon.once("exit", () => resolve()));
  cww("daemon", "stop");
  await exited;
}

test.beforeEach(async () => {
  home = mkdtempSync(join(tmpdir(), "cww-e2e-"));
  mkdirSync(join(home, "config"), { recursive: true });
  // Only this throwaway daemon. Never register a host in the user's profile.
  writeFileSync(join(home, "config", "config.toml"), "[browser]\nenabled = true\n");
  await startDaemon();
});

// Async so a browser call can load the HTTP fixture while the CLI waits.
function browserCall(tool: string, args: object = {}, approved = false): Promise<any> {
  return new Promise((resolve, reject) => {
    const command = ["browser", "call", tool, "--input", "-"];
    if (approved) command.push("--approve");
    const child = spawn(cwwBinary(), command, {
      env: { ...process.env, CWW_HOME: home }, stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "";
    let error = "";
    child.stdout.on("data", chunk => { output += chunk; });
    child.stderr.on("data", chunk => { error += chunk; });
    child.on("error", reject);
    child.on("close", code => {
      try { resolve({ ...JSON.parse(output), exitCode: code }); }
      catch { reject(new Error(error || output)); }
    });
    child.stdin.end(JSON.stringify(args));
  });
}

test("local CLI reads and approved actions use the real confined daemon and extension", async () => {
  const fixture = await serve({ "/": `<!doctype html><title>Local browser fixture</title>
    <h1>Local browser fixture</h1><p id="state">Unchanged</p>
    <button type="button" onclick="document.querySelector('#state').textContent='Updated'">Show update</button>
    <label>Password <input type="password" value="hidden-password"></label>` });
  cww("browser", "allow", fixture.origin);
  cww("browser", "allow-changes");
  const context = await launchBrowser(home);
  try {
    await until(() => JSON.parse(cww("browser", "status", "--json")).connected || undefined, "browser connects");
    const names = JSON.parse(cww("browser", "tools")).tools.map((t: any) => t.name);
    expect(names).toContain("browser_read");
    expect(names.every((name: string) => name.startsWith("browser_"))).toBe(true);
    const outside = await context.newPage();
    await outside.goto(fixture.origin);
    expect((await browserCall("browser_tabs")).structuredContent.tabs).toEqual([]);

    const denied = await browserCall("browser_open", { url: fixture.origin });
    expect(denied.structuredContent.error.code).toBe("needs_approval");
    expect(denied.exitCode).toBe(1);
    const opened = await browserCall("browser_open", { url: fixture.origin }, true);
    expect(opened.exitCode).toBe(0);
    const tab = opened.structuredContent.tab;
    const read = await browserCall("browser_read", { tab });
    expect(read.structuredContent.text).toContain("Local browser fixture");
    expect(read.structuredContent.text).not.toContain("hidden-password");
    const password = /textbox "Password"[^\n]*\[ref=(e\d+)\]/.exec(read.structuredContent.text)?.[1];
    expect(password).toBeTruthy();
    const protectedField = await browserCall("browser_type", { tab, ref: password, text: "blocked" }, true);
    expect(protectedField.structuredContent.error.code).toBe("sensitive_field");
    const forbidden = await browserCall("browser_open", { url: "https://not-allowed.example/" }, true);
    expect(forbidden.structuredContent.error.code).toBe("site_not_allowed");
    const ref = /button "Show update"[^\n]*\[ref=(e\d+)\]/.exec(read.structuredContent.text)?.[1];
    expect(ref).toBeTruthy();
    const preview = await browserCall("browser_click", { tab, ref, submits: false, dry_run: true });
    expect(preview.exitCode).toBe(0);
    const previewImage = preview.content.find((item: any) => item.type === "image");
    expect(previewImage.mimeType).toBe("image/jpeg");
    if (process.env.CWW_BROWSER_PREVIEW) {
      writeFileSync(process.env.CWW_BROWSER_PREVIEW, Buffer.from(previewImage.data, "base64"));
    }
    expect((await browserCall("browser_read", { tab })).structuredContent.text).toContain("Unchanged");
    const action = await browserCall("browser_click", { tab, ref, submits: false }, true);
    expect(action.exitCode).toBe(0);
    expect((await browserCall("browser_read", { tab })).structuredContent.text).toContain("Updated");
    expect(await outside.locator("#state").textContent()).toBe("Unchanged");
    cww("browser", "stop");
    expect((await browserCall("browser_read", { tab })).structuredContent.error.code).toBe("browser_stopped");
    cww("browser", "resume");
  } finally {
    await context.close();
    fixture.server.close();
  }
});

test.afterEach(async () => {
  await stopDaemon();
  rmSync(home, { recursive: true, force: true });
});

test("daemon-only restart keeps browser identity and the extension's Stop state", async () => {
  const context = await launchBrowser(home);
  try {
    const first = await until(() => {
      const status = JSON.parse(cww("browser", "status", "--json"));
      return status.extension?.version ? status : undefined;
    }, "initial browser identity");
    const nativeHost = readFileSync(join(home, "native-host-starts"), "utf8");
    const popup = await context.newPage();
    await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
    await popup.click("#stop");
    await until(() => JSON.parse(cww("browser", "status", "--json")).extension?.stopped || undefined, "extension is stopped");

    await stopDaemon();
    await startDaemon();
    const restored = await until(() => {
      const status = JSON.parse(cww("browser", "status", "--json"));
      return status.extension?.version ? status : undefined;
    }, "browser identity after daemon restart");
    expect(restored.extension.browser).toBe(first.extension.browser);
    expect(restored.extension.version).toBe(first.extension.version);
    expect(restored.extension.stopped).toBe(true);
    expect(readFileSync(join(home, "native-host-starts"), "utf8")).toBe(nativeHost);
    expect((await browserCall("browser_tabs")).structuredContent.error.code).toBe("browser_stopped");
    await popup.click("#resume");
    await until(() => JSON.parse(cww("browser", "status", "--json")).extension?.stopped === false || undefined, "extension resumes");
    expect((await browserCall("browser_tabs")).exitCode).toBe(0);
  } finally {
    await context.close();
  }
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
