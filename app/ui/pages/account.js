// Pairing with Chat with Work: `cww login` opens the approval page in the
// browser, and this page shows the code to compare. The daemon does the
// rest. Organizations, people and everything else about the account are
// in the Chat with Work window.

import { agentMissing, append, button, call, group, h, host, notice, pageHeader } from "../app.js";

export const title = "Account";
export const icon = "user-circle";

// Pairing is shared with the welcome page: one state for both.
function pairing(app) {
  return app.view("pairing");
}

export async function listen(app, on) {
  await on("pairing", (event) => {
    const view = pairing(app);
    view.requesting = false;
    if (event.kind === "code") {
      view.code = event;
    } else if (event.kind === "paired") {
      view.active = false;
      view.code = null;
      view.error = event.daemon_error || null;
    } else {
      view.active = false;
      view.code = null;
      view.error = event.message;
    }
    app.render();
  });
}

export async function pair(app, server) {
  const view = pairing(app);
  view.active = true;
  view.requesting = true;
  view.code = null;
  view.error = null;
  app.render();
  try {
    await call("pair", { server: server || null });
  } catch (error) {
    view.active = false;
    view.requesting = false;
    view.error = error.message;
    app.render();
  }
}

function cancel(app) {
  const view = pairing(app);
  view.active = false;
  view.requesting = false;
  view.code = null;
  call("cancel_pairing");
  app.render();
}

// The server the unpaired card pairs with: "" for Chat with Work's own,
// null while a self-hosted one is chosen but blank.
function chosenServer(view) {
  if (!view.custom) return "";
  const server = (view.server || "").trim();
  return server ? server : null;
}

function pairingCard(app, code) {
  const view = pairing(app);
  return group(h("div", { class: "row row--stack" },
    h("div", { class: "row__body" },
      h("h3", {}, "Approve this computer in your browser"),
      h("p", {}, `Open ${host(code.verification_uri)} and check that it shows this code, then approve “${code.device_name}”.`)),
    h("div", { class: "inline" },
      h("span", { class: "code", "aria-label": "Pairing code" }, code.user_code),
      button(view.copied ? "Copied" : "Copy", async () => {
        try {
          await navigator.clipboard.writeText(code.user_code);
          view.copied = true;
          app.render();
          setTimeout(() => {
            view.copied = false;
            app.render();
          }, 1500);
        } catch {}
      })),
    h("p", { class: "muted" }, "Waiting for approval…"),
    h("div", { class: "inline" },
      button("Open Page Again", () => call("open_url", { url: code.browser_url }), { disabled: !code.browser_url }),
      button("Cancel", () => cancel(app))),
    h("span", { class: "micro" }, `Key fingerprint ${code.fingerprint}`)));
}

function sinceText(since) {
  const at = new Date(since);
  if (Number.isNaN(at.getTime())) return since;
  const now = new Date();
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (at.toDateString() === now.toDateString()) return time;
  return `${at.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}

function pairedCard(app, status) {
  const view = pairing(app);
  const health = app.health;
  let connection = health.label;
  if (health.serving && status.connection?.since) connection = `Online since ${sinceText(status.connection.since)}`;
  const revoked = status.connection?.connection === "revoked";
  return [
    group(
      h("div", { class: "row" }, h("div", { class: "row__body" }, h("h3", {}, "Chat with Work")), h("span", { class: "muted" }, host(status.server))),
      h("div", { class: "row" },
        h("div", { class: "row__body" }, h("h3", {}, "Connection"),
          status.connection?.last_error && !health.serving ? h("span", { class: "micro" }, status.connection.last_error) : null),
        h("span", { class: `state state--${health.tone}` }, connection)),
      h("div", { class: "row" }, h("div", { class: "row__body" }, h("h3", {}, "Device ID")), h("span", { class: "muted mono" }, status.device_id || ""))),
    revoked ? h("div", { class: "section" },
      notice("Chat with Work no longer accepts this computer. Pair it again to keep sharing.", "attention"),
      h("div", { class: "inline" }, button(view.requesting ? "Asking Chat with Work…" : "Pair Again…",
        () => pair(app, status.server), { kind: "btn-primary", disabled: view.requesting }))) : null,
    view.confirmLogout
      ? h("div", { class: "confirm confirm--negative" },
        h("p", {}, "Disconnect this computer? Chat with Work stops reaching it right away. Also remove it in Chat with Work under Settings ▸ Computers."),
        h("div", { class: "inline" },
          button("Cancel", () => {
            view.confirmLogout = false;
            app.render();
          }),
          button("Disconnect", async () => {
            view.confirmLogout = false;
            view.requesting = true;
            app.render();
            try {
              await call("logout");
              view.error = null;
            } catch (error) {
              view.error = error.message;
            }
            view.requesting = false;
            app.render();
          }, { kind: "btn-subtle-negative" })))
      : h("div", { class: "inline" }, button("Disconnect This Computer…", () => {
        view.confirmLogout = true;
        app.render();
      }, { disabled: view.requesting })),
  ];
}

function unpairedCard(app) {
  const view = pairing(app);
  const server = chosenServer(view);
  return group(h("div", { class: "row row--stack" },
    h("div", { class: "row__body" },
      h("h3", {}, "This computer isn't paired"),
      h("p", {}, "Pairing opens Chat with Work in your browser, where you approve this computer. It gets its own key, which never leaves it.")),
    h("div", { class: "inline" }, button(view.requesting ? "Asking Chat with Work…" : "Pair with Chat with Work…",
      // The address as typed now: typing redraws the page only when the
      // field stops or starts being blank.
      () => pair(app, chosenServer(view)), { kind: "btn-primary", disabled: view.requesting || server === null })),
    h("label", { class: "check" },
      h("input", { type: "checkbox", checked: !!view.custom, "data-key": "custom-server", onchange: (event) => {
        view.custom = event.target.checked;
        app.render();
      } }),
      "Use a self-hosted server"),
    view.custom ? h("input", { class: "input", type: "url", placeholder: "https://chat.example.com", value: view.server || "",
      "aria-label": "Server address", "data-key": "server", oninput: (event) => {
        const wasBlank = chosenServer(view) === null;
        view.server = event.target.value;
        if (wasBlank !== (chosenServer(view) === null)) app.render();
      } }) : null,
    view.custom && server === null ? h("p", { class: "note" }, "Enter your server's address to pair with it.") : null));
}

// Pairing state and actions, shared with the welcome page.
export function pairingSection(app, status) {
  const view = pairing(app);
  const parts = [];
  if (view.active) {
    parts.push(view.code ? pairingCard(app, view.code) : group(h("div", { class: "row" },
      h("div", { class: "row__body" }, h("p", {}, "Asking Chat with Work for a code…")),
      h("div", { class: "row__actions" }, button("Cancel", () => cancel(app))))));
  } else if (status.server) {
    parts.push(...pairedCard(app, status));
  } else {
    parts.push(unpairedCard(app));
  }
  if (view.error) parts.push(notice(view.error));
  return parts;
}

export function render(app) {
  const page = h("div", { class: "page" },
    pageHeader("Account", "The account this computer shares with. Your profile, organization and people have their own pages in Settings."));
  if (!app.status) {
    append(page, agentMissing(app));
    return page;
  }
  append(page, h("div", { class: "section" }, ...pairingSection(app, app.status)));
  return page;
}

export default { title, icon, render, listen };
