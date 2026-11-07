// Letting a bot code on this computer: what the app checked, what the yes
// gives, Allow and Cancel (coding_setup.rs). A bundled page in a window of its
// own: no page of the server can see or answer it.

const invoke = window.__TAURI_INTERNALS__.invoke;
const main = document.getElementById("main");

function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === "onclick") element.addEventListener("click", value);
    else if (value === true) element.setAttribute(key, "");
    else if (value !== false && value !== null && value !== undefined) element.setAttribute(key, value);
  }
  element.append(...children.filter((child) => child !== null && child !== undefined));
  return element;
}

let shownError = null;

async function answer(allow) {
  try {
    await invoke("coding_setup_answer", { allow });
  } catch (error) {
    shownError = String(error);
    load();
  }
}

function render(state) {
  const checks = h("ul", { class: "coding-setup__checks" }, ...state.checks.map((check) =>
    h("li", {},
      h("span", { class: `coding-setup__mark coding-setup__mark--${check.ok ? "ok" : "no"}`, "aria-label": check.ok ? "Done" : "Not yet" }, check.ok ? "✓" : "✗"),
      h("span", {}, check.text))));
  const buttons = state.can_allow
    ? [ h("button", { class: "btn", onclick: () => answer(false) }, "Cancel"),
        h("button", { class: "btn btn-primary", onclick: () => answer(true) }, "Allow") ]
    : [ h("button", { class: "btn", onclick: () => answer(false) }, "Cancel"),
        h("button", { class: "btn btn-primary", onclick: load }, "Check again") ];
  main.replaceChildren(...[
    h("div", { class: "coding-setup__where" }, `Chat with Work Desktop · ${state.computer}`),
    h("h1", {}, state.title),
    checks,
    h("p", { class: "coding-setup__note" }, state.note),
    state.repositories ? h("p", { class: "coding-setup__note" }, state.repositories) : null,
    shownError ? h("p", { class: "coding-setup__error", role: "alert" }, shownError) : null,
    h("div", { class: "coding-setup__actions" }, ...buttons),
  ].filter(Boolean));
}

async function load() {
  try {
    render(await invoke("coding_setup_state"));
  } catch (error) {
    main.replaceChildren(h("p", { class: "coding-setup__error", role: "alert" }, String(error)),
      h("div", { class: "coding-setup__actions" }, h("button", { class: "btn", onclick: () => answer(false) }, "Close")));
  }
}

load();
