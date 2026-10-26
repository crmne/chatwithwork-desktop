# Submitting to the Chrome Web Store

Nothing here is submitted without Carmine's go-ahead. In order:

## Before the first upload

- [ ] The browser work is on `main`: the extension, the native messaging host and the daemon's tools (crmne/chatwithwork-desktop#20 and #21), and the server side (crmne/chatwithwork, `Desktop::Device::BrowserTool`, on main).
- [ ] A Chat with Work Desktop release that includes them, so reviewers and people can install a `cww` that answers the extension. `cww browser enable` (or Your Browser in the desktop app's settings, crmne/chatwithwork-desktop#23) registers the host.
- [ ] The privacy policy at https://chatwithwork.com/privacy-policy has its "Your browser" paragraph (on main in crmne/chatwithwork; live once deployed).
- [ ] The privacy policy also has the extension's Limited Use statement, which the store requires on the policy it links (its Google one covers only Google APIs), next to the Google one in crmne/chatwithwork's `app/views/home/privacy_policy.html.erb`: "The Chat with Work browser extension's use of information it receives from your browser adheres to the Chrome Web Store User Data Policy, including the Limited Use requirements." with a link to https://developer.chrome.com/docs/webstore/program-policies/policies.
- [x] The extension's private key is in 1Password, and nowhere else: the Document item "Chat with Work browser extension key" in the chatwithwork vault (`op://chatwithwork/Chat with Work browser extension key/key.pem`). The public half is the manifest's `key`, which gives the ID `ehjfihpogoacjodjnanclodinjnnpmcp` that `cww` allows (`src/browsing/bridge.rs`, `EXTENSION_ID`).
- [ ] A Chrome Web Store developer account for Plenty UG (one-time fee, 2-step verification, the publisher's contact email verified). Group publisher if others should manage it.
- [ ] A reviewer test account on chatwithwork.com with credits, for the dashboard's private test-instructions field.

## The package

- [ ] `cd browser && npm ci && npm test && npm run e2e`, all green.
- [ ] Bump `version` in `static/manifest.json` and `package.json` together.
- [ ] `npm run package` writes `chatwithwork-browser.zip` without the manifest's `key` field.
- [ ] First upload only: add the private key to the zip as `key.pem` at its root, so the store keeps our ID: `(umask 077; op-cache read "op://chatwithwork/Chat with Work browser extension key/key.pem" > build/key.pem) && (cd build && zip -q ../chatwithwork-browser.zip key.pem)`. Check the ID the dashboard shows is `ehjfihpogoacjodjnanclodinjnnpmcp` before anything else; if it isn't, stop: `cww` won't let that extension in. Never commit `key.pem`, and delete `build/key.pem` and the zip with it in after the upload.

## The listing

- [ ] Paste the fields from `listing.md`: summary, description, category, single purpose, each permission's justification, remote code (No), data usage (Website content, Web history, and the categories allowed pages can show, as `listing.md` lists them), the three certifications, privacy policy URL, homepage and support URLs.
- [ ] Upload `images/` (three 1280×800 screenshots and the 440×280 tile).
- [ ] Test instructions from `listing.md`, with the test account in the private field.
- [ ] Visibility: Public. Distribution: all regions.
- [ ] Submit for review, and say so in the release notes once it's live. Reviews of extensions asking for `debugger` and `<all_urls>` can take weeks; Playwriter is the stopgap only if review blocks the launch.

## After it's live

- [ ] Install it from the store in a throwaway profile, `cww browser enable`, and check the menu says Ready.
- [ ] Edge: the same package is accepted from the Chrome Web Store; publishing on Microsoft Edge Add-ons as well would give it another ID, which `cww` would need to allow (`bridge::origins`).
