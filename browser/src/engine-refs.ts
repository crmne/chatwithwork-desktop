// A model ref is a backend node in one document, not a reusable selector.
// Remember only refs actually returned by browser_read. Never equate these
// with Playwright's independently numbered aria-ref selectors.
import { send } from "./cdp.js";
import { ToolError } from "./errors.js";
export interface DocumentIdentity { frame: string; loader: string; root: number }
const seen = new Map<number, { document: DocumentIdentity; refs: Set<number> }>();
export async function documentIdentity(tab: number): Promise<DocumentIdentity> {
  const { frameTree } = await send(tab, "Page.getFrameTree");
  const { root } = await send(tab, "DOM.getDocument", { depth: 0 });
  return { frame: frameTree.frame.id, loader: frameTree.frame.loaderId, root: root.backendNodeId };
}
export function sameDocument(a: DocumentIdentity, b: DocumentIdentity): boolean {
  return a.frame === b.frame && a.loader === b.loader && a.root === b.root;
}
export function rememberRefs(tab: number, document: DocumentIdentity, text: string): void {
  let entry = seen.get(tab);
  if (!entry || !sameDocument(entry.document, document)) {
    entry = { document, refs: new Set() }; seen.set(tab, entry);
  }
  for (const match of text.matchAll(/\[ref=e(\d+)\]/g)) if (entry.refs.size < 20_000) entry.refs.add(Number(match[1]));
}
export async function requireRef(tab: number, ref: number): Promise<DocumentIdentity> {
  const entry = seen.get(tab);
  const current = await documentIdentity(tab);
  if (!entry?.refs.has(ref) || !sameDocument(entry.document, current)) {
    throw new ToolError("not_found", "that ref belongs to an earlier page; read the page again");
  }
  return current;
}
chrome.tabs.onRemoved.addListener(tab => seen.delete(tab));
