//! `patch`: edits to several files and new files, approved as one change
//! set and made all or nothing.
//!
//! Every change is checked first, against the files as they are, with the
//! rules of `edit` and `create` (and every rule a change follows); if any
//! fails, nothing is written. Then:
//!
//! 1. Each file's new version is written to a hidden file beside it and
//!    flushed (`stage`). A failure here removes the hidden files: nothing
//!    changed.
//! 2. One file at a time, the new version takes the file's name. An edited
//!    file is swapped with its new version in one step where the system can
//!    (Linux, macOS), and otherwise moved aside under a hidden name first;
//!    either way its previous version stays beside it, under a hidden name.
//!    A new file is renamed into place without replacing anything. If one
//!    fails, the files already changed get their previous versions back
//!    and the new files are removed, so the folders end as they started.
//! 3. Only once every file is in place do the previous versions go to the
//!    system trash. Should the trash refuse one, every file is put back:
//!    those whose previous version is still beside them from it, the
//!    others from the previous content kept in memory.
//!
//! Patches run one at a time with every other change, and count as one
//! change per file against the change budgets.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Effect, Target, Writer, apply_edit, full_diff, kinds, new_file_mode, sha256};
use crate::error::{ErrorCode, ToolError};
use crate::reader::format_time;
use crate::reader::safe_fs::{ChangeDir, EntryStat, RenameError, Staged, ToolPath, temp_name};

/// Most changes in one patch, and most files they touch.
pub const MAX_CHANGES: usize = 50;
pub const MAX_FILES: usize = 20;
/// The diffs of a dry run together are cut at this many bytes.
pub const MAX_PATCH_DIFF_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRequest {
    pub changes: Vec<Value>,
    #[serde(default)]
    pub expected_sha256: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub dry_run: bool,
}

/// What was in a file a patch edited.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatchPrevious {
    pub size: u64,
    pub modified: String,
    pub sha256: String,
    /// It went to the system trash: only when the patch was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_trash: Option<bool>,
}

/// One file of a patch, as the server sees it, plus what only the local
/// audit log gets.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatchFile {
    pub path: String,
    /// `edited` or `created`.
    pub effect: Effect,
    /// How many edits a dry run makes to the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edits: Option<usize>,
    /// The diff of a dry run, `null` once the diffs were cut before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<Value>,
    /// The diffs were cut before this file's.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub diff_cut: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<PatchPrevious>,
    /// Bytes written to disk. Local only.
    #[serde(skip)]
    pub written: u64,
    /// Where the previous version went in the trash. Local only.
    #[serde(skip)]
    pub trashed_to: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatchResult {
    pub dry_run: bool,
    /// Always `patched`.
    pub effect: &'static str,
    pub files: Vec<PatchFile>,
}

/// One change as the call gave it.
enum Change {
    Edit { old_text: String, new_text: String },
    Create { content: String },
}

/// The changes to one file, in the order given.
struct FilePlan {
    path: String,
    /// Index of its first change.
    first: usize,
    create: Option<(usize, String)>,
    edits: Vec<(usize, String, String)>,
}

/// One file, checked and ready to change.
struct Ready {
    t: Target,
    /// What was there, for an edited file: its state, text and SHA-256.
    old: Option<(EntryStat, String, String)>,
    new: String,
    edits: usize,
}

/// One file as the patch left it, for putting it back.
enum Applied {
    /// A new file, with its identity.
    Created((u64, u64)),
    /// An edited file: the previous version's hidden name beside it, and
    /// the new version's identity.
    Swapped(OsString, (u64, u64)),
}

/// An error about change `index` of the call, at `path` when it has one.
fn at(index: usize, path: Option<&str>, e: ToolError) -> ToolError {
    let mut message = match path {
        Some(path) => format!("Change {index} ({path}): {}", e.message),
        None => format!("Change {index}: {}", e.message),
    };
    if !message.ends_with('.') {
        message.push('.');
    }
    message.push_str(" No file was changed.");
    ToolError::new(e.code, message)
}

fn text_field(
    change: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ToolError> {
    match change.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ToolError::invalid_argument(format!(
            "{key} must be a string"
        ))),
    }
}

/// Read change `index` of the call: an edit (`path`, `old_text`,
/// `new_text`) or a new file (`path`, `content`).
fn parse_change(value: &Value) -> Result<(String, Change), ToolError> {
    let Value::Object(change) = value else {
        return Err(ToolError::invalid_argument(
            "each change is an object: path, old_text and new_text, or path and content",
        ));
    };
    if let Some(key) = change
        .keys()
        .find(|k| !["path", "old_text", "new_text", "content"].contains(&k.as_str()))
    {
        let key: String = key
            .chars()
            .take(64)
            .filter(|c| !crate::reader::safe_fs::is_unsafe_in_path(*c))
            .collect();
        return Err(ToolError::invalid_argument(format!(
            "unknown field {key:?}; a change has path, old_text and new_text, or path and content"
        )));
    }
    let path = text_field(change, "path")?
        .ok_or_else(|| ToolError::invalid_argument("a change needs a path"))?;
    let old_text = text_field(change, "old_text")?;
    let new_text = text_field(change, "new_text")?;
    let content = text_field(change, "content")?;
    let change = match (old_text, new_text, content) {
        (Some(old_text), Some(new_text), None) => Change::Edit { old_text, new_text },
        (None, None, Some(content)) => Change::Create { content },
        _ => {
            return Err(ToolError::invalid_argument(
                "a change is either an edit (old_text and new_text) or a new file (content), \
                 never both",
            ));
        }
    };
    Ok((path, change))
}

/// Group the call's changes by file, in the order the files first appear.
fn plan(req: &PatchRequest) -> Result<Vec<FilePlan>, ToolError> {
    if req.changes.is_empty() || req.changes.len() > MAX_CHANGES {
        return Err(ToolError::invalid_argument(format!(
            "changes must hold from 1 to {MAX_CHANGES} changes"
        )));
    }
    let mut plans: Vec<FilePlan> = Vec::new();
    for (index, value) in req.changes.iter().enumerate() {
        let (path, change) = parse_change(value).map_err(|e| at(index, None, e))?;
        let path = ToolPath::parse(&path)
            .map_err(|e| at(index, None, e))?
            .to_string();
        let plan = match plans.iter_mut().find(|p| p.path == path) {
            Some(plan) => plan,
            None => {
                if plans.len() == MAX_FILES {
                    return Err(at(
                        index,
                        Some(&path),
                        ToolError::invalid_argument(format!(
                            "a patch changes at most {MAX_FILES} files"
                        )),
                    ));
                }
                plans.push(FilePlan {
                    path: path.clone(),
                    first: index,
                    create: None,
                    edits: Vec::new(),
                });
                plans.last_mut().expect("just pushed")
            }
        };
        match change {
            Change::Create { content } => {
                if plan.create.is_some() || !plan.edits.is_empty() {
                    return Err(at(
                        index,
                        Some(&path),
                        ToolError::invalid_argument(
                            "this path has another change in the patch; a new file is created \
                             once, with its whole content, and isn't edited in the same patch",
                        ),
                    ));
                }
                plan.create = Some((index, content));
            }
            Change::Edit { old_text, new_text } => {
                if plan.create.is_some() {
                    return Err(at(
                        index,
                        Some(&path),
                        ToolError::invalid_argument(
                            "this path is created in the patch, and a new file isn't edited in \
                             the same patch; put the text in its content",
                        ),
                    ));
                }
                plan.edits.push((index, old_text, new_text));
            }
        }
    }
    Ok(plans)
}

impl Writer {
    /// Edit several files and create new ones, all or nothing. See the
    /// module's notes for how.
    pub fn patch(&self, req: &PatchRequest) -> Result<PatchResult, ToolError> {
        let _one = self.one_at_a_time();
        let plans = plan(req)?;
        let mut expected = BTreeMap::new();
        for (key, sha) in req.expected_sha256.iter().flatten() {
            let path = ToolPath::parse(key)
                .map_err(|e| {
                    ToolError::new(
                        e.code,
                        format!("expected_sha256 has a key that isn't a path: {}", e.message),
                    )
                })?
                .to_string();
            match plans.iter().find(|p| p.path == path) {
                Some(plan) if plan.create.is_none() => {
                    expected.insert(path, sha.clone());
                }
                Some(_) => {
                    return Err(ToolError::invalid_argument(format!(
                        "expected_sha256 names {path}, which the patch creates; it only takes \
                         files the patch edits"
                    )));
                }
                None => {
                    return Err(ToolError::invalid_argument(format!(
                        "expected_sha256 names {path}, which no change in the patch touches"
                    )));
                }
            }
        }

        let view = self.reader.change_view();
        let cap = view.limits.max_change_file_bytes;
        let mut ready: Vec<Ready> = Vec::new();
        for plan in &plans {
            let path = Some(plan.path.as_str());
            let t = self
                .target(&view, &plan.path)
                .map_err(|e| at(plan.first, path, e))?;
            if let Some((index, content)) = &plan.create {
                let fail = |e| at(*index, path, e);
                kinds::check_new_name(&t.name).map_err(fail)?;
                kinds::check_text_name(&t.name).map_err(fail)?;
                kinds::check_text(content, cap).map_err(fail)?;
                if t.dir.entry(t.name()).map_err(fail)?.is_some() {
                    return Err(fail(ToolError::new(
                        ErrorCode::Exists,
                        "something is already at this path. Edit it with old_text and \
                         new_text, or pick another name",
                    )));
                }
                ready.push(Ready {
                    t,
                    old: None,
                    new: content.clone(),
                    edits: 0,
                });
                continue;
            }
            let fail = |e| at(plan.first, path, e);
            kinds::check_text_name(&t.name).map_err(fail)?;
            let stat = t.dir.entry(t.name()).map_err(fail)?.ok_or_else(|| {
                fail(ToolError::new(
                    ErrorCode::NotFound,
                    "this file doesn't exist",
                ))
            })?;
            Self::check_existing_file(&t, &stat).map_err(fail)?;
            let (bytes, stat) = t.dir.read(t.name(), cap).map_err(fail)?;
            Self::check_existing_file(&t, &stat).map_err(fail)?;
            if let Some(other) = ready.iter().find(|r| {
                r.old
                    .as_ref()
                    .is_some_and(|(s, _, _)| s.identity == stat.identity)
            }) {
                return Err(fail(ToolError::invalid_argument(format!(
                    "this is the same file as {}; give all its edits one path",
                    other.t.tool_path
                ))));
            }
            let old_sha = sha256(&bytes);
            if let Some(sha) = expected.get(&plan.path) {
                Self::check_sha(Some(sha), &old_sha).map_err(fail)?;
            }
            let old = kinds::as_text(bytes, &t.name).map_err(fail)?;
            let mut new = old.clone();
            for (index, old_text, new_text) in &plan.edits {
                let fail = |e| at(*index, path, e);
                kinds::check_text(new_text, cap).map_err(fail)?;
                new = apply_edit(&new, old_text, new_text).map_err(fail)?;
            }
            kinds::check_text(&new, cap).map_err(fail)?;
            ready.push(Ready {
                t,
                old: Some((stat, old, old_sha)),
                new,
                edits: plan.edits.len(),
            });
        }

        if req.dry_run {
            return Ok(dry_run(&ready));
        }
        let sizes: Vec<u64> = ready.iter().map(|r| r.new.len() as u64).collect();
        self.limiter.check_changes(&sizes)?;
        self.apply(&ready)
    }

    /// Make the checked changes: stage, swap, then trash (see the module's
    /// notes), putting everything back when a step fails.
    fn apply(&self, ready: &[Ready]) -> Result<PatchResult, ToolError> {
        // 1. Every new version beside its file.
        let mut staged = Vec::new();
        for (i, r) in ready.iter().enumerate() {
            let mode = match &r.old {
                Some((stat, _, _)) if stat.mode != 0 => stat.mode & 0o666,
                _ => new_file_mode(),
            };
            let like = r.old.is_some().then(|| r.t.name());
            match self
                .injected("stage", i)
                .and_then(|()| r.t.dir.stage(r.new.as_bytes(), mode, like))
            {
                Ok(s) => staged.push(s),
                Err(e) => {
                    for (s, r) in staged.into_iter().zip(ready) {
                        r.t.dir.discard(s);
                    }
                    return Err(failed(r, e, &[]));
                }
            }
        }

        // 2. Each new version in place, the previous one kept beside it.
        let mut applied: Vec<Applied> = Vec::new();
        let mut staged = staged.into_iter();
        for (i, r) in ready.iter().enumerate() {
            let s = staged.next().expect("one staged file per change");
            let done = match self.injected("swap", i) {
                Ok(()) => self.swap(r, s),
                Err(e) => Err((e, Some(s))),
            };
            match done {
                Ok(a) => applied.push(a),
                Err((e, rest)) => {
                    if let Some(s) = rest {
                        r.t.dir.discard(s);
                    }
                    // The files after it weren't touched: only their new
                    // versions go.
                    for (s, later) in staged.zip(&ready[i + 1..]) {
                        later.t.dir.discard(s);
                    }
                    let stuck = self.put_back(&ready[..i], &applied, 0);
                    return Err(failed(r, e, &stuck));
                }
            }
        }

        // 3. The previous versions to the trash.
        let mut files: Vec<PatchFile> = ready.iter().map(applied_file).collect();
        for (i, (r, a)) in ready.iter().zip(&applied).enumerate() {
            let (Applied::Swapped(aside, _), Some((stat, _, _))) = (a, &r.old) else {
                continue;
            };
            let trashed = self
                .injected("trash", i)
                .and_then(|()| r.t.dir.trash_as(aside, r.t.name(), &self.trash, stat));
            match trashed {
                Ok(went) => {
                    files[i].trashed_to = Some(went);
                    if let Some(previous) = &mut files[i].previous {
                        previous.in_trash = Some(true);
                    }
                }
                Err(e) => {
                    let stuck = self.put_back(ready, &applied, i);
                    return Err(failed(r, e, &stuck));
                }
            }
        }
        Ok(PatchResult {
            dry_run: false,
            effect: "patched",
            files,
        })
    }

    /// Put `r`'s new version, staged as `s`, in place: swapped with the
    /// file, or the file moved aside first. Returns how to undo it, or the
    /// error and the staged file when it is still there.
    fn swap(&self, r: &Ready, s: Staged) -> Result<Applied, (ToolError, Option<Staged>)> {
        let dir = &r.t.dir;
        let name = r.t.name();
        if r.old.is_none() {
            dir.commit(s, name).map_err(|e| (e, None))?;
            let identity = dir
                .entry(name)
                .ok()
                .flatten()
                .map(|e| e.identity)
                .unwrap_or_default();
            return Ok(Applied::Created(identity));
        }
        let aside = match self.exchange(dir, &s, name) {
            Ok(true) => s.name(),
            Ok(false) => {
                let aside = match set_aside(dir, name) {
                    Ok(aside) => aside,
                    Err(e) => return Err((e, Some(s))),
                };
                if let Err(e) = dir.commit(s, name) {
                    // The previous version back under its name.
                    let _ = dir.rename(&aside, dir, name);
                    return Err((e, None));
                }
                aside
            }
            Err(e) => return Err((e, Some(s))),
        };
        let identity = dir
            .entry(name)
            .ok()
            .flatten()
            .map(|e| e.identity)
            .unwrap_or_default();
        Ok(Applied::Swapped(aside, identity))
    }

    /// Undo `applied` (one per file of `ready`, in order): new files
    /// removed, edited files back to their previous version, from beside
    /// them, or from memory for the first `trashed` files, whose previous
    /// version already went to the trash. Returns the paths that couldn't
    /// be put back.
    fn put_back(&self, ready: &[Ready], applied: &[Applied], trashed: usize) -> Vec<String> {
        let mut stuck = Vec::new();
        for (i, (r, a)) in ready.iter().zip(applied).enumerate().rev() {
            let dir = &r.t.dir;
            let name = r.t.name();
            let ours = |identity: &(u64, u64)| {
                dir.entry(name)
                    .ok()
                    .flatten()
                    .is_some_and(|e| e.identity == *identity)
            };
            let restored = match a {
                Applied::Created(identity) => {
                    if ours(identity) {
                        dir.remove_own(name);
                        dir.entry(name).ok().flatten().is_none()
                    } else {
                        false
                    }
                }
                Applied::Swapped(aside, identity) if i >= trashed => {
                    ours(identity)
                        && match set_aside(dir, name) {
                            Ok(new) => match dir.rename(aside, dir, name) {
                                Ok(()) => {
                                    dir.remove_own(&new);
                                    true
                                }
                                Err(_) => {
                                    let _ = dir.rename(&new, dir, name);
                                    false
                                }
                            },
                            Err(_) => false,
                        }
                }
                Applied::Swapped(_, identity) => {
                    // The previous version is in the trash: write it again
                    // from what was read.
                    let Some((stat, old, _)) = &r.old else {
                        continue;
                    };
                    let mode = if stat.mode == 0 {
                        new_file_mode()
                    } else {
                        stat.mode & 0o666
                    };
                    ours(identity)
                        && match dir.stage(old.as_bytes(), mode, Some(name)) {
                            Ok(s) => match set_aside(dir, name) {
                                Ok(new) => match dir.commit(s, name) {
                                    Ok(()) => {
                                        dir.remove_own(&new);
                                        true
                                    }
                                    Err(_) => {
                                        let _ = dir.rename(&new, dir, name);
                                        false
                                    }
                                },
                                Err(_) => {
                                    dir.discard(s);
                                    false
                                }
                            },
                            Err(_) => false,
                        }
                }
            };
            if !restored {
                stuck.push(r.t.tool_path.clone());
            }
        }
        stuck.reverse();
        stuck
    }

    /// Swap `s` with the file `name` in one step, where the system can.
    fn exchange(&self, dir: &ChangeDir, s: &Staged, name: &OsStr) -> Result<bool, ToolError> {
        #[cfg(test)]
        if self.no_exchange.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }
        dir.exchange(s, name)
    }

    /// In tests, fail the patch at `step` of file `index` when asked to.
    fn injected(&self, step: &str, index: usize) -> Result<(), ToolError> {
        #[cfg(test)]
        if let Some((fail_step, fail_index)) = *self.fail_patch_at.lock().expect("test hook")
            && fail_step == step
            && fail_index == index
        {
            return Err(ToolError::internal(format!("injected failure at {step}")));
        }
        let _ = (step, index);
        Ok(())
    }
}

/// Move `name` in `dir` aside, to a fresh hidden name. Returns that name.
fn set_aside(dir: &ChangeDir, name: &OsStr) -> Result<OsString, ToolError> {
    for _ in 0..16 {
        let aside = OsString::from(temp_name());
        match dir.rename(name, dir, &aside) {
            Ok(()) => return Ok(aside),
            Err(RenameError::Exists) => continue,
            Err(RenameError::CrossDevice) => {
                return Err(ToolError::internal("the file moved to another drive"));
            }
            Err(RenameError::Other(e)) => return Err(e),
        }
    }
    Err(ToolError::internal("no free name for a temporary file"))
}

/// The error for a patch that failed at `r` while being made, saying
/// whether every file got its previous version back.
fn failed(r: &Ready, e: ToolError, stuck: &[String]) -> ToolError {
    let mut message = format!("{}: {}", r.t.tool_path, e.message);
    if !message.ends_with('.') {
        message.push('.');
    }
    if stuck.is_empty() {
        message.push_str(
            " Every file the patch had changed was put back as it was, so nothing was changed.",
        );
    } else {
        message.push_str(&format!(
            " The other files were put back as they were, except {}: its previous version is \
             beside it under a hidden name, or in the system trash.",
            stuck.join(", ")
        ));
    }
    ToolError::new(e.code, message)
}

/// What a dry run answers: each file's diff, cut together at
/// [`MAX_PATCH_DIFF_BYTES`].
fn dry_run(ready: &[Ready]) -> PatchResult {
    let mut left = MAX_PATCH_DIFF_BYTES;
    let mut cut = false;
    let files = ready
        .iter()
        .map(|r| {
            let old = r.old.as_ref().map_or("", |(_, text, _)| text.as_str());
            let (diff, diff_cut) = if cut {
                (Value::Null, true)
            } else {
                let mut diff = full_diff(&r.t.tool_path, old, &r.new);
                if super::cut_diff(&mut diff, left, "\n… (the diffs go on)\n") {
                    cut = true;
                }
                left = left.saturating_sub(diff.len());
                if left == 0 {
                    cut = true;
                }
                (Value::String(diff), false)
            };
            match &r.old {
                Some((stat, _, sha)) => PatchFile {
                    path: r.t.tool_path.clone(),
                    effect: Effect::Edited,
                    edits: Some(r.edits),
                    diff: Some(diff),
                    diff_cut,
                    size: None,
                    sha256: None,
                    previous: Some(PatchPrevious {
                        size: stat.size,
                        modified: format_time(stat.modified),
                        sha256: sha.clone(),
                        in_trash: None,
                    }),
                    written: 0,
                    trashed_to: None,
                },
                None => PatchFile {
                    path: r.t.tool_path.clone(),
                    effect: Effect::Created,
                    edits: None,
                    diff: Some(diff),
                    diff_cut,
                    size: Some(r.new.len() as u64),
                    sha256: None,
                    previous: None,
                    written: 0,
                    trashed_to: None,
                },
            }
        })
        .collect();
    PatchResult {
        dry_run: true,
        effect: "patched",
        files,
    }
}

/// What a made patch answers for `r`.
fn applied_file(r: &Ready) -> PatchFile {
    PatchFile {
        path: r.t.tool_path.clone(),
        effect: if r.old.is_some() {
            Effect::Edited
        } else {
            Effect::Created
        },
        edits: None,
        diff: None,
        diff_cut: false,
        size: Some(r.new.len() as u64),
        sha256: Some(sha256(r.new.as_bytes())),
        previous: r.old.as_ref().map(|(stat, _, sha)| PatchPrevious {
            size: stat.size,
            modified: format_time(stat.modified),
            sha256: sha.clone(),
            in_trash: Some(false),
        }),
        written: r.new.len() as u64,
        trashed_to: None,
    }
}
