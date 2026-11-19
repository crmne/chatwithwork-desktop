The browser extension can now upload files from a folder you share on your computer to a site, and click, drag and press keys on pages drawn without buttons, working from a screenshot with the spot marked on the card.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.5/chatwithwork-browser-0.1.5-unpacked.zip).
The Chrome Web Store updates the extension by itself. For a manual installation, replace the files in its existing folder, press **Reload** at `chrome://extensions`, and confirm version **0.1.5**. Your saved site choices stay unchanged.

## New

- **Upload files to a site.** With Chat with Work Desktop 0.18.0, the extension hands files from a folder you share to a page's upload field, to a button that opens a file picker, or to a drop zone, in the "Chat with Work" tab group. Each upload is approved by you in Chat with Work first, every time. The extension never opens or reads a file: it gives the browser the private copy Desktop made of exactly the file you approved, then checks the field holds it, by name and size. It refuses password, card and one-time code fields, a file the field doesn't accept, and fields inside another site's frame. By @crmne in [#245](https://github.com/crmne/chatwithwork-desktop/pull/245).
- **Allow access to file URLs, explained.** Browsers let an extension hand files to pages only while this switch is on, and it's off for a Chrome Web Store install. The extension's menu shows whether it's on and where to turn it on (`chrome://extensions`, Chat with Work's **Details**), and an upload asked for while it's off says the same. Nothing else uses it. By @crmne in [#245](https://github.com/crmne/chatwithwork-desktop/pull/245).
- **Click, drag and press keys by position.** On a canvas, a drawn grid or a custom control, Chat with Work works from a screenshot of the tab: a click, double-click or drag at a spot, pointing to show a tooltip, turning the wheel over one part of a page, and keys such as Tab, Enter, Escape and the arrows in whatever has focus. Each action is checked against the screenshot it points at, refused if the page moved, scrolled, resized or zoomed since, and checked again before it runs: what is under the spot follows the same rules as a named button, and a control the page draws itself counts as one that may send something, so it asks with the spot marked on the screenshot. By @crmne in [#248](https://github.com/crmne/chatwithwork-desktop/pull/248).

## Known limitations

- Uploads and clicking by position need Chat with Work Desktop 0.18.0. With an older Desktop, the extension works as 0.1.4 did.
- Sites that pick files with the File System Access API (`showOpenFilePicker`) can't be uploaded to, since no extension can answer that picker.
- Spots and fields inside another site's frame are refused.
- Actions stay in Chat with Work's tab group. Protected fields, blocked sites and Stop still apply.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/browser-v0.1.4...browser-v0.1.5
