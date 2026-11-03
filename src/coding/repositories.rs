//! Repositories coding bots work on: a bare mirror in Desktop's data folder,
//! fetched with the person's own Git credentials, and a checkout in a shared
//! folder whose `origin` is the mirror (docs/coding-computers.md).
//!
//! `git` runs outside a sandbox only here, only on a mirror, whose config
//! only Desktop writes, with hooks off. Never in a shared folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

use crate::config::Repository;
use crate::paths::Paths;

/// A mirror fetched more recently than this isn't fetched again before a run.
pub const FRESH: Duration = Duration::from_secs(60);
/// Longest a fetch may take.
pub const FETCH_LIMIT: Duration = Duration::from_secs(60);
/// The file whose time says when the mirror was last fetched.
const STAMP: &str = "cww-fetched";
/// Where the last fetch's failure is kept, for `cww coding status`.
const FAILURE: &str = "cww-fetch-failed";

pub fn mirrors_dir(paths: &Paths) -> PathBuf {
    paths.data_dir.join("repositories")
}

pub fn mirror_dir(paths: &Paths, repo: &Repository) -> PathBuf {
    mirrors_dir(paths)
        .join(&repo.root)
        .join(format!("{}.git", repo.name))
}

/// The checkout's name for an address: its last part, without `.git`.
pub fn name_from_url(url: &str) -> Option<String> {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .next()?
        .trim_end_matches(".git");
    valid_name(last).then(|| last.to_string())
}

/// A checkout's name: one plain folder name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Refuse an address Desktop shouldn't keep: one with a password or token
/// in it, one that isn't a remote, or one Git would treat as an option.
pub fn check_url(url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_whitespace) {
        bail!("that isn't a repository's address");
    }
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.contains('@') {
            bail!(
                "the address has a user name or token in it. Use one without (an SSH address, or \
                 https with a credential helper), so no token is kept in Desktop's settings"
            );
        }
        return Ok(());
    }
    if url.starts_with("ssh://") || url.starts_with("git@") || url.contains(':') {
        if url.starts_with("ext::") || url.starts_with("fd::") {
            bail!("that kind of address runs a program; use an SSH or https address");
        }
        // A user with a password (`ssh://user:secret@host/`, or
        // `user:secret@host:path` in scp's form): never kept.
        let rest = url.strip_prefix("ssh://").unwrap_or(url);
        let authority = rest.split('/').next().unwrap_or_default();
        if let Some((user, _)) = authority.split_once('@')
            && user.contains(':')
        {
            bail!(
                "the address has a password or token in it. Use an SSH key instead, so no \
                 secret is kept in Desktop's settings"
            );
        }
        return Ok(());
    }
    bail!("use the repository's SSH or https address, like git@github.com:acme/app.git")
}

/// A `git` that ignores hooks and never asks for anything.
fn git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    command
}

/// `git` with `repo`'s deploy key, if it has one: only that key, never the
/// agent's.
fn git_for(repo: Option<&Repository>) -> Command {
    let mut command = git();
    let quote = |path: &str| format!("'{}'", path.replace('\\', "/").replace('\'', "'\\''"));
    // The person's own ~/.ssh/config, never the system's: inside the
    // background service's user namespace, root's files under /etc/ssh look
    // foreign, and ssh refuses to start ("Bad owner or permissions").
    let config = if cfg!(unix) {
        let own = crate::paths::home_dir()
            .ok()
            .map(|home| home.join(".ssh").join("config"))
            .filter(|config| config.is_file());
        Some(own.map_or_else(|| "/dev/null".to_string(), |c| quote(&c.to_string_lossy())))
    } else {
        None
    };
    let mut ssh = String::from("ssh");
    if let Some(config) = &config {
        ssh.push_str(&format!(" -F {config}"));
    }
    if let Some(key) = repo.and_then(|r| r.ssh_key.as_ref()) {
        ssh.push_str(&format!(
            " -i {} -o IdentitiesOnly=yes -o IdentityAgent=none",
            quote(&key.to_string_lossy())
        ));
    }
    if ssh != "ssh" {
        command.env("GIT_SSH_COMMAND", format!("{ssh} -o BatchMode=yes"));
    }
    command
}

/// Refuse a deploy key that isn't a private key file outside the shared
/// folders.
pub fn check_ssh_key(key: &Path, roots: &[crate::config::Root]) -> Result<()> {
    if !key.is_absolute() || !key.is_file() {
        bail!("{} isn't a key file", key.display());
    }
    let real = key.canonicalize()?;
    if roots.iter().any(|r| real.starts_with(&r.path)) {
        bail!("keep the deploy key outside the shared folders, where no command can read it");
    }
    Ok(())
}

/// Run `command`, giving up after `limit`. The error is Git's last line.
fn run(mut command: Command, limit: Duration) -> Result<()> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't start git; is it installed?")?;
    let stderr = child.stderr.take();
    // A helper git started can hold stderr open after git ends: wait for
    // what it said only a moment.
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut text);
        }
        let _ = said.send(text);
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git took longer than {} seconds", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = heard
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    if !status.success() {
        let last = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("git failed");
        bail!("{}", last.trim());
    }
    Ok(())
}

/// Hooks off: an empty folder of Desktop's own.
fn no_hooks(mirror: &Path) -> String {
    format!("core.hooksPath={}", mirror.join("cww-no-hooks").display())
}

/// Make the mirror of `repo`, as the person, with their credentials.
pub fn create_mirror(paths: &Paths, repo: &Repository) -> Result<PathBuf> {
    check_url(&repo.url)?;
    let mirror = mirror_dir(paths, repo);
    if mirror.join("HEAD").is_file() {
        let mut origin = git();
        origin
            .arg("--git-dir")
            .arg(&mirror)
            .args(["config", "--get", "remote.origin.url"]);
        let found = origin
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if found != repo.url {
            bail!(
                "Desktop already mirrors another repository as {} in {}: choose another name",
                repo.name,
                repo.root
            );
        }
        return Ok(mirror);
    }
    std::fs::create_dir_all(mirror.parent().unwrap_or(&mirror))?;
    let mut clone = git_for(Some(repo));
    clone
        .args(["clone", "--bare", "--quiet", "--"])
        .arg(&repo.url)
        .arg(&mirror);
    run(clone, Duration::from_secs(1800)).context("cloning the repository")?;
    std::fs::create_dir_all(mirror.join("cww-no-hooks"))?;
    // Branches and tags, not pull requests' refs.
    let mut refspec = git();
    refspec.arg("--git-dir").arg(&mirror).args([
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/heads/*",
    ]);
    run(refspec, Duration::from_secs(10))?;
    stamp(&mirror);
    Ok(mirror)
}

/// Make the checkout `name` in `folder` from `mirror`: its `origin` is the
/// mirror, so fetching needs no network or credentials, and a push fails.
pub fn create_checkout(folder: &Path, name: &str, mirror: &Path) -> Result<PathBuf> {
    let checkout = folder.join(name);
    let git_dir = checkout.join(".git");
    let config = git_dir.join("config");
    // Each part as it is, never through a link a command could have made to
    // send the write below somewhere else.
    let plain = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink());
    if [&checkout, &git_dir, &config]
        .iter()
        .any(|p| p.exists() && !plain(p))
    {
        bail!(
            "{} is a link, or holds one where its Git settings are: choose another name",
            checkout.display()
        );
    }
    if config.is_file() {
        // A checkout from before: its origin becomes the mirror. Only
        // `git config --file`, which reads and writes that one file and
        // runs nothing it says, since a command may have written it.
        let mut origin = git();
        origin
            .arg("config")
            .arg("--file")
            .arg(&config)
            .args(["remote.origin.url"])
            .arg(mirror);
        run(origin, Duration::from_secs(10)).context("pointing the checkout at the mirror")?;
        // And nowhere else to push to.
        let mut push = git();
        push.arg("config")
            .arg("--file")
            .arg(&config)
            .args(["--unset-all", "remote.origin.pushurl"]);
        match push.stdout(Stdio::null()).stderr(Stdio::null()).status() {
            // 5: there was none.
            Ok(s) if s.success() || s.code() == Some(5) => {}
            _ => bail!("couldn't clear the checkout's push address"),
        }
        return Ok(checkout);
    }
    if checkout.exists() {
        bail!(
            "{} is already there and isn't a checkout: choose another name or move it",
            checkout.display()
        );
    }
    let mut clone = git();
    clone
        .args(["clone", "--quiet", "--no-hardlinks", "--"])
        .arg(mirror)
        .arg(&checkout);
    run(clone, Duration::from_secs(1800)).context("making the checkout")?;
    Ok(checkout)
}

fn stamp(mirror: &Path) {
    let _ = std::fs::write(mirror.join(STAMP), b"");
    let _ = std::fs::remove_file(mirror.join(FAILURE));
}

/// When the mirror was last fetched.
pub fn fetched_at(mirror: &Path) -> Option<SystemTime> {
    std::fs::metadata(mirror.join(STAMP))
        .and_then(|m| m.modified())
        .ok()
}

/// Why the last fetch failed, if it did.
pub fn last_failure(mirror: &Path) -> Option<String> {
    std::fs::read_to_string(mirror.join(FAILURE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Fetch the mirror, as the person. Only a mirror in Desktop's data folder.
pub fn fetch(mirror: &Path, repo: Option<&Repository>) -> Result<()> {
    let mut fetch = git_for(repo);
    fetch
        .arg("--git-dir")
        .arg(mirror)
        .arg("-c")
        .arg(no_hooks(mirror))
        .args([
            "fetch",
            "--prune",
            "--quiet",
            "--no-write-fetch-head",
            "origin",
        ]);
    match run(fetch, FETCH_LIMIT) {
        Ok(()) => {
            stamp(mirror);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::write(mirror.join(FAILURE), format!("{e:#}"));
            Err(e)
        }
    }
}

/// Fetch the mirror unless it was fetched within [`FRESH`]. A failure
/// doesn't stop the run; it's kept for `cww coding status`.
pub fn fetch_if_stale(mirror: &Path, repo: Option<&Repository>) {
    let fresh = fetched_at(mirror)
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < FRESH);
    if !fresh && let Err(e) = fetch(mirror, repo) {
        tracing::info!(error = %e, "fetching a repository's mirror failed");
    }
}
