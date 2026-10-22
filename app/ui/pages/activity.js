// The local audit log: every request from Chat with Work, and the daemon's
// own events, newest first, grouped by day. The sentences come from
// activity.rs.

import { agentMissing, append, button, call, group, h, notice, pageHeader } from "../app.js";

export const title = "Activity";
export const icon = "pulse";

async function load(app) {
  const view = app.view("activity");
  if (view.loading) return;
  view.loading = true;
  try {
    const { rows } = await call("activity", { lines: 300 });
    view.rows = rows;
    view.error = null;
  } catch (error) {
    view.error = error.message;
  }
  view.loading = false;
  if (app.page === "activity") app.render();
}

export const enter = load;

export function statusChanged(app) {
  if (app.page === "activity") load(app);
}

function entry(row) {
  const [label, tone] = row.badge || [];
  return h("div", { class: "row" },
    h("span", { class: "row__time" }, row.clock),
    h("div", { class: "row__body" },
      h("p", { class: "truncate ink" }, row.text),
      row.detail ? h("span", { class: "micro truncate" }, row.detail) : null),
    label ? h("span", { class: `tag tag--${tone}` }, label) : null);
}

export function render(app) {
  const view = app.view("activity");
  view.filter ||= "all";
  const page = h("div", { class: "page" },
    pageHeader("Activity", "Every request from Chat with Work, as logged on this computer. The log never leaves it."));
  if (!app.status) {
    append(page, agentMissing(app));
    return page;
  }
  const filter = (name, label) =>
    h("button", { type: "button", "aria-pressed": String(view.filter === name), onclick: () => {
      view.filter = name;
      app.render();
    } }, label);
  append(page, h("div", { class: "inline" },
    h("div", { class: "tabs", role: "group", "aria-label": "Show" }, filter("all", "All"), filter("refused", "Refused")),
    h("span", { class: "section__meta" }, button("Open Log File", () => call("open_file", { which: "audit" }).catch((error) => {
      view.error = error.message;
      app.render();
    })))));
  if (view.error) {
    append(page, notice(view.error));
    return page;
  }
  const rows = (view.rows || []).filter((row) => view.filter === "all" || row.refused);
  if (rows.length === 0) {
    append(page, group(h("div", { class: "empty" }, h("p", {},
      !view.rows ? "Loading…" : view.filter === "all" ? "No activity yet." : "Nothing was refused."))));
    return page;
  }
  const days = [];
  for (const row of rows) {
    const last = days[days.length - 1];
    if (last && last.day === row.day) last.rows.push(row);
    else days.push({ day: row.day, rows: [row] });
  }
  for (const day of days) {
    append(page, h("section", { class: "section" },
      h("header", { class: "section__header" }, h("h2", {}, day.day)),
      group(...day.rows.map(entry))));
  }
  return page;
}

export default { title, icon, render, enter, statusChanged };
