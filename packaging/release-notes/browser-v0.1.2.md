The browser extension now supports both an allowed-site list and all sites except the ones you block. Blocked sites take priority, including when a page redirects.

[Download the manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.2/chatwithwork-browser-0.1.2-unpacked.zip).
For an existing installation, replace the files in its existing folder and press **Reload** at `chrome://extensions`. Confirm the version is **0.1.2**. Your previous site choices stay unchanged.

![The extension menu with all sites and a blocked site, using synthetic demo content](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.2/browser-menu-light.png)

![The extension menu in the dark theme, using synthetic demo content](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.2/browser-menu-dark.png)

## New

- **Use only selected sites, or all sites with exceptions.** Set the policy in Desktop 0.9.1. Both Desktop and the extension enforce it. A blocked host includes its subdomains; an explicit origin matches that origin. By @crmne.
- **The extension reports when its site policy needs an update.** Older Desktop versions retain their existing allowed-site behavior. Desktop refuses advanced policy on an extension that cannot enforce it. By @crmne.

## Fixed

- **Cookie banners and other custom buttons can be clicked.** Buttons built with web components no longer get mistaken for covered controls. Real overlays still block clicks. By @crmne; thanks @crmne for the TK example.
- **Styled options work through their own labels.** Radio buttons and checkboxes can use a visible label when the input itself is hidden. Previews leave the option unchanged; actual clicks check that the label still belongs to the requested control and refuse other controls or overlays inside it. By @crmne; thanks @crmne for reporting the blocked radio option.
- **Links use the visible part of the control.** Clicks now reuse Playwright's hit checks from Playwriter, handling clipped links, slotted labels and nested components. Previews detect blocking overlays, and clicks wait briefly for moving or disabled controls without forcing through a real obstruction. By @crmne.

## Known limitations

- The extension is not in the Chrome Web Store yet. Manual installations need file replacement and Reload; updating Desktop alone does not update them.
- The new site controls require Desktop 0.9.1. Existing allowed sites remain the default until you explicitly choose broader access.
- Browser actions stay within Chat with Work’s tab group. Protected fields and Stop remain enforced, including when Desktop’s **Ask before browser actions** setting is off.

## Thanks

- @crmne for requesting simpler browser access controls and site exclusions.

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/browser-v0.1.1...browser-v0.1.2
