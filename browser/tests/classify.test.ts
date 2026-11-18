import assert from "node:assert/strict";
import { test } from "node:test";
import { CLASSIFY_IN_PAGE, isSensitiveField, submitReason, type ElementFacts } from "../src/classify.js";

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

test("an unlabelled control the page made clickable is judged like a button (#225)", () => {
  // A bare "×" or an icon says nothing about what it does: it may remove.
  assert.ok(submitReason({ tag: "span", attrs: {}, role: "generic", name: "×" }));
  assert.ok(submitReason({ tag: "div", attrs: { tabindex: "0" }, role: "generic", name: "" }));
  assert.ok(submitReason({ tag: "a", attrs: {}, role: "generic", name: "✕" }));
  // Its title, label or text names it like any button's.
  assert.ok(submitReason({ tag: "div", attrs: {}, role: "generic", name: "X" }));
  assert.ok(submitReason({ tag: "span", attrs: { title: "Remove row" }, role: "generic", name: "×" }));
  assert.ok(submitReason({ tag: "div", attrs: {}, role: "generic", name: "Delete" }));
  assert.equal(submitReason({ tag: "span", attrs: {}, role: "generic", name: "Show details" }), null);
  // Controls that say what they are keep their rules.
  assert.equal(submitReason({ tag: "button", attrs: { type: "button" }, role: "button", name: "×" }), null);
  assert.equal(submitReason({ tag: "a", attrs: { href: "/x" }, role: "link", name: "›" }), null);
});

test("the classification in the page is the same one", () => {
  const classify = new Function(`return ${CLASSIFY_IN_PAGE}`)() as (facts: ElementFacts) => { sensitive: boolean; submits: string | null };
  for (const facts of <ElementFacts[]>[
    { tag: "span", attrs: {}, role: "", name: " ×" },
    { tag: "div", attrs: { title: "Remove row" }, role: "", name: "" },
    { tag: "span", attrs: {}, role: "", name: "Show details" },
    { tag: "button", attrs: { type: "button" }, role: "", name: "×" },
  ]) {
    assert.equal(classify(facts).submits, submitReason(facts), JSON.stringify(facts));
  }
});
