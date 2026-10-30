# Fixed browser engine helper

This private helper runs the unchanged, integrity-pinned
`@xmorse/playwright-core@1.59.13` used by Playwriter. It uses `CRBrowser` and
`ElementHandle.click`, `fill`, `press` and `selectOption`, including their
upstream actionability, scrolling, hit interception, trusted input, waits and
state verification. It does not run Playwriter's arbitrary-JavaScript executor.

The daemon selects this capability before an operation, after verifying the
bundled runtime and an extension advertising `engine_protocol: 1`. Older
extensions and packages without the runtime retain the released engine.
There is no retry or fallback to another implementation after admission or
possible input. ARM64 musl retains the released engine until a supported
runtime is available.

## Private protocol

`entry.mjs` is a fixed entry loaded by bundled Node 22.23.4 with
`--no-addons --no-global-search-paths`. No runtime is downloaded on startup.
The native wrapper sanitizes environment and verifies the complete private
module tree. Node 22 preserves macOS 11 support; make the next runtime/support
decision before its upstream end of support on 30 April 2027.

The helper emits `{type:"ready",protocol:1}` after loading. Initialization
performs no browser access. The daemon prewarms under confinement, without a
browser lease, separately from the bounded real operation. A call carries only
an extension-issued operation for click/type/select. JSON lines are bounded
at 2 MiB, cumulative traffic at 16 MiB, commands at 512, and the action deadline
at the lesser of 14 seconds and the remaining native operation budget.

Commands and events travel only through inherited pipes and the existing
native-messaging connection. There is no socket listener, CDP endpoint, script
path, selector, force flag or arbitrary-code tool. The private extension lease
binds the current connection, tab group, site, read ref and document identity.
It rejects concurrent admission and validates local policy during execution.
Input events retain engine order even when policy checks are asynchronous.

CWW refs are backend DOM node IDs in their observed document, not Playwright
`aria-ref` IDs. The fixed bridge adopts the validated backend node. Main-frame
shadow roots and slots are supported; subframe actions remain refused.

## Action boundary

The upstream engine owns geometry and actionability. CWW separately checks
identity, native label association, protected fields, focused control,
submission classification and local authorization. An associated label may
activate only the original radio/checkbox; nested unrelated interactive
content or closed shadow hosts in an alternate label are refused. A checkbox
click toggles its current state: it is not silently converted to `check()`.

Upstream trial normally dispatches input behind an interceptor. CWW no-ops
**all trial Input commands**, including hover, because earlier page capture
listeners could observe them before that interceptor. Visibility, stability,
scrolling and hit testing still use the unchanged upstream code. A preview
never presses, types or selects. Actual operations use trusted upstream input. After one actual mouse press,
the transport refuses another press in that lease, including an upstream retry
after interception; an earlier page listener may already have observed it.
No forced focus, screencast or virtual page clock is enabled.

Stop, policy changes, disconnection and timeouts invalidate the operation.
Cleanup calls only the known upstream interceptor's `stop()` and removes
CWW's semantic guard. Future-document script registration is synthesized,
because navigation always ends this operation; no engine script is installed
to run automatically in a later document. It does
not authorize queued engine calls after revocation. Navigation before input
invalidates the ref. Navigation or closure after input settles the action
without another attempt; a disallowed destination's content stays withheld.
Raw engine errors, markup, text and protocol payloads are never logged.

## Tests and provenance

`npm test --prefix browser/engine` exercises framing, bounds, readiness,
cancellation and generic errors. `browser/e2e/full-engine.spec.ts` runs the
real helper, packaged extension and native messaging host against synthetic
Chromium pages. It covers ordinary and styled controls, closed shadows/slots,
preview side effects, real overlays, disabled/inert states, stale navigation,
protected/focused-field mutations, label rebinding, Stop, revocation, cleanup
and transport isolation. Hardware capture runs must report the actual GPU.
The earlier prototype's clean, single-debugger native-pipe test additionally
proves background operation without a second CDP client or screencast.

The registry package declares Apache-2.0 but its LICENSE file contains the
outer Playwriter MIT text. Preserve that file as `LICENSE-REGISTRY-PACKAGE`,
along with Playwriter's MIT license, the actual upstream Playwright Apache-2.0
license and NOTICE. The packaging process also retains the complete official
Node license and unchanged dependency notices. No upstream package is patched.
