// Uploading: handing files from the person's shared folders to a page's
// upload field, as if they chose them in the file picker, only while the
// person allowed actions on the computer. The daemon checks every file
// first and sends only where each really is, inside a shared folder; the
// extension never opens a file, and never reads one back from the page,
// only its name and size, to check the page holds what was approved.
//
// Three ways in, by what `ref` is:
// - a file field (`<input type=file>`, hidden or not, in a shadow root or a
//   same-process frame), or a label or button that goes with one: the files
//   are set on the field itself, with no click;
// - a button that makes its field only when pressed: it's clicked with the
//   browser's file chooser intercepted, and the files go to the field that
//   asked. A picker opened through the File System Access API
//   (`showOpenFilePicker`) can't be answered by an extension, so it's refused;
// - `drop: true`: the files are dropped on the element, as if dragged there.
//
// Chrome hands files to pages for an extension only while its "Allow access
// to file URLs" switch is on; without it, the refusal says how to turn it on.

import { about, only, readableText, pageUploads, still, type Method, type Params, type Policy } from "./actions.js";
import { accepts, fileOrdinal, UPLOAD_WORDS } from "./accept.js";
import { allowedChanges, described, dryRun, flag, point, preview, settle, target, type Target } from "./changes.js";
import { cleanUp, nextEvent, send } from "./cdp.js";
import { isSensitiveField, submitReason } from "./classify.js";
import { ToolError } from "./errors.js";
import type { UploadMark } from "./upload-marks.js";

const MAX_FILES = 10;
const MAX_BYTES = 200 * 1024 * 1024;
const CHOOSER_WAIT_MS = 4_000;
const SHOWS_MAX_LINES = 12;
const SHOWS_MAX_CHARS = 2_000;

export const FILE_ACCESS_STEPS =
  "To allow it: open chrome://extensions, find Chat with Work, press Details, and turn on “Allow access to file URLs”.";

interface UploadFile {
  path: string;
  name: string;
  size: number;
  mime_type: string;
}

export async function fileAccess(): Promise<boolean> {
  try {
    return await chrome.extension.isAllowedFileSchemeAccess();
  } catch {
    return false;
  }
}

function fileAccessOff(): ToolError {
  return new ToolError(
    "file_access_off",
    `The browser doesn't let the Chat with Work extension hand files to pages. ${FILE_ACCESS_STEPS} Nothing was uploaded.`,
  );
}

function checkFiles(value: unknown): UploadFile[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > MAX_FILES) {
    throw new ToolError("invalid_argument", `files must list 1 to ${MAX_FILES} files`);
  }
  return value.map((file) => {
    const ok =
      file && typeof file === "object" &&
      typeof file.path === "string" && file.path.length > 0 && file.path.length <= 4096 &&
      // Only a full path, as the daemon resolved it.
      (file.path.startsWith("/") || /^[A-Za-z]:\\/.test(file.path)) &&
      typeof file.name === "string" && file.name.length > 0 && file.name.length <= 255 &&
      Number.isSafeInteger(file.size) && file.size >= 0 && file.size <= MAX_BYTES &&
      typeof file.mime_type === "string" && file.mime_type.length <= 200;
    if (!ok) throw new ToolError("invalid_argument", "each file needs the path, name, size and type Chat with Work Desktop checked");
    return { path: file.path, name: file.name, size: file.size, mime_type: file.mime_type };
  });
}

// What the call's element is for uploading.
interface Field {
  kind: "field" | "button" | "drop";
  // The file field the files go to, when known before anything is pressed.
  input?: number;
  label: string;
  accept?: string;
  multiple?: boolean;
  disabled?: boolean;
}

async function fieldFor(t: Target, drop: boolean): Promise<Field> {
  const marks = await pageUploads(t.tabId);
  const mark: UploadMark | undefined = marks.get(t.backendNodeId);
  const type = (t.facts.attrs.type ?? "").toLowerCase();
  if (drop) return { kind: "drop", label: t.facts.name };
  if (t.facts.tag === "input" && type === "file") {
    return {
      kind: "field",
      input: t.backendNodeId,
      label: mark?.label || t.facts.name,
      accept: t.facts.attrs.accept || undefined,
      multiple: t.facts.attrs.multiple !== undefined,
      disabled: t.facts.attrs.disabled !== undefined,
    };
  }
  if (mark?.field !== undefined) {
    const { node } = await send<{ node: { attributes?: string[] } }>(t.tabId, "DOM.describeNode", { backendNodeId: mark.field });
    const attrs: Record<string, string> = {};
    const list = node.attributes ?? [];
    for (let i = 0; i + 1 < list.length; i += 2) attrs[list[i].toLowerCase()] = list[i + 1];
    return {
      kind: "field",
      input: mark.field,
      label: marks.get(mark.field)?.label || mark.label || t.facts.name,
      accept: attrs.accept || undefined,
      multiple: attrs.multiple !== undefined,
      disabled: attrs.disabled !== undefined,
    };
  }
  if (mark?.kind === "drop") {
    throw new ToolError("invalid_argument", "that element takes files dropped on it; call again with drop: true");
  }
  if (mark) return { kind: "button", label: mark.label || t.facts.name };
  throw new ToolError(
    "invalid_argument",
    "that element isn't an upload field or a button that goes with one; read the page for `[upload]`, or pass drop: true for a zone that says to drop files on it",
  );
}

function checkAgainst(field: Field, files: UploadFile[]): void {
  if (field.disabled) throw new ToolError("invalid_argument", "that upload field is disabled");
  if (field.multiple === false && files.length > 1) {
    throw new ToolError("invalid_argument", "that field takes one file; upload them one at a time, or find a field that takes several");
  }
  files.forEach((file, index) => {
    if (!accepts(field.accept, file.name, file.mime_type)) {
      throw new ToolError(
        "invalid_argument",
        `${fileOrdinal(index, files.length, file.name)} isn't a kind of file that field takes (it accepts ${field.accept!.slice(0, 200)}); nothing was uploaded`,
      );
    }
  });
}

// The files a field holds, by name and size only, never their contents.
const FILES_OF = `function () {
  return this.files ? Array.from(this.files).map((f) => ({ name: f.name, size: f.size })) : null;
}`;

async function filesOf(tabId: number, backendNodeId: number): Promise<{ name: string; size: number }[] | null> {
  // In the field's own frame: an isolated world belongs to the main frame only.
  const { object } = await send<{ object: { objectId: string } }>(tabId, "DOM.resolveNode", { backendNodeId });
  try {
    const result = await send<{ result: { value?: { name: string; size: number }[] | null } }>(tabId, "Runtime.callFunctionOn", {
      objectId: object.objectId, functionDeclaration: FILES_OF, returnByValue: true,
    });
    return result.result.value ?? null;
  } finally {
    await send(tabId, "Runtime.releaseObject", { objectId: object.objectId }).catch(() => {});
  }
}

function notAllowed(error: unknown): boolean {
  return error instanceof ToolError && /Not allowed/.test(error.message);
}

async function setFiles(tabId: number, backendNodeId: number, files: UploadFile[]): Promise<void> {
  await send(tabId, "DOM.setFileInputFiles", { backendNodeId, files: files.map((f) => f.path) }).catch((error) => {
    throw notAllowed(error) ? fileAccessOff() : error;
  });
}

// Press the page's own button with the file chooser intercepted, and give
// the field that asked the files. The chooser is intercepted only for this
// one press, so the person's own pickers in the tab are left alone.
async function throughChooser(t: Target, policy: Policy, files: UploadFile[]): Promise<number> {
  const { surface, ...location } = await point(t, policy);
  void surface;
  await send(t.tabId, "Page.setInterceptFileChooserDialog", { enabled: true });
  const opened = nextEvent<{ backendNodeId?: number; mode?: string }>(t.tabId, "Page.fileChooserOpened", CHOOSER_WAIT_MS);
  try {
    const mouse = (type: string, extra: object = {}) =>
      send(t.tabId, "Input.dispatchMouseEvent", { type, ...location, button: "left", clickCount: 1, ...extra });
    await mouse("mouseMoved", { button: "none", clickCount: 0 });
    allowedChanges(policy);
    await still(t.tabId, policy);
    await mouse("mousePressed", { buttons: 1 });
    await mouse("mouseReleased", { buttons: 0 });
    const event = await opened.event;
    if (!event) {
      throw new ToolError("page_error", "the button was pressed, but no file picker opened; nothing was uploaded. Read the page again");
    }
    if (event.backendNodeId === undefined) {
      throw new ToolError(
        "unsupported",
        "this site's picker is the browser's own file system picker, which an extension can't answer; nothing was uploaded. The person can choose the file themselves",
      );
    }
    if (event.mode === "selectSingle" && files.length > 1) {
      throw new ToolError("invalid_argument", "that field takes one file; nothing was uploaded");
    }
    const { node } = await send<{ node: { attributes?: string[] } }>(t.tabId, "DOM.describeNode", { backendNodeId: event.backendNodeId });
    const list = node.attributes ?? [];
    const attrs: Record<string, string> = {};
    for (let i = 0; i + 1 < list.length; i += 2) attrs[list[i].toLowerCase()] = list[i + 1];
    checkAgainst({ kind: "field", label: "", accept: attrs.accept || undefined, multiple: event.mode !== "selectSingle" }, files);
    allowedChanges(policy);
    await still(t.tabId, policy);
    await setFiles(t.tabId, event.backendNodeId, files);
    return event.backendNodeId;
  } finally {
    opened.cancel();
    await cleanUp(t.tabId, async (options) => {
      await send(t.tabId, "Page.setInterceptFileChooserDialog", { enabled: false }, { ...options, existingOnly: true }).catch(() => {});
    }, { existingOnly: true });
  }
}

// Drop the files on the element, as if dragged there from the computer.
async function dropOn(t: Target, policy: Policy, files: UploadFile[]): Promise<void> {
  const { surface, ...location } = await point(t, policy);
  void surface;
  const data = { items: [], files: files.map((f) => f.path), dragOperationsMask: 1 };
  allowedChanges(policy);
  await still(t.tabId, policy);
  for (const type of ["dragEnter", "dragOver", "drop"]) {
    await send(t.tabId, "Input.dispatchDragEvent", { type, ...location, data }).catch((error) => {
      throw notAllowed(error) ? fileAccessOff() : error;
    });
  }
}

// What the page shows about the upload now: lines that name one of the
// files, and alerts, as browser_read would show them.
async function shows(tabId: number, files: UploadFile[]): Promise<string[]> {
  const text = await readableText(tabId).catch(() => "");
  const names = files.map((f) => f.name.toLowerCase());
  const lines = text
    .split("\n")
    .filter((line) => names.some((name) => line.toLowerCase().includes(name)) || /- (alert|status)\b/.test(line) || /\[invalid/.test(line))
    .map((line) => line.trim())
    .slice(0, SHOWS_MAX_LINES);
  let total = 0;
  return lines.filter((line) => (total += line.length) <= SHOWS_MAX_CHARS);
}

const upload: Method = async (params: Params, policy: Policy) => {
  only(params, ["tab", "ref", "files", "drop", "dry_run"]);
  allowedChanges(policy);
  const files = checkFiles(params.files);
  const drop = flag(params, "drop", false);
  const dry = dryRun(params);
  const t = await target(params, policy);
  if (isSensitiveField(t.facts)) {
    throw new ToolError(
      "sensitive_field",
      "Chat with Work never fills in password, payment or one-time code fields; the person fills those in themselves",
    );
  }
  const field = await fieldFor(t, drop);
  checkAgainst(field, files);
  // A button that may send something isn't pressed to pick a file.
  // Named for uploading is what it's for; any other reason still refuses.
  if (field.kind === "button") {
    const unnamed = (text?: string) => text?.replace(new RegExp(UPLOAD_WORDS.source, "gi"), " ");
    const reason = submitReason({
      ...t.facts,
      name: unnamed(t.facts.name) ?? "",
      text: unnamed(t.facts.text),
      attrs: { ...t.facts.attrs, ...(t.facts.attrs.value ? { value: unnamed(t.facts.attrs.value)! } : {}),
        ...(t.facts.attrs["aria-label"] ? { "aria-label": unnamed(t.facts.attrs["aria-label"])! } : {}),
        ...(t.facts.attrs.title ? { title: unnamed(t.facts.attrs.title)! } : {}) },
    });
    if (reason) {
      throw new ToolError("would_submit", `that button may send something (${reason}); point the upload at its file field instead`);
    }
  }
  if (!(await fileAccess())) throw fileAccessOff();
  const what = {
    target: described(t.facts),
    field: {
      how: field.kind === "drop" ? "drop" : field.kind === "button" ? "picker" : "field",
      label: field.label.slice(0, 200),
      ...(field.accept ? { accept: field.accept.slice(0, 200) } : {}),
      ...(field.multiple !== undefined ? { multiple: field.multiple } : {}),
    },
  };
  if (dry) {
    const shot = await preview(t);
    const now = await still(t.tabId, policy);
    allowedChanges(policy);
    return { dry_run: true, ...about(now), ...what, ...shot };
  }
  allowedChanges(policy);
  await still(t.tabId, policy);
  const started = Date.now();
  let input: number | undefined;
  if (field.kind === "field") {
    input = field.input!;
    await setFiles(t.tabId, input, files);
  } else if (field.kind === "button") {
    input = await throughChooser(t, policy, files);
  } else {
    await dropOn(t, policy, files);
  }
  // The page holds what was approved, by name and size.
  let chosen: { name: string; size: number }[] = [];
  if (input !== undefined) {
    chosen = (await filesOf(t.tabId, input).catch(() => null)) ?? [];
    const same = chosen.length === files.length && chosen.every((c, i) => c.name === files[i].name && c.size === files[i].size);
    // A page may take the files at once and clear its field; one that holds
    // others got something else.
    if (chosen.length > 0 && !same) {
      throw new ToolError("page_error", "the page holds other files than the ones approved; read the page to see what it shows");
    }
  }
  const settled = await settle(t.tabId, policy, started) as { allowed?: boolean; closed?: boolean };
  const left = settled.allowed === false || settled.closed === true;
  return { dry_run: false, ...what, ...settled, chosen, ...(left ? {} : { shows: await shows(t.tabId, files) }) };
};

export const uploadMethods: Record<string, Method> = { upload };
