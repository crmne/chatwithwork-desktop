// Downloads a page starts by itself while the extension acts don't happen.
// Playwright sets its own download behavior for the whole browser, so this
// runs Chromium without it, driven only through the extension.
import { chromium, expect, test } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EXTENSION_ID, FakeDaemon, cwwBinary, extensionDir, serve } from "./harness.js";

test("a download the page starts by itself doesn't happen", async () => {
  const hardwareCheck = process.env.CWW_BROWSER_GPU_CHECK === "1";
  const site = await serve({
    "/": `<title>Files</title><a href="/report.csv" download="report.csv">Download it</a>
      ${hardwareCheck ? `<p id="gpu"></p><script>
        const gl = document.createElement("canvas").getContext("webgl");
        const ext = gl?.getExtension("WEBGL_debug_renderer_info");
        document.querySelector("#gpu").textContent = "Browser GPU: " +
          (ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : "unavailable");
      </script>` : ""}`,
    "/report.csv": "quarter,revenue\n",
  });
  const home = mkdtempSync(join(tmpdir(), "cww-e2e-"));
  const downloads = join(home, "downloads");
  const profile = join(home, "profile");
  mkdirSync(join(profile, "Default"), { recursive: true });
  mkdirSync(join(profile, "NativeMessagingHosts"), { recursive: true });
  mkdirSync(downloads);
  writeFileSync(
    join(profile, "Default", "Preferences"),
    JSON.stringify({ download: { default_directory: downloads, prompt_for_download: false } }),
  );
  const wrapper = join(home, "host.sh");
  writeFileSync(wrapper, `#!/bin/sh\nCWW_HOME='${home}' exec '${cwwBinary()}' "$@"\n`);
  chmodSync(wrapper, 0o755);
  writeFileSync(
    join(profile, "NativeMessagingHosts", "com.chatwithwork.desktop.json"),
    JSON.stringify({
      name: "com.chatwithwork.desktop",
      description: "Chat with Work Desktop",
      path: wrapper,
      type: "stdio",
      allowed_origins: [`chrome-extension://${EXTENSION_ID}/`],
    }),
  );
  const daemon = new FakeDaemon(join(home, "run", "cww.sock"), [site.origin]);
  await daemon.listen();
  let browser: ChildProcess | undefined;
  try {
    browser = spawn(
      chromium.executablePath(),
      [
        "--headless=new",
        // As Playwright launches it: CI's Ubuntu forbids the sandbox's
        // user namespaces.
        "--no-sandbox",
        `--user-data-dir=${profile}`,
        "--no-first-run",
        "--no-default-browser-check",
        ...(hardwareCheck ? ["--use-angle=gl-egl", "--use-gl=angle"] : []),
        `--disable-extensions-except=${extensionDir}`,
        `--load-extension=${extensionDir}`,
        "about:blank",
      ],
      { stdio: "ignore", env: { ...process.env, CWW_HOME: home } },
    );
    await daemon.attached();
    daemon.setPolicy({ changes: true });
    const opened = await daemon.call("open", { url: `${site.origin}/` });
    expect(opened.error).toBeUndefined();
    const text = (await daemon.call("read", { tab: opened.result.tab })).result.text as string;
    if (hardwareCheck) {
      const renderer = text.split("\n").find((line) => line.includes("Browser GPU:"));
      expect(renderer).toContain("NVIDIA");
      expect(renderer).not.toMatch(/SwiftShader|llvmpipe/i);
      console.log(renderer);
    }
    const ref = /link "Download it" \[ref=(e\d+)\]/.exec(text)![1];
    const clicked = await daemon.call("click", { tab: opened.result.tab, ref, submits: false });
    expect(clicked.error).toBeUndefined();
    await new Promise((r) => setTimeout(r, 2000));
    expect(readdirSync(downloads)).toEqual([]);
  } finally {
    if (browser && browser.exitCode === null && browser.signalCode === null) {
      const closed = once(browser, "close");
      browser.kill();
      await closed;
    }
    await daemon.close();
    site.server.close();
    // Chromium's children can finish profile writes after its parent exits.
    // Yield between retries for at most 5.5 seconds; permanent errors still fail.
    await rm(home, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
