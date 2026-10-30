Browser actions now use the complete Playwright engine bundled with Desktop 0.9.2, including the build used by Playwriter. Custom controls use its own action checks and event handling.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.3/chatwithwork-browser-0.1.3-unpacked.zip).
Replace the files in the existing extension folder, press **Reload** at `chrome://extensions`, and confirm version **0.1.3**. Update [Desktop to 0.9.2](https://github.com/crmne/chatwithwork-desktop/releases/tag/v0.9.2) too. Your saved site choices stay unchanged.

![The complete browser engine selects a styled radio control in a synthetic test page](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.3/engine-labelled-radio.png)

## Fixed

- **Clicks, typing and selections use the complete upstream engine.** Desktop supplies Playwright and its Node runtime; the extension connects it to the requested control in your browser. No separate runtime installation is needed. By @crmne; thanks @crmne for the TK examples and requesting the upstream implementation.
- **The requested control stays the target.** An action is tied to its tab, document and control. Navigating, disconnecting or pressing Stop invalidates it. Protected fields, site rules and existing approval choices still apply. By @crmne.
- **A failed action is not silently repeated by a different engine.** Once the new engine begins, failures return directly, avoiding a duplicate click or submission. By @crmne.

## Known limitations

- The extension is not in the Chrome Web Store yet. Manual installations need file replacement and Reload; updating Desktop alone does not update them.
- The new engine requires Desktop 0.9.2. Earlier Desktop versions and Linux Arm64 musl retain the previous browser implementation.
- Actions stay in Chat with Work’s tab group. Real overlays, protected fields and blocked sites can still prevent an action.

## Thanks

- @crmne for reporting the blocked controls and asking to use Playwriter’s complete engine.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/browser-v0.1.2...browser-v0.1.3
