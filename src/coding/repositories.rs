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
    if url.starts_with("ext::") || url.starts_with("fd::") {
        bail!("that kind of address runs a program; use an SSH or https address");
    }
    let secret = || {
        anyhow::anyhow!(
            "the address has a user name, password or token in it. Use one without (an SSH \
             key, or https with a credential helper), so no secret is kept in Desktop's settings"
        )
    };
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority = rest.split('/').next().unwrap_or_default();
        let user = authority.rsplit_once('@').map(|(user, _)| user);
        return match scheme {
            // A user at all, on the web: that's a token.
            "https" | "http" if user.is_some() => Err(secret()),
            "https" | "http" => Ok(()),
            // `git@host` is fine; `user:secret@host` never is.
            "ssh" | "git+ssh" | "ssh+git" if user.is_some_and(|u| u.contains(':')) => Err(secret()),
            "ssh" | "git+ssh" | "ssh+git" | "git" | "file" => Ok(()),
            _ => bail!("use the repository's SSH or https address"),
        };
    }
    // scp's form, `user@host:path`: a user with a password before the host.
    if let Some((before, _)) = url.split_once(':')
        && before.contains('@')
    {
        if before
            .split_once('@')
            .is_some_and(|(user, _)| user.contains(':'))
        {
            return Err(secret());
        }
        return Ok(());
    }
    if let Some((user, rest)) = url.split_once('@')
        && user.contains(':')
        && rest.contains(':')
    {
        return Err(secret());
    }
    if url.contains(':') && !url.starts_with('/') {
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
fn git_for(repo: Option<&Repository>, mirror: &Path) -> Command {
    let mut command = git();
    let quote = |path: &str| format!("'{}'", path.replace('\\', "/").replace('\'', "'\\''"));
    let key = repo.and_then(|r| r.ssh_key.as_ref());
    let config = match key {
        // With a deploy key, an empty config of Desktop's own, on every
        // system: a real one's `IdentityFile` lines would be offered beside
        // the key, `IdentitiesOnly` notwithstanding.
        Some(_) => {
            let empty = mirror
                .parent()
                .unwrap_or(mirror)
                .join("cww-empty-ssh-config");
            let _ = std::fs::create_dir_all(empty.parent().unwrap_or(&empty));
            let _ = std::fs::write(&empty, b"");
            Some(quote(&empty.to_string_lossy()))
        }
        // The person's own ~/.ssh/config, never the system's: inside the
        // background service's user namespace, root's files under /etc/ssh
        // look foreign, and ssh refuses to start ("Bad owner or
        // permissions").
        None if cfg!(unix) => {
            let own = crate::paths::home_dir()
                .ok()
                .map(|home| home.join(".ssh").join("config"))
                .filter(|config| config.is_file());
            Some(own.map_or_else(|| "/dev/null".to_string(), |c| quote(&c.to_string_lossy())))
        }
        None => None,
    };
    let mut ssh = String::from("ssh");
    if let Some(config) = &config {
        ssh.push_str(&format!(" -F {config}"));
    }
    if let Some(key) = key {
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
    let real = crate::roots::canonical(key)?;
    if roots.iter().any(|r| real.starts_with(&r.path)) {
        bail!("keep the deploy key outside the shared folders, where no command can read it");
    }
    Ok(())
}

/// Run `command`, giving up after `limit`. The error is Git's last line.
fn run(command: Command, limit: Duration) -> Result<()> {
    run_until(command, limit, &|| false)
}

/// As [`run`], ended early (and the process killed) once `stop` says so.
fn run_until(mut command: Command, limit: Duration, stop: &dyn Fn() -> bool) -> Result<()> {
    // A group of its own, so ending it ends the ssh or https helper it
    // started too, which would otherwise hold its stderr open.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
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
        if stop() {
            end_all(&mut child);
            bail!("stopped");
        }
        if Instant::now() >= deadline {
            end_all(&mut child);
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

/// End git and everything it started.
fn end_all(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = rustix::process::Pid::from_child(child);
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    // Windows has no process groups to signal: end the tree by its root.
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Hooks off: an empty folder of Desktop's own.
fn no_hooks(mirror: &Path) -> String {
    format!("core.hooksPath={}", mirror.join("cww-no-hooks").display())
}

/// Make the mirror of `repo`, as the person, with their credentials.
/// Whether a command could write in `mirror`: it's in a shared folder that
/// allows changes. Desktop never fetches such a mirror, whose config a
/// command could have written.
pub fn writable_by_commands(mirror: &Path, roots: &[crate::config::Root]) -> bool {
    let mirror = crate::roots::canonical(mirror).unwrap_or_else(|_| mirror.to_path_buf());
    roots.iter().any(|root| {
        root.writable
            && crate::roots::canonical(&root.path).is_ok_and(|path| mirror.starts_with(path))
    })
}

pub fn create_mirror(
    paths: &Paths,
    repo: &Repository,
    roots: &[crate::config::Root],
) -> Result<PathBuf> {
    check_url(&repo.url)?;
    let mirror = mirror_dir(paths, repo);
    if writable_by_commands(&paths.data_dir, roots) {
        bail!(
            "Desktop's data folder is inside a shared folder that allows changes, where a \
             command could change a mirror's settings: share a narrower folder"
        );
    }
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
    let mut clone = git_for(Some(repo), &mirror);
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

/// The command, run in the sandbox in the shared folder, that makes the
/// checkout `name` from `mirror`, or, when there is one already, points its
/// `origin` at the mirror and drops any push address. Its `origin` is then
/// the mirror: fetching needs no network or credentials, and a push fails.
/// `powershell` writes it for Windows PowerShell.
pub fn checkout_command(name: &str, mirror: &Path, exists: bool, powershell: bool) -> String {
    // Single quotes in both shells; a quote inside is doubled for
    // PowerShell, and closed and escaped for sh.
    let quote = |text: &str| {
        if powershell {
            format!("'{}'", text.replace('\'', "''"))
        } else {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
    };
    let (name, mirror) = (quote(name), quote(&mirror.to_string_lossy()));
    let then = if powershell { "; " } else { " && " };
    if exists {
        let set = format!("git -C {name} remote set-url origin {mirror}");
        let unset = format!("git -C {name} config --unset-all remote.origin.pushurl");
        // Nothing to drop is fine: git exits 5.
        if powershell {
            format!(
                "{set}; if ($LASTEXITCODE -eq 0) {{ {unset}; if ($LASTEXITCODE -eq 5) {{ $global:LASTEXITCODE = 0 }} }}"
            )
        } else {
            format!("{set}{then}{{ {unset}; s=$?; [ $s -eq 0 ] || [ $s -eq 5 ]; }}")
        }
    } else {
        format!("git clone --quiet --no-hardlinks -- {mirror} {name}")
    }
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
    fetch_until(mirror, repo, FETCH_LIMIT, &|| false)
}

/// As [`fetch`], within `limit`, ended early once `stop` says so.
pub fn fetch_until(
    mirror: &Path,
    repo: Option<&Repository>,
    limit: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    let _held = lock_mirror(mirror)?;
    fetch_locked(mirror, repo, limit, stop)
}

/// One fetch of a mirror at a time, across every process that fetches it
/// (the daemon's runs, `cww coding fetch`): a lock file in the mirror, held
/// until the returned file is dropped.
fn lock_mirror(mirror: &Path) -> Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(mirror.join("cww-fetch.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn fetch_locked(
    mirror: &Path,
    repo: Option<&Repository>,
    limit: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    let mut fetch = git_for(repo, mirror);
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
    match run_until(fetch, limit.min(FETCH_LIMIT), stop) {
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
pub fn fetch_if_stale(
    mirror: &Path,
    repo: Option<&Repository>,
    limit: Duration,
    stop: &dyn Fn() -> bool,
) {
    let fresh = || {
        fetched_at(mirror)
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < FRESH)
    };
    if fresh() {
        return;
    }
    // Another run may be fetching it now: wait for it, then look again.
    let Ok(_held) = lock_mirror(mirror) else {
        return;
    };
    if !fresh()
        && let Err(e) = fetch_locked(mirror, repo, limit, stop)
    {
        tracing::info!(error = %e, "fetching a repository's mirror failed");
    }
}
