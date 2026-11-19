import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EXTENSION_ID, inWorker, launch, serve, type Setup } from "./harness.js";

// Uploads as the daemon asks for them: each file by the real path it
// checked, with its name, size and type. The page reports what it got.
const REPORT = `<pre id="out"></pre><script>
function show(files){ out.textContent += [...files].map(f => f.name + ":" + f.size).join(",") + "\\n"; }
</script>`;

const PAGES: Record<string, string> = {
  "/plain": `<!doctype html><title>Plain</title><label>Logo <input id="f" type="file" accept=".png,image/*" onchange="show(this.files)"></label>${REPORT}`,
  "/multiple": `<!doctype html><title>Multiple</title><label>Screenshots <input type="file" multiple accept="image/*" onchange="show(this.files)"></label>${REPORT}`,
  "/hidden": `<!doctype html><title>Hidden</title><div class="uploader"><button type="button" onclick="this.nextElementSibling.click()">Add screenshot</button><input type="file" hidden accept=".png" onchange="show(this.files)"></div>${REPORT}`,
  "/dynamic": `<!doctype html><title>Dynamic</title><button type="button" id="b">Upload bundle</button>${REPORT}<script>
b.onclick = () => { const i = document.createElement("input"); i.type = "file"; i.accept = ".aab"; i.onchange = () => show(i.files); i.click(); };
</script>`,
  "/fsa": `<!doctype html><title>Picker</title><button type="button" id="b">Upload file</button>${REPORT}<script>
b.onclick = () => showOpenFilePicker().then(async ([h]) => show([await h.getFile()])).catch(e => out.textContent += "ERR " + e.name);
</script>`,
  "/drop": `<!doctype html><title>Drop</title><div id="z" style="width:320px;height:200px;border:2px dashed" ondragover="event.preventDefault()" ondrop="event.preventDefault(); show(event.dataTransfer.files); z.textContent = 'Received ' + event.dataTransfer.files[0].name">Drag and drop your package here</div>${REPORT}`,
  "/frame": `<!doctype html><title>Frame</title><iframe src="/plain" width="500" height="200"></iframe>`,
  "/shadow": `<!doctype html><title>Shadow</title><x-uploader></x-uploader>${REPORT}<script>
customElements.define("x-uploader", class extends HTMLElement { connectedCallback() {
  const root = this.attachShadow({ mode: "closed" });
  root.innerHTML = '<input type="file" aria-label="Avatar">';
  root.querySelector("input").onchange = (e) => show(e.target.files);
} });
</script>`,
  "/password": `<!doctype html><title>Password</title><label>Password <input type="password"></label>`,
};

let setup: Setup;
let server: { close: () => void };
let origin: string;
let files: Record<string, { path: string; name: string; size: number; mime_type: string }>;

test.beforeAll(async () => {
  ({ server, origin } = await serve(PAGES));
  setup = await launch([origin]);
  const shared = join(setup.home, "shared");
  mkdirSync(shared);
  const make = (name: string, bytes: Buffer, mime_type: string) => {
    const path = join(shared, name);
    writeFileSync(path, bytes);
    return { path, name, size: bytes.length, mime_type };
  };
  files = {
    png: make("listing.png", Buffer.from("89504e470d0a1a0a00000000", "hex"), "image/png"),
    png2: make("listing-2.png", Buffer.from("89504e470d0a1a0a11111111", "hex"), "image/png"),
    txt: make("notes.txt", Buffer.from("hello"), "text/plain"),
    aab: make("app-release.aab", Buffer.alloc(15 * 1024 * 1024, 7), "application/octet-stream"),
  };
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});

test.afterAll(async () => { await setup?.cleanup(); server?.close(); });
test.afterEach(async () => {
  for (const page of setup.context.pages()) if (page.url().startsWith(origin)) await page.close();
});

async function openAndRead(path: string): Promise<{ tab: number; text: string }> {
  const opened = await setup.daemon.call("open", { url: `${origin}${path}` });
  expect(opened.error).toBeUndefined();
  const tab = opened.result.tab;
  // A busy machine can answer before the new tab's address commits.
  let read = await setup.daemon.call("read", { tab });
  for (let i = 0; i < 20 && read.error?.code === "site_not_allowed"; i++) {
    await new Promise((r) => setTimeout(r, 250));
    read = await setup.daemon.call("read", { tab });
  }
  expect(read.error).toBeUndefined();
  return { tab, text: read.result.text };
}

function refOf(text: string, needle: string): string {
  const line = text.split("\n").find((line) => line.includes(needle));
  expect(line, `${needle} in:\n${text}`).toBeTruthy();
  return /\[ref=(e\d+)\]/.exec(line!)![1];
}

// The page's own report, read through Playwright's page for the tab's URL.
async function reported(path: string): Promise<string> {
  const page = setup.context.pages().find((p) => p.url() === `${origin}${path}`)!;
  return page.evaluate(() => document.querySelector("#out")?.textContent ?? "");
}

test("browser_read marks file fields, their buttons, and fields the tree leaves out", async () => {
  const plain = await openAndRead("/plain");
  expect(plain.text).toMatch(/\[ref=e\d+\] \[upload accept="\.png,image\/\*"\]/);
  const hidden = await openAndRead("/hidden");
  expect(hidden.text).toMatch(/button "Add screenshot" \[ref=e\d+\] \[upload accept="\.png"\]/);
  expect(hidden.text).toMatch(/- upload field \[ref=e\d+\] \[upload accept="\.png"\]/);
  const dynamic = await openAndRead("/dynamic");
  expect(dynamic.text).toMatch(/button "Upload bundle" \[ref=e\d+\] \[upload\]/);
});

test("a dry run checks the field and the files, and uploads nothing", async () => {
  const { tab, text } = await openAndRead("/plain");
  const ref = refOf(text, "[upload");
  const dry = await setup.daemon.call("upload", { tab, ref, files: [files.png], dry_run: true });
  expect(dry.error).toBeUndefined();
  expect(dry.result).toMatchObject({ dry_run: true, field: { how: "field", label: "Logo", accept: ".png,image/*", multiple: false } });
  expect(dry.result.image?.data).toBeTruthy();
  expect(await reported("/plain")).toBe("");
});

test("uploads to a plain field and reports what the page holds", async () => {
  const { tab, text } = await openAndRead("/plain");
  const ref = refOf(text, "[upload");
  const done = await setup.daemon.call("upload", { tab, ref, files: [files.png] });
  expect(done.error).toBeUndefined();
  expect(done.result.chosen).toEqual([{ name: "listing.png", size: files.png.size }]);
  expect(JSON.stringify(done.result)).not.toContain(setup.home);
  expect(await reported("/plain")).toBe(`listing.png:${files.png.size}\n`);
});

test("refuses a file the field doesn't accept, and two files for a field that takes one", async () => {
  const { tab, text } = await openAndRead("/plain");
  const ref = refOf(text, "[upload");
  const wrong = await setup.daemon.call("upload", { tab, ref, files: [files.txt] });
  expect(wrong.error?.code).toBe("invalid_argument");
  expect(wrong.error?.message).toContain("the file (.txt) isn't a kind of file that field takes");
  expect(wrong.error?.message).not.toContain("notes");
  const two = await setup.daemon.call("upload", { tab, ref, files: [files.png, files.png2] });
  expect(two.error?.message).toContain("takes one file");
  expect(await reported("/plain")).toBe("");
});

test("several files to a field that takes several", async () => {
  const { tab, text } = await openAndRead("/multiple");
  const done = await setup.daemon.call("upload", { tab, ref: refOf(text, "[upload"), files: [files.png, files.png2] });
  expect(done.error).toBeUndefined();
  expect(await reported("/multiple")).toBe(`listing.png:${files.png.size},listing-2.png:${files.png2.size}\n`);
});

test("a hidden field behind the page's own button gets the file without a click", async () => {
  const { tab, text } = await openAndRead("/hidden");
  const done = await setup.daemon.call("upload", { tab, ref: refOf(text, 'button "Add screenshot"'), files: [files.png] });
  expect(done.error).toBeUndefined();
  expect(done.result.field.how).toBe("field");
  expect(await reported("/hidden")).toBe(`listing.png:${files.png.size}\n`);
});

test("a button that makes its field when pressed goes through the intercepted picker, 15 MB", async () => {
  const { tab, text } = await openAndRead("/dynamic");
  const ref = refOf(text, 'button "Upload bundle"');
  const dry = await setup.daemon.call("upload", { tab, ref, files: [files.aab], dry_run: true });
  expect(dry.result?.field?.how).toBe("picker");
  const done = await setup.daemon.call("upload", { tab, ref, files: [files.aab] });
  expect(done.error).toBeUndefined();
  expect(done.result.chosen).toEqual([{ name: "app-release.aab", size: 15 * 1024 * 1024 }]);
  expect(await reported("/dynamic")).toBe(`app-release.aab:${15 * 1024 * 1024}\n`);
});

test("refuses the browser's own file system picker", async () => {
  const { tab, text } = await openAndRead("/fsa");
  const done = await setup.daemon.call("upload", { tab, ref: refOf(text, 'button "Upload file"'), files: [files.png] });
  expect(done.error?.code).toBe("unsupported");
});

test("drops files on a zone with no field", async () => {
  const { tab, text } = await openAndRead("/drop");
  const ref = refOf(text, "drop zone");
  expect(text).toMatch(/drop zone "Drag and drop your package here" \[ref=e\d+\] \[upload drop\]/);
  // Without drop: true it isn't an upload field.
  const plain = await setup.daemon.call("upload", { tab, ref, files: [files.png] });
  expect(plain.error?.message).toContain("drop: true");
  const done = await setup.daemon.call("upload", { tab, ref, files: [files.png], drop: true });
  expect(done.error).toBeUndefined();
  expect(done.result.field.how).toBe("drop");
  expect(done.result.shows.join("\n")).toContain("Received listing.png");
  expect(await reported("/drop")).toBe(`listing.png:${files.png.size}\n`);
});

test("fields in a frame and in a closed shadow root", async () => {
  const frame = await openAndRead("/frame");
  const inFrame = await setup.daemon.call("upload", { tab: frame.tab, ref: refOf(frame.text, "upload field"), files: [files.png] });
  expect(inFrame.error).toBeUndefined();
  expect(inFrame.result.chosen).toEqual([{ name: "listing.png", size: files.png.size }]);
  const shadow = await openAndRead("/shadow");
  const inShadow = await setup.daemon.call("upload", { tab: shadow.tab, ref: refOf(shadow.text, "[upload"), files: [files.png] });
  expect(inShadow.error).toBeUndefined();
  expect(await reported("/shadow")).toBe(`listing.png:${files.png.size}\n`);
});

test("refuses what isn't an upload field, a password field, and paths it wasn't given whole", async () => {
  const { tab, text } = await openAndRead("/password");
  const ref = refOf(text, "textbox");
  const password = await setup.daemon.call("upload", { tab, ref, files: [files.png] });
  expect(password.error?.code).toBe("sensitive_field");
  const relative = await setup.daemon.call("upload", { tab, ref, files: [{ ...files.png, path: "shared/listing.png" }] });
  expect(relative.error?.code).toBe("invalid_argument");
});

test("without the browser's file access switch, says how to turn it on", async () => {
  // As a Chrome Web Store install starts: the switch off. Changing it
  // reloads the extension, which needs developer mode for an unpacked one.
  const page = await setup.context.newPage();
  await page.goto("chrome://extensions");
  await page.evaluate(async (id) => {
    await (chrome as any).developerPrivate.updateProfileConfiguration({ inDeveloperMode: true });
    await (chrome as any).developerPrivate.updateExtensionConfiguration({ extensionId: id, fileAccess: false });
  }, EXTENSION_ID);
  await page.close();
  // The reloaded extension reconnects and says so in its hello.
  await expect.poll(() => setup.daemon.events.filter((e) => e.event === "hello" && e.file_access === false).length, { timeout: 20_000 }).toBeGreaterThan(0);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
  const { tab, text } = await openAndRead("/plain");
  const done = await setup.daemon.call("upload", { tab, ref: refOf(text, "[upload"), files: [files.png], dry_run: true });
  expect(done.error?.code).toBe("file_access_off");
  expect(done.error?.message).toContain("chrome://extensions");
  expect(done.error?.message).toContain("Allow access to file URLs");
});
