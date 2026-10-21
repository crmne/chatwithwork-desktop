// The prototype's page: no framework, no build step. Every request goes
// through the `control` command to the daemon's socket; the daemon's
// events arrive as Tauri events ("chats", "chat", "status").

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const ASSET_ORIGIN = navigator.userAgent.includes("Windows") ? "http://cwwasset.localhost" : "cwwasset://localhost";

const $ = (id) => document.getElementById(id);
const control = (request) => invoke("control", { request });

const state = {
  list: null,
  open: null, // chat number, as a string
  transcript: null,
  streamed: new Map(), // message id -> HTML so far
  progress: null,
  sending: false,
};

// Small helpers ---------------------------------------------------------

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key === "class") el.className = value;
    else if (key === "html") el.innerHTML = value;
    else if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

function logo(image, label = "") {
  if (!image?.path) return h("span", { class: "avatar-stack__blank", "aria-hidden": "true" });
  return h("img", {
    src: ASSET_ORIGIN + image.path,
    alt: label,
    class: image.monochrome ? "logo--mono" : null,
    loading: "lazy",
    decoding: "async",
  });
}

function initials(name = "") {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0].toUpperCase())
    .join("");
}

// Answers link to the web and to files; nothing navigates the window.
document.addEventListener("click", (event) => {
  if (event.target.closest("a[href]")) event.preventDefault();
});

// Measuring ---------------------------------------------------------------

// Marked once the page is laid out with the daemon's data, as the egui
// app marks the frame it lays out with it: not when the compositor shows
// it. Waiting for a frame would never end in a hidden window, which gets
// no frames, and the benchmark runs on a hidden workspace.
const marked = new Set();
function markPainted(what) {
  if (marked.has(what)) return;
  marked.add(what);
  void document.body.offsetHeight; // forces style and layout now
  invoke("painted", { what });
}

// Pages -------------------------------------------------------------------

function show(page) {
  for (const id of ["folders-page", "chat-page", "empty-page"]) $(id).hidden = id !== page;
  $("this-computer").toggleAttribute("aria-current", page === "folders-page");
  if (page === "folders-page") $("this-computer").setAttribute("aria-current", "page");
}

// The chat list -------------------------------------------------------------

function renderList() {
  const list = state.list;
  if (!list) return;
  const nav = $("chat-list");
  nav.replaceChildren(
    ...list.chats.map((chat) => {
      const tone =
        chat.state === "processing" ? "dot dot--live" : chat.state === "error" ? "dot dot--negative" : null;
      return h(
        "button",
        {
          class: "sidebar__link sidebar__chat",
          type: "button",
          "aria-current": String(chat.number) === state.open ? "page" : null,
          onclick: () => openChat(String(chat.number)),
        },
        h("span", {}, chat.title || "New chat"),
        tone && h("span", { class: tone, "aria-label": "Answering" }),
      );
    }),
  );
  const user = list.user?.name ?? "";
  $("me").replaceChildren(
    h("span", { class: "avatar" }, initials(user)),
    h("span", {}, h("div", {}, user), h("div", { class: "micro" }, list.account?.name ?? "")),
  );
}

async function loadList() {
  try {
    state.list = await control({ cmd: "chats" });
    renderList();
  } catch (error) {
    $("chat-list").replaceChildren(h("p", { class: "micro" }, String(error)));
  }
}

listen("chats", ({ payload }) => {
  if (!state.list) return;
  if (payload.event === "watching") return loadList();
  if (payload.event === "chat") {
    const chats = state.list.chats.filter((c) => c.number !== payload.chat.number);
    chats.push(payload.chat);
    chats.sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""));
    state.list.chats = chats;
  } else if (payload.event === "removed") {
    state.list.chats = state.list.chats.filter((c) => c.number !== payload.number);
  } else if (payload.event === "account") {
    Object.assign(state.list, { account: payload.account, user: payload.user, credits: payload.credits });
  }
  renderList();
});

// A chat ------------------------------------------------------------------

async function openChat(number) {
  state.open = number;
  state.transcript = null;
  state.streamed.clear();
  state.progress = null;
  renderList();
  show("chat-page");
  $("messages").replaceChildren();
  await loadChat();
  invoke("follow_chat", { chat: number }).catch((error) => console.warn(error));
  $("question").focus();
}

async function loadChat() {
  const number = state.open;
  if (!number) return;
  const answer = await control({ cmd: "chat", chat: number });
  if (state.open !== number) return;
  state.transcript = answer;
  // A saved answer replaces what streamed of it.
  for (const entry of answer.entries ?? []) {
    if (entry.kind === "assistant" && entry.content) state.streamed.delete(entry.id);
  }
  renderChat();
  markPainted("chat");
}

listen("chat", ({ payload }) => {
  if (String(payload.chat) !== state.open) return;
  switch (payload.type) {
    case "chunk":
      state.streamed.set(payload.message_id, payload.html);
      updateStreamed(payload.message_id);
      break;
    case "progress":
      state.progress = payload.text;
      updateProgress();
      break;
    case "changed":
    case "watching":
      loadChat();
      break;
  }
});

function activity(entry) {
  const live = entry.pending;
  const steps = (entry.steps ?? []).map((step) =>
    h(
      "div",
      { class: "step" },
      logo(step.logo),
      h("span", {}, step.summary),
      step.files?.length ? h("span", { class: "step__files" }, step.files.join(", ")) : null,
      step.waiting ? h("span", { class: "dot dot--attention", "aria-label": "Waiting" }) : null,
    ),
  );
  return h(
    "details",
    { class: live ? "activity activity--live" : "activity", "data-id": entry.id, open: live || null },
    h(
      "summary",
      {},
      h("span", { class: "avatar-stack" }, (entry.logos ?? []).map((l, i) => logo(l, entry.services?.[i]))),
      h("span", { class: "activity__title" }, entry.title),
      h("span", { class: "activity__details" }, live && state.progress ? state.progress : entry.progress ?? entry.details),
      live ? h("span", { class: "activity__wire", "aria-hidden": "true" }) : null,
    ),
    h("div", { class: "activity__steps" }, steps),
  );
}

function message(entry) {
  switch (entry.kind) {
    case "user":
      return h(
        "div",
        { class: "message--user", "data-id": entry.id },
        entry.content,
        entry.attachments?.length
          ? h(
              "div",
              { class: "message__files" },
              entry.attachments.map((file) => h("span", { class: "file-chip" }, logo(file.icon), file.filename)),
            )
          : null,
      );
    case "assistant": {
      const html = entry.content ? entry.html : state.streamed.get(entry.id) ?? "";
      return h(
        "div",
        { class: "message--assistant", "data-id": entry.id },
        h("div", { class: "prose", html }),
        entry.sources?.length
          ? h(
              "div",
              { class: "sources" },
              entry.sources.map((source) =>
                h("a", { class: "citation-chip", href: source.url, title: source.url }, logo(source.icon), source.title),
              ),
            )
          : null,
      );
    }
    case "activity":
      return activity(entry);
    case "notice":
      return h("div", { class: "chat-notice" }, entry.content ?? entry.text ?? "Something went wrong.");
    default:
      return null;
  }
}

function approvalCard(approval) {
  const destructive = approval.effect === "destructive";
  if (!approval.decidable) {
    return h(
      "div",
      { class: "approval approval--decided" },
      h("span", { class: "approval__kicker" }, `Waiting for ${approval.waiting_for}`),
    );
  }
  const forRest = h("input", { type: "checkbox", class: "switch" });
  const card = h(
    "div",
    { class: destructive ? "approval approval--destructive" : "approval", "data-id": approval.id },
    h(
      "div",
      { class: "approval__header" },
      logo(approval.logo, approval.service),
      h(
        "div",
        {},
        h("div", { class: "approval__kicker" }, `Needs your approval · ${approval.service}`),
        h("div", { class: "approval__summary" }, approval.summary),
      ),
    ),
    h(
      "dl",
      { class: "approval__fields" },
      (approval.details ?? []).map((field) => h("div", {}, h("dt", {}, field.label), h("dd", {}, field.value))),
    ),
    h(
      "div",
      { class: "approval__actions" },
      approval.allow_for_rest_of_chat ? h("label", {}, forRest, `Allow for the rest of this chat`) : h("span", {}),
      h(
        "button",
        {
          class: "btn btn-subtle",
          type: "button",
          onclick: () => decide(card, { cmd: "chat_deny", chat: state.open, tool_call: String(approval.id) }),
        },
        "Deny",
      ),
      h(
        "button",
        {
          class: "btn btn-primary",
          type: "button",
          onclick: () =>
            decide(card, {
              cmd: "chat_approve",
              chat: state.open,
              tool_call: String(approval.id),
              for_rest_of_chat: forRest.checked,
            }),
        },
        "Approve",
      ),
    ),
  );
  return card;
}

async function decide(card, request) {
  card.classList.add("approval--decided");
  for (const button of card.querySelectorAll("button")) button.disabled = true;
  try {
    await control(request);
  } catch (error) {
    card.classList.remove("approval--decided");
    for (const button of card.querySelectorAll("button")) button.disabled = false;
    alert(error);
    return;
  }
  loadChat();
}

function thinkingLine(t) {
  const approvals = t.approvals ?? [];
  if (approvals.some((a) => a.decidable)) return h("div", { class: "thinking thinking--approval" }, "Waiting for your approval");
  if (t.chat?.state !== "processing") return null;
  const last = t.entries?.at(-1);
  const writing = last?.kind === "assistant" && (last.content || state.streamed.has(last.id));
  if (last?.kind === "activity" && last.pending) return null; // the live activity line says it
  return h("div", { class: "thinking", id: "thinking" }, writing ? "Writing" : "Thinking");
}

function renderChat() {
  const t = state.transcript;
  if (!t) return;
  $("chat-title").textContent = t.chat?.title || "New chat";
  $("chat-model").textContent = t.chat?.model?.name ?? "";
  const conversation = $("conversation");
  const pinned = conversation.scrollHeight - conversation.scrollTop - conversation.clientHeight < 40;
  $("messages").replaceChildren(
    ...(t.entries ?? []).map(message).filter(Boolean),
    ...(t.approvals ?? []).map(approvalCard),
    thinkingLine(t) ?? [],
  );
  const working = t.chat?.state === "processing";
  $("prompt").classList.toggle("chat-prompt--working", working);
  $("composer-note").textContent = t.locked_reason ?? (working ? "Your message steers the answer" : "");
  if (pinned) conversation.scrollTop = conversation.scrollHeight;
}

// Only the streaming answer's HTML changes, not the whole transcript.
function updateStreamed(id) {
  const prose = document.querySelector(`.message--assistant[data-id="${id}"] .prose`);
  if (!prose) return loadChat();
  const conversation = $("conversation");
  const pinned = conversation.scrollHeight - conversation.scrollTop - conversation.clientHeight < 40;
  prose.innerHTML = state.streamed.get(id);
  const thinking = $("thinking");
  if (thinking) thinking.textContent = "Writing";
  if (pinned) conversation.scrollTop = conversation.scrollHeight;
}

function updateProgress() {
  const details = document.querySelector(".activity--live .activity__details");
  if (details) details.textContent = state.progress;
}

// The composer ---------------------------------------------------------------

$("composer").addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = $("question").value.trim();
  if (!text || state.sending) return;
  state.sending = true;
  $("send").disabled = true;
  try {
    const request = { cmd: "chat_send", text };
    if (state.open) request.chat = state.open;
    const answer = await control(request);
    $("question").value = "";
    const number = String(answer.chat?.number ?? state.open);
    if (number !== state.open) await openChat(number);
    else await loadChat();
  } catch (error) {
    $("composer-note").textContent = String(error);
  } finally {
    state.sending = false;
    $("send").disabled = false;
  }
});

$("question").addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    $("composer").requestSubmit();
  }
});

$("new-chat").addEventListener("click", () => {
  state.open = null;
  state.transcript = null;
  renderList();
  show("chat-page");
  $("chat-title").textContent = "New chat";
  $("chat-model").textContent = "";
  $("messages").replaceChildren();
  $("prompt").classList.remove("chat-prompt--working");
  $("question").focus();
});

// This computer: the Folders page the egui app opens on ------------------------

function renderStatus(status) {
  const connection = status.connection?.connection ?? "offline";
  const tone = { connected: "positive", connecting: "attention", offline: "negative", revoked: "negative" }[connection];
  $("connection").replaceChildren(
    h(
      "div",
      { class: "rows__row" },
      h(
        "div",
        { class: "rows__body" },
        h("h3", {}, status.paused ? "Paused" : "Local Agent"),
        h("p", {}, status.server ? `Connected to ${status.server}` : "Not paired yet"),
      ),
      h(
        "span",
        { class: "state" },
        h("span", { class: `dot ${tone ? `dot--${tone}` : ""}` }),
        status.paused ? "Paused" : connection.replace("_", " "),
      ),
    ),
  );
  const roots = status.roots ?? [];
  $("roots").replaceChildren(
    ...(roots.length
      ? roots.map((root) => {
          const toggle = h("input", {
            type: "checkbox",
            class: "switch",
            checked: root.writable,
            "aria-label": `Allow changes in ${root.label}`,
            onchange: async (event) => {
              const writable = event.target.checked;
              if (writable && !confirm(`Allow Chat with Work to change files in ${root.label}? Each change asks you first.`)) {
                event.target.checked = false;
                return;
              }
              await control({ cmd: "roots_writable", root: root.id, writable }).catch((e) => alert(e));
            },
          });
          const index =
            root.index === "ready" ? `${(root.indexed_files ?? 0).toLocaleString()} files` : root.index ?? "";
          return h(
            "div",
            { class: "rows__row" },
            h("div", { class: "rows__body" }, h("h3", {}, root.label), h("p", { title: root.local_path }, root.local_path)),
            h("span", { class: "micro" }, index),
            h("span", { class: "micro" }, "Allow changes"),
            toggle,
          );
        })
      : [h("div", { class: "rows__row" }, h("p", { class: "muted" }, "Nothing shared yet."))]),
  );
}

$("this-computer").addEventListener("click", () => {
  state.open = null;
  renderList();
  show("folders-page");
});

listen("status", ({ payload }) => renderStatus(payload));

// In-page checks for the benchmark (CWW_AUTOTEST). They use the page's own
// events, never the system's input, so they measure the page's work
// (script, layout, paint) between an input and its frame, not the input
// path through the compositor.

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(resolve));
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const report = (line) => invoke("report", { line });

function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return { n: sorted.length, median: at(0.5).toFixed(1), p95: at(0.95).toFixed(1), max: sorted.at(-1).toFixed(1) };
}

const autotests = {
  // Clicks the first Approve and reports when the chat shows it decided.
  async approve() {
    await wait(1000);
    const button = [...document.querySelectorAll(".approval .btn-primary")][0];
    if (!button) return report("approve no-card");
    const t0 = performance.now();
    button.click();
    for (let i = 0; i < 200 && document.querySelector(".approval:not(.approval--decided)"); i++) await wait(10);
    await nextFrame();
    report(`approve decided-in-ms ${(performance.now() - t0).toFixed(1)} cards-left ${document.querySelectorAll(".approval").length}`);
  },
  // A long transcript (the chat's entries 30 times over) scrolled top to
  // bottom in 3 s, one step a frame: the time between frames.
  async scroll() {
    await wait(800);
    const column = $("messages");
    const copy = [...column.children];
    for (let i = 0; i < 30; i++) column.append(...copy.map((n) => n.cloneNode(true)));
    const conversation = $("conversation");
    conversation.scrollTop = 0;
    await nextFrame();
    const deltas = [];
    const start = performance.now();
    let last = start;
    const height = conversation.scrollHeight - conversation.clientHeight;
    while (performance.now() - start < 3000) {
      await nextFrame();
      const now = performance.now();
      deltas.push(now - last);
      last = now;
      conversation.scrollTop = (height * (now - start)) / 3000;
    }
    const s = stats(deltas);
    report(`scroll px ${height} frames ${s.n} frame-ms median ${s.median} p95 ${s.p95} max ${s.max} over-25ms ${deltas.filter((d) => d > 25).length}`);
  },
  // 60 characters typed into the composer, one per frame: the time from
  // each input event to the next frame.
  async type() {
    await wait(800);
    const field = $("question");
    field.focus();
    const lags = [];
    for (const ch of "How did Q3 spending compare with the plan, team by team?".padEnd(60, ".")) {
      const t0 = performance.now();
      field.dispatchEvent(new KeyboardEvent("keydown", { key: ch, bubbles: true }));
      field.value += ch;
      field.dispatchEvent(new InputEvent("input", { data: ch, inputType: "insertText", bubbles: true }));
      await nextFrame();
      lags.push(performance.now() - t0);
    }
    const s = stats(lags);
    report(`type input-to-frame-ms median ${s.median} p95 ${s.p95} max ${s.max}`);
  },
};

// Start -------------------------------------------------------------------

(async function start() {
  const options = await invoke("launch_options");
  const listed = loadList().then(() => invoke("follow_list").catch(() => {}));
  if (options.open_chat) {
    await openChat(options.open_chat);
  } else {
    show("folders-page");
    try {
      renderStatus(await control({ cmd: "status" }));
      markPainted("folders");
    } catch (error) {
      $("connection").replaceChildren(h("div", { class: "rows__row" }, h("p", {}, String(error))));
    }
  }
  invoke("follow_status").catch(() => {});
  await listed;
  if (options.autotest && autotests[options.autotest]) autotests[options.autotest]();
})();
