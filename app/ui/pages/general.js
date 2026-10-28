// Start at login, pausing, crash reports, dictation for `cww tui`, the
// background agent and where its files are, and about.

import { agentMissing, append, button, call, control, group, h, notice, pageHeader, row, section, switchInput } from "../app.js";
import { displayPath } from "./folders.js";

export const title = "General";
export const icon = "gear-six";

const REPOSITORY = "https://github.com/crmne/chatwithwork-desktop";

export async function load(app) {
  const view = app.view("general");
  const [autostart, crash, dictation, update] = await Promise.allSettled([
    call("autostart"), call("crash_reports"), call("dictation"), call("update_status"),
  ]);
  if (update.status === "fulfilled") view.update = update.value;
  if (autostart.status === "fulfilled") view.autostart = autostart.value.enabled;
  if (crash.status === "fulfilled") view.crash = crash.value;
  if (dictation.status === "fulfilled") view.dictation = dictation.value;
  if (app.page === "general" || app.page === "welcome") app.render();
}

export const enter = load;

export async function listen(app, on) {
  await on("update", (event) => {
    app.view("general").update = event;
    if (app.page === "general") app.render();
  });
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
    const answer = await call("set_dictation", { mode, model });
    view.dictationError = null;
    // Choosing this computer starts getting its model; the progress
    // events follow.
    if (answer.downloading) view.cancelling = false;
  } catch (error) {
    view.dictationError = error.message;
  }
  await load(app);
}

async function downloadAgain(app, model) {
  const view = app.view("general");
  view.cancelling = false;
  view.dictationError = null;
  view.download = { model: model.id, bytes: 0, total: model.bytes };
  app.render();
  try {
    await call("download_model", { model: model.id });
  } catch (error) {
    view.download = null;
    view.dictationError = error.message;
    app.render();
  }
}

function dictationSection(app) {
  const view = app.view("general");
  const d = view.dictation;
  if (!d) return null;
  const model = d.models.find((m) => m.id === d.model) || d.models[0];
  const recommended = d.models.find((m) => m.id === d.recommended) || model;
  const choice = (value, label) =>
    h("label", {}, h("input", { type: "radio", name: "dictation-mode", value, checked: d.mode === value,
      onchange: () => saveDictation(app, value, d.chosen) }), label);
  const megabytes = (bytes) => Math.round(bytes / 1e6);
  const download = view.download || (d.downloading ? { model: model.id, bytes: 0, total: model.bytes } : null);
  const named = (id) => (d.models.find((m) => m.id === id) || model).name;
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
          h("select", { class: "select", id: "speech-model", disabled: !!download,
            onchange: (event) => saveDictation(app, d.mode, event.target.value || null) },
          h("option", { value: "", selected: !d.chosen }, `Best for this computer (${recommended.name})`),
          d.models.map((m) => h("option", { value: m.id, selected: m.id === d.chosen }, m.label)))),
        download
          ? h("div", { class: "section" },
            h("div", { class: "meter", role: "progressbar", "aria-label": "Whisper download", "aria-valuemin": "0", "aria-valuemax": String(download.total), "aria-valuenow": String(download.bytes) },
              h("span", { style: `inline-size: ${Math.min(100, (100 * download.bytes) / download.total).toFixed(1)}%` })),
            h("div", { class: "inline" },
              h("span", { class: "note" }, `Getting Whisper ${named(download.model)}: ${megabytes(download.bytes)} of ${megabytes(download.total)} MB`),
              button(view.cancelling ? "Cancelling…" : "Cancel", () => {
                view.cancelling = true;
                call("cancel_download");
                app.render();
              }, { disabled: view.cancelling })))
          : model.downloaded
            ? h("div", { class: "inline" }, h("span", { class: "note" }, `${model.name} is ready. Dictation works offline.`),
              button("Remove", async () => {
                try {
                  await call("remove_model", { model: model.id });
                } catch (error) {
                  view.dictationError = error.message;
                }
                load(app);
              }))
            : h("div", { class: "section" },
              h("p", { class: "note" }, `${model.name} (${megabytes(model.bytes)} MB) downloads from Hugging Face the first time you dictate, or now. It's checked before use and kept with the agent's data on this computer.`),
              h("div", { class: "inline" }, button("Download Now", () => downloadAgain(app, model))))) : null),
    d.error ? notice(d.error) : null,
    view.dictationError ? notice(view.dictationError) : null);
}

// What updating says, and the button it offers.
function updateText(u) {
  const release = u.release?.version;
  switch (u.phase) {
    case "checking": return "Checking for updates…";
    case "current": return `Chat with Work Desktop ${u.version} is the latest version.`;
    case "downloading": {
      const percent = u.total ? Math.round((100 * u.received) / u.total) : 0;
      return `Downloading version ${release}: ${percent}%`;
    }
    case "ready": return `Version ${release} is downloaded and checked. Restart to use it; your windows come back.`;
    case "installing": return `Installing version ${release}…`;
    case "unsupported": return `Version ${release} is out. ${u.advice}`;
    case "failed": return u.error;
    default: return u.automatic ? "Checked once a day. Updates download in the background, signed and verified, and install when you restart." : "Automatic checks are off.";
  }
}

function updatesSection(app) {
  const view = app.view("general");
  const u = view.update;
  if (!u) return null;
  const busy = ["checking", "downloading", "installing"].includes(u.phase);
  const action = u.phase === "ready"
    ? button("Restart to Update", async () => {
      try {
        await call("update_restart");
      } catch (error) {
        view.updateError = error.message;
        app.render();
      }
    }, { kind: "btn-primary", key: "update-restart" })
    : u.phase === "unsupported"
      ? button("Release Notes", () => call("open_url", { url: u.release?.url || u.releases }))
      : button(u.phase === "checking" ? "Checking…" : "Check Now", async () => {
        view.updateError = null;
        try {
          view.update = await call("update_check");
        } catch (error) {
          view.updateError = error.message;
        }
        app.render();
      }, { disabled: busy });
  return section("Updates", null,
    group(
      row({ title: "Version", text: updateText(u), actions: action }),
      u.phase === "downloading" && u.total ? h("div", { class: "row row--stack" },
        h("div", { class: "meter", role: "progressbar", "aria-label": "Update download", "aria-valuemin": "0", "aria-valuemax": String(u.total), "aria-valuenow": String(u.received) },
          h("span", { style: `inline-size: ${Math.min(100, (100 * u.received) / u.total).toFixed(1)}%` }))) : null,
      row({
        title: "Check for updates",
        text: "Once a day, from Chat with Work Desktop's releases on GitHub. A copy a package manager installed updates through it instead.",
        actions: switchInput(u.automatic, async (on) => {
          try {
            view.update = await call("set_update_checks", { on });
          } catch (error) {
            view.updateError = error.message;
          }
          app.render();
        }, { label: "Check for updates" }),
      })),
    u.problem ? notice(u.problem) : null,
    view.updateError ? notice(view.updateError) : null);
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
    updatesSection(app),
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
