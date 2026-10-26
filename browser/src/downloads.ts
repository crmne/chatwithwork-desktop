// Downloads a page starts by itself while the extension acts on it don't
// happen: files the agent saves go only through `browser_download`, into a
// shared folder that allows changes. The debugger reports the download
// (`Page.downloadWillBegin`) and the downloads API creates it, in either
// order; whichever comes second cancels it and removes what was written.

const WINDOW_MS = 15_000;

interface Seen {
  url: string;
  at: number;
}

const fromPages: Seen[] = [];
const created: { id: number; url: string; at: number }[] = [];

function prune(now: number): void {
  for (const list of [fromPages, created] as { at: number }[][]) {
    while (list.length && now - list[0].at > WINDOW_MS) list.shift();
  }
}

async function stop(id: number): Promise<void> {
  await chrome.downloads.cancel(id).catch(() => {});
  await chrome.downloads.removeFile(id).catch(() => {});
  await chrome.downloads.erase({ id }).catch(() => {});
}

// A tab the extension is attached to started a download.
export function pageStartedDownload(url: string): void {
  const now = Date.now();
  prune(now);
  const match = created.findIndex((d) => d.url === url);
  if (match >= 0) {
    const [download] = created.splice(match, 1);
    void stop(download.id);
    return;
  }
  fromPages.push({ url, at: now });
}

chrome.downloads.onCreated.addListener((item) => {
  const now = Date.now();
  prune(now);
  const urls = [item.url, item.finalUrl].filter(Boolean);
  const match = fromPages.findIndex((seen) => urls.includes(seen.url));
  if (match >= 0) {
    fromPages.splice(match, 1);
    void stop(item.id);
    return;
  }
  created.push({ id: item.id, url: item.url, at: now });
});
