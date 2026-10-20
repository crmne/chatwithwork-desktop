# ZIP dependency review, 16 October 2026 (updated 20 October 2026)

The direct ZIP reader uses registry `zip` 9. `calamine` 0.37 and
`rust_xlsxwriter` 0.100 use zip 9 too, but `docx-rs` 0.4.22, its latest
release, still requires zip 8, so the lockfile also retains 8.6.0. RustSec now
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
