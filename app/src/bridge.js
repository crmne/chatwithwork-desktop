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

  const api = Object.freeze({
    version: __VERSION__,
    components: Object.freeze(__COMPONENTS__),
    openExternal: (url) => invoke("open_external", { url: String(url) }),
    openPreferences: () => invoke("open_preferences"),
    listen: () => invoke("listen"),
    confirm: (confirmation) => invoke("confirm", { confirmation }).then(Boolean),
  });
  Object.defineProperty(window, "ChatWithWorkDesktop", {
    value: api,
    writable: false,
    configurable: false,
  });

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

  // Turbo's data-turbo-confirm asks in a native dialog: the web view's own
  // window.confirm shows nothing in some of them. The button's or form's
  // data-bridge-* attributes (written for the phone apps) name its buttons.
  const nativeConfirm = (message, form, submitter) => {
    const attribute = (name) =>
      submitter?.getAttribute?.(`data-bridge-${name}`) ?? form?.getAttribute?.(`data-bridge-${name}`);
    const method = (
      form?.querySelector?.("input[name=_method]")?.value || form?.getAttribute?.("method") || ""
    ).toLowerCase();
    const deletes = method === "delete";
    const destructive = attribute("destructive") ? attribute("destructive") === "true" : deletes;
    return api
      .confirm({
        title: String(message ?? ""),
        description: attribute("description") ?? null,
        confirm: attribute("confirm") || (deletes ? "Delete" : "OK"),
        dismiss: attribute("dismiss") || "Cancel",
        destructive,
      })
      .catch(() => false);
  };
  const useNativeConfirm = () => {
    const turbo = window.Turbo;
    if (turbo?.config?.forms) {
      if (turbo.config.forms.confirm !== nativeConfirm) turbo.config.forms.confirm = nativeConfirm;
    } else if (typeof turbo?.setConfirmMethod === "function") {
      turbo.setConfirmMethod(nativeConfirm);
    }
  };
  document.addEventListener("DOMContentLoaded", useNativeConfirm);
  document.addEventListener("turbo:load", useNativeConfirm);

  // Ctrl+, (Cmd+, on macOS) opens the settings, as in most apps.
  window.addEventListener("keydown", (event) => {
    const mac = navigator.platform.startsWith("Mac");
    if (event.key === "," && (mac ? event.metaKey : event.ctrlKey) && !event.altKey && !event.shiftKey) {
      event.preventDefault();
      api.openPreferences();
    }
  });
})();
