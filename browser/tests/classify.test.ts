import assert from "node:assert/strict";
import { test } from "node:test";
import { isSensitiveField, submitReason, type ElementFacts } from "../src/classify.js";

const el = (facts: Partial<ElementFacts>): ElementFacts => ({ tag: "input", attrs: {}, role: "textbox", name: "", ...facts });

test("passwords, cards and one-time codes are sensitive", () => {
  assert.ok(isSensitiveField(el({ attrs: { type: "password" } })));
  assert.ok(isSensitiveField(el({ attrs: { autocomplete: "cc-number" } })));
  assert.ok(isSensitiveField(el({ attrs: { autocomplete: "section-pay cc-csc" } })));
  assert.ok(isSensitiveField(el({ attrs: { autocomplete: "one-time-code" } })));
  assert.ok(isSensitiveField(el({ attrs: { name: "cardNumber" } })));
  assert.ok(isSensitiveField(el({ attrs: { id: "cvv" } })));
  assert.ok(isSensitiveField(el({ name: "IBAN" })));
  assert.ok(isSensitiveField(el({ attrs: { placeholder: "Kennwort" } })));
});

test("ordinary fields aren't", () => {
  assert.equal(isSensitiveField(el({ attrs: { type: "email", name: "email" }, name: "Email" })), false);
  assert.equal(isSensitiveField(el({ attrs: { type: "search" }, name: "Search mail" })), false);
  assert.equal(isSensitiveField(el({ tag: "textarea", name: "Message" })), false);
  assert.equal(isSensitiveField(el({ tag: "button", role: "button", name: "Password settings" })), false);
});

test("submit buttons and action names submit", () => {
  assert.ok(submitReason(el({ attrs: { type: "submit" } })));
  assert.ok(submitReason({ tag: "button", attrs: {}, role: "button", name: "Next", form: { method: "post" } }));
  assert.ok(submitReason({ tag: "div", attrs: { role: "button" }, role: "button", name: "Send" }));
  assert.ok(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Place order" }));
  assert.ok(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Jetzt kaufen" }));
  assert.ok(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Elimina" }));
  assert.ok(submitReason({ tag: "a", attrs: { href: "/x", "data-turbo-method": "delete" }, role: "link", name: "Remove" }));
  assert.ok(submitReason({ tag: "a", attrs: { href: "/checkout" }, role: "link", name: "Checkout" }));
});

test("navigation and toggles don't", () => {
  assert.equal(submitReason({ tag: "a", attrs: { href: "/posts" }, role: "link", name: "Posts" }), null);
  assert.equal(submitReason({ tag: "a", attrs: { href: "/p/2" }, role: "link", name: "Next page" }), null);
  assert.equal(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Next page" }), null);
  assert.equal(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Show more" }), null);
  assert.equal(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "Bookmarks" }), null);
  assert.equal(submitReason({ tag: "div", attrs: {}, role: "tab", name: "Posts" }), null);
});
