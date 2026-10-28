//! Adding and removing shared roots.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{BotLoan, Config, MAX_READ_BUDGET, Root};
use crate::paths::{Paths, home_dir};
use crate::policy::DenyList;
use crate::reader::safe_fs::is_valid_root_id;

/// System directories that can't be shared, nor anything inside them,
/// without `--i-know`.
#[cfg(not(windows))]
const SYSTEM_DIRS: &[&str] = &[
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/opt",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/nix",
    "/snap",
    "/Applications",
    "/Library",
    "/System",
    "/cores",
    "/private",
    "/Users",
    "/home",
];

/// Shared scratch and mount directories. The directory itself is refused
/// (it holds other programs' and users' files), but a folder inside it is
/// fine: `/tmp/project` is as narrow as `~/project`. These take precedence
/// over `SYSTEM_DIRS`, so `/var/tmp/x` and macOS's `/private/tmp/x` (what
/// `/tmp/x` canonicalizes to) are allowed too.
#[cfg(not(windows))]
const SCRATCH_DIRS: &[&str] = &[
    "/tmp",
    "/var/tmp",
    "/private/tmp",
    "/private/var/tmp",
    "/private/var/folders",
    "/mnt",
    "/media",
    "/Volumes",
];

pub struct NewRoot<'a> {
    pub path: &'a Path,
    pub label: Option<String>,
    pub i_know: bool,
    pub follow_symlinks: bool,
    /// Allow changes in it from the start.
    pub writable: bool,
}

/// Validate and add a root to `config`. Returns the new root.
pub fn add_root(config: &mut Config, paths: &Paths, new: NewRoot<'_>) -> Result<Root> {
    let path = canonical(new.path)?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    if let Some(reason) = too_broad(&path)?
        && !new.i_know
    {
        bail!(
            "refusing to share {}: {reason}. Share a narrower folder, or pass --i-know if you \
             really mean it",
            path.display()
        );
    }
    let deny = DenyList::from_config(&config.deny, paths)?;
    if let Some(pattern) = deny.denied_by(&path) {
        bail!(
            "{} matches the deny list ({pattern}); edit [deny] in {} to change that",
            path.display(),
            paths.config_file().display()
        );
    }
    for dir in paths.all_dirs() {
        if dir.starts_with(&path) && dir.exists() {
            eprintln!(
                "note: {} is inside this root; cww always hides its own files",
                dir.display()
            );
        }
    }
    if let Some(existing) = config.roots.iter().find(|r| r.path == path) {
        bail!("{} is already shared as {}", path.display(), existing.id);
    }
    let label = new
        .label
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        });
    if label.chars().count() > 80 {
        bail!("the label is longer than 80 characters");
    }
    let root = Root {
        bots: Vec::new(),
        id: new_id(config, &label),
        label,
        path,
        follow_symlinks: new.follow_symlinks,
        writable: new.writable,
        commands: false,
        network: false,
        read_budget: None,
    };
    config.roots.push(root.clone());
    Ok(root)
}

/// Allow or stop changes in a shared folder, by ID, label or path. Only the
/// user does this, here or through the control channel; the server can't.
/// Changes in a folder too broad to share without `--i-know` (the home
/// folder, one that holds it, a whole drive, a system folder) need
/// `i_know` too, even when the folder was shared that way: there, every
/// program's settings and startup files are in reach.
pub fn set_writable(
    config: &mut Config,
    which: &str,
    writable: bool,
    i_know: bool,
) -> Result<Root> {
    let index = find_root(config, which)?;
    let root = &mut config.roots[index];
    if writable
        && !i_know
        && let Some(reason) = too_broad(&root.path)?
    {
        bail!(
            "refusing to allow changes in {}: {reason}, where changes could reach every \
             program's settings. Allow changes in a narrower folder, or pass --i-know if you \
             really mean it",
            root.path.display()
        );
    }
    root.writable = writable;
    if !writable {
        // Commands change files too: they stop with changes.
        root.commands = false;
        root.network = false;
        root.bots
            .iter_mut()
            .for_each(|loan| loan.unattended = false);
    }
    Ok(root.clone())
}

/// Allow commands in a shared folder (`run`), or stop them, and say
/// whether they may use the network. Only where changes are allowed: a
/// command can change anything in its folder.
pub fn set_commands(
    config: &mut Config,
    which: &str,
    commands: bool,
    network: bool,
) -> Result<Root> {
    let index = find_root(config, which)?;
    let root = &mut config.roots[index];
    if commands && !root.writable {
        bail!(
            "{} doesn't allow changes. Allow changes there first (`cww roots allow-changes {}`): \
             a command can change anything in its folder",
            root.label,
            root.id
        );
    }
    // Windows sandboxes commands only from Windows 11 24H2 with the August
    // 2026 update: say so now rather than at the first command.
    #[cfg(windows)]
    if commands && let Err(e) = crate::commands::windows::supported() {
        bail!("{}", e.message);
    }
    root.commands = commands;
    root.network = commands && network;
    if !commands {
        // No command runs there now, so none runs unattended either.
        root.bots
            .iter_mut()
            .for_each(|loan| loan.unattended = false);
    }
    Ok(root.clone())
}

/// Lend a shared folder to a Chat with Work bot (`lent`), or stop lending it.
/// `unattended` lets the bot's commands there start without anyone
/// approving each one, only where commands are allowed. Lending again
/// replaces the loan, so a name or `unattended` changes in place.
pub fn set_bot(
    config: &mut Config,
    which: &str,
    bot: &str,
    name: Option<&str>,
    lent: bool,
    unattended: bool,
) -> Result<Root> {
    if bot.is_empty() || bot.len() > 20 || !bot.bytes().all(|b| b.is_ascii_digit()) {
        bail!(
            "{bot:?} isn't a bot's ID: it's the number in the bot's address in Chat with Work, like 42"
        );
    }
    let index = find_root(config, which)?;
    let root = &mut config.roots[index];
    if lent && unattended && !root.runs_commands() {
        bail!(
            "{} doesn't allow commands. Allow them there first (`cww roots allow-commands {}`)",
            root.label,
            root.id
        );
    }
    let name: String = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(80).collect())
        .or_else(|| root.loan(bot).map(|loan| loan.name.clone()))
        .unwrap_or_default();
    root.bots.retain(|loan| loan.id != bot);
    if lent {
        root.bots.push(BotLoan {
            id: bot.to_string(),
            name,
            unattended,
        });
    }
    Ok(root.clone())
}

/// Give a shared folder a read budget of its own (`Some`: characters a
/// chat may read from it per rolling hour, all chats together twice that),
/// or put it back on the budget every folder shares (`None`). Only the
/// user does this, here or through the control channel; the server can't.
pub fn set_read_budget(config: &mut Config, which: &str, budget: Option<usize>) -> Result<Root> {
    let least = config.limits.read_chars_per_chat_hour.min(MAX_READ_BUDGET);
    if let Some(budget) = budget
        && !(least..=MAX_READ_BUDGET).contains(&budget)
    {
        bail!(
            "a folder's read budget is from {least} characters an hour (what every folder \
             has) to {MAX_READ_BUDGET}"
        );
    }
    let index = find_root(config, which)?;
    let root = &mut config.roots[index];
    root.read_budget = budget;
    Ok(root.clone())
}

/// The canonical form of a folder the user named. On Windows, the `\\?\`
/// prefix that `canonicalize` adds to ordinary drive paths is dropped, so
/// the config stays readable and comparable.
pub fn canonical(path: &Path) -> Result<PathBuf> {
    let real = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    #[cfg(windows)]
    if let Some(rest) = real.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return Ok(PathBuf::from(rest));
    }
    Ok(real)
}

/// Remove a root by ID, label or path.
pub fn remove_root(config: &mut Config, which: &str) -> Result<Root> {
    let index = find_root(config, which)?;
    Ok(config.roots.remove(index))
}

/// The index of the root `which` names: its ID, its path, or a label only
/// one root has.
fn find_root(config: &Config, which: &str) -> Result<usize> {
    let as_path = canonical(Path::new(which)).ok();
    config
        .roots
        .iter()
        .position(|r| r.id == which || as_path.as_ref() == Some(&r.path))
        .or_else(|| {
            let matches: Vec<usize> = config
                .roots
                .iter()
                .enumerate()
                .filter(|(_, r)| r.label == which)
                .map(|(i, _)| i)
                .collect();
            (matches.len() == 1).then(|| matches[0])
        })
        .with_context(|| format!("no shared folder matches {which:?}; see `cww roots list`"))
}

/// Why a path is too broad to share by default, if it is.
#[cfg(windows)]
fn too_broad(path: &Path) -> Result<Option<String>> {
    // Reference folders can come as 8.3 short names (`C:\Users\RUNNER~1`),
    // so compare canonical forms.
    let lower = |p: &Path| -> Vec<String> {
        canonical(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let target = lower(path);
    if path.parent().is_none() || target.len() <= 2 {
        return Ok(Some("it is a whole drive".into()));
    }
    let home = home_dir()?;
    let home_parts = lower(&home);
    if target == home_parts {
        return Ok(Some("it is your whole home folder".into()));
    }
    if home_parts.starts_with(&target) {
        return Ok(Some("it contains your home folder".into()));
    }
    // Like /tmp on Unix: the temporary folder itself is refused, a folder
    // inside it is fine.
    let temp = lower(&std::env::temp_dir());
    if target == temp {
        return Ok(Some(
            "it is the temporary folder; share a folder inside it".into(),
        ));
    }
    if target.starts_with(&temp) {
        return Ok(None);
    }
    let mut system: Vec<PathBuf> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .collect();
    // Application data holds tokens, cookies and keys for every program.
    system.push(home.join("AppData"));
    for dir in system {
        let parts = lower(&dir);
        if target.starts_with(&parts) {
            return Ok(Some(format!(
                "it is a system or application directory ({})",
                dir.display()
            )));
        }
    }
    Ok(None)
}

/// Why a path is too broad to share by default, if it is.
#[cfg(not(windows))]
fn too_broad(path: &Path) -> Result<Option<String>> {
    if path == Path::new("/") {
        return Ok(Some("it is the whole filesystem".into()));
    }
    let home = home_dir()?;
    let home = home.canonicalize().unwrap_or(home);
    if path == home {
        return Ok(Some("it is your whole home folder".into()));
    }
    if home.starts_with(path) {
        return Ok(Some("it contains your home folder".into()));
    }
    // This session's own temporary directory ($TMPDIR) counts as scratch
    // too, wherever it lives (Nix builds keep it under /nix), unless it is
    // something broad like / or the home folder.
    if let Ok(temp) = std::env::temp_dir().canonicalize()
        && temp.components().count() > 2
        && !home.starts_with(&temp)
    {
        if path == temp {
            return Ok(Some(
                "it is the temporary folder; share a folder inside it".into(),
            ));
        }
        if path.starts_with(&temp) {
            return Ok(None);
        }
    }
    for dir in SCRATCH_DIRS.iter().map(Path::new) {
        if path == dir {
            return Ok(Some(format!(
                "it is a shared system directory ({}); share a folder inside it",
                dir.display()
            )));
        }
        if path.starts_with(dir) {
            return Ok(None);
        }
    }
    for dir in SYSTEM_DIRS.iter().map(Path::new) {
        if path == dir || (path.starts_with(dir) && !path.starts_with(&home)) {
            return Ok(Some(format!(
                "it is a system directory ({})",
                dir.display()
            )));
        }
    }
    Ok(None)
}

/// A short readable ID derived from the label, made unique.
fn new_id(config: &Config, label: &str) -> String {
    let mut base: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    base.truncate(24);
    let base = base.trim_matches('-').to_string();
    let base = if is_valid_root_id(&base) {
        base
    } else {
        "root".to_string()
    };
    if !config.roots.iter().any(|r| r.id == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !config.roots.iter().any(|r| &r.id == id))
        .expect("an unused ID exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_readable_and_unique() {
        let mut config = Config::default();
        assert_eq!(new_id(&config, "Work Docs!"), "work-docs");
        config.roots.push(Root {
            bots: Vec::new(),
            id: "work-docs".into(),
            label: "x".into(),
            path: "/x".into(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        });
        assert_eq!(new_id(&config, "Work docs"), "work-docs-2");
        assert_eq!(new_id(&config, "文档"), "root");
        assert_eq!(new_id(&config, "A"), "root");
    }

    #[test]
    fn sets_a_folders_read_budget_within_its_bounds() {
        let mut config = Config::default();
        config.roots.push(Root {
            bots: Vec::new(),
            id: "app".into(),
            label: "App".into(),
            path: "/x".into(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        });
        let root = set_read_budget(&mut config, "app", Some(5_000_000)).unwrap();
        assert_eq!(root.read_budget, Some(5_000_000));
        assert_eq!(config.roots[0].read_budget(&config.limits), Some(5_000_000));
        for wrong in [999_999, 20_000_001] {
            let err = set_read_budget(&mut config, "app", Some(wrong)).unwrap_err();
            assert!(err.to_string().contains("1000000"), "{err}");
        }
        assert_eq!(config.roots[0].read_budget, Some(5_000_000));
        let root = set_read_budget(&mut config, "App", None).unwrap();
        assert_eq!(root.read_budget, None);
        assert!(set_read_budget(&mut config, "nope", None).is_err());
        // A budget written by hand out of bounds counts within them.
        config.roots[0].read_budget = Some(10);
        assert_eq!(config.roots[0].read_budget(&config.limits), Some(1_000_000));
        config.roots[0].read_budget = Some(usize::MAX);
        assert_eq!(
            config.roots[0].read_budget(&config.limits),
            Some(20_000_000)
        );
    }

    #[test]
    fn lends_a_folder_to_a_bot_and_stops() {
        let mut config = Config::default();
        config.roots.push(Root {
            bots: Vec::new(),
            id: "app".into(),
            label: "App".into(),
            path: "/x".into(),
            follow_symlinks: false,
            writable: true,
            commands: false,
            network: false,
            read_budget: None,
        });

        let root = set_bot(&mut config, "app", "42", Some("Issue fixer"), true, false).unwrap();
        assert_eq!(root.loan("42").unwrap().name, "Issue fixer");
        assert!(!root.loan("42").unwrap().unattended);
        assert!(
            set_bot(&mut config, "app", "42", None, true, true).is_err(),
            "unattended only where commands are allowed"
        );
        assert!(set_bot(&mut config, "app", "issue-fixer", None, true, false).is_err());

        // As set_commands would.
        config.roots[0].commands = true;
        let root = set_bot(&mut config, "app", "42", None, true, true).unwrap();
        assert_eq!(root.bots.len(), 1, "lending again replaces the loan");
        assert_eq!(
            root.loan("42").unwrap().name,
            "Issue fixer",
            "keeps its name"
        );
        assert!(root.loan("42").unwrap().unattended);

        set_commands(&mut config, "app", false, false).unwrap();
        assert!(
            !config.roots[0].loan("42").unwrap().unattended,
            "stopping commands stops unattended ones"
        );

        let root = set_bot(&mut config, "app", "42", None, false, false).unwrap();
        assert!(root.loan("42").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn refuses_broad_windows_roots() {
        assert!(too_broad(Path::new(r"C:\")).unwrap().is_some());
        assert!(
            too_broad(Path::new(r"C:\Windows\System32"))
                .unwrap()
                .is_some()
        );
        assert!(
            too_broad(Path::new(r"c:\program files\app"))
                .unwrap()
                .is_some()
        );
        let home = home_dir().unwrap();
        assert!(too_broad(&home).unwrap().is_some());
        assert!(
            too_broad(&home.join("AppData/Roaming/x"))
                .unwrap()
                .is_some()
        );
        assert!(too_broad(&home.join("Documents")).unwrap().is_none());
        assert!(too_broad(&std::env::temp_dir()).unwrap().is_some());
        assert!(
            too_broad(&std::env::temp_dir().join("x"))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn refuses_broad_roots() {
        assert!(too_broad(Path::new("/")).unwrap().is_some());
        assert!(too_broad(Path::new("/etc")).unwrap().is_some());
        assert!(too_broad(Path::new("/usr/share")).unwrap().is_some());
        // Build sandboxes can set HOME to a folder that doesn't exist.
        if let Ok(home) = home_dir().unwrap().canonicalize() {
            assert!(too_broad(&home).unwrap().is_some());
            assert!(too_broad(&home.join("Documents")).unwrap().is_none());
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn scratch_dirs_allow_subfolders_only() {
        for refused in ["/tmp", "/var/tmp", "/private/tmp", "/mnt", "/Volumes"] {
            assert!(
                too_broad(Path::new(refused)).unwrap().is_some(),
                "{refused}"
            );
        }
        for allowed in [
            "/tmp/project",
            "/tmp/a/b",
            "/var/tmp/export",
            "/private/tmp/project",
            "/private/var/folders/xy/abc/T/docs",
            "/mnt/usb/Reports",
            "/Volumes/Backup/Work",
        ] {
            assert!(
                too_broad(Path::new(allowed)).unwrap().is_none(),
                "{allowed}"
            );
        }
        // Scratch exceptions don't open up the rest of /var or /private.
        assert!(too_broad(Path::new("/var/lib/secrets")).unwrap().is_some());
        assert!(too_broad(Path::new("/private/etc")).unwrap().is_some());
        assert!(too_broad(Path::new("/tmpfoo")).unwrap().is_none());
    }

    #[test]
    fn adds_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(&tmp.path().join("cww"));
        let dir = tmp.path().join("Shared Docs");
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config::default();
        let new = |i_know| NewRoot {
            path: &dir,
            label: None,
            i_know,
            follow_symlinks: false,
            writable: false,
        };
        // A folder inside a temporary directory needs no --i-know.
        let root = add_root(&mut config, &paths, new(false)).unwrap();
        assert_eq!(root.id, "shared-docs");
        assert_eq!(root.label, "Shared Docs");
        assert!(!root.writable, "read-only unless asked");
        assert!(
            set_writable(&mut config, "shared-docs", true, false)
                .unwrap()
                .writable
        );
        assert!(config.roots[0].writable);
        assert!(
            !set_writable(&mut config, "Shared Docs", false, false)
                .unwrap()
                .writable
        );
        assert!(set_writable(&mut config, "nope", true, false).is_err());
        assert!(
            add_root(&mut config, &paths, new(true)).is_err(),
            "duplicate"
        );
        assert_eq!(
            remove_root(&mut config, "Shared Docs").unwrap().id,
            "shared-docs"
        );
        assert!(config.roots.is_empty());
    }

    /// Sharing the home folder takes --i-know; allowing changes there takes
    /// it again, even though it is already shared.
    #[test]
    fn changes_in_the_home_folder_need_i_know() {
        let home = home_dir().unwrap();
        let home = home.canonicalize().unwrap_or(home);
        let mut config = Config::default();
        config.roots.push(Root {
            bots: Vec::new(),
            id: "home".into(),
            label: "Home".into(),
            path: home,
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        });
        let err = set_writable(&mut config, "home", true, false).unwrap_err();
        assert!(err.to_string().contains("--i-know"), "{err}");
        assert!(!config.roots[0].writable);
        assert!(
            set_writable(&mut config, "home", true, true)
                .unwrap()
                .writable
        );
        // Stopping changes never needs it.
        assert!(
            !set_writable(&mut config, "home", false, false)
                .unwrap()
                .writable
        );
    }

    #[test]
    fn refuses_denied_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(&tmp.path().join("cww"));
        let dir = tmp.path().join(".ssh");
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config::default();
        let err = add_root(
            &mut config,
            &paths,
            NewRoot {
                path: &dir,
                label: None,
                i_know: true,
                follow_symlinks: false,
                writable: false,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("deny list"), "{err}");
    }
}
