# Chat with Work browser extension

Lets Chat with Work use tabs in its own tab group, "Chat with Work", in your Chromium-family browser (Chrome, Chromium, Edge, Brave, Vivaldi), on sites you allow, through Chat with Work Desktop. How it works and what it never does: [docs/browser.md](../docs/browser.md). The wire contract: [PROTOCOL.md 8.16](../PROTOCOL.md) and [CONTROL.md, Browser](../CONTROL.md).

It's TypeScript compiled by `tsc`, with no runtime dependencies and no bundler.

```sh
npm ci
npm run build      # build/: the unpacked extension
npm test           # unit tests (sites, classification, the page as text)
npm run e2e        # Chromium with the extension and `cww` as its native host
npm run package    # chatwithwork-browser.zip for the Chrome Web Store
npm run package:unpacked  # versioned ZIP for manual installation
```

`npm run e2e` builds `cww` (`cargo build --bin cww` in the repository root) if it isn't built, starts a throwaway Chromium profile with the extension loaded and `cww` registered as its native messaging host in that profile, and stands in for the daemon on the control socket, except `daemon.spec.ts`, which runs the real `cww daemon run`. `HEADED=1` shows the browser.

## Trying it

1. Download the [0.1.3 manual installation ZIP](https://github.com/crmne/chatwithwork-desktop/releases/download/browser-v0.1.3/chatwithwork-browser-0.1.3-unpacked.zip), extract it into a folder you will keep, then select that folder at `chrome://extensions` with "Load unpacked" (Developer mode on). To build from source instead, run `npm run build` and select `browser/build`.
2. `cww browser enable` registers `cww` with the browsers it finds and turns on the browser tools; `cww browser allow github.com` allows a site.
3. The extension's menu says whether it reached Chat with Work Desktop. Drag a tab into a group named "Chat with Work", or let Chat with Work open one.

To update a manual installation, replace the files in its existing folder with
the newly extracted files, then press **Reload** on its card at
`chrome://extensions`. Confirm the card shows **0.1.3**. Updating Desktop alone
does not update an unpacked extension. Keep using the `-unpacked.zip` download:
the store upload ZIP intentionally omits the public key that keeps a manual
installation's identity stable.

## Releasing the extension

Its version is independent of Desktop's. Bump `static/manifest.json`,
`package.json` and the root package entries in `package-lock.json` together.
Run `npm test`, `npm run test:package` and the Chromium end-to-end tests, then
prepare both `npm run package` (store upload) and `npm run package:unpacked`
(manual installation). Neither command reads or packages a private key.

After main's CI and the release review pass, tag `browser-v<version>` on main
and publish the manual ZIP with the committed notes in
`packaging/release-notes/browser-v<version>.md`. Use `gh release create
browser-v<version> <manual-zip> --latest=false --notes-file <notes>`: extension
releases must never replace Desktop as the repository's latest release, which
the Desktop updater reads. There is no automatic browser-tag workflow.
Uploading to the Chrome Web Store is a separate, explicitly authorized step
in [the store checklist](store/checklist.md).

## The extension's ID

`static/manifest.json` carries the public half of the extension's key, so the unpacked build and the Chrome Web Store item have the same ID, `ehjfihpogoacjodjnanclodinjnnpmcp`, which `cww` allows in its native messaging manifest. The private key is kept outside the repository and uploaded once, as `key.pem` in the first package sent to the store; `npm run package` drops the `key` field the store doesn't accept. A build with another key needs its ID in `[browser] extension_ids` in `config.toml`.
