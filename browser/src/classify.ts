// What kind of element a call is about to touch: a field the extension
// never types into or reads (passwords, payment cards, one-time codes), or
// a control whose click may send, buy, post or delete something. Pure, so
// it can be tested without a browser.

export interface ElementFacts {
  // Lowercase tag name, such as `input` or `button`.
  tag: string;
  // Its attributes, names lowercase.
  attrs: Record<string, string>;
  // Its accessible role and name, as the browser computes them.
  role: string;
  name: string;
  // The form it belongs to, if any.
  form?: { method: string } | null;
}

const SENSITIVE_AUTOCOMPLETE =
  /(^|\s)(cc-number|cc-csc|cc-exp|cc-exp-month|cc-exp-year|cc-type|current-password|new-password|one-time-code)(\s|$)/;

const SENSITIVE_WORDS =
  /(passw|passwd|passcode|kennwort|mot.?de.?passe|contrase|card.?num|cc.?num|credit.?card|kreditkarte|carta.?di.?credito|cvc|cvv|csc|security.?code|card.?code|card.?verif|expir|exp.?date|iban|routing.?num|account.?num|sort.?code|\bpin\b|one.?time|\botp\b|verification.?code|2fa|mfa.?code|totp)/i;

const TEXT_INPUT_TYPES = new Set(["", "text", "search", "tel", "email", "url", "number", "password"]);

// A field whose value the extension never reads, types into, or shows in a
// screenshot.
export function isSensitiveField(el: ElementFacts): boolean {
  const type = (el.attrs.type ?? "").toLowerCase();
  if (el.tag === "input" && type === "password") return true;
  const autocomplete = (el.attrs.autocomplete ?? "").toLowerCase();
  if (SENSITIVE_AUTOCOMPLETE.test(autocomplete)) return true;
  const isField =
    (el.tag === "input" && TEXT_INPUT_TYPES.has(type)) ||
    el.tag === "textarea" ||
    el.attrs.contenteditable !== undefined ||
    el.role === "textbox" ||
    el.role === "spinbutton";
  if (!isField) return false;
  const words = [
    el.attrs.name,
    el.attrs.id,
    el.attrs["aria-label"],
    el.attrs.placeholder,
    el.attrs["data-testid"],
    el.attrs.inputmode === "numeric" ? el.name : undefined,
    el.name,
  ]
    .filter(Boolean)
    .join(" ");
  return SENSITIVE_WORDS.test(words);
}

// Words that name an action that sends, buys, posts, publishes, deletes or
// otherwise commits something, in English, German, Italian, French and
// Spanish. Matched at the start of a word, so "Payment" and "Deleting"
// count.
const ACTION_STEMS = [
  // English
  "buy", "pay", "purchas", "order", "checkout", "check out", "send", "post", "publish",
  "submit", "delet", "remov", "confirm", "transfer", "sign", "approv", "place", "donat",
  "subscri", "unsubscri", "reply", "share", "tweet", "merge", "deploy", "book now", "booking", "reserv",
  "destroy", "erase", "archiv", "invite", "accept", "decline", "reject", "withdraw",
  "close account", "deactivat", "revok", "save", "apply", "upload", "finish", "complete",
  // German
  "kauf", "bezahl", "bestell", "absend", "senden", "veröffentlich", "lösch", "entfern",
  "bestätig", "überweis", "unterschreib", "unterzeichn", "buch", "abschick", "speicher",
  "zahlungspflichtig", "kündig",
  // Italian
  "acquist", "compra", "paga", "ordina", "invia", "pubblica", "elimina", "cancella", "rimuov",
  "conferma", "trasferi", "firma", "prenota", "salva", "inoltra",
  // French
  "achet", "payer", "commander", "envoy", "publier", "supprim", "effac", "confirm", "virement",
  "sign", "réserv", "enregistr", "valider", "résili",
  // Spanish
  "comprar", "pagar", "pedir", "enviar", "publicar", "elimin", "borrar", "confirm", "transfer",
  "firmar", "reserv", "guardar",
];

// Links usually only navigate, so only the strongest words make one an
// action.
const LINK_STEMS = [
  "buy", "pay", "purchas", "checkout", "delet", "remov", "unsubscri", "destroy", "transfer",
  "kauf", "bezahl", "lösch", "kündig", "acquist", "paga", "elimina", "achet", "payer",
  "supprim", "résili", "comprar", "pagar", "elimin", "borrar",
];

const ACTION_RE = stemsRegex(ACTION_STEMS);
const LINK_RE = stemsRegex(LINK_STEMS);

function stemsRegex(stems: string[]): RegExp {
  const escaped = [...new Set(stems)].map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  return new RegExp(`(^|[^\\p{L}\\p{N}])(${escaped.join("|")})`, "iu");
}

const BUTTON_ROLES = new Set(["button", "menuitem"]);

// Why clicking this element may send, buy, post or delete something, or
// null when it looks like plain navigation or a toggle.
export function submitReason(el: ElementFacts): string | null {
  const type = (el.attrs.type ?? "").toLowerCase();
  if (el.tag === "input" && (type === "submit" || type === "image")) {
    return "it submits a form";
  }
  if (el.tag === "button" && el.form && (type === "" || type === "submit")) {
    return "it submits a form";
  }
  const method = (el.attrs["data-method"] ?? el.attrs["data-turbo-method"] ?? "").toLowerCase();
  if (method && method !== "get") return `it sends a ${method.toUpperCase()} request`;
  if (el.attrs.formaction !== undefined) return "it submits a form";
  const label = [el.name, el.attrs.value, el.attrs["aria-label"], el.attrs.title]
    .filter(Boolean)
    .join(" ");
  const isLink = el.role === "link" || (el.tag === "a" && el.attrs.href !== undefined);
  if (isLink) {
    return LINK_RE.test(label) ? `it's named “${short(label)}”` : null;
  }
  const isButton =
    BUTTON_ROLES.has(el.role) ||
    el.tag === "button" ||
    (el.tag === "input" && type === "button") ||
    el.attrs.role === "button";
  if (isButton && ACTION_RE.test(label)) return `it's named “${short(label)}”`;
  return null;
}

function short(text: string): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length > 60 ? `${clean.slice(0, 59)}…` : clean;
}

// The same patterns, for the script that blurs fields in screenshots.
export const SENSITIVE_PATTERNS = {
  autocomplete: SENSITIVE_AUTOCOMPLETE.source,
  words: SENSITIVE_WORDS.source,
};
