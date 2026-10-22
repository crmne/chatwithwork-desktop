// Renders the Chrome Web Store images (images/) from the captures of the
// extension's menu and of an approval card (captures/), with Playwright:
// node store/render.mjs <folder with geist-variable.woff2 and unbounded-variable.woff2>
// The fonts are Chat with Work's own (OFL), from the web app's repository.
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const fonts = process.argv[2];
const data = (path, type) => `data:${type};base64,${readFileSync(path).toString("base64")}`;
const capture = (name) => data(join(here, "captures", name), "image/png");
const logo = data(join(here, "..", "static", "icons", "logo.svg"), "image/svg+xml");

const style = `
  @font-face { font-family: Geist; src: url(${data(join(fonts, "geist-variable.woff2"), "font/woff2")}); font-weight: 100 900; }
  @font-face { font-family: Unbounded; src: url(${data(join(fonts, "unbounded-variable.woff2"), "font/woff2")}); font-weight: 200 900; }
  * { box-sizing: border-box; margin: 0; }
  body { width: 1280px; height: 800px; background: #0b0b0c; color: #f4f4f5; font-family: Geist, sans-serif;
    display: grid; grid-template-columns: 520px 1fr; align-items: center; gap: 56px; padding: 0 88px; overflow: hidden; }
  .brand { display: flex; align-items: center; gap: 12px; margin-bottom: 40px; font-weight: 600; font-size: 20px; }
  .brand img { width: 36px; height: 36px; }
  h1 { font-family: Unbounded; font-weight: 600; font-size: 44px; line-height: 1.12; letter-spacing: -0.02em; }
  p { margin-top: 22px; color: #a1a1aa; font-size: 21px; line-height: 1.5; }
  .shot { justify-self: center; border-radius: 14px; overflow: hidden; box-shadow: 0 0 0 1px #27272a, 0 30px 80px rgba(0,0,0,.6); }
  .shot img { display: block; }
`;

const slides = [
  { file: "1-tab-group.png", title: "Your browser, in its own tab group", text: "Chat with Work uses tabs only in its “Chat with Work” group, on sites you allow in Chat with Work Desktop. You keep using the rest.", image: capture("menu-ready-dark.png"), width: 380 },
  { file: "2-approval.png", title: "Every action asks you first", text: "Before it clicks, types or opens anything, you see what it will do. Anything that may send, buy, post or delete shows you a picture of it.", image: capture("approval-card-dark.png"), width: 560 },
  { file: "3-stop.png", title: "Stop it at any time", text: "Stop in the extension's menu or in Chat with Work Desktop ends it at once. It never reads or types passwords, card numbers or one-time codes.", image: capture("menu-stopped-dark.png"), width: 380 },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
for (const slide of slides) {
  await page.setContent(`<style>${style}</style>
    <div><div class="brand"><img src="${logo}" alt="">Chat with Work</div><h1>${slide.title}</h1><p>${slide.text}</p></div>
    <div class="shot"><img src="${slide.image}" width="${slide.width}" alt=""></div>`);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: join(here, "images", slide.file) });
}
await page.setViewportSize({ width: 440, height: 280 });
await page.setContent(`<style>${style}
  body { width: 440px; height: 280px; display: flex; flex-direction: column; justify-content: center; align-items: flex-start; padding: 0 36px; gap: 0; }
  h1 { font-size: 26px; margin-top: 18px; } img { width: 52px; height: 52px; }</style>
  <img src="${logo}" alt=""><h1>Chat with Work in your own browser</h1>`);
await page.evaluate(() => document.fonts.ready);
await page.screenshot({ path: join(here, "images", "promo-small-440x280.png") });
await browser.close();
