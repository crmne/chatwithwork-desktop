# Full Playwriter engine prototype

This isolated prototype is **not wired into the product or release**. It tests
whether the unchanged Playwriter action engine can replace CWW's custom
pointer orchestration. It never connects to the person's profile or accounts.

## What is reused

`@xmorse/playwright-core@1.59.13` is pinned from npm, the version named by the
inspected Playwriter checkout. Its `CRBrowser`, frame/execution-context handling,
`ElementHandle.click`, `ElementHandle.check`, `ProgressController`, injected
script, actionability polling, scrolling, hit-target interception and trusted
input dispatch run unchanged. No copied replacement click routine is used.
The dependency is Apache-2.0; its LICENSE and upstream NOTICE accompany this prototype.

Playwriter itself is MIT. Its `cdp-relay.ts` maps each attached extension tab to
an engine session, synthesizes root Target events, and forwards page commands.
The prototype uses the same mapping structure in a small test-only bridge.
It does not import Playwriter's arbitrary-JavaScript executor, cloud services,
telemetry, recording, unrestricted HTTP server or remote browser APIs.

Sources inspected:

- Playwriter `9d5a1ab2d0f343cd231f5ecceb885921fb3fc0dd`,
  `playwriter/src/{executor,cdp-relay,aria-snapshot,cdp-session}.ts`.
- Its Playwright submodule `1ca511869834700a64cc6d72ad228c2dd0e3ddb8`,
  `packages/playwright-core/src/server/{dom,progress}.ts`,
  `server/chromium/{crBrowser,crPage}.ts` and `packages/injected`.

## Concrete architecture

A throwaway Chromium profile loads the small prototype extension. The test
starts it over a private process pipe, verifies NVIDIA rendering, and opens
one inactive tab in its own group on a synthetic loopback site. A second,
unchanged engine instance connects through a `ConnectionTransport` that sends
commands to `chrome.debugger` in that extension. Events are mapped back to its
single owned session. There is **no TCP CDP listener**. Test-driver polling is
only a fixture transport, not a proposed production message pump.

`FixedTools.call` accepts only `click`, `check` and `fill`, with a closed set of
arguments. It cannot accept code, CDP, selectors or `force`. Refs are bound to
backend-node identities and converted to upstream ElementHandles; even closed
shadow targets use the upstream action pipeline after adoption. This adoption
and the custom transport use pinned private server APIs, an upgrade surface
that needs its own compatibility tests.

Playwriter's snapshot/ref path is different: `getAriaSnapshot` builds a full
AX/flattened DOM snapshot, selects stable IDs/test IDs or role/name selectors,
and the executor's `refToLocator` returns that selector. Document generation
is tracked across snapshot/navigation. Reusing that wholesale requires a
CWW decision about replacement nodes: a selector can resolve a newly inserted
node with the same ID, while approval of the old backend node should expire.
The prototype keeps the stricter identity binding and tests replacement.

## Production boundary still required

This is an engine compatibility proof, **not a finished security boundary**:

- Bind every ref/approval to tab, document/navigation generation and original
  control identity; revalidate effect and protected fields at actual input.
  Sanitize upstream error/log text, which may contain markup or suggest force;
  neither is appropriate to expose through the fixed CWW tool.
- Keep existing CWW submission classification, field/payment redaction, local
  policies, server approval provenance, bot limits and audit intact.
- Filter event delivery as well as commands after site/group changes; abort
  in-flight engine operations and detach on Stop or policy revocation.
- Replace the fixture policy/deny examples with the daemon's existing fixed
  tool pipeline. Do not ship a new generic CDP gateway or a mutable remote
  policy API. The trusted, bundled engine alone owns its private transport.
- Support child/OOPIF sessions only within the same validated tab and allowed
  document origins, including redirects and navigation during waits.
- Keep cleanup and cancellation bounded even when extension/daemon disconnect.
- Full no-rAF background-tab testing and bundled runtime packaging are part of
  this investigation; results will be recorded below, not assumed.

A product implementation should bundle a signed fixed-method Bun/Node sidecar
with the app, communicating only through inherited stdio/native IPC, so users
need no separate runtime. The Rust daemon remains the entry point and policy
owner. Importing only the upstream injected bundle would retain our own action
orchestration, which would not satisfy the goal of reusing the engine.

## Run

```
npm ci --ignore-scripts
CWW_CHROMIUM=/path/to/headless/chromium npm test
```

The optional `CWW_ENGINE_EVIDENCE` path receives synthetic JSON outcomes. Tests
run serially and assert NVIDIA hardware. They close their own browser/profile.
No runtime or extension is installed into the user's current browser.

## Results

The 30 October synthetic run used NVIDIA RTX3090 through ANGLE. Full engine
clicks and trial previews work; `label.check()` selects the exactly associated
radio and verifies its checked state even when the input is covered or
`display:none`. Calling `input.check()` directly refuses those cases. This is
an explicit target-selection requirement, not evidence that the engine can
force-click invisible inputs.

With a tiny 8×8 `Page.startScreencast` stream, the unmodified engine also passed
`display:contents`, closed-shadow bound-node and slotted-link clicks. A real
overlay refused with its interceptor in the upstream actionability trace.
Stale refs, protected passwords, unsupported methods/force, another target
session, global cookie deletion and Stop during auto-wait all refused.

Without that stream, some stability waits stalled: the engine's captured rAF
callback ran once in300ms despite visible/focused emulation. With the stream
it ran18times. Frames are acknowledged and discarded without logging content.
The active Chrome tab remained unchanged. `Page.bringToFront` was separately
rejected as a mitigation: it DID change the active tab in this Chromium155
fixture, contrary to an upstream test's environment-specific comment.

The first test uses a private Playwright launch pipe as the fixture driver in
addition to the extension debugger. A single-debugger/native-messaging control
is required before attributing the rAF observation to normal user browsing.
Production liveness would run only during an action and stop on all exits,
revocation and disconnect; this fixture currently keeps it for the test.

Reproduce the passing action matrix with `CWW_ENGINE_RENDER_MODE=screencast`.
The default mode records its actual successes/refusals rather than claiming all
positive compatibility cases passed. No force or virtual clock is used.
