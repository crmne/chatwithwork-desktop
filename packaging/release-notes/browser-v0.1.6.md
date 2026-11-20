The browser extension now says plainly what happened when a click, a typed word or a choice can't finish, shows where links on your allowed sites go when Chat with Work reads a page, clicks a part of a link that shows when a bar covers its middle, and no longer leaves a new tab blank now and then.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.6/chatwithwork-browser-0.1.6-unpacked.zip).
The Chrome Web Store updates the extension by itself. For a manual installation, replace the files in its existing folder, press **Reload** at `chrome://extensions`, and confirm version **0.1.6**. Your saved site choices stay unchanged.

## New

- **Where a link goes, as far as Chat with Work could go anyway.** When a link goes to a site you allowed, a page reading now shows its path, such as `link "Style" [to=/admin/theme/style]`, with the site in front when it's another one (`[to=https://docs.example.com/guide]`), so when a click fails Chat with Work can open the address instead. Never the query, the fragment or a login; parts that look like tokens or ids show as `…`; a link to a site you didn't allow, or one that isn't a web address, shows nothing, not even its host. Addresses never leave the extension in full: only that shortened form does. By @crmne in [#267](https://github.com/crmne/chatwithwork-desktop/pull/267).

## Fixed

- **A browser action that can't finish says what happened.** When Chat with Work Desktop 0.19.0 gives up on a click, a typed word or a choice, the extension tells it whether any input reached the page, why not (disabled, hidden, covered, the page changed, Stop pressed, changes turned off), and if some did, what the page looks like now: the same address and document, reloaded, gone elsewhere, the element gone, a dialog dismissed. It looks for at most a second, and never after Stop, with changes off, or once the site isn't allowed. By @crmne in [#266](https://github.com/crmne/chatwithwork-desktop/pull/266).
- **A link partly covered by a bar's handle can be clicked.** The extension tries the visible parts of an element, not only its middle, and clicks one where the element itself is on top. One covered everywhere is still refused, naming what covers it. By @crmne in [#263](https://github.com/crmne/chatwithwork-desktop/pull/263).
- **A new tab no longer stays blank now and then.** Opening a site waits for the new tab to be ready before sending it there, two seconds at most. By @crmne in [#268](https://github.com/crmne/chatwithwork-desktop/pull/268).

![What Chat with Work hears back from two clicks that couldn't finish, before and now; from a test with synthetic data](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.6/browser-answer.png)

## Known limitations

- The plain answers need Chat with Work Desktop 0.19.0, and so do partly covered links wherever Desktop's bundled browser engine does the click; with an older Desktop, both work as before. Link paths and new tabs work with any Desktop that has the browser features.
- Each part of a link's path is judged on its own between its `/`s, so a token split across several short parts would still show.
- Links inside frames aren't shown, and a page shows at most 5,000.

## Thanks

- CodeRabbit and Greptile for reviewing every change.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/browser-v0.1.5...browser-v0.1.6
