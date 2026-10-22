// The offline page: says which server can't be reached, retries every 15
// seconds and on Retry, and opens the settings (offline.rs).

const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);

const retry = document.getElementById("retry");
const status = document.getElementById("status");
let busy = false;

async function check(byHand) {
  if (busy) return;
  busy = true;
  retry.disabled = true;
  if (byHand) status.textContent = "Trying again…";
  try {
    const { online } = await invoke("offline_retry");
    if (!online) status.textContent = byHand ? "Still offline. It tries again by itself every 15 seconds." : status.textContent;
  } catch (error) {
    status.textContent = error?.message || String(error);
  }
  busy = false;
  retry.disabled = false;
}

retry.addEventListener("click", () => check(true));
document.getElementById("settings").addEventListener("click", () => invoke("offline_settings"));
window.addEventListener("online", () => check(false));
// A connection attempt to one host, so cheap enough to make even while the
// window is hidden.
setInterval(() => check(false), 15000);

invoke("offline_info")
  .then(({ host }) => {
    document.getElementById("reason").textContent = `Chat with Work can't reach ${host} right now. Check your connection.`;
  })
  .catch(() => {});
