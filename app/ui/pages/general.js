// Start at login, pausing, crash reports, dictation for `cww tui`, the
// background agent and where its files are, and about.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader, row, section, switchInput } from "../app.js";
import { displayPath } from "./folders.js";

export const title = "General";
export const icon = "gear-six";

const REPOSITORY = "https://github.com/crmne/chatwithwork-desktop";

export async function load(app) {
  const view = app.view("general");
  const [autostart, crash, dictation] = await Promise.allSettled([
    call("autostart"), call("crash_reports"), call("dictation"),
  ]);
  if (autostart.status === "fulfilled") view.autostart = autostart.value.enabled;
  if (crash.status === "fulfilled") view.crash = crash.value;
  if (dictation.status === "fulfilled") view.dictation = dictation.value;
  if (app.page === "general" || app.page === "welcome") app.render();
}

export const enter = load;

export async function listen(app, on) {
  await on("dictation-download", (event) => {
    const view = app.view("general");
    if (event.done) {
      view.download = null;
      view.dictationError = event.error || null;
      load(app);
    } else {
      view.download = event;
      app.render();
    }
  });
}

export async function setAutostart(app, enabled) {
  const view = app.view("general");
  view.autostart = enabled;
  view.settingAutostart = true;
  view.error = null;
  app.render();
  try {
    await call("set_autostart", { enabled });
  } catch (error) {
    view.error = error.message;
  }
  try {
    view.autostart = (await call("autostart")).enabled;
  } catch {}
  view.settingAutostart = false;
  app.render();
}

export async function setCrashReports(app, enabled) {
  const view = app.view("general");
  try {
    await call("set_crash_reports", { enabled });
    view.crashError = null;
  } catch (error) {
    view.crashError = error.message;
  }
  await load(app);
}

async function setPaused(app, paused) {
  const view = app.view("general");
  view.pausing = true;
  app.render();
  try {
    await control({ cmd: paused ? "pause" : "resume" });
    view.error = null;
  } catch (error) {
    view.error = error.message;
  }
  view.pausing = false;
  app.render();
}

async function saveDictation(app, mode, model) {
  const view = app.view("general");
  try {
    await call("set_dictation", { mode, model });
    view.dictationError = null;
  } catch (error) {
    view.dictationError = error.message;
  }
  await load(app);
}

function dictationSection(app) {
  const view = app.view("general");
  const d = view.dictation;
  if (!d) return null;
  const model = d.models.find((m) => m.id === d.model) || d.models[0];
  const choice = (value, label) =>
    h("label", {}, h("input", { type: "radio", name: "dictation-mode", value, checked: d.mode === value,
      onchange: () => saveDictation(app, value, d.model) }), label);
  const megabytes = (bytes) => (bytes / 1e6).toFixed(1);
  return section("Dictation", "How the terminal app, cww tui, turns your voice into text. The Chat with Work window dictates through your server.",
    group(
      row({
        title: "Transcribe",
        text: d.mode === "local"
          ? "Whisper keeps audio on this computer and uses no credits. All models support multiple languages. Local failures never upload your recording."
          : "Uses credits. Audio is transcribed in the EU through your Chat with Work server.",
        actions: h("div", { class: "segmented", role: "radiogroup", "aria-label": "Transcribe" },
          choice("cloud", "In the cloud"), choice("local", "On this computer")),
      }),
      d.mode === "local" ? h("div", { class: "row row--stack" },
        h("div", { class: "inline" },
          h("label", { class: "check", for: "speech-model" }, "Whisper model"),
          h("select", { class: "select", id: "speech-model", disabled: !!view.download,
            onchange: (event) => saveDictation(app, d.mode, event.target.value) },
          d.models.map((m) => h("option", { value: m.id, selected: m.id === d.model }, m.label)))),
        view.download
          ? h("div", { class: "section" },
            h("div", { class: "meter", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": String(view.download.total), "aria-valuenow": String(view.download.bytes) },
              h("span", { style: `inline-size: ${Math.min(100, (100 * view.download.bytes) / view.download.total).toFixed(1)}%` })),
            h("div", { class: "inline" },
              h("span", { class: "note" }, `Downloading ${view.download.model}: ${megabytes(view.download.bytes)} / ${megabytes(view.download.total)} MB`),
              button(view.cancelling ? "Cancelling…" : "Cancel download", () => {
                view.cancelling = true;
                call("cancel_download");
                app.render();
              }, { disabled: view.cancelling })))
          : model.downloaded
            ? h("div", { class: "inline" }, h("span", { class: "note" }, "Downloaded. Ready for local dictation."),
              button("Remove model", async () => {
                try {
                  await call("remove_model", { model: model.id });
                } catch (error) {
                  view.dictationError = error.message;
                }
                load(app);
              }))
            : h("div", { class: "section" },
              h("p", { class: "note" }, "Download from Hugging Face once, then use it offline. Model files are verified before use and are not included in the app."),
              h("div", { class: "inline" }, button("Download model", async () => {
                view.cancelling = false;
                view.download = { model: model.id, bytes: 0, total: model.bytes };
                app.render();
                try {
                  await call("download_model", { model: model.id });
                } catch (error) {
                  view.download = null;
                  view.dictationError = error.message;
                  app.render();
                }
              })))) : null),
    d.error ? notice(d.error) : null,
    view.dictationError ? notice(view.dictationError) : null);
}

export function render(app) {
  const view = app.view("general");
  const status = app.status;
  const crash = view.crash;
  const page = h("div", { class: "page" }, pageHeader("General"));
  append(page, 
    group(
      row({
        title: "Start at login",
        text: `Show Chat with Work in the ${app.info.tray_place} when you log in. The agent itself always runs in the background once it's started.`,
        actions: switchInput(!!view.autostart, (on) => setAutostart(app, on), { label: "Start at login", disabled: view.settingAutostart || view.autostart === undefined }),
      }),
      row({
        title: "Pause sharing",
        text: "While paused, the background agent refuses every request from Chat with Work.",
        actions: switchInput(!!status?.paused, (on) => setPaused(app, on), { label: "Pause sharing", disabled: !status || view.pausing }),
      }),
      crash && crash.available ? row({
        title: "Send crash reports",
        text: crash.from_env ? `${crash.about} Set by ${crash.env} in the environment.` : crash.about,
        actions: switchInput(crash.enabled === true, (on) => setCrashReports(app, on), { label: "Send crash reports", disabled: crash.from_env }),
      }) : null),
    view.error ? notice(view.error) : null,
    view.crashError ? notice(view.crashError) : null,
    dictationSection(app),
    section("Background agent", null,
      status
        ? group(
          row({ title: "Background agent", actions: h("span", { class: "muted" }, `Running · version ${status.version} · process ${status.pid}`) }),
          status.config_file ? row({ title: "Configuration", micro: displayPath(status.config_file, app.info.home),
            actions: button("Open", () => call("open_file", { which: "config" })) }) : null,
          status.audit_file ? row({ title: "Activity log", micro: displayPath(status.audit_file, app.info.home),
            actions: button("Open", () => call("open_file", { which: "audit" })) }) : null)
        : agentMissing(app)),
    section("About", null,
      group(
        row({
          title: `Chat with Work Desktop ${app.info.version}`,
          text: "Open source under the MIT or Apache 2.0 license. The code, the wire protocol and the threat model are public.",
          extra: h("p", {}, h("button", { type: "button", class: "link", onclick: () => call("open_url", { url: REPOSITORY }) }, "Source code and security notes")),
        }),
        row({
          title: "Quit",
          text: `Closes the windows and the ${app.info.tray_place} item. The background agent keeps running in the background.`,
          actions: button(app.info.quit_item, () => call("quit")),
        }))));
  return page;
}

export default { title, icon, render, enter, listen };
