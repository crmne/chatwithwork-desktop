# Product integration contract (under implementation)

The reviewed direction is the unchanged registry engine, hosted by bundled
Node22.23.4, behind CWW's existing fixed tools. Node22 preserves the macOS11
minimum; plan a runtime/support decision before30April2027 (upstream EOL).
This document defines ownership for parallel implementation, not a shipped
capability or a replacement for existing tests.

## Route selection

`BrowserTools` and `LocalFiles` retain approval, organization/bot policy, audit,
limits and effect handling. Before executing click/type/select, select the new
path only when the extension advertises `engine_protocol:1` and a complete
trusted helper installation is present. Otherwise select the released legacy
path at admission. Never fall back to legacy after any new-engine error or
possible input dispatch. Existing ARM64-musl functionality remains on legacy
until an officially supported runtime build exists.

## Internal extension protocol

These methods are daemon-internal, never listed as tools/control commands:

- `engine_begin {action:"click"|"type"|"select",params:{existing tool args},timeout_ms}`
  validates existing target/effect/protected-field policy, local actions/site/
  owned group and current read-ref/document generation. It acquires one
  operation (at most the remaining native action budget) and returns `{lease,tab,session,targetInfo,document,backendNodeId,
  labelBackendNodeIds,facts,action,params}`. Lease is a fresh unguessable string,
  expires within the existing operation deadline, and is bound to the native
  connection, exact tab/document and logical control.
- `engine_command {lease,command:{id,method,params,sessionId?}}` accepts commands
  only during that operation. They come solely from the private trusted engine
  process, not model arguments. Revalidate owned group/site/local policy and
  original control/effect/protected field before dispatch. Navigation or lost
  binding closes the operation. Root Browser mutations are not forwarded;
  setup synthesizes only the small session/target responses required by the
  unchanged engine. Page.bringToFront is never forwarded. No global Chrome
  cookie/cache/permission/download configuration changes.
- `engine_finish {lease,surfaceBackendNodeId?}` performs the existing bounded
  screenshot/outline for a trial or existing navigation-settle response for a
  completed action, then releases operation resources. An alternate label must
  still belong to the original native radio/checkbox. It cannot borrow an
  unrelated interactive descendant's activation surface.
- `engine_cancel {lease}` releases operation/guards on every failure, timeout,
  Stop, disconnect or local policy change. It is idempotent and does not
  recreate debugger attachment after Stop.
- Event `{event:"engine",lease,message:{method,params,sessionId}}` is forwarded
  only while the operation still owns the same allowed tab/document. No events
  from other tabs, new unapproved origins or unrelated browser targets.

No default screencast or page-clock changes. A genuine single-debugger/native
pipe control passes background operations without it. Any later liveness fix
must retain active-tab invariance and bounded cleanup.

## Helper protocol and confinement

Bundle a fixed Node executable, fixed JS entry and immutable locked module tree
under a known browser-engine installation directory. Never resolve executable
or entry from model input, PATH, NODE_OPTIONS, NODE_PATH or a user-selected
script. Sanitize environment, use --no-addons and --no-global-search-paths,
retain exact Node/npm/upstream notices, and re-sign the Mac runtime with
reviewed helper-only entitlements for notarization.

Start one bounded helper per operation over inherited stdin/stdout. No TCP
listener, CDP endpoint, arbitrary-JS entry, cloud service or runtime download.
Initialization emits ready before any browser lease and is prewarmed under
confinement. Actual requests retain the native 18-second total budget, with
at most 14 seconds inside a lease. Messages are bounded JSON lines (2 MiB per
frame, 16 MiB cumulative, 512 commands):

- helper -> daemon: `{type:"ready",protocol:1}` after loading, before a call;
- daemon -> helper: `{type:"call",operation:<engine_begin result>}` once;
- helper -> daemon: `{type:"command",id,command:<engine-generated CDP>}`;
- daemon -> helper: `{type:"reply",id,result}` or `{type:"reply",id,error}`;
- daemon -> helper: `{type:"event",message:<validated extension CDP event>}`;
- daemon -> helper: `{type:"cancel"}` on cancellation/revocation/disconnect;
- helper -> daemon: `{type:"result",result:{surfaceBackendNodeId}}` or
  `{type:"error",error:{code,message}}` once.

The Rust wrapper attaches the lease to extension calls itself. A helper cannot
request another lease, action, script or tool. Reject malformed/out-of-sequence
messages and bound buffering. Terminate the exact child on timeout/cancel and
always send engine_cancel on unsuccessful exits. Never log input strings, page
contents, raw upstream markup, CDP payloads or stdout/stderr diagnostics.

Give the confined daemon only exact helper Execute/read plus locked dependency
root Read permissions. Preserve existing Landlock/Seatbelt/Windows isolation;
no blanket Execute/read or sandbox exemptions. The child inherits confinement.

## Division of work

- Vendor agent: production JS sidecar and extension integration, capability/ref
  binding, actionability/security/browser tests and docs.
- Native Rust wrapper/sandbox and packaging: coordinated with root/designers;
  source ownership must be explicitly agreed before editing.

## Required integration checks

Retain legacy tests. New native-pipe tests must exercise full unchanged engine
click/type/select, styling/shadows/slots, trial with no effects, protected-field
mutation, logical label association/nested-interactive/race refusal, stale refs
across navigation, genuine overlays, Stop and policy revocation during waits,
other tab/origin isolation, unknown method/code/force rejection, event filtering,
bounds/child failure cleanup, and no duplicate dispatch or fallback after input.
Verify packaged helper actions, not just constructors, on supported platforms.

Trials use unchanged upstream checks with every Input command suppressed in
the private extension transport, including hover. Earlier page capture listeners
therefore cannot observe preview events. Actual actions retain upstream trusted
input; only Input commands are serialized to preserve their original wire order
through asynchronous policy checks.
