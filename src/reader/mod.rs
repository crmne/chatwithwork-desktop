//! The reader: everything that touches shared files.
//!
//! This module has a deliberately narrow interface, four operations with
//! plain serializable requests and responses ([`Reader::roots`],
//! [`Reader::search`], [`Reader::list`], [`Reader::read`]). In phase 3 it
//! moves into its own sandboxed process (Landlock/seccomp, Seatbelt) with no
//! network access, and the network side talks to it over a socketpair using
//! these same types. Nothing else in the daemon opens user files, except
//! the writer (`crate::writer`), which makes changes through the same safe
//! resolution (`safe_fs::ChangeDir`) in folders that allow them.

pub mod code;
pub mod extract;
pub mod grep;
pub mod index;
pub mod indexer;
pub mod safe_fs;
pub mod walk;
pub mod watch;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Limits};
use crate::error::{ErrorCode, ToolError};
use crate::paths::Paths;
use crate::policy::DenyList;
use crate::status::Changes;
use indexer::{IndexState, Indexer, IndexerSettings};
use safe_fs::{EntryKind, OpenPolicy, RootHandle, ToolPath};

/// Characters of extracted text kept per file for paged reads.
const MAX_EXTRACTED_CHARS: usize = 20_000_000;
const CACHE_ENTRIES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RootInfo {
    pub id: String,
    pub label: String,
    /// False if the folder is missing or unreadable right now.
    pub available: bool,
    /// The user allowed changes in this folder.
    #[serde(default)]
    pub writable: bool,
    /// The user allowed commands here (`run`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allows_commands: bool,
    /// Those commands may reach the internet (and the local network, local
    /// services with them).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// How far those commands reach: `none`, `local` (this computer's own
    /// services) or `internet`. Only where commands are allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_access: Option<crate::reach::Level>,
    /// `pending`, `indexing`, `ready`, `error` or `disabled`.
    pub index: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_files: Option<u64>,
    /// For a bot's own list of the folders lent to it: whether its commands
    /// there start without anyone approving each one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattended: Option<bool>,
    /// For a bot's own list: whether Claude Code is ready for it there,
    /// and the repositories' names (docs/coding-computers.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding: Option<serde_json::Value>,
    /// Characters a chat may read from this folder per rolling hour, when
    /// the person gave it a budget of its own on this computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_budget: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub path_glob: Option<String>,
    #[serde(default)]
    pub modified_after: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// The folders a bot's search may look in: only those lent to it.
    #[serde(skip)]
    pub only: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub modified: String,
    pub size: u64,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    /// `index`, `grep`, or `index+grep`.
    pub engine: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    pub path: String,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListResponse {
    pub path: String,
    pub entries: Vec<ListEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadBytesRequest {
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadBytesResponse {
    pub path: String,
    pub offset: u64,
    /// Offset of the next chunk, or null at the end of the file.
    pub next_offset: Option<u64>,
    pub size: u64,
    pub modified: String,
    /// The bytes, in standard base64.
    pub data: String,
}

/// The largest file `read_bytes` copies, and the most one chunk holds.
pub const MAX_COPY_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_COPY_CHUNK_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadResponse {
    pub path: String,
    pub text: String,
    pub offset: usize,
    /// Offset of the next chunk, or null at the end of the file.
    pub next_offset: Option<usize>,
    pub total_chars: usize,
    pub modified: String,
    pub size: u64,
}

struct State {
    roots: Vec<Arc<RootHandle>>,
    configured: Vec<crate::config::Root>,
    /// Coding bots' setup, for what a bot's `roots` says is ready.
    coding: crate::config::CodingConfig,
    deny: Arc<DenyList>,
    write_deny: Arc<DenyList>,
    allow_hardlinks: bool,
    limits: Limits,
    index_enabled: bool,
}

type CacheKey = (String, (u64, u64), i64, u64);

/// A file checked for an upload (`Reader::upload_file`).
#[derive(Debug, Clone, Serialize)]
pub struct UploadFile {
    /// `root_id:relative/path`, as the call named it.
    pub path: String,
    /// The shared folder's label.
    pub folder: String,
    pub name: String,
    pub size: u64,
    pub modified: String,
    pub mime_type: String,
    pub sha256: String,
    /// Where it really is, for the browser only: never sent to the server,
    /// logged or audited.
    #[serde(skip)]
    pub real: PathBuf,
}

impl UploadFile {
    /// What binds an approval to these files, in this order: each one's
    /// shared path, size, modification time and SHA-256.
    pub fn fingerprint(files: &[UploadFile]) -> String {
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        for file in files {
            digest.update(file.path.as_bytes());
            digest.update(&[0]);
            digest.update(file.size.to_string().as_bytes());
            digest.update(&[0]);
            digest.update(file.modified.as_bytes());
            digest.update(&[0]);
            digest.update(file.sha256.as_bytes());
            digest.update(b"\n");
        }
        digest
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// How long the copies handed to the browser are kept: a page reads a
/// chosen file only when it sends it, which may be long after it was
/// chosen (a form filled in over an afternoon), so a day, and only a newer
/// upload clears older ones.
pub const SNAPSHOT_LIFE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// A new private file for a copy of `name`, in a folder of its own under
/// `dir`, after clearing copies older than [`SNAPSHOT_LIFE`].
fn upload_snapshot_file(
    dir: &std::path::Path,
    name: &str,
) -> Result<(PathBuf, std::fs::File), ToolError> {
    let unavailable =
        |e: std::io::Error| ToolError::internal(format!("keeping a copy to upload: {}", e.kind()));
    crate::paths::ensure_private_dir(dir)
        .map_err(|_| ToolError::internal("keeping a copy to upload"))?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok());
            if old.is_some_and(|age| age > SNAPSHOT_LIFE) {
                let path = entry.path();
                if let Ok(files) = std::fs::read_dir(&path) {
                    for file in files.flatten() {
                        if let Ok(meta) = file.metadata() {
                            let mut permissions = meta.permissions();
                            #[allow(clippy::permissions_set_readonly_false)]
                            permissions.set_readonly(false);
                            let _ = std::fs::set_permissions(file.path(), permissions);
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    let folder = dir.join(
        safe_fs::temp_name()
            .trim_start_matches('.')
            .trim_end_matches(".tmp"),
    );
    crate::paths::ensure_private_dir(&folder)
        .map_err(|_| ToolError::internal("keeping a copy to upload"))?;
    let path = folder.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(unavailable)?;
    Ok((safe_fs::without_verbatim_prefix(&path), file))
}

/// The MIME type a page's `accept` is checked against, by the name's
/// extension, as browsers guess it for a chosen file.
pub fn upload_mime_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "jxl" => "image/jxl",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "ogv" => "video/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/x-m4a",
        "aac" => "audio/aac",
        "weba" => "audio/webm",
        "mpeg" | "mpg" => "video/mpeg",
        "m4v" => "video/x-m4v",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "3gp" => "video/3gpp",
        "rtf" => "application/rtf",
        "epub" => "application/epub+zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "yaml" | "yml" => "application/yaml",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "crx" => "application/x-chrome-extension",
        "apk" => "application/vnd.android.package-archive",
        "json" => "application/json",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "text/xml",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

pub struct Reader {
    state: RwLock<State>,
    indexer: Option<Indexer>,
    cache: Mutex<VecDeque<(CacheKey, Arc<String>)>>,
}

impl Reader {
    pub fn new(config: &Config, paths: &Paths, changes: Changes) -> Result<Self> {
        let state = build_state(config, paths)?;
        let indexer = config
            .index
            .enabled
            .then(|| Indexer::start(paths.index_dir(), indexer_settings(&state, config), changes));
        Ok(Self {
            state: RwLock::new(state),
            indexer,
            cache: Mutex::new(VecDeque::new()),
        })
    }

    pub fn stop_indexing(&self) {
        if let Some(indexer) = &self.indexer {
            indexer.stop();
        }
    }

    /// Apply a changed config (roots, deny list, limits).
    pub fn reload(&self, config: &Config, paths: &Paths) -> Result<()> {
        let mut state = build_state(config, paths)?;
        state.index_enabled = self.indexer.is_some();
        if let Some(indexer) = &self.indexer {
            indexer.configure(indexer_settings(&state, config));
        }
        *self.state.write().expect("reader state") = state;
        self.cache.lock().expect("cache").clear();
        Ok(())
    }

    /// The loan of folder `root` to the bot with ID `bot`, if it's lent to it.
    pub fn loan(&self, root: &str, bot: &str) -> Option<crate::config::BotLoan> {
        let state = self.state.read().expect("reader state");
        state
            .configured
            .iter()
            .find(|r| r.id == root)?
            .loan(bot)
            .cloned()
    }

    /// Whether Claude Code is ready for bot `bot` in folder `root`, and the
    /// repositories checked out there (docs/coding-computers.md).
    pub fn coding_for_bot(
        &self,
        root: &str,
        bot: &str,
        signed_out: &[String],
    ) -> Option<serde_json::Value> {
        let state = self.state.read().expect("reader state");
        let configured = state.configured.iter().find(|r| r.id == root)?;
        Some(crate::coding::coding_for_bot(
            &state.coding,
            configured,
            bot,
            signed_out,
        ))
    }

    /// A repository a run just prepared: what `roots` says of its folder, until
    /// the next reload reads the same from the config.
    pub fn note_repository(&self, repository: crate::config::Repository) {
        let mut state = self.state.write().expect("reader state");
        let same =
            |r: &crate::config::Repository| r.root == repository.root && r.name == repository.name;
        if !state.coding.repositories.iter().any(same) {
            state.coding.repositories.push(repository);
        }
    }

    /// The IDs of the folders lent to the bot with ID `bot`.
    pub fn lent_to(&self, bot: &str) -> Vec<String> {
        let state = self.state.read().expect("reader state");
        state
            .configured
            .iter()
            .filter(|r| r.loan(bot).is_some())
            .map(|r| r.id.clone())
            .collect()
    }

    pub fn roots(&self) -> Vec<RootInfo> {
        let state = self.state.read().expect("reader state");
        let status = self
            .indexer
            .as_ref()
            .map(Indexer::status)
            .unwrap_or_default();
        state
            .configured
            .iter()
            .map(|root| {
                let available = state.roots.iter().any(|r| r.id == root.id);
                let s = status.get(&root.id);
                RootInfo {
                    id: root.id.clone(),
                    label: root.label.clone(),
                    available,
                    writable: root.writable,
                    allows_commands: root.runs_commands(),
                    network: root.runs_commands() && root.network,
                    network_access: root
                        .runs_commands()
                        .then(|| crate::commands::network_access(root)),
                    index: match (state.index_enabled, s.map(|s| s.state)) {
                        (false, _) => "disabled".into(),
                        (true, Some(IndexState::Ready)) => "ready".into(),
                        (true, Some(IndexState::Indexing)) => "indexing".into(),
                        (true, Some(IndexState::Error)) => "error".into(),
                        (true, _) => "pending".into(),
                    },
                    indexed_files: s.map(|s| s.files),
                    unattended: None,
                    coding: None,
                    read_budget: root.read_budget(&state.limits),
                }
            })
            .collect()
    }

    pub fn search(&self, req: &SearchRequest) -> Result<SearchResponse, ToolError> {
        let state = self.state.read().expect("reader state");
        let query = req.query.trim();
        if query.is_empty() {
            return Err(ToolError::invalid_argument("query must not be empty"));
        }
        if query.chars().count() > 1000 {
            return Err(ToolError::invalid_argument("query is too long"));
        }
        let limit = req
            .limit
            .unwrap_or(state.limits.search_hits)
            .clamp(1, state.limits.search_hits);
        let roots: Vec<Arc<RootHandle>> = match &req.root {
            Some(id) => vec![root_by_id(&state, id)?],
            None => state
                .roots
                .iter()
                .filter(|r| req.only.as_ref().is_none_or(|only| only.contains(&r.id)))
                .cloned()
                .collect(),
        };
        let modified_after = req.modified_after.as_deref().map(parse_time).transpose()?;
        let glob = req.path_glob.as_deref().map(compile_glob).transpose()?;
        let glob_matches = |rel: &str| match &glob {
            None => true,
            Some((matcher, whole_path)) => {
                if *whole_path {
                    matcher.is_match(rel)
                } else {
                    matcher.is_match(rel.rsplit('/').next().unwrap_or(rel))
                }
            }
        };

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut engines = Vec::new();
        if let Some(indexer) = &self.indexer {
            // Folders past the watch limit catch up now, for the next search.
            indexer.refresh();
        }
        if let Some(index) = self.indexer.as_ref().and_then(Indexer::index) {
            let ids: Vec<String> = roots.iter().map(|r| r.id.clone()).collect();
            let fetch = if glob.is_some() {
                (limit * 10).min(500)
            } else {
                limit * 2
            };
            let found = index
                .search(&index::SearchQuery {
                    text: query,
                    roots: &ids,
                    modified_after,
                    limit: fetch,
                    snippet_chars: state.limits.snippet_chars,
                })
                .map_err(|e| ToolError::internal(format!("search failed: {e}")))?;
            engines.push("index");
            for hit in found {
                if hits.len() >= limit {
                    break;
                }
                let Ok(path) = ToolPath::parse(&hit.path) else {
                    continue;
                };
                let Some(root) = roots.iter().find(|r| r.id == path.root_id) else {
                    continue;
                };
                // The deny list may have grown since the file was indexed.
                if state.deny.is_denied(&root.abs_path(&path.rel))
                    || !glob_matches(&path.rel.as_string())
                {
                    continue;
                }
                hits.push(SearchHit {
                    path: hit.path,
                    title: hit.title,
                    modified: format_time(hit.modified),
                    size: hit.size,
                    snippet: hit.snippet,
                });
            }
        }

        let not_ready = self
            .indexer
            .as_ref()
            .is_none_or(|ix| roots.iter().any(|r| !ix.is_ready(&r.id)));
        if hits.len() < limit && (hits.is_empty() || not_ready) {
            let file_glob = glob.as_ref().filter(|(_, whole)| *whole).map(|(m, _)| m);
            let name_glob = glob.as_ref().filter(|(_, whole)| !*whole).map(|(m, _)| m);
            let found = grep::grep(
                &roots,
                &state.deny,
                state.allow_hardlinks,
                &grep::GrepQuery {
                    text: query,
                    glob: file_glob,
                    modified_after,
                    limit: limit * 2,
                    snippet_chars: state.limits.snippet_chars,
                    max_file_bytes: state.limits.max_index_file_bytes,
                    deadline: Instant::now() + Duration::from_millis(state.limits.grep_timeout_ms),
                },
            );
            engines.push("grep");
            for hit in found {
                if hits.len() >= limit {
                    break;
                }
                if name_glob.is_some_and(|g| !g.is_match(&hit.title))
                    || hits.iter().any(|h| h.path == hit.path)
                {
                    continue;
                }
                hits.push(SearchHit {
                    path: hit.path,
                    title: hit.title,
                    modified: format_time(hit.modified),
                    size: hit.size,
                    snippet: hit.snippet,
                });
            }
        }
        Ok(SearchResponse {
            hits,
            engine: engines.join("+"),
        })
    }

    pub fn list(&self, req: &ListRequest) -> Result<ListResponse, ToolError> {
        let state = self.state.read().expect("reader state");
        let path = ToolPath::parse(&req.path)?;
        let root = root_by_id(&state, &path.root_id)?;
        let entries = root.list_dir(&path.rel, policy(&state))?;
        let start = match &req.cursor {
            None => 0,
            Some(c) => c
                .parse::<usize>()
                .map_err(|_| ToolError::invalid_argument("invalid cursor"))?,
        };
        let page = state.limits.list_page;
        let end = (start + page).min(entries.len());
        let next_cursor = (end < entries.len()).then(|| end.to_string());
        let entries = entries
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|e| ListEntry {
                path: ToolPath::display(&root.id, &path.rel.join(&e.name)),
                name: e.name.clone(),
                kind: e.kind,
                size: e.size,
                modified: e.modified.map(format_time),
            })
            .collect();
        Ok(ListResponse {
            path: ToolPath::display(&root.id, &path.rel),
            entries,
            next_cursor,
        })
    }

    pub fn read(&self, req: &ReadRequest) -> Result<ReadResponse, ToolError> {
        let state = self.state.read().expect("reader state");
        let path = ToolPath::parse(&req.path)?;
        if path.rel.is_root() {
            return Err(ToolError::new(
                ErrorCode::NotAFile,
                "this path is a directory",
            ));
        }
        let root = root_by_id(&state, &path.root_id)?;
        let max_chars = req
            .max_chars
            .unwrap_or(state.limits.read_chars_per_call)
            .clamp(1, state.limits.read_chars_per_call);
        let offset = req.offset.unwrap_or(0);

        let mut opened = root.open_file(&path.rel, policy(&state))?;
        if opened.size > state.limits.max_file_bytes {
            return Err(ToolError::new(
                ErrorCode::TooLarge,
                format!(
                    "the file is {} bytes; the limit is {}",
                    opened.size, state.limits.max_file_bytes
                ),
            ));
        }
        let text = self.extracted(
            &path.to_string(),
            path.rel.file_name().unwrap_or_default(),
            &mut opened,
            state.limits.max_file_bytes,
            None,
        )?;

        let total_chars = text.chars().count();
        if offset > total_chars {
            return Err(ToolError::invalid_argument(format!(
                "offset {offset} is past the end ({total_chars} characters)"
            )));
        }
        let chunk: String = text.chars().skip(offset).take(max_chars).collect();
        let end = offset + chunk.chars().count();
        Ok(ReadResponse {
            path: path.to_string(),
            text: chunk,
            offset,
            next_offset: (end < total_chars).then_some(end),
            total_chars,
            modified: format_time(opened.modified),
            size: opened.size,
        })
    }

    /// One chunk of a file's bytes, whatever the file holds, for copying
    /// it into a chat's files: at most `allowance` bytes (what the copy
    /// budget and the message size leave). Resolved, and refused, exactly
    /// as `read` is.
    pub fn read_bytes(
        &self,
        req: &ReadBytesRequest,
        allowance: u64,
    ) -> Result<ReadBytesResponse, ToolError> {
        use base64::Engine;
        use std::io::{Read, Seek, SeekFrom};
        let state = self.state.read().expect("reader state");
        let path = ToolPath::parse(&req.path)?;
        if path.rel.is_root() {
            return Err(ToolError::new(
                ErrorCode::NotAFile,
                "this path is a directory",
            ));
        }
        let max = req.max_bytes.unwrap_or(MAX_COPY_CHUNK_BYTES);
        if !(1..=MAX_COPY_CHUNK_BYTES).contains(&max) {
            return Err(ToolError::invalid_argument(format!(
                "max_bytes must be from 1 to {MAX_COPY_CHUNK_BYTES}"
            )));
        }
        let root = root_by_id(&state, &path.root_id)?;
        let mut opened = root.open_file(&path.rel, policy(&state))?;
        if opened.size > MAX_COPY_FILE_BYTES {
            return Err(ToolError::new(
                ErrorCode::TooLarge,
                format!(
                    "the file is {} bytes; files up to {MAX_COPY_FILE_BYTES} bytes can be copied",
                    opened.size
                ),
            ));
        }
        let offset = req.offset.unwrap_or(0);
        if offset > opened.size {
            return Err(ToolError::invalid_argument(format!(
                "offset {offset} is past the end ({} bytes)",
                opened.size
            )));
        }
        let want = max.min(allowance).min(opened.size - offset);
        opened
            .file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ToolError::internal(format!("reading file: {e}")))?;
        let mut bytes = Vec::with_capacity(want as usize);
        (&mut opened.file)
            .take(want)
            .read_to_end(&mut bytes)
            .map_err(|e| ToolError::internal(format!("reading file: {e}")))?;
        let end = offset + bytes.len() as u64;
        Ok(ReadBytesResponse {
            path: path.to_string(),
            offset,
            next_offset: (end < opened.size).then_some(end),
            size: opened.size,
            modified: format_time(opened.modified),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
    }

    /// A file in a shared folder to hand to a page's upload field: resolved
    /// and refused exactly as `read` is (the folder's symlink rule, the
    /// deny list, hard links, only a regular file), then vouched for by
    /// where the open file really is, inside the folder, every symlink
    /// resolved, so the browser gets that path and nothing the page or the
    /// model named. Its size, modification time and SHA-256 bind an
    /// approval to this very file: a file that changes after it was checked
    /// is refused at upload (`Conflict`). Never reads more than `max_bytes`.
    pub fn upload_file(&self, path: &str, max_bytes: u64) -> Result<UploadFile, ToolError> {
        self.checked_upload(path, max_bytes, None)
    }

    /// [`Reader::upload_file`], and a private copy of exactly the bytes it
    /// hashed, under the name the call gave, in a new folder of its own in
    /// `snapshots` (cww's own, never shared): what the browser gets to
    /// upload, so a file changed, replaced or relinked after it was checked
    /// can't reach the page in its place. `real` is the copy.
    pub fn upload_snapshot(
        &self,
        path: &str,
        max_bytes: u64,
        snapshots: &std::path::Path,
    ) -> Result<UploadFile, ToolError> {
        self.checked_upload(path, max_bytes, Some(snapshots))
    }

    fn checked_upload(
        &self,
        path: &str,
        max_bytes: u64,
        snapshots: Option<&std::path::Path>,
    ) -> Result<UploadFile, ToolError> {
        use std::io::{Read, Write};
        let state = self.state.read().expect("reader state");
        let tool_path = ToolPath::parse(path)?;
        let Some(name) = tool_path.rel.file_name().map(str::to_string) else {
            return Err(ToolError::new(
                ErrorCode::NotAFile,
                "this path is a folder; name a file in it",
            ));
        };
        let root = root_by_id(&state, &tool_path.root_id)?;
        let mut opened = root.open_file(&tool_path.rel, policy(&state))?;
        if opened.size > max_bytes {
            return Err(ToolError::new(
                ErrorCode::TooLarge,
                format!(
                    "the file is {} bytes; files up to {max_bytes} bytes can be uploaded",
                    opened.size
                ),
            ));
        }
        let real = safe_fs::opened_file_path(&opened.file).ok_or_else(|| {
            ToolError::new(
                ErrorCode::Unsupported,
                "this computer can't say where the file is, so it can't be uploaded",
            )
        })?;
        let folder = safe_fs::without_verbatim_prefix(&root.path);
        if !safe_fs::is_within(&folder, &real) || real == folder {
            return Err(ToolError::new(
                ErrorCode::OutsideRoot,
                "the file is outside the shared folder",
            ));
        }
        // The browser opens the file by its path: that path must name this
        // very file, not one put there since.
        if safe_fs::path_identity(&real) != Some(opened.identity) {
            return Err(ToolError::new(
                ErrorCode::Conflict,
                "the file changed while it was checked; try again",
            ));
        }
        let mut copy = match snapshots {
            Some(dir) => Some(upload_snapshot_file(dir, &name)?),
            None => None,
        };
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0u8; 256 * 1024];
        let mut read = 0u64;
        loop {
            let n = opened
                .file
                .read(&mut buffer)
                .map_err(|e| ToolError::internal(format!("reading file: {}", e.kind())))?;
            if n == 0 {
                break;
            }
            read += n as u64;
            if read > max_bytes {
                return Err(ToolError::new(
                    ErrorCode::Conflict,
                    "the file grew while it was checked; try again",
                ));
            }
            digest.update(&buffer[..n]);
            if let Some((_, file)) = copy.as_mut() {
                file.write_all(&buffer[..n])
                    .map_err(|e| ToolError::internal(format!("copying the file: {}", e.kind())))?;
            }
        }
        if read != opened.size {
            return Err(ToolError::new(
                ErrorCode::Conflict,
                "the file changed while it was checked; try again",
            ));
        }
        let sha256 = digest
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let real = match copy {
            Some((copied, file)) => {
                file.sync_all()
                    .map_err(|e| ToolError::internal(format!("copying the file: {}", e.kind())))?;
                let mut permissions = file
                    .metadata()
                    .map_err(|e| ToolError::internal(format!("copying the file: {}", e.kind())))?
                    .permissions();
                permissions.set_readonly(true);
                let _ = std::fs::set_permissions(&copied, permissions);
                copied
            }
            None => real,
        };
        Ok(UploadFile {
            path: tool_path.to_string(),
            folder: root.label.clone(),
            mime_type: upload_mime_type(&name).to_string(),
            name,
            size: opened.size,
            modified: format_time(opened.modified),
            sha256,
            real,
        })
    }

    /// Whose read budget reads from the shared folder `id` count against:
    /// the folder's own, when the person gave it one, or the shared one.
    pub fn read_scope(&self, id: &str) -> crate::limits::ReadScope {
        let state = self.state.read().expect("reader state");
        match state
            .configured
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.read_budget(&state.limits))
        {
            Some(per_chat) => crate::limits::ReadScope::Folder {
                id: id.to_string(),
                per_chat,
            },
            None => crate::limits::ReadScope::Global,
        }
    }

    /// What the writer needs to make a change: the open roots, the deny
    /// lists and the limits, as they are right now.
    pub fn change_view(&self) -> ChangeView {
        let state = self.state.read().expect("reader state");
        ChangeView {
            roots: state.roots.clone(),
            configured: state.configured.clone(),
            deny: Arc::clone(&state.deny),
            write_deny: Arc::clone(&state.write_deny),
            limits: state.limits.clone(),
        }
    }

    /// Make every folder but `still_writable` (IDs) read-only right away,
    /// whatever the config said when it was loaded.
    pub fn stop_changes(&self, still_writable: &[String]) {
        let mut state = self.state.write().expect("reader state");
        for root in &mut state.configured {
            if !still_writable.contains(&root.id) {
                root.writable = false;
            }
        }
    }

    /// Whether any shared folder allows changes.
    pub fn any_writable(&self) -> bool {
        let state = self.state.read().expect("reader state");
        state.configured.iter().any(|r| r.writable)
    }

    /// The text of the file opened as `opened`, at `path` (`root:rel`) and
    /// called `name`: from the cache when this version of it was turned into
    /// text already, else read (at most `max_bytes`), extracted and cached. `read` and
    /// `grep` share it, so a document searched is not converted again when
    /// it is read, nor the other way round.
    pub(crate) fn extracted(
        &self,
        path: &str,
        name: &str,
        opened: &mut safe_fs::OpenedFile,
        max_bytes: u64,
        deadline: Option<std::time::Instant>,
    ) -> Result<Arc<String>, ToolError> {
        // Asked of this version: the file may have changed since the caller
        // looked at its size, and its text may be in the cache.
        if opened.size > max_bytes {
            return Err(ToolError::new(
                ErrorCode::TooLarge,
                format!("{path} is over the {max_bytes} bytes this reads"),
            ));
        }
        let key: CacheKey = (
            path.to_string(),
            opened.identity,
            opened.modified,
            opened.size,
        );
        if let Some(text) = self.cached(&key) {
            return Ok(text);
        }
        let bytes = extract::read_capped(&mut opened.file, max_bytes)?;
        let text = Arc::new(extract::extract_until(
            name,
            &bytes,
            MAX_EXTRACTED_CHARS,
            deadline,
        )?);
        self.store(key, Arc::clone(&text));
        Ok(text)
    }

    fn cached(&self, key: &CacheKey) -> Option<Arc<String>> {
        let cache = self.cache.lock().expect("cache");
        cache
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| Arc::clone(v))
    }

    fn store(&self, key: CacheKey, text: Arc<String>) {
        let mut cache = self.cache.lock().expect("cache");
        cache.retain(|(k, _)| k.0 != key.0);
        cache.push_front((key, text));
        cache.truncate(CACHE_ENTRIES);
    }
}

/// A snapshot of the reader's state for one change.
pub struct ChangeView {
    pub roots: Vec<Arc<RootHandle>>,
    pub configured: Vec<crate::config::Root>,
    pub deny: Arc<DenyList>,
    pub write_deny: Arc<DenyList>,
    pub limits: Limits,
}

impl ChangeView {
    /// The shared folder whose folder has `identity`, other than `except`,
    /// whatever name or case reaches it.
    pub fn root_with_identity(&self, identity: (u64, u64), except: &str) -> Option<&RootHandle> {
        self.roots
            .iter()
            .filter(|r| r.id != except)
            .find(|r| r.identity == identity)
            .map(|r| r.as_ref())
    }

    /// Whether the shared folder `id` allows changes, as configured now.
    pub fn writable(&self, id: &str) -> bool {
        self.configured.iter().any(|r| r.id == id && r.writable)
    }

    /// The open root `id`, as reads find it.
    pub fn root(&self, id: &str) -> Result<Arc<RootHandle>, ToolError> {
        if let Some(root) = self.roots.iter().find(|r| r.id == id) {
            return Ok(Arc::clone(root));
        }
        if self.configured.iter().any(|r| r.id == id) {
            return Err(ToolError::new(
                ErrorCode::NotFound,
                "this folder is not available on the computer right now",
            ));
        }
        Err(ToolError::new(
            ErrorCode::UnknownRoot,
            format!("no shared folder has the ID {id:?}; call roots for the IDs"),
        ))
    }
}

fn build_state(config: &Config, paths: &Paths) -> Result<State> {
    let deny = Arc::new(DenyList::from_config(&config.deny, paths)?);
    let mut roots = Vec::new();
    for root in &config.roots {
        match RootHandle::open(root) {
            Ok(handle) => roots.push(Arc::new(handle)),
            Err(e) => tracing::warn!(root = %root.id, "root unavailable: {e}"),
        }
    }
    Ok(State {
        roots,
        configured: config.roots.clone(),
        coding: config.coding.clone(),
        deny,
        write_deny: Arc::new(DenyList::write_from_config(
            &config.deny,
            crate::paths::home_dir()
                .ok()
                .map(|h| h.canonicalize().unwrap_or(h))
                .as_deref(),
        )?),
        allow_hardlinks: config.deny.allow_hardlinks,
        limits: config.limits.clone(),
        index_enabled: config.index.enabled,
    })
}

fn indexer_settings(state: &State, config: &Config) -> IndexerSettings {
    IndexerSettings {
        roots: state.roots.clone(),
        deny: Arc::clone(&state.deny),
        allow_hardlinks: state.allow_hardlinks,
        max_file_bytes: state.limits.max_index_file_bytes,
        watch: config.index.watch,
        max_watches: watch::default_limit(),
        rescan: Duration::from_secs(config.index.rescan_secs.max(60)),
    }
}

fn policy(state: &State) -> OpenPolicy<'_> {
    OpenPolicy {
        deny: &state.deny,
        allow_hardlinks: state.allow_hardlinks,
    }
}

fn root_by_id(state: &State, id: &str) -> Result<Arc<RootHandle>, ToolError> {
    if let Some(root) = state.roots.iter().find(|r| r.id == id) {
        return Ok(Arc::clone(root));
    }
    if state.configured.iter().any(|r| r.id == id) {
        return Err(ToolError::new(
            ErrorCode::NotFound,
            "this folder is not available on the computer right now",
        ));
    }
    Err(ToolError::new(
        ErrorCode::UnknownRoot,
        format!("no shared folder has the ID {id:?}; call roots for the IDs"),
    ))
}

/// Compile a `path_glob`. Globs without `/` match the file name; globs with
/// `/` match the whole relative path.
fn compile_glob(glob: &str) -> Result<(GlobMatcher, bool), ToolError> {
    if glob.len() > 256 {
        return Err(ToolError::invalid_argument("path_glob is too long"));
    }
    let whole_path = glob.contains('/');
    let matcher = GlobBuilder::new(glob)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
        .map_err(|e| ToolError::invalid_argument(format!("invalid path_glob: {e}")))?
        .compile_matcher();
    Ok((matcher, whole_path))
}

/// Accepts RFC 3339 timestamps and plain `YYYY-MM-DD` dates (UTC).
pub fn parse_time(s: &str) -> Result<i64, ToolError> {
    use time::format_description::well_known::Rfc3339;
    if let Ok(t) = time::OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(t.unix_timestamp());
    }
    let format = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(s, &format)
        .map(|d| d.midnight().assume_utc().unix_timestamp())
        .map_err(|_| {
            ToolError::invalid_argument("modified_after must be an RFC 3339 time or YYYY-MM-DD")
        })
}

pub fn format_time(secs: i64) -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod upload_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1970-01-02").unwrap(), 86_400);
        assert_eq!(parse_time("1970-01-01T00:01:00Z").unwrap(), 60);
        assert!(parse_time("yesterday").is_err());
        assert_eq!(format_time(60), "1970-01-01T00:01:00Z");
    }

    #[test]
    fn globs() {
        let (m, whole) = compile_glob("*.MD").unwrap();
        assert!(!whole && m.is_match("notes.md"));
        let (m, whole) = compile_glob("plans/**/*.pdf").unwrap();
        assert!(whole && m.is_match("plans/2026/q3.pdf") && !m.is_match("other/q3.pdf"));
    }
}
