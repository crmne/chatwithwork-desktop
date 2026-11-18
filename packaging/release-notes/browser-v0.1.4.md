The browser extension now reaches controls that pages draw without a name, says plainly when a banner covers a button, replaces rich text without taking on its old formatting, and stops a stuck page from holding up the browser.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.4/chatwithwork-browser-0.1.4-unpacked.zip).
The Chrome Web Store updates the extension by itself. For a manual installation, replace the files in its existing folder, press **Reload** at `chrome://extensions`, and confirm version **0.1.4**. Your saved site choices stay unchanged.

## Fixed

- **Unlabelled controls can be clicked.** A "×" that removes a row, or an "X" that closes a banner, drawn as a bare element with no role or name, now gets a reference when Chat with Work reads the page. It is named by its text or by the text beside it, and since a glyph doesn't say what it does, clicking one asks first with a screenshot. By @crmne in [#225](https://github.com/crmne/chatwithwork-desktop/issues/225).
- **A button under a fixed banner gets a clear answer.** The extension scrolls the button clear of fixed and sticky layers, trying the middle of the window. If something still covers it, the click fails at once, says that nothing was clicked, and names what covers it and where, such as a fixed banner along the bottom of the window, instead of answering that the result was uncertain. By @crmne in [#226](https://github.com/crmne/chatwithwork-desktop/issues/226).
- **Replacing text in a rich-text editor no longer copies its formatting.** Typing over a bold line used to make the new text bold. The extension now deletes the old text first, as a person would, then types. By @crmne in [#218](https://github.com/crmne/chatwithwork-desktop/issues/218).
- **A stuck page no longer holds up later steps.** When a page stops responding, for example right after a click, that step fails with a clear message and later steps on the tab fail within seconds instead of timing out. Other tabs keep working, and the stuck tab can be closed. By @crmne in [#218](https://github.com/crmne/chatwithwork-desktop/issues/218).

## Known limitations

- Desktop bundles the complete engine that types for most installations, so replacing rich text without its formatting there needs the next Desktop release as well. With this extension alone, only installations that use the extension's own engine get that fix. The other fixes are in the extension.
- The freeze reported in [#218](https://github.com/crmne/chatwithwork-desktop/issues/218) after clicking an editor's Bold button did not reproduce with Trix 2.1.19, the editor Basecamp uses. This release makes any page that hangs fail quickly instead of blocking later steps; it can't unfreeze the page itself.
- Finding unlabelled controls relies on what the page shows: a click handler, a pointer cursor or a tab stop. A control with none of these still reads as text. Line breaks are typed as line breaks, never as Enter, so in some editors several lines become one paragraph.
- Actions stay in Chat with Work's tab group. Protected fields, blocked sites and Stop still apply.

## Thanks

- @crmne for reporting the bold text and the frozen Bold button, the unreachable "×" controls, and the button under a banner.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/browser-v0.1.3...browser-v0.1.4
