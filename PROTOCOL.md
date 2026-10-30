# Chat with Work Desktop: tunnel protocol, version 1

This document describes everything that goes over the wire between the background agent (`cww`) and a Chat with Work server: pairing, tokens, the WebSocket tunnel, and the MCP messages inside it. A server implementation (the Rails app, or a self-hosted one) can be checked against it line by line.

The key words MUST, SHOULD and MAY are used as in RFC 2119.

## Contents

1. [Roles and overview](#1-roles-and-overview)
2. [Server URL](#2-server-url)
3. [Device key and DPoP proofs](#3-device-key-and-dpop-proofs)
4. [Pairing (RFC 8628)](#4-pairing-rfc-8628)
5. [Token endpoint](#5-token-endpoint)
6. [The WebSocket tunnel](#6-the-websocket-tunnel)
7. [MCP inside the tunnel](#7-mcp-inside-the-tunnel)
8. [Tools](#8-tools)
9. [Errors](#9-errors)
10. [Limits](#10-limits)
11. [Decisions this spec makes](#11-decisions-this-spec-makes)
12. [Chats for the terminal UI](#12-chats-for-the-terminal-ui)
13. [Scans](#13-scans)

## 1. Roles and overview

- The **daemon** runs on the user's computer. It is an MCP *server* that lets Chat with Work read, and with the user's approval change files and run commands, in folders the user shares: seven read-only tools, eight change tools while at least one shared folder allows changes (section 8.7), and the command tools while one also allows commands (section 8.15).
- The **server** is Chat with Work. It is the MCP *client*: the model loop runs there and calls the tools.
- The daemon opens the only connection, outbound, to `wss://<server>/desktop`. The server never connects to the daemon.
- Over that socket the server sends JSON-RPC requests and the daemon answers them. The daemon never sends requests: no sampling, no elicitation, no roots.

```
cww login        POST /desktop/device_authorizations   (DPoP)
                 user approves at /device
                 POST /desktop/token  device_code grant (DPoP) -> device_id + refresh_token

cww daemon run   POST /desktop/token  refresh_token grant (DPoP) -> access_token (10 min)
                 GET  /desktop  Upgrade: websocket
                      Authorization: DPoP <access_token>
                      DPoP: <proof bound to the token>
                 <- MCP requests / -> MCP responses, until the socket closes
                 reconnect with backoff, refreshing the token when needed
```

## 2. Server URL

The user gives the server as an origin, for example `https://chatwithwork.com` (the default). The daemon rejects URLs with a path, query, fragment or credentials.

- `https://` is required. Plain `http://` is accepted only when the host is `localhost` or a loopback IP, for development.
- HTTP endpoints are `<origin>/desktop/device_authorizations` and `<origin>/desktop/token`.
- The WebSocket URL is `wss://<host[:port]>/desktop` (`ws://` for the loopback exception).
- TLS uses rustls. The daemon trusts the operating system's certificate store (macOS keychains, the Windows certificate store, the distribution's CA bundle on Linux, or `SSL_CERT_FILE`/`SSL_CERT_DIR` when set) plus Mozilla's root certificates, so servers behind private CAs work. The daemon never follows redirects: a 3xx from any endpoint is an error.

## 3. Device key and DPoP proofs

At pairing the daemon creates an **Ed25519** key pair. The private key never leaves the machine (it lives in the OS keychain, or in a 0600 file when no keychain is available). The public key is sent as **`public_key`**: the raw 32-byte key, base64url without padding. This is the same value as the JWK `x` parameter.

Every call to the token endpoints and the WebSocket upgrade carries a **`DPoP`** header with a proof as defined by [RFC 9449](https://www.rfc-editor.org/rfc/rfc9449): a compact JWS signed with the device key.

**Proof header:**

```json
{ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": { "kty": "OKP", "crv": "Ed25519", "x": "<public_key>" } }
```

**Proof claims:**

| Claim | Value |
|---|---|
| `jti` | 16 random bytes, base64url. Unique per proof. |
| `htm` | `POST` for the token endpoints, `GET` for the WebSocket upgrade. |
| `htu` | The full HTTP(S) URL of the request, without query: `https://<host>/desktop/token`, `https://<host>/desktop/device_authorizations`, or `https://<host>/desktop` for the upgrade (the HTTPS form, even though the socket URL is `wss://`). |
| `iat` | Current Unix time in seconds. |
| `nonce` | The latest `DPoP-Nonce` the server sent, if the daemon has one. |
| `ath` | Only on the WebSocket upgrade: base64url(SHA-256(access token)). |

**The server MUST verify** every proof: signature, `typ`, `alg`, that `jwk.x` equals the device's registered public key (at pairing: equals the `public_key` form field), `htm`, `htu`, `iat` within a small window (60 seconds is suggested), `jti` not seen before within that window, `ath` on the upgrade, and `nonce` when the server requires nonces.

**Nonces.** The server SHOULD send a `DPoP-Nonce` response header on every token endpoint response (success or error) and on a rejected upgrade. The daemon puts the latest nonce it has seen into every proof. When a proof has no nonce or a stale one, the token endpoint answers `400 {"error":"use_dpop_nonce"}` with a fresh `DPoP-Nonce` header, and the daemon retries once. The daemon's very first request carries no nonce.

The JWK thumbprint ([RFC 7638](https://www.rfc-editor.org/rfc/rfc7638)) is `base64url(SHA-256('{"crv":"Ed25519","kty":"OKP","x":"<public_key>"}'))`. `cww login` prints it, so the approval page MAY show it for comparison. Servers that issue self-contained access tokens SHOULD bind them with `cnf.jkt` set to this thumbprint.

## 4. Pairing (RFC 8628)

### 4.1 Device authorization request

```
POST /desktop/device_authorizations
Content-Type: application/x-www-form-urlencoded
Accept: application/json
DPoP: <proof, htm=POST, htu=https://<host>/desktop/device_authorizations>

client_id=cww
&scope=desktop:serve desktop:chat
&public_key=<base64url Ed25519 public key>
&name=<device name, defaults to the hostname>
&platform=<linux | macos | windows>
&client_version=<cww version, e.g. 0.1.0>
```

**Success: `200 OK`**

```json
{
  "device_code": "…",
  "user_code": "WDJB-MJHT",
  "verification_uri": "https://chatwithwork.com/device",
  "verification_uri_complete": "https://chatwithwork.com/device?user_code=WDJB-MJHT",
  "expires_in": 900,
  "interval": 5
}
```

`verification_uri_complete` is optional and `interval` defaults to 5. Any other status is an error with an OAuth error body (see [section 9](#9-errors)).

The server stores the public key with the pending authorization, together with the name, platform and version it shows on the approval page. The approval page (`/device`) is server-side UI and not part of this protocol. The design calls for re-authentication, showing the device name, OS, IP and approximate location, and an email to the user after approval.

### 4.2 Polling

The daemon waits `interval` seconds, then polls until it is approved, denied, or `expires_in` passes:

```
POST /desktop/token
Content-Type: application/x-www-form-urlencoded
DPoP: <proof, htm=POST, htu=https://<host>/desktop/token>

grant_type=urn:ietf:params:oauth:grant-type:device_code
&device_code=<device_code>
&client_id=cww
```

The proof MUST be signed by the key registered in 4.1.

| Response | Daemon behavior |
|---|---|
| `400 {"error":"authorization_pending"}` | Keep polling. |
| `400 {"error":"slow_down"}` | Add 5 seconds to the interval, keep polling. |
| `400 {"error":"access_denied"}` | Stop: "pairing was denied". |
| `400 {"error":"expired_token"}` | Stop: "the pairing code expired". |
| `200` token response (below) | Paired. |

**Approval: `200 OK`**

```json
{
  "access_token": "…",
  "token_type": "DPoP",
  "expires_in": 600,
  "scope": "desktop:serve",
  "refresh_token": "…",
  "device_id": "42"
}
```

`device_id` and `refresh_token` are REQUIRED here. `device_id` is opaque to the daemon; a JSON string or number is accepted. The daemon stores the refresh token and the key in the secret store, and the origin and device ID in `~/.config/cww/config.toml`.

## 5. Token endpoint

Before connecting, the daemon exchanges its refresh credential for a short-lived access token:

```
POST /desktop/token
Content-Type: application/x-www-form-urlencoded
DPoP: <proof, htm=POST, htu=https://<host>/desktop/token>

grant_type=refresh_token
&refresh_token=<refresh_token>
&client_id=cww
```

**Success: `200 OK`**

```json
{
  "access_token": "…",
  "token_type": "DPoP",
  "expires_in": 600,
  "scope": "desktop:serve",
  "refresh_token": "…"
}
```

- The access token is opaque to the daemon. The design calls for a signed token with a 10-minute lifetime, `aud` equal to the server origin, `scope=desktop:serve`, and binding to the device key (`cnf.jkt`). That scope MUST NOT grant access to any other API. While the owner lets the computer use their chats, the scope also carries `desktop:chat`, which opens the chat API of section 12 and nothing else.
- `refresh_token` in the response is OPTIONAL. If present and different, the daemon replaces the stored one (rotation). Servers that rotate SHOULD accept the previous refresh token for a short grace period, because the daemon may crash between receiving and storing it.
- The daemon caches the access token in memory and refreshes it 60 seconds before `expires_in` runs out, or after the server rejects it.
- The refresh request MUST be rejected with `400 {"error":"invalid_grant"}` once the device is revoked. On `invalid_grant`, `unauthorized_client` or `access_denied`, the daemon marks itself **revoked**, closes the tunnel, stops reconnecting, and waits for the user to run `cww login`. Any other failure (network error, 5xx, other error codes) is retried with backoff.

## 6. The WebSocket tunnel

### 6.1 Upgrade request

```
GET /desktop HTTP/1.1
Host: chatwithwork.com
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: …
Sec-WebSocket-Protocol: actioncable-v1-json, mcp
Authorization: DPoP <access_token>
DPoP: <proof, htm=GET, htu=https://<host>/desktop, ath=…, nonce=…>
Origin: https://chatwithwork.com
User-Agent: cww/0.1.0
```

- The access token is only ever sent in the `Authorization` header, never in the URL.
- `Origin` is the server's own origin, so Action Cable's same-origin check (`allow_same_origin_as_host`) accepts it.
- Behind an HTTP proxy, the daemon first opens a tunnel with `CONNECT <host>:<port>` and runs TLS and this request inside it, as it does for the pairing and token requests. The server sees the same bytes either way.

**The server MUST**, before accepting the upgrade: validate the access token (signature, expiry, audience, scope `desktop:serve`, device not revoked), validate the DPoP proof as in section 3, including `ath`, and check that the proof key is the one the token is bound to.

**Rejection.** Respond `401` (or `403`) instead of `101`, with a fresh `DPoP-Nonce` header. The daemon retries once right away with the new nonce, or with a freshly refreshed token if no nonce came back. After that it falls back to the normal backoff.

### 6.2 Subprotocols and framing

The daemon offers two subprotocols, and the server MUST select one in `Sec-WebSocket-Protocol`:

| Subprotocol | Framing |
|---|---|
| `actioncable-v1-json` | Action Cable (what Rails speaks). If the server selects no subprotocol, the daemon assumes this one. |
| `mcp` | Bare JSON-RPC: every text frame is exactly one JSON-RPC message, with no envelope. |

Any other selection is an error, and the daemon disconnects.

**Action Cable framing.** The channel identifier is this exact string:

```
{"channel":"Desktop::Channel"}
```

1. The server MAY send `{"type":"welcome"}`.
2. The daemon sends `{"command":"subscribe","identifier":"{\"channel\":\"Desktop::Channel\"}"}`.
3. The server answers `{"identifier":"…","type":"confirm_subscription"}`, or `reject_subscription`, which the daemon treats as unauthorized (see 6.4). Only this channel's rejection counts: a refused chat channel (section 12) is just a chat that can't be followed.
4. **Server to daemon:** each JSON-RPC message is the `message` of a channel transmission:
   `{"identifier":"{\"channel\":\"Desktop::Channel\"}","message":{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{…}}}`.
   `message` SHOULD be a JSON object. A string containing serialized JSON is also accepted. Frames for other identifiers are ignored, except the chat channels of section 12, whose messages go to the terminal UI and never to the MCP server.
5. **Daemon to server:** each JSON-RPC message is sent as a `message` command whose `data` is the serialized JSON-RPC message:
   `{"command":"message","identifier":"{\"channel\":\"Desktop::Channel\"}","data":"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{…}}"}`.
   JSON-RPC messages never contain an `action` key, so Rails dispatches them to `Desktop::Channel#receive(data)`, where `data` is the parsed JSON-RPC message.
6. `{"type":"ping"}` frames are accepted and ignored (they count as traffic). `{"type":"disconnect","reason":…,"reconnect":…}` is handled as in 6.4.

In Rails terms: authenticate in the `Desktop` connection's `connect` from the request headers. `transmit(jsonrpc_hash)` sends a request to the daemon. `receive(data)` gets each response.

### 6.3 Heartbeats and liveness

- The daemon sends a WebSocket **ping** frame every 20 seconds. The server's WebSocket stack answers with a pong, which is automatic in Rails and most libraries.
- If the daemon receives no frame at all (pong, Action Cable ping, or message) for 60 seconds, it treats the connection as dead and reconnects. Action Cable's 3-second pings keep it alive. With `mcp` framing, pongs do.
- The server SHOULD treat the device as offline as soon as the socket closes, and fail pending tool calls fast rather than waiting out their deadline.

### 6.4 Closing, revocation and reconnects

| Server action | Daemon behavior |
|---|---|
| Close code **4001** ("re-authenticate") | Drop the cached access token, refresh it, reconnect. |
| Close code **4003** ("revoked") | Drop the cached token and try to refresh. The refresh gets `invalid_grant`, so the daemon becomes revoked and stops. |
| Action Cable `disconnect` with `reconnect:false`, or with `reason:"unauthorized"`, or `reject_subscription` | Same as 4001. |
| Any other close, network loss, or idle timeout | Reconnect with backoff, reusing the cached token if it is still valid. |

To revoke a device, the server sets `revoked_at`, closes the device's socket with **4003**, and rejects every later refresh with `invalid_grant`. The socket's lifetime is **not** tied to the access token's lifetime: a token is checked at upgrade time only, and the server ends sessions by closing the socket. A server that wants periodic re-authentication MAY close with 4001 at any time.

**Backoff** is full-jitter exponential: a random delay in `[0.25 s, min(60 s, 1 s × 2^attempt)]`, reset after a connection that lasted at least 60 seconds.

### 6.5 Message size

- A JSON-RPC message MUST NOT exceed **262 144 bytes (256 KiB)** in either direction, measured on the serialized JSON-RPC message (the `data` string or `message` object, not the Action Cable envelope).
- WebSocket frames and messages are capped at `2 × 256 KiB + 16 KiB`, because Action Cable string-encodes `data` and can double its size. A larger frame ends the connection.
- A request larger than 256 KiB (but inside the frame cap) gets JSON-RPC error `-32600` "message too large".
- The daemon shrinks tool results to fit: `read` halves its chunk (and moves `next_offset` back), `list` halves its page (and adjusts `next_cursor`), and `search` drops trailing hits. Any other oversized response is replaced by error `-32603` "response too large".

## 7. MCP inside the tunnel

### 7.1 Versions and sessions

Each WebSocket connection is a **new MCP session**. The first JSON-RPC message the server sends on a connection chooses the mode, and rmcp (the official Rust MCP SDK) implements both:

- **Stateless, MCP `2026-07-28` (preferred).** There is no handshake. Every request's `params._meta` MUST carry:

  ```json
  "_meta": {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientCapabilities": {},
    "io.modelcontextprotocol/clientInfo": { "name": "chatwithwork", "version": "…" }
  }
  ```

  `clientInfo` is optional. A request without the two required keys gets `-32602`. `server/discover` returns the supported versions and capabilities.
- **Legacy handshake, MCP `2025-11-25` and earlier.** The first message is `initialize`. The daemon answers with the negotiated version, and later requests need no `_meta`. The server SHOULD send `notifications/initialized`. This exists because RubyLLM's MCP client falls back to it.

In stateless mode `ping` is not available (`-32601`). Liveness comes from WebSocket pings (6.3).

### 7.2 Chat ID

Every `tools/call` SHOULD carry the Chat with Work chat ID in its request metadata:

```json
"_meta": { "com.chatwithwork/chatId": "1234", … }
```

A string or a number is accepted. The daemon uses it for the per-chat read budget (section 10) and writes it to the local audit log. Calls without it share one budget.

### 7.2.1 Bots

A Chat with Work bot (an assistant an organization sets up to work by itself) reaches a computer only through the folders the computer's owner lends it, on the computer itself (`cww roots lend`, or the control API's `roots_bot`, CONTROL.md), never from the server. A call from one of a bot's chats carries the bot's ID, a string or a number, in its request metadata:

```json
"_meta": { "com.chatwithwork/chatId": "1234", "com.chatwithwork/botId": "42", … }
```

For such a call the daemon:

- answers `roots` with only the folders lent to that bot, each with `"unattended": true` or `false` (whether its commands may start without a person approving each one there);
- refuses, with `not_lent`, any call whose `path`, `from`, `to` or `root` names a folder not lent to it (for `patch`, the `path` of any change and any key of `expected_sha256`), and a `search` without a `root` (or with `"root": null`), or a `grep` or `glob` without a `path`, when nothing is lent to it; such a call looks only in the lent folders;
- refuses, with `not_lent`, `run_status`, `run_output` and `run_stop` for a command that isn't running in a folder lent to it;
- refuses, with `needs_approval`, a `run` (not a dry run) that also carries `"com.chatwithwork/unattended": true` in a folder whose loan doesn't allow unattended commands. The server sends that flag when it runs the bot's command without asking anyone, which it does only where `roots` said `"unattended": true`.

Each folder still allows only what it allows anyone: changes where changes are allowed, commands where commands are allowed. Calls without `com.chatwithwork/botId` are a person's, as before. The audit log's entries for a bot's calls say `for bot <id>`. Lending a folder, stopping, and changing whether its commands run unattended write `root_bot_lent` and `root_bot_withdrawn` entries. Stopping commands in a folder, or making it read-only, ends every loan's unattended commands there.

### 7.3 Methods

| Method | Answer |
|---|---|
| `server/discover` | `supportedVersions`, `capabilities: {"tools":{}}`, and `_meta["io.modelcontextprotocol/serverInfo"] = {"name":"cww","version":…}`. |
| `initialize` | Legacy handshake (7.1). `serverInfo.name` is `cww`, and the result carries instructions. |
| `tools/list` | The seven read-only tools (section 8), then the change tools while a shared folder allows changes (section 8.7), the command tools while one allows commands (8.15), and the browser tools while the person turned the browser on (8.16), in one page. The list only changes when the user turns changes, commands or the browser on or off, and the daemon then reconnects, so a server that lists tools on each new connection is always current. |
| `tools/call` | See section 8. |
| `notifications/initialized`, `notifications/cancelled` | Accepted. Cancelling abandons a call in progress. |
| `resources/*`, `prompts/*`, `completion/complete`, `logging/setLevel`, anything else | `-32601` method not found. |

The daemon ignores JSON-RPC responses from the server, since it never sends requests, and ignores unknown notifications.

### 7.4 Tool naming on the server

The daemon's tool names are `roots`, `search`, `list`, `read`, `grep`, `glob` and `read_bytes`, and the change tools `create`, `write`, `edit`, `mkdir`, `move`, `delete`, `create_document` and `patch`, with no prefix. The server namespaces them per device, for example `local_<device_id>_read`, following its existing `mcp_<id>_` convention.

## 8. Tools

The read-only tools (8.3 to 8.6, and `grep`, `glob` and `read_bytes` in 8.17 to 8.19) are annotated `readOnlyHint: true`, `destructiveHint: false`, `idempotentHint: true`, `openWorldHint: false`. The change tools carry their own annotations (8.7; `patch` is in 8.20). Unknown argument keys are rejected with `invalid_argument`.

The sections keep their numbers from earlier versions: the tools added for working in code come after the browser's, in 8.17 to 8.20.

### 8.1 Paths

Every path is **`<root_id>:<relative path>`**. The daemon rejects the following before touching the filesystem:

| Rule | Error |
|---|---|
| No `:`, or a root ID outside `[a-z0-9][a-z0-9_-]{1,31}` (2 to 32 characters; `C:` never parses as a root) | `invalid_path` |
| Absolute paths (leading `/`), a leading `~`, or a relative part starting with `/` | `invalid_path` |
| `..` in any component | `invalid_path` |
| NUL bytes or backslashes anywhere | `invalid_path` |
| Control characters (newlines, escapes), direction marks and isolates (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069), zero-width characters (U+200B to U+200D, U+2060 to U+2064, U+FEFF), or line and paragraph separators anywhere: they could forge or hide part of an audit log line. A file whose name has one can't be read or changed by path | `invalid_path` |
| Longer than 4096 bytes, or a component longer than 255 bytes | `invalid_path` |
| Well-formed, but no shared root has this ID | `unknown_root` |

`.` components and empty components (`a//b`) are ignored. `<root_id>:` on its own means the root folder. Paths the daemon returns are always normalized in this form. Absolute local paths are never sent.

Resolution then follows the rules in README.md ("Threat model"): symlinks are not followed, the path must stay inside the root, the deny list applies, and only regular files (for `read`) and directories (for `list`) are served.

### 8.2 Results

A successful call returns:

```json
{
  "content": [ { "type": "text", "text": "<the same JSON as structuredContent, serialized>" } ],
  "structuredContent": { … },
  "isError": false
}
```

(`resultType: "complete"` is added in `2026-07-28` sessions.) Servers SHOULD read `structuredContent`.

Timestamps are RFC 3339 in UTC, for example `2026-09-24T12:00:00Z`. Sizes are bytes. **Offsets and character counts are Unicode scalar values**, which is what Ruby's `String#length` counts, not bytes or UTF-16 units.

### 8.3 `roots`

Arguments: none (`{}`).

```json
{
  "roots": [
    { "id": "work-docs", "label": "Work docs", "available": true, "writable": false, "index": "ready", "indexed_files": 1532 },
    { "id": "app", "label": "App", "available": true, "writable": true, "index": "ready", "indexed_files": 412, "read_budget": 5000000 }
  ]
}
```

`writable` is true when the user allowed changes in the folder; the change tools only work there. `read_budget` is there when the person gave the folder a read budget of its own on the computer: the characters a chat may read from it per rolling hour (section 10). `allows_commands` is there, true, when it also allows commands (section 8.15), and `network` when they may use the network. `index` is `pending`, `indexing`, `ready`, `error` or `disabled`. `available` is false when the folder is missing, for example on an unmounted drive. Labels are chosen by the user and are the only description of a root the server gets.

### 8.4 `search`

| Argument | Type | Notes |
|---|---|---|
| `query` | string, required | 1 to 1000 characters. BM25 over file names (boosted ×2) and extracted text. |
| `root` | string | Only search this root ID. |
| `path_glob` | string | Without `/`, it matches the file name (`*.pdf`). With `/`, it matches the relative path (`plans/**/*.md`). Case-insensitive. |
| `modified_after` | string | RFC 3339 time or `YYYY-MM-DD` (UTC midnight). |
| `limit` | integer | 1 to 20, default 20. |

```json
{
  "hits": [
    {
      "path": "work-docs:plans/q3.md",
      "title": "q3.md",
      "modified": "2026-09-20T08:14:03Z",
      "size": 5120,
      "snippet": "The quarterly budget for Project Falcon is approved…"
    }
  ],
  "engine": "index"
}
```

`engine` is `index`, `grep`, or `index+grep`. When the index returns nothing, or a searched root hasn't finished its first index pass, the daemon also runs a live grep: a case-insensitive literal match of `query`, in text files only, with a 5-second time budget. Snippets are at most 300 characters. Files that match the deny list, or that the daemon refuses to open (hard links, special files), are never indexed or returned.

### 8.5 `list`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | A directory: `<root_id>:` or `<root_id>:dir/sub`. |
| `cursor` | string | The `next_cursor` of the previous page. Opaque. |

```json
{
  "path": "work-docs:plans",
  "entries": [
    { "name": "q3.md", "path": "work-docs:plans/q3.md", "kind": "file", "size": 5120, "modified": "2026-09-20T08:14:03Z" },
    { "name": "archive", "path": "work-docs:plans/archive", "kind": "dir", "modified": "2026-09-01T10:00:00Z" }
  ],
  "next_cursor": "200"
}
```

`kind` is `file`, `dir`, `symlink` (listed, never followed), or `other`. `size` appears only for files. Entries are sorted by name, at most 200 per page. `next_cursor` is `null` on the last page. Denied entries and names that aren't valid UTF-8 are left out.

### 8.6 `read`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | A file. |
| `offset` | integer | Character offset, default 0. |
| `max_chars` | integer | 1 to 32000, default 32000. It is lowered further by the remaining hourly budget (section 10). |

```json
{
  "path": "work-docs:plans/q3.pdf",
  "text": "…",
  "offset": 0,
  "next_offset": 32000,
  "total_chars": 81234,
  "modified": "2026-09-20T08:14:03Z",
  "size": 482113
}
```

`next_offset` is `null` at the end of the file. `max_chars` is lowered by what is left of the read budget of the file's folder (section 10). PDF, DOCX, PPTX (slides marked `--- Slide N ---`), and XLSX/XLS/ODS (one `## Sheet` section per sheet, tab-separated rows) are converted to text. Other files are read as UTF-8 (invalid bytes are replaced) unless they look binary, which gives `unsupported`. Files over 64 MiB give `too_large`.

### 8.7 Changes

The user allows changes per shared folder, on their computer (`writable = true` in `config.toml`, `cww roots allow-changes`, or a switch in the terminal UI and the desktop app). Nothing the server sends can turn it on. While no folder allows changes, `tools/list` has only the seven read-only tools and a call to a change tool is an unknown tool (`-32602`). While one does, these eight are listed after them:

| Tool | Does | `readOnlyHint` | `destructiveHint` | `idempotentHint` | `_meta["com.chatwithwork/writeWhen"]` |
|---|---|---|---|---|---|
| `create` | New text file; fails if the path exists | false | false | false | |
| `write` | Replace a text file's content, or append to it; creates it if missing | false | **true** | false | `{"mode": "append"}` |
| `edit` | Replace one exact span of a text file | false | false | false | |
| `mkdir` | New folder, `parents` optional | false | false | true | |
| `move` | Move or rename a file or folder; `replace` optional | false | **true** | false | `{"replace": false}` |
| `delete` | Move a file or folder to the system trash | false | **true** | false | |
| `create_document` | New Word (`.docx`) or Excel (`.xlsx`) file; `replace` optional | false | **true** | false | `{"replace": false}` |
| `patch` | Edits to several text files and new text files, approved as one change set and made all or nothing (8.20) | false | false | false | |

All of them are `openWorldHint: false`.

**`writeWhen`.** MCP annotations can't vary with arguments, so a tool that is destructive only with some arguments is annotated `destructiveHint: true` (the worst case) and names, in `_meta["com.chatwithwork/writeWhen"]`, the argument values that make a call a plain write: the call is not destructive when every argument listed has the value given, where a missing argument counts as its default (`mode` defaults to `"replace"`, `replace` to `false`). So `write` with `mode: "append"`, `move` without `replace` (or with `replace: false`), and `create_document` without `replace` are writes; `write` without `mode` (a replace), `move` or `create_document` with `replace: true`, and every `delete` are destructive. A server that ignores the hint treats them all as destructive, which is safe.

**Rules every change follows**, in the daemon, whatever the server decided:

- The folder must allow changes (`not_writable`), and so must any folder shared on its own inside it (a folder shared read-only stays read-only inside one that allows changes), and the path follows 8.1. The shared folder itself can't be changed, moved or deleted (`invalid_path`).
- Folders are resolved on handles as reads are, but **never through a symlink or junction**, even in a folder that follows links for reads, and the change happens relative to the open folder handle. Symlinks, files with more than one hard link, FIFOs, sockets and devices are never changed, moved or deleted (`denied`); a symlink is never followed.
- The deny list applies to every path a change touches, sources and destinations, and to everything inside a folder that is moved or deleted (`denied`). So does a second list of paths that can be read but are **never changed**: every entry of the user's home folder whose name starts with a dot, version control internals (`.git`, `.hg`, `.svn`, `.bzr`, `.jj`, and any folder holding Git's `HEAD`, `objects` and `refs`), shell startup files (`.bashrc`, `.zshrc`, `.profile`, ...), Git and terminal settings (`.gitconfig`, `.config/git`, `.tmux.conf`), desktop session and window manager settings (`.xinitrc`, `.xprofile`, `.config/hypr`, `.config/i3`, `.config/sway`, ...), autostart and launch agents, editor, hook, interpreter and tool settings that run commands (`.vscode`, `.idea`, Vim and Emacs settings, `.husky`, `.pre-commit-config.yaml`, `mise.toml`, `*.pth`, `.envrc`, `.direnv`, `.cargo`, ...), and `desktop.ini` and `autorun.inf` (`denied`). The list is the daemon's; it may grow, and a server should only show the refusal.
- **No programs are made:** no name with an extension that runs or launches something on any of the three systems (`.exe`, `.bat`, `.ps1`, `.lnk`, `.url`, `.desktop`, `.app`, `.command`, `.jar`, `.so`, Office files with macros, documents that make Windows or Office run or fetch things such as `.rtf`, `.mht`, `.slk`, `.iqy`, `.rdp` and `.theme`, and more; also `.js` on Windows), no text with an Office processing instruction (`<?mso-application?>`), and no file ever gets an execute bit. Every file the daemon makes carries the system's mark for downloads (Mark-of-the-Web on Windows, quarantine on macOS). Existing executable files, files marked read-only, and files owned by another user are never changed (`not_changeable` or `denied`); executables and read-only files can still go to the trash. New names can't contain control characters, direction marks, `\ / : * ? " < > |`, start with a space or end with a dot or a space, or be a Windows device name (`invalid_path`).
- **Text tools write text.** `create`, `write` and `edit` take UTF-8 without NUL characters, refuse names of documents and media that text would corrupt (`.pdf`, `.docx`, `.xlsx`, `.zip`, images, ...; `not_changeable`, pointing `.docx` and `.xlsx` to `create_document`), and only change existing files that are UTF-8 text (`not_changeable`). `edit` and `append` keep the file's line endings and byte order mark.
- **Nothing is deleted for good.** A replaced, appended or edited file's previous version, a deleted file or folder, and a file a `move` or `create_document` replaces all go to the **system trash** on the user's computer: the freedesktop.org trash on Linux (`$XDG_DATA_HOME/Trash`, or `$topdir/.Trash/$uid` or `$topdir/.Trash-$uid` for other filesystems, with a `.trashinfo` record so file managers can restore it), the Trash on macOS (`~/.Trash`, or `.Trashes/$uid` on other volumes), and the Recycle Bin on Windows (with the `$I` record Restore uses). When the trash can't take an item (a network drive, a removable drive on Windows, a mount inside the folder), the change is refused with `trash_unavailable` and nothing changes.
- **Atomic.** New content is written to a hidden file beside the target and flushed to disk. A new file is renamed into place without replacing anything (`renameat2(RENAME_NOREPLACE)`, `renamex_np(RENAME_EXCL)`, or a handle rename without `ReplaceIfExists`). A replaced file is swapped with its new version in one step where the filesystem can (Linux, macOS), keeping its group, extended attributes and ACLs, and the old version then goes to the trash; elsewhere (Windows, which keeps the access list) the old version goes to the trash first. A crash leaves the old file, the new file, or the old one in the trash or beside the file under a hidden name; never half a file. A move to another filesystem copies, then moves the original to the trash.
- **Budgets** (section 10): changes per minute and per day and bytes written per hour, separate from the read budgets, and a size cap per file. A `patch` counts as one change per file it changes. Dry runs and refused calls don't count against them.
- Changes run one at a time, and every one is in the local audit log with its effect, the paths before and after, the bytes written, and where the old version went in the trash.

**Dry runs.** Every change tool takes `dry_run: true`: the daemon runs every check above against the files as they are now and answers what the call would do (`"dry_run": true`), without changing anything or counting against the change budgets. For `write` and `edit` the answer includes a unified diff (`diff`, at most 32 KiB) and the file's current SHA-256 (`previous.sha256`); for `patch`, one per file (8.20). The diff shows the file as it is, so it counts against the read budgets of the file's folder (section 10) like a `read` of as many characters: it is cut to what the budget allows (ending with a line that says so), and once the budget is used up, a dry run of `write` or `edit` is refused with `rate_limited`, as a read would be. A server that gets `rate_limited` for its own preview can show the card without the diff (with `old_text` and `new_text` for `edit`). A server can make a dry run itself, with the model's arguments plus `dry_run: true`, to fill the approval card, and then pass the SHA-256 back as `expected_sha256` when the user approves, so `write` and `edit` fail with `conflict` if the file changed in between. Dry runs are logged locally like other calls.

**Results.** A change answers:

```json
{
  "path": "work-docs:plans/q4.md",
  "effect": "replaced",
  "dry_run": false,
  "kind": "file",
  "size": 2100,
  "sha256": "9f2c…",
  "previous": { "size": 2048, "modified": "2026-09-20T08:14:03Z", "sha256": "4b1a…", "in_trash": true }
}
```

| Field | Present | Meaning |
|---|---|---|
| `path` | always | The path the change produced: the destination of a move. |
| `effect` | always | `created`, `replaced`, `appended`, `edited`, `created_folder`, `moved`, `trashed`, or `unchanged` (`mkdir` of a folder that exists). |
| `dry_run` | always | True when nothing changed. |
| `kind` | always | `file` or `dir`. |
| `from` | `move` | Where it came from. |
| `size`, `sha256` | files written | The new file's size and SHA-256. |
| `previous` | replaced, edited, appended, deleted, or replaced by a move | What was there: `size` (for a folder, the bytes it holds), `modified`, `sha256` (text changes only), and `in_trash`, true once it is in the trash. |
| `diff` | dry runs of `write` and `edit` | A unified diff, cut at 32 KiB. |
| `entries` | folders moved or deleted, `mkdir` with `parents` | How many files and folders a folder holds, or how many folders were made. |

Where the trash is on the computer is never sent; only the local audit log has it.

### 8.8 `create`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | The new file. Its folder must exist (`not_found` says to use `mkdir`). |
| `content` | string, required | UTF-8 text. |
| `dry_run` | boolean | |

Fails with `exists` if anything is at the path. Effect `created`.

### 8.9 `write`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | |
| `content` | string, required | The new text, or the text to add. |
| `mode` | `"replace"` (default) or `"append"` | |
| `expected_sha256` | string | Hex SHA-256 the file's current content must have (`conflict` otherwise, or when the file is gone). |
| `dry_run` | boolean | |

Effect `replaced` or `appended`, or `created` when the file didn't exist. The previous version goes to the trash.

### 8.10 `edit`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | An existing text file. |
| `old_text` | string, required | Must appear exactly once. Not empty. At most 8 KiB and at most half the file, unless it is 64 bytes or less. |
| `new_text` | string, required | Different from `old_text`. |
| `expected_sha256` | string | As for `write`. |
| `dry_run` | boolean | |

`invalid_argument` when `old_text` isn't found ("Read the file again and quote the text exactly") or appears more than once ("appears N times; include more of the surrounding text"), and when it is larger than the limits above ("Use write to replace the file's content"): an edit is a write, which a server may let through for the rest of a chat, so replacing most of a file takes `write`, which is destructive and always asks. When the file uses Windows line endings and `old_text` has none, `\n` in both texts matches `\r\n`. Effect `edited`; the previous version goes to the trash.

### 8.11 `mkdir`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | |
| `parents` | boolean | Also make missing folders on the way (`entries` says how many were made). |
| `dry_run` | boolean | |

Effect `created_folder`, or `unchanged` when the folder exists. `exists` when a file is in the way.

### 8.12 `move`

| Argument | Type | Notes |
|---|---|---|
| `from` | string, required | A file or folder. |
| `to` | string, required | The new path, name included; in the same shared folder or another that allows changes. |
| `replace` | boolean | Default false. With true, a **file** already at `to` goes to the trash first; a file can only replace a file. |
| `dry_run` | boolean | |

`exists` when something is at `to` and `replace` isn't true. A folder can't move into itself, and a folder holding a shared folder or anything on the deny list can't move (`denied`), nor one holding more than the entry limit (`too_large`). Executables, read-only files and other users' files don't move. Between filesystems the item is copied (regular files and folders only, up to the size cap; `not_changeable` for folders with links, special files or programs), then the original goes to the trash. Effect `moved`, with `from`; `previous` describes a replaced file.

### 8.13 `delete`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | A file or folder. |
| `dry_run` | boolean | |

Moves it to the system trash, where the user can restore it. Effect `trashed`; `entries` counts what a folder held. The same refusals as a move's source apply, except that executables and read-only files may go.

### 8.14 `create_document`

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | Ends in `.docx` or `.xlsx`; anything else is `invalid_argument` (PowerPoint and macro formats aren't made). |
| `content` | string | For `.docx`, required: the document in Markdown. Headings, paragraphs, bold, italic, strikethrough, inline code, bulleted and numbered lists (nested), quotes, code blocks and tables. Links become their text with the address after it in parentheses, never a live link; images become nothing. |
| `sheets` | array | For `.xlsx`, required: `[{"name": "Budget", "rows": [["Item", "Cost"], ["Falcon", 10.5]], "header": true}]`. Names have 1 to 31 characters, none of `[ ] : * ? / \`, unique ignoring case. Cells are strings, numbers, booleans, or `null` for empty; never formulas (a string starting with `=` stays text). `header` bolds and freezes the first row. |
| `replace` | boolean | Default false. With true, an existing file at the path goes to the trash and a new one is written: existing documents are never edited in place. |
| `dry_run` | boolean | |

Effect `created`, or `replaced`. `exists` without `replace`.

### 8.15 Commands

Offered only while a shared folder allows commands (`allows_commands` in `roots`, only where `writable` too), which the user turns on on this computer, never the server. See [docs/commands.md](docs/commands.md) for the sandbox each command runs in.

Every command has an approval decided in Chat with Work (the web or any of its apps, never on the computer), or runs without a card in a chat where the person chose "Allow everything" as their permission mode, as file changes do; the server still records it. Guests, scheduled and event runs, and bots' and agents' chats always ask (a bot's unattended loan aside, 7.2.1). The daemon can't tell either from a forged approval: what it decides is where commands may run at all, per folder, and whether they may use the network, which only the person on the computer turns on. There is no allowlist of command lines.

`run` (`destructiveHint: true`, `openWorldHint: true`):

| Argument | Type | Notes |
|---|---|---|
| `command` | string, required | One line of shell, run with `sh -c`, or on Windows with Windows PowerShell 5.1 (`;` between commands, no `&&`), at most 8 000 characters. |
| `path` | string, required | The folder it starts in: `<root_id>:` or `<root_id>:dir`. `not_writable` where commands aren't allowed. |
| `background` | boolean | Answer at once and leave it running. |
| `dry_run` | boolean | Answer `{"dry_run": true, "path", "folder", "dir", "command", "shell", "network", "background", "limits": {"cpu_seconds", "memory_mb", "timeout_seconds", "processes", "output_chars"}}` and run nothing. `shell` is `sh`, or `powershell` on Windows. |

Without `background` it waits up to 15 seconds, under the server's 20-second wait for a call. While it waits, a request with a `progressToken` gets `notifications/progress` with `progress` the bytes printed so far and `message` its latest line. It answers:

```json
{ "path": "app:api", "folder": "App", "command": "bin/rails test", "network": false,
  "id": "3f9a0c1d2e4b", "state": "exited", "exit_code": 1, "duration_ms": 8120, "output_bytes": 5120,
  "stdout": "...", "stderr": "...", "stdout_cut": 0, "stderr_cut": 0 }
```

`state` is `running`, `exited` (with `exit_code`; a command ended by a signal exits with 128 plus the signal, as a shell reports it), `timed_out`, `stopped`, or `failed` (the sandbox couldn't start it, with `error`). `limit` is there when a limit ended it: `cpu_time` (it `exited`, killed at its CPU limit), `processes` (on Linux `stopped`: it had more processes at once than `limits.processes`, checked a few times a second; on Windows it `exited` with an error after a process past the limit couldn't start), or `memory` (Windows: it `exited` with an error after an allocation past `limits.memory_mb` failed). On Windows the exit code is the last command's, as with `sh -c`. `stdout` and `stderr` are the end of each stream, `*_cut` the characters left out before it. A command still running has a `note` saying how to follow it. `rate_limited` when 4 commands already run, `paused` while the computer is paused. Pausing the computer, or its revocation, stops the commands running.

`run_status` (`{"run": id}`, read-only) answers `{"status": {...}, "stdout", "stderr", "stdout_cut", "stderr_cut"}`. `run_output` (`{"run": id, "offset": 0, "max_chars": 16000}`, read-only) answers `{"id", "offset", "next_offset", "skipped", "text", "done"}`: both streams as they came, from a byte offset, at most 32 000 characters; `skipped` is what's no longer kept (the latest megabyte is). `run_stop` (`{"run": id}`, a write) stops it and everything it started, and answers as `run_status` once it has. An unknown handle is `not_found`; commands are forgotten when the daemon restarts.

### 8.16 Browser

Offered only while the person turned the browser on on this computer (`[browser] enabled` in `config.toml`, `cww browser enable`, or the desktop app), never from the server, and only to a person's chats or a bot's the browser is lent to. The daemon then lists these tools after the others; turning the browser on or off reconnects, as for folders. They act through the Chat with Work extension in the person's own Chromium-family browser, only on tabs in its "Chat with Work" tab group, and only on the sites the person allowed on the computer (`[browser] sites`, `cww browser allow`), or all HTTP(S) sites when explicitly selected locally (`all_sites`, `cww browser allow-all`). `blocked_sites` takes precedence in both modes. Nothing in the remote protocol changes these settings. The extension enforces every rule, and the daemon again those it can check (the kill switch, the sites, bots, budgets); see [docs/browser.md](docs/browser.md).

| Tool | `readOnlyHint` | `openWorldHint` | Arguments | Answer |
|---|---|---|---|---|
| `browser_tabs` | true | true | none | `tabs`: `{tab, title, url, allowed: true, loading}` for tabs on allowed sites, `{tab, site, allowed: false}` for the others (never their title or address); `sites`: the saved allowed list; `all_sites`: whether all HTTP(S) sites are enabled; `blocked_sites`: blocked entries |
| `browser_read` | true | true | `tab`, `offset`, `max_chars` (1 to 32 000) | `tab`, `url`, `title`, `text`, `offset`, `next_offset` (null at the end), `total_chars` |
| `browser_screenshot` | true | true | `tab` | `tab`, `url`, `title`, `width`, `height`, and an `image` content block (`image/jpeg`, at most about 180 KB) after the text block |
| `browser_scroll` | true | true | `tab`, `direction` (`up` or `down`), `pages` (1 to 10), or `ref` | `tab`, `url`, `title`, `scroll_y`, `page_height`, `view_height`, `at_end` |
| `browser_wait` | true | true | `tab`, `text` (1 to 500 characters), `seconds` (1 to 15) | `tab`, `url`, `title`, `found`, `waited_ms` |

While the person also allowed actions (`[browser] changes`, `cww browser allow-changes`, or the desktop app), these follow, all `readOnlyHint: false` and `openWorldHint: true`, each taking `dry_run`:

| Tool | `destructiveHint` | `writeWhen` | Arguments | Answer |
|---|---|---|---|---|
| `browser_open` | false | | `url` (required, on an allowed site), `tab` | where the tab ended up, `new_tab` |
| `browser_back` | false | | `tab` | where the tab ended up |
| `browser_close` | false | | `tab` (required) | `closed` |
| `browser_select` | false | | `ref`, `values` (1 to 50 option values or labels), `tab` | `target`, `chosen` |
| `browser_click` | **true** | `{"submits": false}` | `ref`, `submits` (default true), `tab` | `target`, `submits`, `reason`, where the tab ended up |
| `browser_type` | **true** | `{"submit": false}` | `ref`, `text` (up to 10 000 characters), `clear` (default true), `submit` (press Enter, default false), `tab` | `target`, `submit`, where the tab ended up |
| `browser_download` | false | | `path` (required: `<root_id>:relative/path`, a new file in a folder that allows changes), `url` or `ref`, `tab` | the change result (8.7) with `url` and `mime_type` |

The computer's **Ask before browser actions** setting defaults on. When the person explicitly switches it off locally, each offered browser action carries literal boolean `_meta["com.chatwithwork/browserAutoApprove"]: true`. Read tools, files and commands never carry it; all effect annotations and `writeWhen` remain unchanged. The paired server may use this device-specific consent for a person's eligible chat, audit `via: "browser_local_policy"`, and send the same literal boolean in the **request metadata**, never model arguments. Bots do not inherit it. A request carrying it is limited to browser tools; the daemon checks the current local opt-out, performs a dry run, then rechecks before execution. Restoring prompts invalidates already-listed automatic calls (`needs_approval`). An older server keeps asking. The extension still enforces sites, tab groups, Stop, protected fields and actions-off. Local control calls without `approved` follow the same opt-out and preflight; the local audit records `browser_local_policy` without page contents. CLI: `cww browser automatic-actions` and `cww browser ask`.

`browser_download` is listed only while a shared folder allows changes. The schemas give each flag's `default` (`submits` is `true`), which is what a missing argument counts as for `writeWhen`. A click is a plain write only when the call says `submits: false`, and the extension refuses such a call (`would_submit`) when the element submits a form, is a submit button, sends a non-GET request (`data-method`, `data-turbo-method`), or is named like an action that buys, pays, sends, posts, publishes, deletes, removes, confirms, transfers or signs (in English, German, Italian, French and Spanish). `browser_type` refuses password, payment and one-time code fields (`sensitive_field`). A click is refused when something else is drawn over its element (`page_error`).

A dry run checks the call and answers what it would do without doing it; for `browser_click`, `browser_type` and `browser_select` it adds an `image` content block, a screenshot with the element outlined (at most about 120 KB), for the approval card. For `browser_download` it checks the path and the address, fetches nothing, and answers as a change's dry run, with `url` and the file's `name`. Actions that aren't dry runs count against the change budgets (section 10, one change each; a download, its size). `browser_download` fetches with the person's sign-in to the site, follows no redirect (an address that redirects, even within an allowed site, fails with `page_error`, so the person's sign-in never goes to an address that wasn't checked first), takes at most 10 MB, and saves through the same rules as every new file (8.7): never over anything, never a program or launcher, with the system's download mark.

An action that leaves the tab on a site that isn't allowed answers `{"tab", "allowed": false, "site", "note"}`, with nothing from the page. `browser_back` and `browser_close` work from such a page; nothing else does until it's back.

`tab` is a tab's id from `browser_tabs`; without it, the group's tab used last. `browser_read`'s text is the page's accessibility tree, one line per item (`- heading "Inbox" [level=1]`, `- link "Compose" [ref=e41]`, `- textbox "Search" [ref=e110]: budget`, `- paragraph: text`), indented by nesting; things that can be clicked or typed in carry `[ref=e<n>]`. Password, card and one-time code fields show `[sensitive]: (hidden)`, and are blurred in screenshots. `max_chars` is lowered by the read budget (section 10), and the characters returned count against it like a `read`. An answer whose `url` isn't on an allowed site is dropped by the daemon (`site_not_allowed`). When the page opened a JavaScript dialog, the extension dismissed it and the answer's `dialog` says what it was.

Errors (section 9) the browser adds: `browser_unavailable` (no browser with the extension is connected), `browser_stopped` (the person stopped it, in the extension or on the computer, or paused the computer), `site_not_allowed`, `not_in_group` (the tab isn't in the group), `no_tab` (the group is empty), `sensitive_field`, `would_submit`, and `page_error` (the page or the browser failed). A bot's call to a browser not lent to it is `not_lent`.

### 8.17 `grep`

Searches the text of files for lines matching a regular expression, as ripgrep does. Offered always, with the read tools.

| Argument | Type | Notes |
|---|---|---|
| `pattern` | string, required | 1 to 1000 characters, a regular expression in ripgrep's syntax (Rust's `regex`). A match never spans lines. |
| `path` | string | A shared folder (`<root_id>:`), a folder in one, or a file. Without it, every shared folder the caller reaches (for a bot, only the folders lent to it), in the order of their IDs. |
| `glob` | string | Only files matching it, with `search`'s `path_glob` rules: without `/` the file name, with `/` the path relative to `path` (or to the shared folder). Case-insensitive. |
| `ignore_case` | boolean | Default false. |
| `output` | `"content"` (default), `"files"`, or `"count"` | |
| `context` | integer | 0 to 10 lines before and after each match, default 0. Only with `content`. |
| `limit` | integer | 1 to 500, default 100: matching lines for `content`, files for `files` and `count`. |
| `offset` | integer | Results to skip, for the next page, default 0. |

```json
{ "output": "content", "matches": [ { "path": "app:src/lib.rs", "line": 12, "text": "fn main() {",
    "before": [ { "line": 11, "text": "" } ], "after": [ { "line": 13, "text": "    run();" } ] } ],
  "files_searched": 412, "truncated": false, "timed_out": false, "next_offset": null }
{ "output": "files", "files": [ "app:src/lib.rs", "app:src/main.rs" ], "files_searched": 412, "truncated": false, "timed_out": false, "next_offset": null }
{ "output": "count", "counts": [ { "path": "app:src/lib.rs", "count": 3 } ], "files_searched": 412, "truncated": false, "timed_out": false, "next_offset": null }
```

The walk is ripgrep's: it respects `.gitignore`, `.ignore` and Git's `info/exclude` (from the shared folder down, also when `path` is a folder inside it), skips hidden files and folders, and never follows a symlink. A folder named as `path` is searched even when hidden or ignored, and a file named as `path` is searched as it is. The deny list applies exactly as for `read` and the index, and files are opened with `read`'s refusals (no links, no special files, no files with more than one hard link unless the config allows them). Binary files (any NUL byte), documents (PDF, Office) and files over the index's size cap (32 MiB) are skipped. `files_searched` counts the files whose text was searched. Lines are cut at 500 characters; `before` and `after` appear only with `context` above 0, and a match's context may repeat lines another match shows. Results are in path order (by shared folder, then one folder level at a time, names in byte order), then line order. `truncated` is true, and `next_offset` the `offset` of the next page, when there were more results than `limit`. After 10 seconds the search stops and answers what it found, with `timed_out: true` and a `note`.

The characters returned (paths and line texts) count against the read budgets of their folders (section 10). When a budget runs out mid-answer, the answer stops before the first result that doesn't fit, with `truncated: true`, `next_offset`, and a `note`; when not even the first fits, the call is `rate_limited`. An answer too large for one message is cut the same way. The audit log records it as `search` is: the pattern (its first 256 characters), the path, and how many results it returned.

### 8.18 `glob`

Finds files by name. Offered always, with the read tools.

| Argument | Type | Notes |
|---|---|---|
| `pattern` | string, required | 1 to 500 characters: `**/*.rb`, `src/**/test_*.py`, `*.md`. Without `/` it matches the file name at any depth; with `/`, the path relative to `path`. Case-sensitive. |
| `path` | string | A folder (or `<root_id>:`). Without it, every shared folder the caller reaches. A file is `not_a_directory`. |
| `hidden` | boolean | Default false: hidden files and folders are skipped unless this is true. |
| `limit` | integer | 1 to 1000, default 200. |
| `offset` | integer | Default 0. |

```json
{ "paths": [ { "path": "app:src/lib.rs", "size": 2048, "modified": "2026-10-20T08:00:00Z" } ],
  "truncated": false, "next_offset": null }
```

Files only, walked as `grep` walks (ignore files respected, the deny list, no symlinks followed), sorted by path. The paths returned count against the read budgets as `grep`'s do, and the answer is cut the same way. After 10 seconds the walk stops and the answer adds `"timed_out": true` and a `note`; the field is there only then.

### 8.19 `read_bytes`

Copies one file's bytes, whatever it holds, a chunk at a time. Offered always, with the read tools (read annotations). The server never gives it to a model: it uses it for its own tool that copies one file the model names into the chat's files.

| Argument | Type | Notes |
|---|---|---|
| `path` | string, required | A file, of any type (binary included). |
| `offset` | integer | Byte offset, default 0. Past the end is `invalid_argument`. |
| `max_bytes` | integer | 1 to 65536, default 65536. |

```json
{ "path": "app:assets/logo.png", "offset": 0, "next_offset": 65536, "size": 204800, "modified": "2026-10-20T08:00:00Z", "data": "iVBORw0KGgo…" }
```

`data` is the bytes in standard base64; `next_offset` is `null` at the end. A full chunk of 64 KiB, which travels twice in base64 (8.2), stays within one message. A chunk is shorter than `max_bytes` without being the last only when the copy budget has less left. The path is resolved, and refused, exactly as for `read` (the deny list, links, special files, hard links); files over 25 MiB are `too_large`. It counts against its own budget, not the read characters: `[limits] copy_bytes_per_hour`, 100 MiB for all chats per rolling hour, counted per chunk returned; past it, `rate_limited`. The audit log records the path and the size of the answer, never the contents.

### 8.20 `patch`

A change tool (8.7), listed with the others while a folder allows changes: edits to several text files and new text files, approved on one card and made all or nothing. `readOnlyHint: false`, `destructiveHint: false`, `idempotentHint: false`, `openWorldHint: false`, without `writeWhen`: like `edit`, a write.

| Argument | Type | Notes |
|---|---|---|
| `changes` | array, required | 1 to 50 changes, touching at most 20 distinct files. Each is either an edit `{ "path", "old_text", "new_text" }`, with exactly `edit`'s rules (8.10), or a new file `{ "path", "content" }`, with `create`'s rules (8.8: `exists` if anything is there, and its folder must exist). Several edits to one file apply in order, each to the text the one before left. A path can't be both created and edited, nor created twice. |
| `expected_sha256` | object | A path (any spelling of `<root>:rel` that normalizes to a file the patch edits) to the hex SHA-256 its current content must have; `conflict` otherwise. A key naming a file the patch creates, or doesn't touch, is `invalid_argument`. |
| `dry_run` | boolean | |

Every rule of 8.7 applies to every file (the folder allows changes, the deny lists, the never-changed list, no programs, text only, the trash, the budgets, the size cap). **All or nothing:** every change is checked against the files as they are before anything is written; if one fails, nothing changes, and the error names the change by its index in `changes` (from 0) and its path, in the usual error sentence: `invalid_argument: Change 2 (app:src/lib.rs): old_text was not found in the file. Read the file again and quote the text exactly, whitespace included. No file was changed.` Then the files are changed: each new version is written to a hidden file beside its file and flushed; then, one file at a time, it takes the file's name, an edited file's previous version staying beside it under a hidden name (swapped in one step on Linux and macOS, moved aside first on Windows); only once every file is in place do the previous versions go to the system trash. If any step fails, the files already changed get their previous versions back (from beside them, or, once a previous version is in the trash, from the content read before), new files are removed, and the error says the folder is as it was. Should putting one back fail too, the error names it and says its previous version is beside it under a hidden name or in the trash. See [docs/file-changes.md](docs/file-changes.md).

Dry run:

```json
{ "dry_run": true, "effect": "patched", "files": [
    { "path": "app:src/lib.rs", "effect": "edited", "edits": 2, "diff": "--- app:src/lib.rs\n+++ app:src/lib.rs\n@@ ...",
      "previous": { "size": 2048, "modified": "2026-10-20T08:00:00Z", "sha256": "4b1a..." } },
    { "path": "app:src/new.rs", "effect": "created", "diff": "...", "size": 120 } ] }
```

Files are in the order they first appear in `changes`. Each diff is a unified diff like `edit`'s dry run (a new file's adds every line). The diffs together are cut at 64 KiB: the file whose diff was cut ends with a line saying so, and later files get `"diff": null` with `"diff_cut": true`. The diffs read the files as they are, so each counts against the read budgets of its file's folder like `edit`'s dry run: cut where a budget ends (ending with a line that says so, later files `null` and `diff_cut`), and a dry run that touches a folder whose budget is spent is `rate_limited`. A diff that would make the answer too large for one message is dropped the same way, from the last file.

Applied:

```json
{ "dry_run": false, "effect": "patched", "files": [
    { "path": "app:src/lib.rs", "effect": "edited", "size": 2100, "sha256": "9f2c...",
      "previous": { "size": 2048, "modified": "...", "sha256": "4b1a...", "in_trash": true } },
    { "path": "app:src/new.rs", "effect": "created", "size": 120, "sha256": "..." } ] }
```

It counts as one change per file against the changes-per-minute and per-day budgets, plus the bytes written, all or none. Each file gets its own entry in the local audit log, as a change of its own would (tool `patch`, the path, its effect, the bytes written, where its previous version went in the trash; never contents); a refused patch is one entry naming its first file.

## 9. Errors

**OAuth endpoints** return `{"error": "<code>", "error_description": "<optional>"}` with a 4xx status, using the codes above plus RFC 6749 codes. `use_dpop_nonce` triggers one retry. `invalid_grant`, `unauthorized_client` and `access_denied` on refresh mean the device is revoked.

**JSON-RPC errors** are for requests that can't be routed:

| Code | When |
|---|---|
| `-32600` | Request larger than 256 KiB. |
| `-32601` | Unknown or unsupported method. |
| `-32602` | Unknown tool name, missing `_meta` in a stateless session, or malformed params. |
| `-32603` | Response too large, or internal failure. |

**Tool errors** are for everything that happens inside a known tool. They come back as a normal result so the model can read them:

```json
{
  "content": [ { "type": "text", "text": "denied: this path is on the deny list (.env*)" } ],
  "structuredContent": { "error": { "code": "denied", "message": "this path is on the deny list (.env*)" } },
  "isError": true
}
```

| `code` | Meaning |
|---|---|
| `invalid_argument` | Missing, malformed, unknown, or out-of-range arguments. |
| `invalid_path` | The path breaks a rule in 8.1. |
| `unknown_root` | No shared root has that ID. |
| `outside_root` | Resolution would leave the root. |
| `denied` | Deny list, a symlink (not followed), a file with more than one hard link, or a FIFO, socket or device. |
| `not_found` | No such file or directory, or the root folder is missing right now. |
| `not_a_file` / `not_a_directory` | Wrong kind of entry for `read` / `list`, or for a change. |
| `too_large` | The file is over the size limit, or a folder holds more than a change may move. |
| `unsupported` | The file can't be turned into text (binary, encrypted or damaged). |
| `rate_limited` | A daemon-side rate or volume limit was hit, for reads or for changes. The refusals of the tool calls per minute and of the hourly read budgets say when they free up ("It frees up in about 12 minutes: stop calling this computer's tools until then, and tell the person."), so the caller stops calling instead of spending its turns on refusals; the change budgets and the copy budget say only to try again later. A `grep` or `glob` whose first result is larger than what's left of the read budget says so, and to narrow the search, instead. |
| `paused` | The user ran `cww pause`. |
| `not_writable` | A change in a shared folder that doesn't allow changes. The message names the folder's label and says the person can allow changes on their computer. |
| `exists` | Something is already at the path (or at `to`), and the call doesn't replace it. |
| `not_changeable` | A kind of file the daemon never makes or changes: a program or launcher, an executable file, a binary file as text, an Office file as text. |
| `trash_unavailable` | The system trash can't take the old version or the item, so nothing changed. |
| `conflict` | The file changed since the caller looked (`expected_sha256`, or during the change). |
| `not_lent` | A bot's call (7.2.1) named a folder not lent to that bot, followed a command outside them, or searched with nothing lent to it, or used a browser not lent to it. |
| `needs_approval` | A bot's unattended `run` (7.2.1) in a folder whose loan wants a person to approve each command. |
| `browser_unavailable`, `browser_stopped`, `site_not_allowed`, `not_in_group`, `no_tab`, `sensitive_field`, `would_submit`, `page_error` | The browser tools' refusals and failures (8.16). A browser action while the person allows only reading is `not_writable`. |
| `internal` | Unexpected failure. |

Change errors are sentences, for example `not_writable: Changes aren't allowed in the shared folder “Work docs”. The person can turn on Allow changes for it in Chat with Work Desktop on their computer.` or `trash_unavailable: The system trash can't take this item, so nothing was changed. cww never deletes a file for good.` `not_writable`, `not_changeable`, `denied`, `invalid_path`, `unknown_root`, `outside_root`, `rate_limited`, `paused`, `not_lent`, `needs_approval`, `browser_stopped`, `site_not_allowed`, `not_in_group`, `sensitive_field` and `would_submit` are logged as refusals, the others as errors.

Messages are written for the model and the user. They never include absolute paths.

## 10. Limits

These are enforced by the daemon whatever the server does. The defaults can be changed only in the local config file, under `[limits]`.

| Limit | Default |
|---|---|
| Tool calls per rolling minute (all chats) | 240 |
| Characters per `read` call | 32 000 |
| Characters read per chat per rolling hour, from folders without a budget of their own | 5 000 000 |
| Characters read per rolling hour, all chats, from those folders | 10 000 000 |
| A folder's own read budget (`read_budget`), per chat per rolling hour | 5 000 000 (the shared per-chat budget) to 20 000 000; all chats together twice it |
| Bytes `read_bytes` returns per rolling hour, all chats | 100 MiB |
| Search hits per call / snippet length | 20 / 300 characters |
| `list` entries per page | 200 |
| Largest file `read` opens | 64 MiB |
| Largest file indexed | 32 MiB |
| JSON-RPC message size | 256 KiB |
| Live grep time budget (`search`) | 5 seconds |
| `grep` and `glob` time budget | 10 seconds |
| `grep` line length | 500 characters |
| Largest file `grep` searches | 32 MiB (as indexed) |
| Largest file `read_bytes` copies | 25 MiB |
| Changes per rolling minute / day (all chats) | 30 / 500 |
| Bytes changes may write per rolling hour | 50 MiB |
| Largest file a change creates, edits or copies | 10 MiB |
| Files and folders one move or delete of a folder may carry | 1000 |
| Changes in one `patch` / files they touch | 50 / 20 |
| A `patch` dry run's diffs, together | 64 KiB |

**Read budgets.** "Characters read" are what `read`, `grep` and `glob` return (paths and line texts), what `browser_read` returns, and the diffs of dry runs of `write`, `edit` and `patch`. They count against the budgets of the folder they come from: the shared ones (`read_chars_per_chat_hour`, `read_chars_per_hour`), or, for a folder the person gave a budget of its own on the computer (`read_budget` on the root in `config.toml`, `cww roots read-budget`, or the control API's `roots_read_budget`, CONTROL.md), a per-chat budget of that size and an all-chats budget of twice it for that folder alone, instead of the shared ones. The server can't set or raise either. `read` stops at the budget left; `grep` and `glob` stop their answer at the first result that doesn't fit (with `truncated`, `next_offset` and a `note`), and answer `rate_limited` when not even the first does; a dry run's diff is cut where the budget ends, and a dry run touching a folder whose budget is spent is `rate_limited`. `read_bytes` has its own budget, in bytes, for all chats (`copy_bytes_per_hour`), and doesn't count against the read budgets.

## 11. Decisions this spec makes

`docs/desktop.md` left these points open or ambiguous. This is what the daemon does:

1. **Framing.** The design says both "JSON-RPC frames on a WebSocket" and "terminated by an Action Cable channel". The daemon speaks Action Cable (`actioncable-v1-json`, channel `Desktop::Channel`) by default, with bare JSON-RPC (`mcp`) as a negotiated alternative for relays and other servers.
2. **Tool names** are `roots`, `search`, `list`, `read` and the others in section 8, without the `local_` prefix the design's table shows. The server adds `local_<device_id>_`, which would otherwise produce `local_7_local_read`.
3. **DPoP.** Standard RFC 9449 proofs with `alg: EdDSA` on every token call and on the upgrade, with `ath` on the upgrade. "Signs a server nonce" is implemented as the RFC 9449 `DPoP-Nonce` mechanism, not as a custom challenge frame.
4. **`htu` for the upgrade** is the `https://` form of the socket URL, which is what Rails sees as `request.url`.
5. **Form-encoded OAuth requests** (as in RFC 6749 and 8628) with JSON responses. The device metadata travels as extra form fields.
6. **The token only gates the upgrade.** An open socket is not tied to the 10-minute token lifetime. The server revokes by closing with 4003 and refusing refreshes, and can force re-authentication with 4001.
7. **Chat ID** travels in request `_meta` under `com.chatwithwork/chatId` and is optional.
8. **The size cap** is measured on the JSON-RPC message, not the Action Cable envelope. Frames get 2× headroom.
9. **Results** carry both `structuredContent` and the same JSON as a text block. Tool-level problems are `isError` results with a stable `code`, and JSON-RPC errors are kept for routing failures.
10. **Root labels and IDs are visible to the server.** Open question 7 in the design is answered in favor of labels. Absolute paths are never visible.
11. **Hard links** are refused by default: a file with a link count above 1 is `denied`. A hard link inside a root can point at data from outside it, and after the fact it can't be told apart from a legitimate one.
12. **Hidden files** (dot-files) and git-ignored files are not indexed or searched (`search`, `grep`, `glob`), unless a call names them as its `path` or `glob` asks for `hidden` ones, but can still be listed and read if they aren't on the deny list.
13. **Offsets** count Unicode scalar values.
14. **Revocation handling.** `invalid_grant`, `unauthorized_client` and `access_denied` on refresh stop the daemon's reconnect loop until `cww login`. Everything else is retried.
15. **One server per daemon.** Open question 1 in the design is answered as one pairing at a time. `cww login` again replaces it.
16. **Chats for the terminal** use the device's own token with a second scope, not a separate grant, and the daemon makes every call, so the terminal UI never holds a token. Answers stream over the tunnel's socket (section 12) rather than Server-Sent Events.
17. **Changes are opt-in per folder, on the computer.** Approval of each change happens on the server, and the daemon can't tell a real approval from a forged one, so the daemon also makes every change recoverable (the system trash), bounded (budgets and caps), contained (handle-based resolution, the deny lists, the sandbox's write rights for exactly those folders) and visible (the audit log). See README.md, "Threat model".

## 12. Chats for the terminal UI

`cww tui` shows the owner's chats and asks questions. It never talks to the server: it asks the daemon over the control socket (CONTROL.md), and the daemon calls the server as the device.

**Permission.** Chats are a second permission, apart from answering tools. `cww login` asks for `desktop:chat` in its scope (`--no-chats` leaves it out), and the approval page lets the user decline it. A computer paired without it asks on first use with `POST /desktop/chat_access_request`, and the owner answers in the server's settings. The owner can take chats back at any time without unpairing; the server then refuses the chat API and closes the socket with 4001, so followed chats stop.

**Requests.** JSON over HTTPS, each with `Authorization: DPoP <access token>` and a `DPoP` proof for that method and URL with `ath`, exactly as the upgrade in 6.1. Errors are `{"error": <code>, "error_description": <sentence>, ...}`.

| Request | Answers |
|---|---|
| `GET /desktop/chats` | `chats` (newest first: `number`, `title`, `state`, `project`, `mine`, `updated_at`, `url`, `model`, `can`), `projects` (`id`, `name`, `icon`, `hq`, `all_access`, `url`), `account`, `user`, `credits` (`left`, `capacity`, `running_low`), `locked_reason`, `pins` (`projects`, `chats`), `links` (`new_chat`, `chats`, `projects`, `settings`: `tab`, `label`, `icon`, `url`) |
| `GET /desktop/chats/<number>` | `chat`, `locked_reason`, `entries` (`user` with `attachments`, `activity` with title, details, `pending`, steps with file names and `waiting`, `assistant` with Markdown and `sources`, `notice`), `approvals`, and `questions` |
| `GET /desktop/models` | `{"default_model_id", "models": [{"id","name","provider","description","rate","selectable","reason"}]}` |
| `POST /desktop/chats` `{"content":…}` | `201 {"chat":…}`: a new chat with its first question |
| `POST /desktop/chats/<number>/messages` `{"content":…, "steer":…}` | `202 {"chat":…}`; while an answer is written, `202 {"chat":…, "queued":…}` (older servers: `409 chat_busy`) |
| `DELETE /desktop/chats/<number>/queued_messages/<id>` | `204`: takes back a queued message; `403 forbidden` for someone else's, `404 not_found` once the answer took it |
| `POST /desktop/chats/<number>/cancellation` | `202`: stops the answer |
| `POST /desktop/chats/<number>/tool_calls/<id>/approval` `{"for_rest_of_chat":…}` | `202 {"chat":…}` |
| `POST /desktop/chats/<number>/tool_calls/<id>/denial` `{"reason":…}` | `202 {"chat":…}` |
| `POST /desktop/chats/<number>/tool_calls/<id>/input` `{"input":{…}}` | `202 {"chat":…}`: answers a question (no `input` for a page to visit) |
| `DELETE /desktop/chats/<number>/tool_calls/<id>/input` | `202 {"chat":…}`: declines it |
| `POST /desktop/uploads` (multipart, one `file`) | `{"signed_id","filename","byte_size","content_type"}`, or `422 attachment_refused` |
| `POST /desktop/chats/<number>/retry` `{"message_id":…}` | `202 {"chat":…}`, or `409 chat_busy` |
| `POST /desktop/chats/<number>/branches` `{"message_id":…}` | `201 {"chat":…}`: the new chat |
| `PATCH /desktop/chats/<number>` `{"title":…}` | `{"chat":…}` |
| `DELETE /desktop/chats/<number>` | `204` |
| `POST /desktop/chats/<number>/share` | `201 {"url","expires_at","chat"}` |
| `DELETE /desktop/chats/<number>/share` | `{"chat":…}`: stops sharing |
| `POST /desktop/chat_access_request` | `202 {"granted","requested","approve_url"}` |
| `GET /assets/<path>` (no token, no proof) | The image a logo or icon names, served to anyone: only images, at most 1 MiB, never through a redirect |

Both ways of asking take an optional `model_id` (from `/desktop/models`) and `attachments`, a list of `signed_id`s from uploads; a new chat also takes `project_id`. An unknown or unselectable model fails with `422 model_unavailable`. `message_id` is optional for retry and branch (the latest question or answer without one). Every chat's JSON carries `model` (`{"id","name"}` or null), `share` (`{"url","expires_at"}` or null) and `can` (`retry`, `branch`, `rename`, `delete`, `share`); a client treats a missing `can` as allowing none of them. `approvals` lists the changes the answer stopped at: `id`, `service`, `effect`, and either `decidable: true` with `summary`, `details` (`label`, `value`) and `allow_for_rest_of_chat`, or `decidable: false` with `waiting_for`. `questions` lists what a tool's server asked the person (MCP elicitation): `id`, `service`, and either `decidable: true` with `message`, `kind` (`form` with `fields`, or `url` with `url` and `host`) and a `note`, or `decidable: false` with `waiting_for`; a missing required field is `422 invalid`. Only the person who drove that turn decides, only while the call waits (`409 already_decided` otherwise). The upload is `multipart/form-data` and can take longer than other calls; files are at most 25 MB.

**Queued messages.** While an answer is written, a message to `/messages` waits in the chat's queue instead of being refused. `"steer": true` (the default) lets the running answer take it at its next step and answer it too; `false` waits for the answer to end and makes it the next question. The answer keeps its model and web search, so `model_id` and `web_search` are ignored then, and where the server scans files it refuses them with `422 attachment_refused` until the answer finishes. The transcript lists the queue as `queued`, oldest first: `id`, `content`, `steer`, `queued_at`, plus `author` when someone else in a project chat sent it, `attachments` (a count) when it has files, and `unqueue_path` (the `DELETE` above) only for its sender. A server from before the queue sends no `queued`, and the terminal treats that as empty. Every change to the queue reaches a follower as `{"type":"changed"}`; stopping the answer leaves the queue alone, and once nothing answers, the next question takes the queued messages in first.

`403 chat_access_required` (with `requested` and `approve_url`) means the owner hasn't allowed chats. `403 insufficient_scope` means the token predates the permission: the daemon refreshes the token and retries once. `401` is handled the same way. Other codes (`locked`, `rate_limited`, `invalid`, `not_found`, `forbidden`) are passed to the terminal as they are. An answer that isn't this JSON means the server has no chat API, or not that call yet (an older server without models, uploads or the chat actions), and the terminal says which.

**Images.** Wherever the web shows a logo or a file-type icon, the JSON gives `{"path": "/assets/…", "monochrome": …}`: `logo` on a chat's `model`, on each model, on approvals, questions and activity steps, `logos` beside an activity's `services`, and `icon` on attachments, uploads and sources. When a client asks the daemon for one, the daemon fetches that path from the paired server's origin, without the token or a proof (the server serves `/assets/` to anyone), and through the proxy when there is one. It only asks for paths under `/assets/` made of letters, digits, `-`, `_`, `.` and `/`, never follows a redirect, and only takes an image of at most 1 MiB.

**Streaming.** With Action Cable framing, the daemon follows a chat by subscribing to one more channel on the same socket:

```
{"command":"subscribe","identifier":"{\"channel\":\"Desktop::ChatChannel\",\"chat\":\"42\"}"}
```

The server confirms it only for a device allowed to chat and a chat its owner may watch, and then sends updates as the channel's `message`: `{"type":"chunk","message_id":5,"text":"…"}` (answer text as it's written), `{"type":"progress","text":"…"}` (what the running step does), and `{"type":"changed"}` (anything else: read the chat again). The daemon subscribes once per chat however many terminals follow it, unsubscribes when the last one stops, and subscribes again after a reconnect. A refused chat channel never ends the session. With `mcp` framing there are no chat channels.

**The chat list.** While a terminal shows the list, the daemon also subscribes, once, to the list's channel, which takes no parameters:

```
{"command":"subscribe","identifier":"{\"channel\":\"Desktop::ChatsChannel\"}"}
```

The server confirms it for a device allowed to chat whose owner is an active member of its organization (never a guest), and rejects it otherwise. After the confirmation the terminal lists the chats once, then applies each `message` on that identifier: `{"event":"chat","chat":{…}}` (a chat started, renamed, starting or stopping being answered, switching model, getting or losing its public link, or moving in or out of a project: the same JSON as the list's, inserted by `number` or replaced, sorted by `updated_at`), `{"event":"removed","number":42}` (deleted, or out of the owner's sight), `{"event":"projects","projects":[…]}` (the whole list again), and `{"event":"account","account":{…},"user":{…},"credits":{…},"locked_reason":…}` (the list's header again). When the owner leaves a project, the server closes the socket instead; the daemon reconnects, subscribes again, and the terminal lists again on the confirmation. A rejected list channel never ends the session, and the next terminal to watch the list asks again. The daemon unsubscribes when the last terminal stops watching. A server without the channel never confirms it, and the terminal reads the list after each change it makes, as before.

## 13. Scans

The desktop app's scan page (docs/scanning.md) makes a searchable PDF on the computer and hands it to the daemon over the control socket (CONTROL.md, `scan_destinations` and `scan_send`); the daemon sends it as the device's owner, with the same DPoP-bound token and the same chat permission as section 12. A scan goes to one of two places:

- **A new chat**: `POST /desktop/uploads` with the PDF, then `POST /desktop/chats` with its `signed_id` in `attachments` and the person's message, if any, as `content` (which may be empty with a file), exactly as section 12.
- **A bot**, through a trigger of its that takes scans ("A scan from Chat with Work Desktop", `Trigger::Scan` on the server):

| Request | Answers |
|---|---|
| `GET /desktop/scans/new` | `{"destinations": [{"id", "name", "bot", "paused", "project"}]}`: the scan triggers the owner may send to (their own, and those of bots they handle); `id` is the trigger's |
| `POST /desktop/scans` (multipart: `file`, `trigger_id`, `pages`, `scan_id`, optional `scanner`) | `202 {"event": {"id", "state"}, "destination": {...}}` |

`file` is the PDF (its first bytes `%PDF-`, at most 25 MB). `pages` is 1 to 500. `scan_id` is the app's id for the scan (8 to 64 letters, digits and hyphens): the server records a scan once per bot, computer and id, so a retry after a lost answer is the same event while the same pages sent to another bot are that bot's own, and the app makes a new id whenever the pages change. Refusals: `404 not_found` (no such trigger, or not one the owner may send to), `409 paused`, `413 too_large`, `415 unsupported` (not a PDF), `422 invalid`, `429 rate_limited`, and section 12's `403`s. The PDF becomes the bot's run's file, malware-scanned like any upload; the server records `desktop.scan_sent` with the trigger, the page count and the size, never the scan. Nothing on the server can ask the computer to scan.
