The browser extension now opens tabs correctly when a popup has focus and
reconnects to Desktop without losing its identity or Stop state.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.1/chatwithwork-browser-0.1.1-unpacked.zip).
Extract it into a permanent folder and load that folder with **Load unpacked**
at `chrome://extensions`. For an existing installation, replace the files in
its existing folder and press **Reload** on its extension card. Confirm the
version is **0.1.1**. Your allowed sites and access settings stay as they were.

![The extension's Ready menu and allowed sites, illustrated with synthetic example settings](https://raw.githubusercontent.com/crmne/chatwithwork-desktop/browser-v0.1.1/browser/store/images/1-tab-group.png)

## Fixed

- **Opening a tab works while an app or popup has focus.** The tab joins Chat with Work's group in its own window. If grouping fails, the new blank tab is removed before the requested site loads. By @crmne ([5b86cb1](https://github.com/crmne/chatwithwork-desktop/commit/5b86cb1)).
- **Desktop restarts preserve the browser connection's identity and Stop state.** The extension announces itself when the native host reconnects. By @crmne ([2273f51](https://github.com/crmne/chatwithwork-desktop/commit/2273f51)).

## Known limitations

- The extension is not available in the Chrome Web Store yet. Manual installations need an explicit file replacement and Reload; they do not update when Desktop updates.
- The local browser CLI needs Desktop 0.9.0 or newer. Earlier Desktop browser tools remain compatible.
- Allowed sites, the Chat with Work tab group, protected fields and explicit approval for changes still apply. This update does not grant broader access.

## Thanks

- @crmne for reporting the installed-extension failures while testing the local browser CLI.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/v0.7.0...browser-v0.1.1
