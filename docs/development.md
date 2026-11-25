# Development

Back to the [README](../README.md).


```sh
cargo test                                  # unit tests and the end-to-end test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo deny check                            # licenses, advisories, bans, sources
```

`tests/e2e.rs` runs a real daemon against a fake Chat with Work server: DPoP-checked token endpoint, device flow, and an Action Cable WebSocket. It covers reads outside the roots, denied secrets, symlink, junction and hard-link escapes, pause, revocation, and every change tool with its escapes, the deny lists, read-only folders, limits and the trash. `tests/sandbox.rs` checks under the real kernel sandbox that only folders that allow changes, and their trash, can be written. CI runs everything on Linux, macOS and Windows, and builds the Nix package.

| Module | Role |
|---|---|
| `reader` | Everything that touches your files: safe path resolution, extraction, the index, grep, watching. This is the future sandboxed process. |
| `tools` | The MCP server (via [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk)): the tools, limits, audit. |
| `writer`, `trash` | Changes in folders that allow them (path rules, kinds of file, Word and Excel documents), and the system trash on each platform. |
| `tunnel` | The outbound WebSocket, framing, reconnects. |
| `auth` | Device key, DPoP proofs, device flow, secret storage. |
| `daemon`, `control`, `service` | The daemon, its control channel ([CONTROL.md](../CONTROL.md)), and systemd, launchd and Scheduled Task registration. |
| `tui` | The terminal UI, a client of the control socket. Screens are pinned as text snapshots in `src/tui/snapshots`; `UPDATE_SNAPSHOTS=1 cargo test` rewrites them. |
| `app/` | The desktop app (`cww-app`), on Tauri: the web app's window, the tray item, and the bundled settings pages (`app/ui`), the only part that is a client of the control socket. See [docs/desktop-app.md](desktop-app.md). |

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed, and packaged with [native-packages](https://github.com/crmne/native-packages); see [PACKAGING.md](../PACKAGING.md).
