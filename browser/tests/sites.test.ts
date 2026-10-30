import assert from "node:assert/strict";
import { test } from "node:test";
import { fileName, siteAllowed } from "../src/sites.js";

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

test("a file's name is its path's last segment, decoded where it can be", () => {
  assert.equal(fileName("/files/Q3%20report.pdf"), "Q3 report.pdf");
  assert.equal(fileName("/files/100%.csv"), "100%.csv");
  assert.equal(fileName("/files/"), "download");
  assert.equal(fileName("/"), "download");
});


test("all sites is explicit and blocked hosts win over either mode", () => {
  assert.equal(siteAllowed("https://example.com/", []), false);
  assert.equal(siteAllowed("http://unlisted.example:8080/", [], true), true);
  for (const all of [false, true]) {
    for (const url of ["https://example.com/", "https://sub.example.com/", "http://example.com:8080/", "https://example.com./"]) {
      assert.equal(siteAllowed(url, ["example.com", "http://example.com:8080"], all, ["example.com"]), false, url);
    }
  }
  assert.equal(siteAllowed("https://notexample.com/", [], true, ["example.com"]), true);
  assert.equal(siteAllowed("https://example.com.evil.test/", [], true, ["example.com"]), true);
  assert.equal(siteAllowed("http://localhost:3000/", [], true, ["http://localhost:3000"]), false);
  assert.equal(siteAllowed("http://localhost:3001/", [], true, ["http://localhost:3000"]), true);
  assert.equal(siteAllowed("https://localhost:3000/", [], true, ["http://localhost:3000"]), true);
});

test("all sites never grants non-web URLs or URLs containing credentials", () => {
  for (const url of ["file:///etc/passwd", "chrome://settings", "chrome-extension://abc/", "data:text/html,secret", "javascript:alert(1)", "ftp://example.com/", "https://user:password@example.com/", "not a url"]) {
    assert.equal(siteAllowed(url, [], true), false, url);
  }
});
