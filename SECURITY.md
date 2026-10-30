# Security policy

Chat with Work Desktop (`cww`) reads files on people's computers on behalf of a remote server, and changes them in the folders where the user allows changes. We treat any way to read something the user didn't share, to change anything outside a folder that allows changes, to delete anything for good, or to make the daemon do something other than answer its tools, as a serious vulnerability.

## Reporting a vulnerability

Please report vulnerabilities privately:

- Use GitHub's **"Report a vulnerability"** button on this repository (Security ▸ Advisories), or
- Email **hello@chatwithwork.com** (the contact on [chatwithwork.com/security](https://chatwithwork.com/security)) with "Chat with Work Desktop vulnerability" in the subject.

Please don't open a public issue. Include the cww version (`cww --version`), your OS, and steps to reproduce. A proof of concept against the fake server in `tests/e2e.rs` is ideal.

We will acknowledge your report within 3 working days, keep you updated, and credit you in the advisory unless you prefer otherwise. We ask that you give us a reasonable chance to release a fix before disclosing the issue.

## In scope

- Reading anything outside the shared folders, or anything on the deny list, through the tunnel: path traversal, symlinks, hard links, races, encoding tricks, or parser bugs.
- Getting the daemon to write, move or delete anything outside a folder where the user allowed changes, or on the deny list or the never-changed list there; to delete anything without it going to the system trash; to write anything that runs (a program, a launcher, an execute bit); to execute anything; or to open network connections other than its own tunnel (and, only when the user turned them on, its crash reports).
- Turning on changes for a folder from the server, or keeping them after the user turned them off.
- Getting the browser extension or the daemon to act on a tab outside the "Chat with Work" tab group or on a site the user didn't allow, to type into or read a password, payment or one-time code field, to act while stopped, to run code a server sends in a page, or to let another extension or a web page drive it.
- Bypassing the daemon's rate and volume limits, pause, or audit log.
- Dispatching non-browser tools through the local browser-call adapter,
  changing its fixed read-budget scope, or bypassing its per-action approval
  acknowledgement and mandatory dry run. Local clients still run as the
  user and are responsible for obtaining that user's actual authorization.
- Stealing or replaying device credentials or access tokens, or pairing a device without the user's approval.
- Driving the daemon through its control socket as another local user.
- Memory-safety or denial-of-service bugs triggered by a malicious server or a malicious local file.
- Supply-chain issues in the release process.

## Out of scope

- Reading files the user shared that aren't on the deny list. That is the product working as designed; see the threat model in README.md.
- Attacks that need the user's own account on the machine (for example malware running as the user).
- Vulnerabilities in the Chat with Work web app itself. Report those to the same address, but they are handled separately.

## Supported versions

Only the latest release gets security fixes during the beta.

## Bundled browser engine boundary

The optional full browser engine is trusted installed code, not code supplied
by the model or a page. Its fixed Node executable, entry and locked modules live
in an immutable manifest-addressed payload outside CWW state and writable shared
folders. The daemon verifies the locator, versions, architecture, complete file
inventory and every SHA-256 before confinement. It refuses symlinks, unlisted
files and group/world-writable payloads. It caches the resolved payload until
restart, so replacing an installation cannot mix old and new modules.

Landlock adds only the exact helper executable and read-only module tree to the
existing daemon rules. Seatbelt adds that exact process-exec path, process-fork
and read-only payload access. This does not claim a Windows daemon sandbox;
Windows retains its existing boundary. The child inherits confinement, receives
no loader/module environment, has no stdin script interface, and exposes no
network listener. The remote API still offers only fixed browser tools.

The private JSON-line protocol is bounded to 2 MiB per frame, 16 MiB cumulative,
512 commands and 128 pending commands. A tool's preview and action share one
18-second deadline; extension leases enforce the remaining action budget too.
Policy/origin revocation, disconnect and cancellation terminate the exact child
and release its lease. There is no legacy fallback after admission to an engine
operation, and no raw engine error, local path or CDP payload is returned to the
model or logged. Engine warm-up happens without an operation or browser input.
