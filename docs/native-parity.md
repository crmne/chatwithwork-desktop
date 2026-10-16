# Native desktop parity audit

Scope confirmed by Carmine on 16 October 2026: reproduce every signed-in
website feature, its appearance and interaction states with native controls.
The existing chat API is not the scope limit. Missing API support must be
implemented with the same account, role, ownership and approval rules.
The iOS and Android Hotwire apps remain outside this desktop work.

This is a working inventory, not a claim that parity is complete. A row is
complete only after behavior tests and light/dark visual comparison against
the website at narrow and wide widths. OS dialogs and vendor OAuth use their
native/system flows. Website marketing and staff administration are separate
from the workplace application's user features.

| Website surface | Native status at audit | Work required |
| --- | --- | --- |
| Window decorations | Implemented in 279949b | Saved Custom title bar preference, Linux off, Windows/macOS on; reopening applies it. Desktop tests and screenshots pass; platform CI pending. |
| Organizations | Missing | Account picker, switcher, marks/roles, invitation state, new organization, remember each organization's page. Extend pairing securely, never reuse an organization-bound grant across accounts. |
| New chat | Initial native slice implemented | Fan-out and refreshed service availability, pause/resume/remove confirmation, searchable connect dialog and directory, suggested questions that start chats, instruction links and editing. Focused interaction tests, API relay and wide/narrow light/dark snapshots pass. Still compare dense fan-outs, empty/error layouts and dialog row layout against the website; finish per-connection management and custom setup forms. |
| Chat navigation | Partial | Native all-chats page, search parity, pin/unpin, moving to projects, guest visibility, remembered page. |
| Composer | Partial | Exact attachment/upload states, model selection and costs, web search policy/disclosure, locked and offline states, keyboard and caret behavior. |
| Dictation | Partial | Actual audio waveform, timer, listening/transcribing/cancel/error states, microphone style, permission and credit availability, insertion at caret and undo, five-minute limit, silence rejection. Current UI appends words and shows only a text status. |
| Local dictation | Research underway | Selectable downloadable Whisper models, download progress/cancel/removal and integrity verification; no model in release artifacts; on-demand inference; local audio never falls back to upload without choosing cloud. |
| Answers | Partial | All Markdown and code/table selection, rich/plain copying preference, attachment previews, images, accessible links, retries and branching, share dialog. |
| Tool activity and results | Partial | Compare every server activity/result partial, complete result details, previews, code files and images, source lists, progress, failures and other people's private-result restrictions. |
| Approval and questions | Partial | Every preview type, denial reason, rest-of-chat choices, destructive warnings, schema forms, validation, resolved state and concurrent decisions. Native MCP app content needs explicit design for its interactive surface. |
| Projects | Partial | Native list/detail/create/edit, membership/access, HQ and All-Access rules, people counts, archive/unarchive/delete, instructions/templates, pins, guest restrictions. |
| Account settings | Browser links only | Name/profile, password/security/account deletion, copy format, timezone and default settings, correct permission checks. |
| Instructions | Initial native slice implemented | Native personal/organization editor, templates, token count, revisions/restore, unsaved-change guard, learned memories and memory preference. Server enforces roles and project scope. Finish native project context and detailed visual/state comparison with the website. |
| Templates | Browser links only | List, upload, delete, personal/project scope and attachment validation. |
| People and organization | Browser links only | Invitations/join links, role changes/removal, organization name, change policy, monthly caps, restrictions and confirmations. |
| Models | Partial | Native Settings page, persistent personal default, current catalog and rates, free tier/credit state. |
| Connectors and computers | Partial | Native connector settings, multiple connections/labels, access levels, reconnect, pause, organization apps, directory, custom MCP setup/check/OAuth, device permissions/revocation/activity. Keep local folder controls native. |
| Schedules | Context only | List/create/edit, recurrence/timezone, run now, pause/resume/delete, live next/last run state. |
| Notifications | Implemented; staging and final platform CI pending | Daemon delivery with GUI/TUI closed, shared server preferences, focused-chat presence, explicit permission, linked-browser deduplication and fallback, withdrawal, reconnect and account-scoped click navigation. Control requests have a whole-request deadline. Native-free root daemon; separate helper supplies native APIs. Only correlated native Shown acknowledges delivery. Full local root, broker, desktop and Clippy checks pass, with wide/narrow light/dark UI evidence and a real Linux helper smoke. |
| Billing and usage | Browser links only | Native plan/credit/usage/cap views and role rules; payment provider checkout remains its system-browser flow. |
| Sharing | Partial | Settings controls and exact share-link lifecycle, expiration and authorization. |
| Cross-cutting | Partial | Same typography/icons/colors, focus/keyboard navigation and screen-reader labels, reduced motion, responsive drawers/dialogs, loading/error/empty/reconnect states, zero idle redraw work. |

References: Rails `config/routes.rb`, `SettingsController::TABS`, website view
partials and Stimulus controllers, `AGENTS.md` design conventions, and the
current limitations recorded in `docs/chat-window.md`. Recheck the implementation
while closing each row; older documentation already overstates some parity.
