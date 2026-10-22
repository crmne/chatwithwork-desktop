// Notifications on this computer: the Local Agent shows them natively,
// whether a window is open or not. What notifies you (chats, emails) is
// chosen in Chat with Work's own settings.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader, row, section } from "../app.js";

export const title = "Notifications";
export const icon = "bell";

// What the native notifications can do here, as the egui app said it.
export function describe(native) {
  switch (native.permission) {
    case "denied":
      return "Notifications are blocked in your system settings. Allow Chat with Work there to receive them on this computer.";
    case "not_determined":
      return "Allow notifications on this computer. Your system will ask for permission.";
    case "unavailable":
      return "Native notifications are unavailable. Your browser and email preferences still apply.";
    default:
      if (!native.connected) return "Waiting for the notification connection. Browser and email notifications remain available.";
      return native.permission === "granted"
        ? "This computer can receive native notifications, even while the app window is closed."
        : "Chat with Work can send notifications on this computer. If the system cannot show one, your linked browser can notify you instead.";
  }
}

async function check(app, ask = false) {
  const view = app.view("notifications");
  view.busy = true;
  try {
    if (ask) await control({ cmd: "notification_permission", request: true });
    view.native = await control({ cmd: "notification_status" });
    view.error = null;
  } catch (error) {
    view.error = error.message;
  }
  view.busy = false;
  if (app.page === "notifications") app.render();
}

let timer = null;

// Checked again every few seconds while the page shows, since permission
// changes in the system's settings, not here.
export function enter(app) {
  check(app);
  clearInterval(timer);
  timer = setInterval(() => {
    if (app.page !== "notifications") {
      clearInterval(timer);
      timer = null;
      return;
    }
    if (!document.hidden) check(app);
  }, 3000);
}

export function render(app) {
  const view = app.view("notifications");
  const page = h("div", { class: "page" },
    pageHeader("Notifications", "Choose when Chat with Work gets your attention."));
  if (!app.status) {
    append(page, agentMissing(app));
    return page;
  }
  const native = view.native;
  append(page, 
    section("On this computer", "The Local Agent shows a notification when a chat needs you or has finished while you're not looking at it. It never shows what the chat says.",
      group(row({
        title: "Native notifications",
        text: native ? describe(native) : "Checking this computer's notification support…",
        actions: native?.permission === "not_determined"
          ? button("Turn on native notifications…", () => check(app, true), { kind: "btn-primary", disabled: view.busy })
          : null,
      })),
      view.error ? notice(view.error) : null),
    section("What notifies you", "Which chats and emails notify you is part of your Chat with Work settings, the same on every computer and phone.",
      h("div", { class: "inline" }, button("Open Notification Settings", () =>
        call("show_main", { path: "/settings?tab=notifications" }).catch((error) => {
          view.error = error.message;
          app.render();
        }), { disabled: !app.status?.server }))),
  );
  if (!app.status?.server) append(page, h("p", { class: "note" }, "Pair this computer to manage your notification preferences."));
  return page;
}

export default { title, icon, render, enter };
