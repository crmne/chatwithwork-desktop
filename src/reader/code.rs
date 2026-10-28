//! Code search: `grep` (lines matching a regular expression) and `glob`
//! (files by name), walked like ripgrep.
//!
//! The walk respects `.gitignore`, `.ignore` and Git's own excludes, skips
//! hidden files and folders, never follows a symlink, and prunes whatever
//! the deny list covers, so nothing denied is ever visited. Ignore files
//! apply from the shared folder down, also when the search starts in a
//! folder inside it. Files are found by name in the walk and opened through
//! the root handle with the same policy as `read`, so a swap between the
//! walk and the open can't escape the folder. Both stop at a time budget.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};

use super::extract::{Format, format_for, read_capped};
use super::safe_fs::{RelPath, RootHandle, ToolPath};
use super::{format_time, root_by_id};
use crate::error::{ErrorCode, ToolError};
use crate::policy::DenyList;

/// How long one `grep` or `glob` may walk and read.
pub const TIME_BUDGET: Duration = Duration::from_secs(10);
/// Characters of a line `grep` returns; longer lines are cut.
pub const MAX_LINE_CHARS: usize = 500;
const MAX_PATTERN_CHARS: usize = 1000;
const MAX_GLOB_CHARS: usize = 500;
const MAX_CONTEXT: usize = 10;
const GREP_LIMIT: usize = 100;
const GREP_MAX_LIMIT: usize = 500;
const GLOB_LIMIT: usize = 200;
const GLOB_MAX_LIMIT: usize = 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrepOutput {
    #[default]
    Content,
    Files,
    Count,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepRequest {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub output: GrepOutput,
    #[serde(default)]
    pub context: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// The folders a bot's call may look in: only those lent to it.
    #[serde(skip)]
    pub only: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Line {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GrepMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Vec<Line>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<Line>>,
}

impl GrepMatch {
    fn chars(&self) -> usize {
        let lines = |lines: &Option<Vec<Line>>| {
            lines
                .iter()
                .flatten()
                .map(|l| l.text.chars().count())
                .sum::<usize>()
        };
        self.path.chars().count()
            + self.text.chars().count()
            + lines(&self.before)
            + lines(&self.after)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileCount {
    pub path: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GrepResponse {
    pub output: GrepOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GrepMatch>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Vec<FileCount>>,
    pub files_searched: usize,
    pub truncated: bool,
    pub timed_out: bool,
    pub next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The offset this page starts at.
    #[serde(skip)]
    pub offset: usize,
}

impl GrepResponse {
    /// Each result's shared folder and the characters it returns, in order.
    pub fn costs(&self) -> Vec<(String, usize)> {
        let root = |path: &str| path.split(':').next().unwrap_or_default().to_string();
        match self.output {
            GrepOutput::Content => self
                .matches
                .iter()
                .flatten()
                .map(|m| (root(&m.path), m.chars()))
                .collect(),
            GrepOutput::Files => self
                .files
                .iter()
                .flatten()
                .map(|p| (root(p), p.chars().count()))
                .collect(),
            GrepOutput::Count => self
                .counts
                .iter()
                .flatten()
                .map(|c| (root(&c.path), c.path.chars().count()))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.costs().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keep the first `n` results; the next page starts after them.
    pub fn keep(&mut self, n: usize, note: Option<&str>) {
        if n >= self.len() {
            return;
        }
        if let Some(m) = self.matches.as_mut() {
            m.truncate(n);
        }
        if let Some(f) = self.files.as_mut() {
            f.truncate(n);
        }
        if let Some(c) = self.counts.as_mut() {
            c.truncate(n);
        }
        self.truncated = true;
        self.next_offset = Some(self.offset + n);
        if let Some(note) = note {
            self.note = Some(note.to_string());
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobRequest {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// The folders a bot's call may look in: only those lent to it.
    #[serde(skip)]
    pub only: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GlobEntry {
    pub path: String,
    pub size: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GlobResponse {
    pub paths: Vec<GlobEntry>,
    pub truncated: bool,
    pub next_offset: Option<usize>,
    /// Only when the time budget ran out before the walk ended.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip)]
    pub offset: usize,
}

impl GlobResponse {
    pub fn costs(&self) -> Vec<(String, usize)> {
        self.paths
            .iter()
            .map(|e| {
                (
                    e.path.split(':').next().unwrap_or_default().to_string(),
                    e.path.chars().count(),
                )
            })
            .collect()
    }

    pub fn keep(&mut self, n: usize, note: Option<&str>) {
        if n >= self.paths.len() {
            return;
        }
        self.paths.truncate(n);
        self.truncated = true;
        self.next_offset = Some(self.offset + n);
        if let Some(note) = note {
            self.note = Some(note.to_string());
        }
    }
}

/// Where a search looks: a folder (the shared folder itself, or one in
/// it), or one file.
struct Start {
    root: Arc<RootHandle>,
    rel: RelPath,
    is_file: bool,
}

/// The places a `path` argument names: one, or every shared folder the
/// caller reaches (`only`, for a bot) in the order of their IDs.
fn starts(
    state: &super::State,
    path: Option<&str>,
    only: Option<&[String]>,
    files_allowed: bool,
) -> Result<Vec<Start>, ToolError> {
    let Some(path) = path else {
        let mut roots: Vec<Arc<RootHandle>> = state
            .roots
            .iter()
            .filter(|r| only.is_none_or(|only| only.contains(&r.id)))
            .cloned()
            .collect();
        roots.sort_by(|a, b| a.id.cmp(&b.id));
        return Ok(roots
            .into_iter()
            .map(|root| Start {
                root,
                rel: RelPath::default(),
                is_file: false,
            })
            .collect());
    };
    let parsed = ToolPath::parse(path)?;
    let root = root_by_id(state, &parsed.root_id)?;
    let policy = super::policy(state);
    if parsed.rel.is_root() {
        return Ok(vec![Start {
            root,
            rel: parsed.rel,
            is_file: false,
        }]);
    }
    // A file is searched as named, a folder walked; either is resolved
    // first as `read` and `list` would, deny list and links included.
    match root.open_file(&parsed.rel, policy) {
        Ok(_) if files_allowed => Ok(vec![Start {
            root,
            rel: parsed.rel,
            is_file: true,
        }]),
        Ok(_) => Err(ToolError::new(
            ErrorCode::NotADirectory,
            "this path is a file; name a folder",
        )),
        Err(e) if e.code == ErrorCode::NotAFile => {
            root.list_dir(&parsed.rel, policy)?;
            Ok(vec![Start {
                root,
                rel: parsed.rel,
                is_file: false,
            }])
        }
        Err(e) => Err(e),
    }
}

/// A file the walk found.
struct Found {
    rel: RelPath,
    size: u64,
    modified: i64,
}

/// Walk `start` for files, in path order (siblings by name), calling
/// `visit` for each until it says to stop. `hidden` also visits hidden
/// files and folders. Returns false when the time budget ran out.
fn walk(
    start: &Start,
    deny: &Arc<DenyList>,
    hidden: bool,
    deadline: Instant,
    mut visit: impl FnMut(Found) -> bool,
) -> bool {
    let root = &start.root;
    let base = root.abs_path(&start.rel);
    let mut builder = WalkBuilder::new(&base);
    builder
        .follow_links(false)
        .hidden(!hidden)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .parents(false)
        .require_git(false)
        .same_file_system(false)
        .sort_by_file_name(Ord::cmp);
    // The ignore files of the folders above the start, from the shared
    // folder down, apply as they would in a walk from the top; the walk
    // reads those from the start down itself.
    let above = ignores_above(&root.path, &start.rel);
    let filter_deny = Arc::clone(deny);
    builder.filter_entry(move |entry| {
        if filter_deny.is_denied(entry.path()) {
            return false;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        // The nearest folder's rules win; a `!` rule keeps what a folder
        // above ignores. Each entry is matched as the walk reaches it, so
        // a folder named as the start is searched even where a folder
        // above ignores it, as ripgrep does.
        for gitignore in above.iter().rev() {
            match gitignore.matched(entry.path(), is_dir) {
                ignore::Match::Ignore(_) => return false,
                ignore::Match::Whitelist(_) => return true,
                ignore::Match::None => {}
            }
        }
        true
    });
    for entry in builder.build() {
        if Instant::now() >= deadline {
            return false;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&root.path) else {
            continue;
        };
        let Some(rel) = RelPath::from_relative_path(rel) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        if !visit(Found {
            rel,
            size: meta.len(),
            modified,
        }) {
            break;
        }
    }
    true
}

/// The ignore rules of the folders from the shared folder at `top` down to
/// the folder holding `start`: each folder's `.gitignore` and `.ignore`, and
/// Git's `info/exclude` at the top. Empty when the walk starts at the top.
fn ignores_above(top: &Path, start: &RelPath) -> Vec<Gitignore> {
    let parts = start.components();
    if parts.is_empty() {
        return Vec::new();
    }
    let mut dirs = vec![top.to_path_buf()];
    for part in &parts[..parts.len() - 1] {
        let next = dirs[dirs.len() - 1].join(part);
        dirs.push(next);
    }
    dirs.iter()
        .enumerate()
        .filter_map(|(i, dir)| {
            let mut builder = GitignoreBuilder::new(dir);
            let mut files = Vec::new();
            if i == 0 {
                files.push(dir.join(".git").join("info").join("exclude"));
            }
            files.extend([dir.join(".gitignore"), dir.join(".ignore")]);
            let mut any = false;
            for file in files.into_iter().filter(|f| f.is_file()) {
                any = true;
                let _ = builder.add(file);
            }
            any.then(|| builder.build().ok()).flatten()
        })
        .collect()
}

/// A glob that matches the file name without `/`, and the path relative
/// to where the search starts with one.
struct NameGlob {
    matcher: GlobMatcher,
    whole_path: bool,
}

impl NameGlob {
    fn new(glob: &str, case_insensitive: bool, what: &str) -> Result<Self, ToolError> {
        let matcher = GlobBuilder::new(glob)
            .case_insensitive(case_insensitive)
            .literal_separator(true)
            .build()
            .map_err(|e| ToolError::invalid_argument(format!("invalid {what}: {e}")))?
            .compile_matcher();
        Ok(Self {
            matcher,
            whole_path: glob.contains('/'),
        })
    }

    /// Whether `rel` (relative to the shared folder) matches, seen from
    /// `start` (a folder in it).
    fn matches(&self, start: &RelPath, rel: &RelPath) -> bool {
        if self.whole_path {
            let inner = &rel.components()[start.components().len().min(rel.components().len())..];
            self.matcher.is_match(inner.join("/"))
        } else {
            self.matcher.is_match(rel.file_name().unwrap_or_default())
        }
    }
}

/// Cut `line` to [`MAX_LINE_CHARS`], dropping a line ending.
fn line_text(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let text = String::from_utf8_lossy(line);
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => text[..end].to_string(),
        None => text.into_owned(),
    }
}

fn matcher(req: &GrepRequest) -> Result<RegexMatcher, ToolError> {
    let count = req.pattern.chars().count();
    if count == 0 {
        return Err(ToolError::invalid_argument("pattern must not be empty"));
    }
    if count > MAX_PATTERN_CHARS {
        return Err(ToolError::invalid_argument(format!(
            "pattern is longer than {MAX_PATTERN_CHARS} characters"
        )));
    }
    RegexMatcherBuilder::new()
        .case_insensitive(req.ignore_case)
        .multi_line(true)
        .crlf(true)
        .line_terminator(Some(b'\n'))
        .build(&req.pattern)
        .map_err(|e| ToolError::invalid_argument(format!("invalid pattern: {e}")))
}

fn bounded(value: Option<usize>, name: &str, min: usize, max: usize) -> Result<(), ToolError> {
    match value {
        Some(v) if v < min || v > max => Err(ToolError::invalid_argument(format!(
            "{name} must be from {min} to {max}"
        ))),
        _ => Ok(()),
    }
}

/// What one file holds that a `grep` returns.
enum FileHits {
    Lines(Vec<GrepMatch>),
    Count(usize),
}

/// Search `bytes`, the text of `path`. `None` when nothing matches.
fn search_file(
    matcher: &RegexMatcher,
    path: &str,
    bytes: &[u8],
    output: GrepOutput,
    context: usize,
) -> Option<FileHits> {
    if !matcher.is_match(bytes).unwrap_or(false) {
        return None;
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let lines: Vec<&[u8]> = body.split(|b| *b == b'\n').collect();
    let is_match = |line: &[u8]| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        matcher.is_match(line).unwrap_or(false)
    };
    if output != GrepOutput::Content {
        let count = lines.iter().filter(|line| is_match(line)).count();
        return (count > 0).then_some(FileHits::Count(count));
    }
    let around = |from: usize, to: usize| {
        (from..to)
            .map(|i| Line {
                line: i + 1,
                text: line_text(lines[i]),
            })
            .collect::<Vec<_>>()
    };
    let hits: Vec<GrepMatch> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_match(line))
        .map(|(i, line)| GrepMatch {
            path: path.to_string(),
            line: i + 1,
            text: line_text(line),
            before: (context > 0).then(|| around(i.saturating_sub(context), i)),
            after: (context > 0).then(|| around(i + 1, (i + 1 + context).min(lines.len()))),
        })
        .collect();
    (!hits.is_empty()).then_some(FileHits::Lines(hits))
}

impl super::Reader {
    /// Lines matching a regular expression in the shared folders, as
    /// ripgrep finds them (see the module's notes on the walk).
    pub fn grep(&self, req: &GrepRequest) -> Result<GrepResponse, ToolError> {
        self.grep_until(req, Instant::now() + TIME_BUDGET)
    }

    pub(crate) fn grep_until(
        &self,
        req: &GrepRequest,
        deadline: Instant,
    ) -> Result<GrepResponse, ToolError> {
        let state = self.state.read().expect("reader state");
        let matcher = matcher(req)?;
        bounded(req.limit, "limit", 1, GREP_MAX_LIMIT)?;
        bounded(req.context, "context", 0, MAX_CONTEXT)?;
        let limit = req.limit.unwrap_or(GREP_LIMIT);
        let context = req.context.unwrap_or(0);
        if context > 0 && req.output != GrepOutput::Content {
            return Err(ToolError::invalid_argument(
                "context only goes with output \"content\"",
            ));
        }
        let offset = req.offset.unwrap_or(0);
        let glob = match req.glob.as_deref() {
            Some(glob) if glob.chars().count() > MAX_GLOB_CHARS => {
                return Err(ToolError::invalid_argument("glob is too long"));
            }
            Some(glob) => Some(NameGlob::new(glob, true, "glob")?),
            None => None,
        };
        let starts = starts(&state, req.path.as_deref(), req.only.as_deref(), true)?;
        let policy = super::policy(&state);
        let max_bytes = state.limits.max_index_file_bytes;

        let mut matches = Vec::new();
        let mut files = Vec::new();
        let mut counts = Vec::new();
        // Results seen so far, those before `offset` included.
        let mut seen = 0usize;
        let mut more = false;
        let mut files_searched = 0usize;
        let mut timed_out = false;
        let mut search = |start: &Start, found: Found| -> bool {
            if found.size > max_bytes
                || format_for(found.rel.file_name().unwrap_or_default()) != Some(Format::Text)
                || (!start.is_file
                    && glob
                        .as_ref()
                        .is_some_and(|g| !g.matches(&start.rel, &found.rel)))
            {
                return true;
            }
            let Ok(mut opened) = start.root.open_file(&found.rel, policy) else {
                return true;
            };
            let Ok(bytes) = read_capped(&mut opened.file, max_bytes) else {
                return true;
            };
            // Binary files are skipped, as ripgrep does: a NUL byte.
            if bytes.contains(&0) {
                return true;
            }
            files_searched += 1;
            let path = ToolPath::display(&start.root.id, &found.rel);
            match search_file(&matcher, &path, &bytes, req.output, context) {
                None => true,
                Some(FileHits::Lines(hits)) => {
                    for hit in hits {
                        seen += 1;
                        if seen <= offset {
                            continue;
                        }
                        if matches.len() == limit {
                            more = true;
                            return false;
                        }
                        matches.push(hit);
                    }
                    true
                }
                Some(FileHits::Count(count)) => {
                    seen += 1;
                    if seen <= offset {
                        return true;
                    }
                    if files.len() + counts.len() == limit {
                        more = true;
                        return false;
                    }
                    match req.output {
                        GrepOutput::Files => files.push(path),
                        _ => counts.push(FileCount { path, count }),
                    }
                    true
                }
            }
        };
        for start in &starts {
            if start.is_file {
                if Instant::now() >= deadline {
                    timed_out = true;
                    break;
                }
                let Ok(opened) = start.root.open_file(&start.rel, policy) else {
                    continue;
                };
                let found = Found {
                    rel: start.rel.clone(),
                    size: opened.size,
                    modified: opened.modified,
                };
                drop(opened);
                if !search(start, found) {
                    break;
                }
                continue;
            }
            let mut stopped = false;
            if !walk(start, &state.deny, false, deadline, |found| {
                let go_on = search(start, found);
                stopped = !go_on;
                go_on
            }) {
                timed_out = true;
                break;
            }
            if stopped {
                break;
            }
        }
        let output = req.output;
        Ok(GrepResponse {
            output,
            matches: (output == GrepOutput::Content).then_some(matches),
            files: (output == GrepOutput::Files).then_some(files),
            counts: (output == GrepOutput::Count).then_some(counts),
            files_searched,
            truncated: more,
            timed_out,
            next_offset: more.then_some(offset + limit),
            note: timed_out.then(|| {
                "The search stopped at its time budget; these are the results found until \
                 then. Search a narrower path."
                    .to_string()
            }),
            offset,
        })
    }

    /// Files whose names match a glob, in the shared folders.
    pub fn glob(&self, req: &GlobRequest) -> Result<GlobResponse, ToolError> {
        self.glob_until(req, Instant::now() + TIME_BUDGET)
    }

    pub(crate) fn glob_until(
        &self,
        req: &GlobRequest,
        deadline: Instant,
    ) -> Result<GlobResponse, ToolError> {
        let state = self.state.read().expect("reader state");
        let count = req.pattern.chars().count();
        if count == 0 {
            return Err(ToolError::invalid_argument("pattern must not be empty"));
        }
        if count > MAX_GLOB_CHARS {
            return Err(ToolError::invalid_argument(format!(
                "pattern is longer than {MAX_GLOB_CHARS} characters"
            )));
        }
        bounded(req.limit, "limit", 1, GLOB_MAX_LIMIT)?;
        let limit = req.limit.unwrap_or(GLOB_LIMIT);
        let offset = req.offset.unwrap_or(0);
        let glob = NameGlob::new(&req.pattern, false, "pattern")?;
        let starts = starts(&state, req.path.as_deref(), req.only.as_deref(), false)?;
        let mut paths = Vec::new();
        let mut seen = 0usize;
        let mut more = false;
        let mut timed_out = false;
        for start in &starts {
            let finished = walk(start, &state.deny, req.hidden, deadline, |found| {
                if !glob.matches(&start.rel, &found.rel) {
                    return true;
                }
                seen += 1;
                if seen <= offset {
                    return true;
                }
                if paths.len() == limit {
                    more = true;
                    return false;
                }
                paths.push(GlobEntry {
                    path: ToolPath::display(&start.root.id, &found.rel),
                    size: found.size,
                    modified: format_time(found.modified),
                });
                true
            });
            if !finished {
                timed_out = true;
                break;
            }
            if more {
                break;
            }
        }
        Ok(GlobResponse {
            paths,
            truncated: more,
            next_offset: more.then_some(offset + limit),
            timed_out,
            note: timed_out.then(|| {
                "The walk stopped at its time budget; these are the files found until then. \
                 Look in a narrower path."
                    .to_string()
            }),
            offset,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config::{Config, Root};
    use crate::paths::Paths;
    use crate::reader::Reader;
    use crate::status::Changes;

    struct Fixture {
        _tmp: tempfile::TempDir,
        app: PathBuf,
        reader: Reader,
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let app = base.join("app");
        let docs = base.join("docs");
        let outside = base.join("outside");
        write(
            &app.join("src/lib.rs"),
            "fn main() {\n    run();\n}\n\nfn run() {}\n",
        );
        write(
            &app.join("src/util.rs"),
            "pub fn helper() {}\n// TODO: Run faster\n",
        );
        write(&app.join("README.md"), "# App\nRun it with cargo run.\n");
        write(&app.join(".gitignore"), "target/\n*.log\n");
        write(&app.join("target/debug/out.rs"), "fn run() {}\n");
        write(&app.join("build.log"), "run run run\n");
        write(&app.join("app/sub/.gitignore"), "skipped.rs\n");
        write(&app.join("app/sub/skipped.rs"), "fn run() {}\n");
        write(&app.join("app/sub/kept.rs"), "fn run() {}\n");
        write(&app.join(".github/workflows/ci.yml"), "run: cargo test\n");
        write(&app.join(".env"), "RUN_TOKEN=secret\n");
        write(&app.join("src/blob.dat"), "run\0binary\n");
        write(&outside.join("secret.rs"), "fn run() { secret }\n");
        symlink(outside.join("secret.rs"), app.join("src/linked.rs")).unwrap();
        symlink(&outside, app.join("linked")).unwrap();
        write(&docs.join("notes.md"), "run the numbers\n");
        let paths = Paths::under(&base.join("cww"));
        let root = |id: &str, path: &Path| Root {
            bots: Vec::new(),
            id: id.into(),
            label: id.to_uppercase(),
            path: path.to_path_buf(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        };
        let mut config = Config {
            roots: vec![root("docs", &docs), root("app", &app)],
            ..Config::default()
        };
        config.index.enabled = false;
        let reader = Reader::new(&config, &paths, Changes::default()).unwrap();
        Fixture {
            _tmp: tmp,
            app,
            reader,
        }
    }

    fn grep(f: &Fixture, req: GrepRequest) -> GrepResponse {
        f.reader.grep(&req).unwrap()
    }

    fn req(pattern: &str) -> GrepRequest {
        GrepRequest {
            pattern: pattern.into(),
            ..GrepRequest::default()
        }
    }

    fn lines(resp: &GrepResponse) -> Vec<String> {
        resp.matches
            .as_ref()
            .unwrap()
            .iter()
            .map(|m| format!("{}:{}", m.path, m.line))
            .collect()
    }

    #[test]
    fn greps_like_ripgrep() {
        let f = fixture();
        let resp = grep(
            &f,
            GrepRequest {
                path: Some("app:".into()),
                ..req(r"\brun\b")
            },
        );
        // Not in target/ or *.log (ignored), not in app/sub/skipped.rs
        // (ignored below), not hidden, not denied, not binary, never
        // through a link; in path order.
        assert_eq!(
            lines(&resp),
            [
                "app:README.md:2",
                "app:app/sub/kept.rs:1",
                "app:src/lib.rs:2",
                "app:src/lib.rs:5"
            ]
        );
        assert_eq!(resp.matches.as_ref().unwrap()[1].text, "fn run() {}");
        assert!(!resp.truncated && !resp.timed_out && resp.next_offset.is_none());
        assert_eq!(resp.files_searched, 4);

        // Without a path, every shared folder, in the order of their IDs.
        let all = grep(&f, req(r"\brun\b"));
        assert_eq!(lines(&all)[0], "app:README.md:2");
        assert_eq!(lines(&all).last().unwrap(), "docs:notes.md:1");
        // A bot sees only the folders lent to it.
        let lent = grep(
            &f,
            GrepRequest {
                only: Some(vec!["docs".into()]),
                ..req(r"\brun\b")
            },
        );
        assert_eq!(lines(&lent), ["docs:notes.md:1"]);
    }

    #[test]
    fn ignore_case_files_counts_and_context() {
        let f = fixture();
        let any_case = grep(
            &f,
            GrepRequest {
                path: Some("app:src".into()),
                ignore_case: true,
                ..req("RUN")
            },
        );
        assert_eq!(
            lines(&any_case),
            ["app:src/lib.rs:2", "app:src/lib.rs:5", "app:src/util.rs:2"]
        );
        let files = grep(
            &f,
            GrepRequest {
                path: Some("app:".into()),
                output: GrepOutput::Files,
                ..req("run")
            },
        );
        assert_eq!(
            files.files.unwrap(),
            ["app:README.md", "app:app/sub/kept.rs", "app:src/lib.rs"]
        );
        let counts = grep(
            &f,
            GrepRequest {
                path: Some("app:src/lib.rs".into()),
                output: GrepOutput::Count,
                ..req("run")
            },
        );
        assert_eq!(
            counts.counts.unwrap(),
            [FileCount {
                path: "app:src/lib.rs".into(),
                count: 2
            }]
        );
        let context = grep(
            &f,
            GrepRequest {
                path: Some("app:src/lib.rs".into()),
                context: Some(1),
                limit: Some(1),
                ..req("run")
            },
        );
        let first = &context.matches.as_ref().unwrap()[0];
        assert_eq!(
            first.before.as_ref().unwrap(),
            &[Line {
                line: 1,
                text: "fn main() {".into()
            }]
        );
        assert_eq!(first.after.as_ref().unwrap()[0].text, "}");
        let json = serde_json::to_value(grep(&f, req("helper"))).unwrap();
        assert!(json["matches"][0].get("before").is_none());
        assert_eq!(json["next_offset"], serde_json::Value::Null);
    }

    #[test]
    fn pages_through_results() {
        let f = fixture();
        let page = |offset| {
            grep(
                &f,
                GrepRequest {
                    path: Some("app:".into()),
                    limit: Some(2),
                    offset: Some(offset),
                    ..req(r"\brun\b")
                },
            )
        };
        let first = page(0);
        assert_eq!(lines(&first), ["app:README.md:2", "app:app/sub/kept.rs:1"]);
        assert!(first.truncated);
        assert_eq!(first.next_offset, Some(2));
        let second = page(2);
        assert_eq!(lines(&second), ["app:src/lib.rs:2", "app:src/lib.rs:5"]);
        assert!(!second.truncated && second.next_offset.is_none());
        assert!(lines(&page(4)).is_empty());
    }

    #[test]
    fn globs_narrow_a_grep() {
        let f = fixture();
        let by_name = grep(
            &f,
            GrepRequest {
                path: Some("app:".into()),
                glob: Some("*.RS".into()),
                ..req(r"\brun\b")
            },
        );
        assert_eq!(
            lines(&by_name),
            [
                "app:app/sub/kept.rs:1",
                "app:src/lib.rs:2",
                "app:src/lib.rs:5"
            ]
        );
        let by_path = grep(
            &f,
            GrepRequest {
                path: Some("app:".into()),
                glob: Some("src/**/*.rs".into()),
                ..req(r"\brun\b")
            },
        );
        assert_eq!(lines(&by_path), ["app:src/lib.rs:2", "app:src/lib.rs:5"]);
    }

    #[test]
    fn a_hidden_or_ignored_folder_named_as_the_path_is_searched() {
        let f = fixture();
        let hidden = grep(
            &f,
            GrepRequest {
                path: Some("app:.github".into()),
                ..req("cargo")
            },
        );
        assert_eq!(lines(&hidden), ["app:.github/workflows/ci.yml:1"]);
        // Inside a folder, the ignore files above it still apply.
        let below = grep(
            &f,
            GrepRequest {
                path: Some("app:app/sub".into()),
                ..req("run")
            },
        );
        assert_eq!(lines(&below), ["app:app/sub/kept.rs:1"]);
        let ignored = grep(
            &f,
            GrepRequest {
                path: Some("app:target".into()),
                ..req("run")
            },
        );
        assert_eq!(lines(&ignored), ["app:target/debug/out.rs:1"]);
    }

    #[test]
    fn refuses_what_read_refuses() {
        let f = fixture();
        let code = |r: GrepRequest| f.reader.grep(&r).unwrap_err().code;
        for (path, expected) in [
            ("app:.env", ErrorCode::Denied),
            ("app:src/linked.rs", ErrorCode::Denied),
            ("app:linked", ErrorCode::Denied),
            ("app:../outside", ErrorCode::InvalidPath),
            ("/etc", ErrorCode::InvalidPath),
            ("nope:", ErrorCode::UnknownRoot),
            ("app:missing", ErrorCode::NotFound),
        ] {
            let got = code(GrepRequest {
                path: Some(path.into()),
                ..req("run")
            });
            assert_eq!(got, expected, "{path}");
        }
        for bad in [
            req(""),
            req(&"x".repeat(1001)),
            req("(unclosed"),
            GrepRequest {
                limit: Some(0),
                ..req("run")
            },
            GrepRequest {
                limit: Some(501),
                ..req("run")
            },
            GrepRequest {
                context: Some(11),
                ..req("run")
            },
            GrepRequest {
                context: Some(1),
                output: GrepOutput::Files,
                ..req("run")
            },
        ] {
            assert_eq!(code(bad), ErrorCode::InvalidArgument);
        }
        let unknown: Result<GrepRequest, _> =
            serde_json::from_value(serde_json::json!({ "pattern": "x", "regex": true }));
        assert!(unknown.is_err());
    }

    #[test]
    fn long_lines_are_cut_and_time_runs_out() {
        let f = fixture();
        write(
            &f.app.join("long.txt"),
            &format!("{}run\n", "é".repeat(600)),
        );
        let resp = grep(
            &f,
            GrepRequest {
                path: Some("app:long.txt".into()),
                ..req("run")
            },
        );
        assert_eq!(
            resp.matches.unwrap()[0].text.chars().count(),
            MAX_LINE_CHARS
        );
        let late = f
            .reader
            .grep_until(&req("run"), Instant::now() - Duration::from_secs(1))
            .unwrap();
        assert!(late.timed_out && late.note.is_some());
        assert!(lines(&late).is_empty());
    }

    fn glob(f: &Fixture, pattern: &str, path: Option<&str>, hidden: bool) -> Vec<String> {
        f.reader
            .glob(&GlobRequest {
                pattern: pattern.into(),
                path: path.map(str::to_string),
                hidden,
                ..GlobRequest::default()
            })
            .unwrap()
            .paths
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    #[test]
    fn globs_files_by_name_and_path() {
        let f = fixture();
        assert_eq!(
            glob(&f, "*.rs", Some("app:"), false),
            ["app:app/sub/kept.rs", "app:src/lib.rs", "app:src/util.rs"]
        );
        // Case-sensitive.
        assert!(glob(&f, "*.RS", Some("app:"), false).is_empty());
        // With a slash, relative to the path.
        assert_eq!(
            glob(&f, "sub/*.rs", Some("app:app"), false),
            ["app:app/sub/kept.rs"]
        );
        assert_eq!(
            glob(&f, "**/*.yml", Some("app:"), true),
            ["app:.github/workflows/ci.yml"]
        );
        assert!(glob(&f, "**/*.yml", Some("app:"), false).is_empty());
        // Denied files stay out even with hidden ones.
        assert!(glob(&f, ".env", Some("app:"), true).is_empty());
        assert_eq!(
            glob(&f, "*.md", None, false),
            ["app:README.md", "docs:notes.md"]
        );
        let entry = &f
            .reader
            .glob(&GlobRequest {
                pattern: "lib.rs".into(),
                ..GlobRequest::default()
            })
            .unwrap()
            .paths[0];
        assert_eq!(entry.size, 38);
        assert!(entry.modified.ends_with('Z'));
    }

    #[test]
    fn pages_globs_and_refuses_bad_ones() {
        let f = fixture();
        let page = f
            .reader
            .glob(&GlobRequest {
                pattern: "*.rs".into(),
                limit: Some(2),
                offset: Some(1),
                ..GlobRequest::default()
            })
            .unwrap();
        assert_eq!(
            page.paths
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["app:src/lib.rs", "app:src/util.rs"]
        );
        assert!(!page.truncated);
        let first = f
            .reader
            .glob(&GlobRequest {
                pattern: "*.rs".into(),
                limit: Some(1),
                ..GlobRequest::default()
            })
            .unwrap();
        assert!(first.truncated);
        assert_eq!(first.next_offset, Some(1));
        let code = |r: GlobRequest| f.reader.glob(&r).unwrap_err().code;
        for bad in [
            GlobRequest::default(),
            GlobRequest {
                pattern: "x".repeat(501),
                ..GlobRequest::default()
            },
            GlobRequest {
                pattern: "[".into(),
                ..GlobRequest::default()
            },
            GlobRequest {
                pattern: "*".into(),
                limit: Some(1001),
                ..GlobRequest::default()
            },
        ] {
            assert_eq!(code(bad), ErrorCode::InvalidArgument);
        }
        assert_eq!(
            code(GlobRequest {
                pattern: "*".into(),
                path: Some("app:src/lib.rs".into()),
                ..GlobRequest::default()
            }),
            ErrorCode::NotADirectory
        );
        assert_eq!(
            code(GlobRequest {
                pattern: "*".into(),
                path: Some("app:linked".into()),
                ..GlobRequest::default()
            }),
            ErrorCode::Denied
        );
    }
}
