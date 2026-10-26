// Not a test: pictures of the extension's menu for review (SHOTS=dir).
import { test } from "@playwright/test";
import { EXTENSION_ID, launch } from "./harness.js";

test.skip(!process.env.SHOTS, "only when SHOTS names a folder");

test("menu pictures", async () => {
  const setup = await launch(["github.com", "linear.app", "http://localhost:3000"]);
  try {
    await setup.daemon.attached();
    for (const scheme of ["light", "dark"] as const) {
      const page = await setup.context.newPage();
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width: 320, height: 470 });
      await page.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
      await page.waitForSelector(".state--positive");
      await page.screenshot({ path: `${process.env.SHOTS}/menu-ready-${scheme}.png` });
      await page.click("#stop");
      await page.waitForSelector(".state--negative");
      await page.screenshot({ path: `${process.env.SHOTS}/menu-stopped-${scheme}.png` });
      await page.click("#resume");
      await page.close();
    }
    await setup.daemon.close();
    const page = await setup.context.newPage();
    await page.setViewportSize({ width: 320, height: 470 });
    await page.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
    await page.waitForSelector(".state--attention", { timeout: 10_000 });
    await page.screenshot({ path: `${process.env.SHOTS}/menu-not-running-light.png` });
  } finally {
    await setup.cleanup();
  }
});
