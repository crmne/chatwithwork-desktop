// Whether a file is one an upload field takes, by its `accept` attribute,
// the way the browser's own file picker filters: `.png` by the name's
// ending, `image/*` by the type's family, `image/png` by the whole type.
// Chrome doesn't hold a page's `accept` against files handed to it, so the
// extension does. Pure, so it can be tested without a browser.

export function accepts(accept: string | undefined, name: string, mimeType: string): boolean {
  const tokens = (accept ?? "")
    .split(",")
    .map((token) => token.trim().toLowerCase())
    .filter(Boolean);
  if (tokens.length === 0) return true;
  const lowerName = name.toLowerCase();
  const type = mimeType.toLowerCase();
  return tokens.some((token) => {
    if (token.startsWith(".")) return lowerName.endsWith(token) && lowerName.length > token.length;
    if (token.endsWith("/*")) return type.startsWith(token.slice(0, -1));
    return token === type;
  });
}

// "the 2nd file (.txt)": a file named in a refusal without its name, which
// may say more than the person meant to share.
export function fileOrdinal(index: number, count: number, name: string): string {
  const dot = name.lastIndexOf(".");
  const kind = dot > 0 && name.length - dot <= 10 ? ` (${name.slice(dot).toLowerCase()})` : "";
  if (count === 1) return `the file${kind}`;
  const n = index + 1;
  const suffix = n % 10 === 1 && n !== 11 ? "st" : n % 10 === 2 && n !== 12 ? "nd" : n % 10 === 3 && n !== 13 ? "rd" : "th";
  return `the ${n}${suffix} file${kind}`;
}

// Words that say a button picks files to upload, in English, German,
// Italian, French and Spanish.
export const UPLOAD_WORDS =
  /(upload|choose.?file|select.?file|browse|attach|add.?file|add.?photo|add.?image|drop.?file|hochladen|datei.?ausw|durchsuchen|carica|scegli.?file|allega|t[ée]l[ée]verser|choisir.?(un.?)?fichier|joindre|parcourir|subir|cargar|seleccionar.?archivo|adjuntar|examinar)/i;

// Words that say an element takes files dropped on it.
export const DROP_WORDS =
  /(drag.{0,12}drop|drop.{0,20}(here|files?|them)|ziehen|hierher|trascina|rilascia|glisser|d[ée]poser|arrastr|suelt)/i;
