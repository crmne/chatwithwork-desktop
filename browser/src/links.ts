// Where a link goes, as `browser_read` shows it (#261): only what
// `browser_open` could open anyway, and only the path. A link to a site the
// person allowed shows its path (with its origin when it isn't the page's
// own), never its query, fragment or credentials, and a path segment that
// looks like a token is shown as `…`. A link anywhere else shows nothing, not
// even its host.

import { siteAllowed } from "./sites.js";

export interface LinkSites {
  sites: readonly string[];
  all_sites: boolean;
  blocked_sites: readonly string[];
}

export const LINK_TARGET_LIMIT = 200;

// A path segment that looks like a secret rather than a name: long random
// letters and digits, hex, base64, a JWT's parts, a UUID, or a number too long
// to be a record id. Words joined by hyphens, dates, short ids and record
// numbers stay, and so does anything under 16 characters.
export function tokenLike(segment: string): boolean {
  let text = segment;
  try { text = decodeURIComponent(segment); } catch { /* shown as encoded */ }
  // UUIDs: often a record's id, but as often an unguessable share, invite or
  // reset link, which this can't tell apart; a click still follows one.
  // Anywhere in the segment, so a prefix (`invite-<uuid>`) doesn't hide one.
  if (/[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}/i.test(text)) return true;
  if (/^\d+$/.test(text)) return text.length > 19;
  // Base64's own characters (and padding) in a long segment; a `/` only
  // when encoded, since a literal one splits the path into segments.
  if (text.length >= 16 && /[+/=]/.test(text) && /^[A-Za-z0-9+/=_-]+$/.test(text)) return true;
  // Base64url (`-` and `_`, no padding) before it's split at them: a long run
  // mixing upper and lower case with digits. Word slugs don't.
  if (text.length >= 20 && /^[A-Za-z0-9_-]+$/.test(text) && /[a-z]/.test(text) && /[A-Z]/.test(text) && /[0-9]/.test(text)) return true;
  for (const part of text.split(/[-_.~:,;!@$&'()*]+/)) {
    if (/^\d+$/.test(part)) { if (part.length > 19) return true; continue; }
    if (part.length >= 16 && /[0-9]/.test(part) && /[A-Za-z]/.test(part)) return true;
    if (part.length >= 16 && /^[0-9a-f]+$/i.test(part)) return true;
    if (part.length >= 24 && /[a-z]/.test(part) && /[A-Z]/.test(part)) return true;
    if (part.length >= 32) return true;
  }
  return false;
}

// What `browser_read` shows after a link's ref, or undefined for nothing.
// `href` is the link's address resolved against its document; `page` the
// tab's address.
export function linkTarget(href: string, page: string, sites: LinkSites): string | undefined {
  let url: URL;
  try { url = new URL(href); } catch { return undefined; }
  if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
  if (url.username || url.password) return undefined;
  if (!siteAllowed(url.href, sites.sites, sites.all_sites, sites.blocked_sites)) return undefined;
  let origin = "";
  try { origin = new URL(page).origin === url.origin ? "" : url.origin; } catch { origin = url.origin; }
  const segments = url.pathname.split("/").slice(1)
    .map((segment) => (tokenLike(segment) ? "…" : segment));
  // Never anything that would read as markup in the read text; then the cap,
  // which never splits an encoded character.
  let path = ("/" + segments.join("/")).replace(/[\s"\]\[<>\\`]/g, (c) => encodeURIComponent(c));
  if (path.length > LINK_TARGET_LIMIT) {
    path = path.slice(0, LINK_TARGET_LIMIT - 1).replace(/%[0-9A-Fa-f]?$/, "") + "…";
  }
  return origin + path;
}
