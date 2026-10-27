// Chat with Work Desktop's bridge, run before every page of the main
// window. On pages of the server it belongs to, it gives the page
// `window.ChatWithWorkDesktop` (docs/desktop-server-contract.md) and sends
// links and forms that leave the server to the default browser. On any
// other page it does nothing: the app refuses their requests anyway.
(() => {
  const ORIGIN = __ORIGIN__;
  if (window.location.origin !== ORIGIN || window.ChatWithWorkDesktop) return;
  const internals = window.__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== "function") return;
  const invoke = (command, args) => internals.invoke(command, args || {});

  const change = (action, fields) => invoke("computer_change", { change: { action, ...fields } });

  // This computer: what it shares and allows, decided here. Narrowing
  // happens at once; widening only after the person's yes in the app's own
  // dialog, which resolves { declined: true } when they say no.
  const computer = Object.freeze({
    status: () => invoke("computer"),
    pause: () => change("pause"),
    resume: () => change("resume"),
    addFolder: () => invoke("computer_add_folder"),
    removeFolder: (folder) => change("remove", { folder: String(folder) }),
    renameFolder: (folder, label) => change("rename", { folder: String(folder), label: String(label) }),
    allowChanges: (folder, on) => change("changes", { folder: String(folder), on: Boolean(on) }),
    allowCommands: (folder, on) => change("commands", { folder: String(folder), on: Boolean(on) }),
    allowNetwork: (folder, on) => change("network", { folder: String(folder), on: Boolean(on) }),
    notifications: () => invoke("computer_notifications", { ask: false }),
    allowNotifications: () => invoke("computer_notifications", { ask: true }),
  });

  // A scan for the composer: the scan window opens, and once the person
  // attaches what they scanned, it comes back as a PDF File (null if they
  // close the window instead).
  //
  // `scan_file` answers raw bytes: an ArrayBuffer through Tauri's IPC
  // protocol, but a plain array of numbers once the page's Content Security
  // Policy made Tauri fall back to postMessage, as Chat with Work's pages
  // do. A File made from that array holds its numbers as text ("37,80,68,"),
  // which no PDF reader opens, so the bytes become bytes again first.
  const fileOf = async (scanned) => {
    if (!scanned) return null;
    const answer = await invoke("scan_file", { id: scanned.id });
    const bytes = Array.isArray(answer) ? Uint8Array.from(answer) : answer;
    return new File([bytes], scanned.name, { type: "application/pdf" });
  };
  const scan = async () => fileOf(await invoke("scan"));

  // A scan the person sent to a new chat from the scan window: the new
  // chat's composer takes it as it opens (null when there is none).
  const pendingScan = async () => fileOf(await invoke("scan_pending"));

  const api = Object.freeze({
    version: __VERSION__,
    components: Object.freeze(__COMPONENTS__),
    openExternal: (url) => invoke("open_external", { url: String(url) }),
    openPreferences: (page) => invoke("open_preferences", page ? { page: String(page) } : {}),
    listen: () => invoke("listen"),
    computer,
    scan,
    pendingScan,
  });
  Object.defineProperty(window, "ChatWithWorkDesktop", {
    value: api,
    writable: false,
    configurable: false,
  });

  // Dialogs. The dialog plugin's own script replaces window.alert() and
  // window.confirm() with calls these pages may not make, so a
  // data-turbo-confirm button did nothing and an alert showed nothing.
  // Here Turbo's confirmation, confirm() and alert() ask through the bridge
  // instead, in the system's message box titled with the server's name. The
  // labels are the ones the phone apps read: data-bridge-confirm names the
  // button, data-bridge-description adds detail, and a DELETE (or
  // data-bridge-destructive="true") is destructive with "Delete" as its
  // button. confirm() answers a promise, as the plugin's did: a native
  // dialog can't answer at once. prompt() stays the web view's own.
  const dialog = (fields) =>
    invoke("page_dialog", { dialog: fields }).then((answer) => answer === true, () => false);

  const confirmSubmission = (message, form, submitter) => {
    const attribute = (name) =>
      submitter?.getAttribute?.(`data-bridge-${name}`) ?? form?.getAttribute?.(`data-bridge-${name}`) ?? null;
    const method = (form?.querySelector?.("input[name=_method]")?.value || form?.getAttribute?.("method") || "").toLowerCase();
    const deletes = method === "delete";
    const destructive = attribute("destructive") ? attribute("destructive") === "true" : deletes;
    return dialog({
      kind: "confirm",
      message: String(message ?? ""),
      detail: attribute("description"),
      confirm: attribute("confirm") || (deletes ? "Delete" : "OK"),
      destructive,
    });
  };

  // Turbo loads after this script, so its confirmation is taken over once
  // it has: Turbo 8's config.forms.confirm, or setConfirmMethod before it.
  let confirming = false;
  const useAppDialogs = () => {
    if (confirming) return;
    const turbo = window.Turbo;
    if (turbo?.config?.forms) {
      turbo.config.forms.confirm = confirmSubmission;
      confirming = true;
    } else if (typeof turbo?.setConfirmMethod === "function") {
      turbo.setConfirmMethod(confirmSubmission);
      confirming = true;
    }
  };
  document.addEventListener("turbo:load", useAppDialogs);
  window.addEventListener("DOMContentLoaded", useAppDialogs);
  window.addEventListener("load", useAppDialogs);

  window.alert = (message) => {
    dialog({ kind: "alert", message: String(message ?? "") });
  };
  window.confirm = (message) => dialog({ kind: "confirm", message: String(message ?? "") });

  const leaves = (url) =>
    url.origin !== ORIGIN && /^(https?|mailto):$/.test(url.protocol);

  // Links: after the page's own handlers, so a link the page handles itself
  // (and prevents) is left to it.
  window.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    const link = event.target instanceof Element && event.target.closest("a[href]");
    if (!link || link.hasAttribute("download")) return;
    let url;
    try {
      url = new URL(link.href, document.baseURI);
    } catch {
      return;
    }
    if (!leaves(url)) return;
    event.preventDefault();
    api.openExternal(url.href);
  });

  // Forms that GET another site (a search box, say) open it in the browser.
  // Posts to other sites are left to the page and the app's navigation rules.
  window.addEventListener("submit", (event) => {
    if (event.defaultPrevented) return;
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    const submitter = event.submitter;
    const method = (submitter?.getAttribute("formmethod") || form.method || "get").toLowerCase();
    if (method !== "get") return;
    let url;
    try {
      url = new URL(submitter?.getAttribute("formaction") || form.action, document.baseURI);
    } catch {
      return;
    }
    if (!leaves(url)) return;
    event.preventDefault();
    url.search = new URLSearchParams(new FormData(form, submitter)).toString();
    api.openExternal(url.href);
  });

  // Ctrl+, (Cmd+, on macOS) opens the settings, as in most apps: this
  // computer's part of Chat with Work's Settings where the page names it
  // (a server that has it), else the app's own settings window.
  window.addEventListener("keydown", (event) => {
    const mac = navigator.platform.startsWith("Mac");
    if (event.key === "," && (mac ? event.metaKey : event.ctrlKey) && !event.altKey && !event.shiftKey) {
      event.preventDefault();
      const path = document.querySelector('meta[name="chatwithwork-desktop-settings"]')?.content;
      if (path && path.startsWith("/") && !path.startsWith("//")) {
        window.location.assign(path);
      } else {
        api.openPreferences();
      }
    }
  });
})();
