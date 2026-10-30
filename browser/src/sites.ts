// Which pages the person allowed, on their computer, in Chat with Work
// Desktop. The daemon checks the same rules (src/browsing/sites.rs); keep
// them identical.
//
// - A bare host (`github.com`) allows it and its subdomains over HTTPS, on
//   the default port.
// - An origin (`http://localhost:3000`) allows exactly that scheme, host and
//   port, without subdomains.
// - Nothing else is ever allowed: no `file:`, `data:`, `chrome:` or
//   extension pages.

export function siteAllowed(url: string, sites: readonly string[], allSites = false, blockedSites: readonly string[] = []): boolean {
  let page: URL;
  try {
    page = new URL(url);
  } catch {
    return false;
  }
  if (page.protocol !== "https:" && page.protocol !== "http:") return false;
  if (page.username || page.password) return false;
  return !blockedSites.some((site) => blockedMatch(page, site)) &&
    (allSites || sites.some((site) => matches(page, site)));
}

function blockedMatch(page: URL, site: string): boolean {
  const entry = site.trim().toLowerCase();
  if (entry.includes("://")) return matches(page, entry);
  const host = hostOf(entry);
  return !!host && (page.hostname.replace(/\.+$/, "") === host || page.hostname.replace(/\.+$/, "").endsWith(`.${host}`));
}

function matches(page: URL, site: string): boolean {
  const entry = site.trim().toLowerCase();
  if (!entry) return false;
  if (entry.includes("://")) {
    let origin: URL;
    try {
      origin = new URL(entry);
    } catch {
      return false;
    }
    return (
      (origin.protocol === "https:" || origin.protocol === "http:") &&
      origin.protocol === page.protocol &&
      origin.hostname === page.hostname &&
      origin.port === page.port
    );
  }
  const host = hostOf(entry);
  if (!host) return false;
  return (
    page.protocol === "https:" &&
    page.port === "" &&
    (page.hostname.replace(/\.+$/, "") === host || page.hostname.replace(/\.+$/, "").endsWith(`.${host}`))
  );
}

// The host an entry names, in the form `URL` gives it (punycode,
// lowercase), or null for something that isn't one.
function hostOf(entry: string): string | null {
  if (/[/?#@:*\s\\]/.test(entry)) return null;
  try {
    const host = new URL(`https://${entry}`).hostname.replace(/\.+$/, "");
    return host.includes(".") || host === "localhost" ? host : null;
  } catch {
    return null;
  }
}

// The page's host, for messages and the audit log.
export function hostOfUrl(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

// The last segment of a path, decoded where its escapes are well formed.
export function fileName(pathname: string): string {
  const segment = pathname.split("/").pop() || "";
  let decoded = segment;
  try {
    decoded = decodeURIComponent(segment);
  } catch {
    // A malformed escape: keep the segment as written.
  }
  return decoded || "download";
}
