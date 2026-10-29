# ZIP dependency review, 16 October 2026 (updated 29 October 2026)

The direct ZIP reader uses registry `zip` 9.1.3. `calamine` 0.37 and
`rust_xlsxwriter` 0.101 use zip 9 too, but the locked `docx-rs` 0.4.22
still requires zip 8, so the lockfile also retains 8.6.0. RustSec now
lists the three advisories below for zip 8 (RUSTSEC-2026-0367, -0368 and
-0369), and `deny.toml` ignores exactly those IDs. Remove them when docx-rs
publishes a zip 9 release. Any other advisory remains an error.

The published upstream advisories concern extracting archives to disk:

- [GHSA-4684-h2q9-2vcp](https://github.com/zip-rs/zip2/security/advisories/GHSA-4684-h2q9-2vcp): applying permissions through symlinks.
- [GHSA-4v9h-p7m9-rh26](https://github.com/zip-rs/zip2/security/advisories/GHSA-4v9h-p7m9-rh26): creating symlinks outside the extraction directory.
- [GHSA-vv5m-jxxh-4627](https://github.com/zip-rs/zip2/security/advisories/GHSA-vv5m-jxxh-4627): preserving SUID/SGID permission bits.

CWW never extracts an archive to disk. Its document reader selects XML entries
from a capped in-memory buffer. Calamine likewise reads spreadsheet entries
as XML/binary data; its source has no archive `extract` call. CWW uses
docx-rs and rust_xlsxwriter only to construct documents in memory, then writes
the finished bytes through its own approved file-change path. Neither
library's source calls the affected archive extraction APIs.

This is a limited assessment of these published issues, not a claim that
zip 8 has no other defects. Keep the transitive dependencies under review;
do not add disk extraction or enable new dependency features under this
exception. Document read/write round trips and malformed-input tests cover
the direct ZIP upgrade.

The 29 October dependency review also checked
[GHSA-m665-v7pm-7grj](https://github.com/zip-rs/zip2/security/advisories/GHSA-m665-v7pm-7grj)
(dual-header inconsistencies, fixed in zip 9.0.5). The direct reader is on
9.1.3. The remaining zip 8 copy is used only by docx-rs's document writer:
`src/writer/office.rs` builds a new `Docx`, then packs it into a memory
cursor. It does not call docx-rs's reader. Its packer creates fresh entries
with `ZipWriter::start_file` and never calls `shallow_copy_file`,
`ZipArchive`, or `ZipStreamReader`. That path cannot read a supplied archive
whose two headers disagree. This is an assessment of the current use, not
an upstream patch; the lockfile still contains the affected version. Keep
[the dependency alert](https://github.com/crmne/chatwithwork-desktop/security/dependabot/2)
open until docx-rs adopts a patched release. No new audit exception was added.
