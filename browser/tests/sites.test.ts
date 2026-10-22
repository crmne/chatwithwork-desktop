import assert from "node:assert/strict";
import { test } from "node:test";
import { siteAllowed } from "../src/sites.js";

test("a bare host allows it and its subdomains over HTTPS only", () => {
  const sites = ["github.com"];
  assert.equal(siteAllowed("https://github.com/crmne", sites), true);
  assert.equal(siteAllowed("https://gist.github.com/x", sites), true);
  assert.equal(siteAllowed("http://github.com/", sites), false);
  assert.equal(siteAllowed("https://github.com:8443/", sites), false);
  assert.equal(siteAllowed("https://notgithub.com/", sites), false);
  assert.equal(siteAllowed("https://github.com.evil.example/", sites), false);
});

test("an origin allows exactly that scheme, host and port", () => {
  const sites = ["http://localhost:3000"];
  assert.equal(siteAllowed("http://localhost:3000/chats", sites), true);
  assert.equal(siteAllowed("http://localhost:3001/", sites), false);
  assert.equal(siteAllowed("https://localhost:3000/", sites), false);
  assert.equal(siteAllowed("http://a.localhost:3000/", sites), false);
});

test("never other schemes, credentials in URLs, or odd entries", () => {
  assert.equal(siteAllowed("file:///etc/passwd", ["github.com"]), false);
  assert.equal(siteAllowed("chrome://settings", ["settings"]), false);
  assert.equal(siteAllowed("javascript:alert(1)", ["github.com"]), false);
  assert.equal(siteAllowed("https://user:pw@github.com/", ["github.com"]), false);
  assert.equal(siteAllowed("https://github.com/", ["*.github.com"]), false);
  assert.equal(siteAllowed("https://github.com/", ["com"]), false);
  assert.equal(siteAllowed("https://github.com/", [""]), false);
  assert.equal(siteAllowed("not a url", ["github.com"]), false);
});

test("hosts compare as the browser writes them", () => {
  assert.equal(siteAllowed("https://GitHub.com/", ["GITHUB.COM"]), true);
  assert.equal(siteAllowed("https://xn--mnchen-3ya.de/", ["münchen.de"]), true);
});
