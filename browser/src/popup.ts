// The extension's menu: whether it's connected, its tab group, the sites
// allowed on this computer, and the kill switch.

interface PopupState {
  native: string;
  daemon: string;
  error: string;
  stopped: boolean;
  stoppedOnComputer: boolean;
  sites: string[];
  all_sites?: boolean;
  blocked_sites?: string[];
  changes: boolean;
  tabs: number;
}

const $ = (id: string) => document.getElementById(id)!;

function render(state: PopupState): void {
  const box = $("state");
  let tone = "positive";
  let text = "Ready";
  let detail = state.changes
    ? "Chat with Work can read and act in its tab group, on the sites below. Each action asks you first."
    : "Chat with Work can read the tabs in its tab group, on the sites below.";
  if (state.stopped) {
    tone = "negative";
    text = "Stopped";
    detail = "Chat with Work can't use the browser until you resume it.";
  } else if (state.stoppedOnComputer) {
    tone = "negative";
    text = "Stopped on this computer";
    detail = "Resume it from Chat with Work Desktop's menu, or with cww browser resume.";
  } else if (state.native !== "connected") {
    tone = "attention";
    text = "Chat with Work Desktop isn't installed";
    detail = "Install Chat with Work Desktop, then turn on the browser in its settings.";
  } else if (state.daemon === "not_running") {
    tone = "attention";
    text = "Chat with Work Desktop isn't running";
    detail = "Open Chat with Work Desktop, or start it with cww daemon install.";
  } else if (state.daemon === "refused") {
    tone = "negative";
    text = "Not turned on";
    detail = state.error || "Turn on the browser in Chat with Work Desktop's settings.";
  } else if (state.daemon !== "connected") {
    tone = "attention";
    text = "Connecting…";
    detail = "";
  } else if (!state.all_sites && state.sites.length === 0) {
    tone = "attention";
    text = "No sites allowed yet";
    detail = "Allow the sites Chat with Work may use in Chat with Work Desktop's settings.";
  }
  box.className = `state state--${tone}`;
  $("state-text").textContent = text;
  $("state-detail").textContent = detail;
  $("tabs").textContent =
    state.tabs === 0
      ? "No tabs yet. Chat with Work opens its tabs in a group of its own; drag a tab in to lend it."
      : `${state.tabs} ${state.tabs === 1 ? "tab" : "tabs"} in the “Chat with Work” group.`;
  const list = $("sites");
  list.replaceChildren(
    ...(state.all_sites ? ["All sites", ...(state.blocked_sites ?? []).map((site) => `Blocked: ${site}`)] : [...(state.sites.length ? state.sites : ["None yet"]), ...(state.blocked_sites ?? []).map((site) => `Blocked: ${site}`)]).map((site) => {
      const item = document.createElement("li");
      item.textContent = site;
      return item;
    }),
  );
  $("stop").hidden = state.stopped;
  $("resume").hidden = !state.stopped;
}

async function ask(type: string): Promise<void> {
  render((await chrome.runtime.sendMessage({ type })) as PopupState);
}

$("stop").addEventListener("click", () => void ask("stop"));
$("resume").addEventListener("click", () => void ask("resume"));
void ask("state");
// The connection can change while the menu is open.
setInterval(() => void ask("state"), 1000);
