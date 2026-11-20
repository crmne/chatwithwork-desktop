import assert from "node:assert/strict";
import { test } from "node:test";
import { linkTarget, tokenLike, LINK_TARGET_LIMIT } from "../src/links.js";

const allowed = { sites: ["example.com", "http://localhost:3000"], all_sites: false, blocked_sites: [] };
const page = "https://example.com/admin";

test("token-like segments: long hex, base64, JWT parts, UUIDs and over-long numbers", () => {
  for (const token of [
    "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", // SHA-256 hex
    "deadbeefcafebabe1234", // hex
    "deadbeefcafebabe", // hex, letters only
    "dGhpcyBpcyBhIHNlY3JldCB0b2tlbg==", // base64 with padding
    "dGhpcyBpcyBhIHNlY3JldA", // base64url without padding
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U", // JWT
    "123e4567-e89b-12d3-a456-426614174000", // UUID
    "12345678901234567890123", // longer than any record number
    "AbCdEfGhIjKlMnOpQrStUvWxYz", // mixed-case random letters
    "a8Kd93kfLq0Zm2Xp", // 16 random letters and digits
    "reset-Zx81kQm2Lp0Wn4Rt", // a token behind a word
    "abcdefghijklmnopqrstuvwxyzabcdefgh", // 34 letters in one run
  ]) assert.equal(tokenLike(token), true, token);
});

test("names, slugs, dates and record numbers stay", () => {
  for (const name of [
    "admin", "theme", "style", "2024-annual-report", "how-to-configure-your-account-settings",
    "12345", "9007199254740991", "v2", "README.md", "a1b2c3d4", "cus_NffrFeUfNV2Hib",
    "Q3-2026", "page-17", "en-US", "settings.json", "%E2%9C%93-done",
  ]) assert.equal(tokenLike(name), false, name);
});

test("a link on the page's own site shows its path only", () => {
  assert.equal(linkTarget("https://example.com/admin/theme/style", page, allowed), "/admin/theme/style");
  assert.equal(linkTarget("https://example.com/", page, allowed), "/");
  assert.equal(linkTarget("https://example.com/orders/12345/edit", page, allowed), "/orders/12345/edit");
});

test("nothing of the query, fragment or credentials, and tokens as …", () => {
  assert.equal(linkTarget("https://example.com/reset?token=s3cr3t&email=a%40b.c#step-2", page, allowed), "/reset");
  assert.equal(linkTarget("https://example.com/password/reset/a8Kd93kfLq0Zm2XpQ7", page, allowed), "/password/reset/…");
  assert.equal(linkTarget("https://example.com/share/123e4567-e89b-12d3-a456-426614174000/view", page, allowed), "/share/…/view");
  assert.equal(linkTarget("https://example.com/d/eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJlMTIzNA/file.pdf", page, allowed), "/d/…/file.pdf");
  // A user and password in a link: not something browser_open would open.
  assert.equal(linkTarget("https://user:pass@example.com/admin", page, allowed), undefined);
  for (const href of [
    "https://example.com/reset?token=s3cr3t",
    "https://example.com/a/b#access_token=abc",
  ]) {
    const shown = linkTarget(href, page, allowed)!;
    assert.ok(!shown.includes("?") && !shown.includes("#") && !shown.includes("s3cr3t") && !shown.includes("access_token"), shown);
  }
});

test("another allowed site shows its origin with the path; any other site shows nothing", () => {
  assert.equal(linkTarget("https://docs.example.com/guide", page, allowed), "https://docs.example.com/guide");
  assert.equal(linkTarget("http://localhost:3000/admin?x=1", page, allowed), "http://localhost:3000/admin");
  assert.equal(linkTarget("https://evil.test/collect", page, allowed), undefined);
  assert.equal(linkTarget("http://example.com/admin", page, allowed), undefined); // HTTP isn't the allowed HTTPS
  assert.equal(linkTarget("https://example.com/admin", page, { ...allowed, blocked_sites: ["example.com"] }), undefined);
  assert.equal(linkTarget("https://anything.test/x", page, { sites: [], all_sites: true, blocked_sites: ["anything.test"] }), undefined);
  assert.equal(linkTarget("https://other.test/x", page, { sites: [], all_sites: true, blocked_sites: [] }), "https://other.test/x");
});

test("non-web links, and markup in a path, never come through", () => {
  for (const href of ["javascript:alert(1)", "mailto:a@example.com", "tel:+1555", "data:text/html,hi", "blob:https://example.com/x", "not a url"]) {
    assert.equal(linkTarget(href, page, allowed), undefined, href);
  }
  const shown = linkTarget("https://example.com/a[ref=e9]/\"b\"", page, allowed)!;
  assert.ok(!/[\[\]"]/.test(shown), shown);
  const long = linkTarget(`https://example.com/${"section/".repeat(60)}`, page, allowed)!;
  assert.ok(long.length <= LINK_TARGET_LIMIT && long.endsWith("…"), long);
});
