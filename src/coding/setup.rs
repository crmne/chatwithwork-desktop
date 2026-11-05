//! `cww coding`: lending this computer to a coding bot, one step at a time
//! (docs/coding-computers.md). Each step says what it's about to do, asks
//! before anything that widens access or runs something as the person, and
//! prints how it stands. Run again, it skips what's done.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{ClaudeState, Login, detect, repositories};
use crate::config::{Config, Repository};
use crate::control::{self, ControlRequest};
use crate::paths::Paths;
use crate::roots::{self, NewRoot};

/// What the person said up front, as flags. Anything missing is asked.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub bot: Option<String>,
    pub bot_name: Option<String>,
    pub folder: Option<PathBuf>,
    pub repo: Option<String>,
    /// A deploy key to fetch the repository with, outside every shared folder.
    pub ssh_key: Option<PathBuf>,
    /// The bot's commands start without anyone approving each one.
    pub unattended: Option<bool>,
    pub test_command: Option<String>,
    /// Answer yes to every question: for a person who already decided,
    /// running it where nothing can ask.
    pub yes: bool,
    pub skip_install: bool,
    pub skip_proof: bool,
}

/// How a step stands.
pub enum Step {
    Ok(String),
    NeedsYou(String),
    Skipped(String),
    Failed(String),
}

impl Step {
    pub fn print(&self, title: &str) {
        let (tag, text) = match self {
            Self::Ok(t) => ("ok", t),
            Self::NeedsYou(t) => ("needs you", t),
            Self::Skipped(t) => ("skipped", t),
            Self::Failed(t) => ("failed", t),
        };
        println!("  [{tag}] {title}: {text}");
    }

    fn failed(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::NeedsYou(_))
    }
}

struct Asker {
    yes: bool,
}

impl Asker {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    /// A yes or no, `default` on an empty answer; yes with `--yes`; no
    /// where nothing can ask.
    fn confirm(&self, question: &str, default: bool) -> Result<bool> {
        if self.yes {
            println!("{question} yes (--yes)");
            return Ok(true);
        }
        if !self.interactive() {
            println!("{question} no (nothing to ask on; pass --yes)");
            return Ok(false);
        }
        print!("{question} {} ", if default { "[Y/n]" } else { "[y/N]" });
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        Ok(match answer.trim().to_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    /// A line of text, `default` on an empty answer.
    fn text(&self, question: &str, default: Option<&str>) -> Result<Option<String>> {
        if self.yes || !self.interactive() {
            return Ok(default.map(String::from));
        }
        match default {
            Some(d) => print!("{question} [{d}] "),
            None => print!("{question} "),
        }
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        let answer = answer.trim();
        Ok(if answer.is_empty() {
            default.map(String::from)
        } else {
            Some(answer.to_string())
        })
    }
}

thread_local! {
    /// The config as this setup last read or wrote it. Setup waits on
    /// people, browsers and clones; if the file changed meanwhile (a folder
    /// unshared, commands turned off), saving its older copy would put back
    /// what was taken away, so it stops instead. One per thread, so two
    /// setups never share a snapshot.
    static LAST_SEEN: std::cell::RefCell<Option<Config>> = const { std::cell::RefCell::new(None) };
}

/// Forget the snapshot when setup ends, however it ends.
struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        LAST_SEEN.with(|last| *last.borrow_mut() = None);
    }
}

fn seen(config: &Config) {
    LAST_SEEN.with(|last| *last.borrow_mut() = Some(config.clone()));
}

fn save(config: &Config, paths: &Paths) -> Result<()> {
    let last = LAST_SEEN.with(|last| last.borrow().clone());
    if let Some(last) = last
        && Config::load(paths)? != last
    {
        bail!(
            "this computer's settings changed while setup ran (in the app, or with `cww \
             roots`); nothing was saved over them. Run `cww coding setup` again"
        );
    }
    config.save(paths)?;
    // As it reads back, which is what the next save compares.
    seen(&Config::load(paths)?);
    let socket = paths.socket_path();
    let before = control::request(&socket, ControlRequest::Status)
        .ok()
        .flatten()
        .and_then(|status| status["pid"].as_u64());
    match control::request(&socket, ControlRequest::Reload) {
        Ok(Some(answer)) if answer["restarting"] == true => wait_for_worker(paths, before),
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("warning: couldn't reach the daemon: {e:#}");
            Ok(())
        }
    }
}

/// The daemon restarts its worker under a sandbox that fits a folder just
/// allowed more: wait, a bounded while, for the new one to answer, so a
/// check isn't sent to a worker on its way out.
fn wait_for_worker(paths: &Paths, before: Option<u64>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let pid = control::request(&paths.socket_path(), ControlRequest::Status)
            .ok()
            .flatten()
            .and_then(|status| status["pid"].as_u64());
        if pid.is_some() && pid != before {
            return Ok(());
        }
    }
    eprintln!("warning: the background service is taking long to restart");
    Ok(())
}

/// `cww coding setup`.
pub fn setup(paths: &Paths, options: Options) -> Result<()> {
    let ask = Asker { yes: options.yes };
    let mut config = Config::load(paths)?;
    seen(&config);
    let _reset = Reset;
    println!("Lending this computer to a coding bot. Each step says what it does first.\n");

    // 1. Paired, and the daemon running.
    let paired = match &config.server {
        Some(_) if control::request(&paths.socket_path(), ControlRequest::Status)?.is_some() => {
            Step::Ok("paired, and the background service is running".into())
        }
        Some(_) => Step::NeedsYou(
            "paired, but the background service isn't running: run `cww daemon install`".into(),
        ),
        None => Step::NeedsYou("not paired: run `cww login`, then this again".into()),
    };
    paired.print("This computer");
    if config.server.is_none() {
        return Ok(());
    }

    // 2. The bot.
    let bot = match options.bot.clone() {
        Some(bot) => bot,
        None => match ask.text(
            "Which bot? The number in its address in Chat with Work (/bots/42):",
            None,
        )? {
            Some(bot) => bot,
            None => bail!("say which bot with --bot <number>"),
        },
    };
    let bot = bot.trim().trim_start_matches('#').to_string();
    let bot_name = options
        .bot_name
        .clone()
        .or_else(|| {
            config
                .roots
                .iter()
                .find_map(|r| r.loan(&bot))
                .map(|l| l.name.clone())
        })
        .filter(|n| !n.is_empty())
        .or_else(|| {
            ask.text("Its name, to recognize it here:", None)
                .ok()
                .flatten()
        });
    Step::Ok(format!(
        "bot {bot}{}",
        bot_name
            .as_deref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default()
    ))
    .print("Bot");

    // 3. The folder, shared with changes, commands and the network, and
    // lent to the bot.
    let root_id = folder_step(
        &mut config,
        paths,
        &ask,
        &options,
        &bot,
        bot_name.as_deref(),
    )?;
    let Some(root_id) = root_id else {
        return Ok(());
    };

    // 4. Claude Code, installed.
    let claude = claude_step(&mut config, paths, &ask)?;

    // 5. Signed in, and this bot's runs here allowed to use it.
    if claude {
        sign_in_step(&mut config, paths, &ask, &root_id, &bot)?;
    }

    // 6. The repository.
    let checkout = repository_step(&mut config, paths, &ask, &options, &root_id, &bot)?;

    // 7. The toolchain.
    if let Some((name, _, mirror)) = &checkout {
        if options.skip_install {
            Step::Skipped("--skip-install".into()).print("Toolchain");
        } else {
            toolchain_step(&ask, paths, &root_id, name, mirror)?;
        }
    }

    // 8. Proof, in the sandbox a bot's command gets.
    if options.skip_proof {
        Step::Skipped("--skip-proof".into()).print("Proof");
    } else {
        let checkout = checkout.map(|(name, path, _)| (name, path));
        proof_step(paths, &ask, &options, &root_id, &bot, checkout.as_ref())?;
    }

    println!();
    let config = Config::load(paths)?;
    summary(&config, paths, Some(&bot));
    Ok(())
}

fn folder_step(
    config: &mut Config,
    paths: &Paths,
    ask: &Asker,
    options: &Options,
    bot: &str,
    bot_name: Option<&str>,
) -> Result<Option<String>> {
    // A folder already lent to the bot that allows commands, unless one
    // was named.
    let lent = config
        .roots
        .iter()
        .find(|r| r.loan(bot).is_some() && r.runs_commands())
        .map(|r| r.path.clone());
    let default = crate::paths::home_dir()?.join("bots");
    let folder = match options.folder.clone().or(lent) {
        Some(folder) => folder,
        None => PathBuf::from(
            ask.text(
                "Which folder should the bot work in?",
                Some(&default.to_string_lossy()),
            )?
            .unwrap_or_default(),
        ),
    };
    let folder = expand_home(&folder)?;
    if !folder.is_dir() {
        if !ask.confirm(&format!("Make the folder {}?", folder.display()), true)? {
            Step::NeedsYou("no folder".into()).print("Folder");
            return Ok(None);
        }
        std::fs::create_dir_all(&folder)?;
    }
    let canonical = roots::canonical(&folder)?;
    let existing = config.roots.iter().find(|r| r.path == canonical).cloned();
    let root = match existing {
        Some(root) => root,
        None => {
            println!(
                "Sharing {} lets Chat with Work search and read it (keys and .env files stay \
                 private).",
                canonical.display()
            );
            if !ask.confirm("Share it?", true)? {
                Step::NeedsYou("not shared".into()).print("Folder");
                return Ok(None);
            }
            roots::add_root(
                config,
                paths,
                NewRoot {
                    path: &canonical,
                    label: None,
                    i_know: false,
                    follow_symlinks: false,
                    writable: false,
                },
            )?
        }
    };
    if !root.writable {
        println!(
            "Allowing changes lets Chat with Work create, edit, move and delete files in {}, \
             each approved in Chat with Work, with old versions in the trash.",
            root.label
        );
        if !ask.confirm("Allow changes there?", true)? {
            Step::NeedsYou("changes aren't allowed there".into()).print("Folder");
            return Ok(None);
        }
        roots::set_writable(config, &root.id, true, false)?;
    }
    let root = config.root(&root.id).cloned().context("the folder")?;
    if !root.runs_commands() || !root.network {
        println!(
            "Allowing commands with the network lets Chat with Work run one line of shell in \
             {}, in a sandbox that can change only that folder and reach the internet (Claude \
             Code needs it). Each is approved in Chat with Work, unless you let this bot's \
             commands start unattended. A compromised server could run commands there.",
            root.label
        );
        if !ask.confirm("Allow commands with the network there?", true)? {
            Step::NeedsYou("commands aren't allowed there".into()).print("Folder");
            return Ok(None);
        }
        roots::set_commands(config, &root.id, true, true)?;
    }
    let loan = config.root(&root.id).and_then(|r| r.loan(bot)).cloned();
    let unattended = match options.unattended {
        Some(u) => u,
        None => {
            // Never on with --yes alone: only --unattended or a yes here.
            loan.as_ref().is_some_and(|l| l.unattended)
                || (!ask.yes && ask.interactive() && {
                    println!(
                        "Unattended lets this bot's commands in {} start without anyone \
                         approving each one: it can work while you're away, and a compromised \
                         server, or a bot misled by what it read, could run any command there.",
                        root.label
                    );
                    ask.confirm("Let its commands start unattended?", false)?
                })
        }
    };
    if loan.is_none() {
        println!(
            "Sharing {} with bot {bot} lets that bot's chats read it, change files and run \
             commands there, as the folder allows.",
            root.label
        );
        if !ask.confirm("Share it with the bot?", true)? {
            Step::NeedsYou("not shared with the bot".into()).print("Folder");
            return Ok(None);
        }
    }
    roots::set_bot(config, &root.id, bot, bot_name, true, unattended)?;
    save(config, paths)?;
    Step::Ok(format!(
        "{} ({}): changes, commands and the network allowed, shared with bot {bot}, {}",
        root.label,
        root.path.display(),
        if unattended {
            "its commands start unattended"
        } else {
            "each of its commands asks you"
        }
    ))
    .print("Folder");
    Ok(Some(root.id))
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    match path.strip_prefix("~") {
        Ok(rest) => Ok(crate::paths::home_dir()?.join(rest)),
        Err(_) if path.is_relative() => Ok(std::env::current_dir()?.join(path)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

/// Where `name` would run from the person's `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// The `claude` this computer has and that runs: the person's `PATH`, then
/// mise's, then where its installers put it. A mise shim is followed to the
/// program it runs. A leftover that doesn't run (an old `~/.claude/local`
/// install, say) is passed over.
pub fn find_claude() -> Option<PathBuf> {
    find_claude_outside(&[])
}

/// As [`find_claude`], passing over any candidate a command could change:
/// one inside a shared folder that allows changes. Probing it would run what
/// a bot wrote, as the person, outside the sandbox.
pub fn find_claude_outside(roots: &[crate::config::Root]) -> Option<PathBuf> {
    let home = crate::paths::home_dir().ok()?;
    let exe = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let mise = || {
        let mut command = Command::new("mise");
        command.args(["which", "claude"]);
        output_within(command, PROBE_LIMIT)
            .map(PathBuf::from)
            .filter(|p| p.is_file())
    };
    let writable = |p: &PathBuf| {
        let real = crate::roots::canonical(p).unwrap_or_else(|_| p.clone());
        roots.iter().any(|root| {
            root.writable
                && crate::roots::canonical(&root.path).is_ok_and(|path| real.starts_with(path))
        })
    };
    let runs = |p: &PathBuf| p.is_file() && !writable(p) && version(p).is_some();
    // Each asked only when the one before didn't give a `claude` that runs.
    let on_path = || {
        which("claude").and_then(|found| {
            let text = found.to_string_lossy();
            if text.contains("mise") && text.contains("shims") {
                mise()
            } else {
                Some(found)
            }
        })
    };
    if let Some(found) = on_path().filter(runs) {
        return Some(found);
    }
    if let Some(found) = mise().filter(runs) {
        return Some(found);
    }
    [
        home.join(".local").join("bin").join(exe),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home.join(".npm-global").join("bin").join("claude"),
        home.join(".claude").join("local").join(exe),
    ]
    .into_iter()
    .find(runs)
}

fn version(program: &Path) -> Option<String> {
    let mut command = Command::new(program);
    command.arg("--version");
    output_within(command, PROBE_LIMIT)
}

/// How long a probe (`claude --version`, `mise which claude`) may take
/// before it counts as not running.
const PROBE_LIMIT: Duration = Duration::from_secs(15);

/// What `command` prints, if it succeeds within `limit`; a probe that hangs
/// counts as one that failed.
fn output_within(mut command: Command, limit: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = said.send(text);
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = heard.recv_timeout(Duration::from_secs(2)).ok()?;
    status
        .success()
        .then(|| text.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn install_command() -> (&'static str, Vec<&'static str>, &'static str) {
    if cfg!(windows) {
        (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "irm https://claude.ai/install.ps1 | iex",
            ],
            "irm https://claude.ai/install.ps1 | iex",
        )
    } else {
        (
            "sh",
            vec!["-c", "curl -fsSL https://claude.ai/install.sh | bash"],
            "curl -fsSL https://claude.ai/install.sh | bash",
        )
    }
}

fn claude_step(config: &mut Config, paths: &Paths, ask: &Asker) -> Result<bool> {
    let mut found = find_claude_outside(&config.roots);
    if found.is_none() {
        let (program, args, shown) = install_command();
        println!("Claude Code isn't installed. Anthropic's installer is:\n  {shown}");
        if ask.confirm("Run it now?", true)? {
            let status = Command::new(program).args(&args).status();
            if !status.is_ok_and(|s| s.success()) {
                Step::Failed("the installer failed; run it yourself, then this again".into())
                    .print("Claude Code");
                return Ok(false);
            }
            found = find_claude_outside(&config.roots);
        }
    }
    let Some(program) = found else {
        Step::NeedsYou("not installed: install it, then run this again".into())
            .print("Claude Code");
        return Ok(false);
    };
    let Some(version) = version(&program) else {
        Step::Failed(format!("{} doesn't run", program.display())).print("Claude Code");
        return Ok(false);
    };
    if config.coding.claude.as_ref() != Some(&program) {
        config.coding.claude = Some(program.clone());
        save(config, paths)?;
    }
    Step::Ok(format!("{version}, at {}", program.display())).print("Claude Code");
    Ok(true)
}

/// Claude Code's sign-in on this computer, where Claude Code keeps it.
fn login(paths: &Paths) -> Login {
    paths
        .person_home
        .as_deref()
        .map(Login::find)
        .unwrap_or_default()
}

fn sign_in_step(
    config: &mut Config,
    paths: &Paths,
    ask: &Asker,
    root: &str,
    bot: &str,
) -> Result<()> {
    if !login(paths).signed_in() {
        Step::NeedsYou(
            "Claude Code isn't signed in on this computer: run `claude` once and sign in, then \
             run this again"
                .into(),
        )
        .print("Sign-in");
        return Ok(());
    }
    let allowed = config
        .root(root)
        .and_then(|r| r.loan(bot))
        .is_some_and(|l| l.claude_code);
    if !allowed {
        let label = config.root(root).map_or(root, |r| r.label.as_str());
        println!(
            "Bot {bot}'s coding runs in {label} would use your Claude Code sign-in on this \
             computer: they could read and change ~/.claude and ~/.claude.json{}, where Claude \
             Code keeps it and refreshes it, and nothing else of your home folder. A command \
             there could read its tokens, and change Claude Code's settings, which the next \
             `claude` you run yourself starts outside the sandbox. Desktop removes the tokens \
             from what commands print, which doesn't stop a command written to hide them.",
            if cfg!(target_os = "macos") {
                " and reach your login keychain as Claude Code does (any item that lets the \
                 `security` command read it, other programs' too)"
            } else {
                ""
            }
        );
        if ask.confirm(
            "Let bots' coding runs use this computer's Claude Code sign-in?",
            false,
        )? {
            roots::set_bot_claude_code(config, root, bot, true)?;
            save(config, paths)?;
        }
    }
    let state = config
        .root(root)
        .map(|r| super::claude_state(&config.coding, r, bot));
    match state {
        Some(ClaudeState::Ready) => {
            Step::Ok("this computer's Claude Code sign-in, for this bot in this folder".into())
        }
        Some(state) => Step::NeedsYou(state.sentence().into()),
        None => Step::Failed("the folder is gone".into()),
    }
    .print("Sign-in");
    Ok(())
}

fn repository_step(
    config: &mut Config,
    paths: &Paths,
    ask: &Asker,
    options: &Options,
    root_id: &str,
    bot: &str,
) -> Result<Option<(String, PathBuf, PathBuf)>> {
    let root = config.root(root_id).cloned().context("the folder")?;
    let existing = config
        .coding
        .repositories
        .iter()
        .find(|r| r.root == root_id && options.repo.as_ref().is_none_or(|u| *u == r.url))
        .cloned();
    let repo = match existing {
        Some(repo) => repo,
        None => {
            let Some(url) = options.repo.clone().or_else(|| {
                ask.text(
                    "The repository's address (git@github.com:acme/app.git), or Enter to skip:",
                    None,
                )
                .ok()
                .flatten()
            }) else {
                Step::Skipped("no repository".into()).print("Repository");
                return Ok(None);
            };
            repositories::check_url(&url)?;
            let name = repositories::name_from_url(&url)
                .context("couldn't tell the repository's name from its address")?;
            Repository {
                root: root_id.to_string(),
                name,
                url,
                ssh_key: None,
            }
        }
    };
    let mut repo = repo;
    if let Some(key) = &options.ssh_key {
        // Where it really is: a link could be changed later to point at
        // another key.
        let key = crate::roots::canonical(&expand_home(key)?)?;
        repositories::check_ssh_key(&key, &config.roots)?;
        repo.ssh_key = Some(key);
    }
    println!(
        "Desktop keeps a mirror of {} in its own data folder, fetched with your Git credentials \
         outside the sandbox, and a checkout in {} whose origin is that mirror: a bot fetches \
         without keys, and can't push.",
        repo.url, root.label
    );
    let mirror = match repositories::create_mirror(paths, &repo, &config.roots) {
        Ok(m) => m,
        Err(e) => {
            Step::Failed(format!("{e:#}")).print("Repository");
            return Ok(None);
        }
    };
    // Kept first: a run reads only the mirrors of the folder's repositories.
    if !config.coding.repositories.contains(&repo) {
        config
            .coding
            .repositories
            .retain(|r| !(r.root == repo.root && r.name == repo.name));
        config.coding.repositories.push(repo.clone());
        save(config, paths)?;
    }
    // Made, or pointed at the mirror, by a command in the sandbox, as the
    // bot: that can change only the shared folder, so a link a command left
    // there can't send the write anywhere else. Desktop itself writes
    // nothing in a shared folder.
    let checkout = root.path.join(&repo.name);
    let exists = std::fs::symlink_metadata(&checkout).is_ok();
    let line = repositories::checkout_command(&repo.name, &mirror, exists, cfg!(windows));
    let (state, code, output) = check(paths, root_id, "", &line, bot, Duration::from_secs(1800))
        .unwrap_or_else(|e| ("failed".into(), None, format!("{e:#}")));
    if state != "exited" || code != Some(0) {
        Step::Failed(format!(
            "making the checkout in the sandbox {state}{}\n{}",
            code.map(|c| format!(" with {c}")).unwrap_or_default(),
            tail(&output, 8)
        ))
        .print("Repository");
        return Ok(None);
    }
    let _ = ask;
    Step::Ok(format!(
        "{} checked out as {}:{}, fetching from Desktop's mirror",
        repo.url, root.label, repo.name
    ))
    .print("Repository");
    Ok(Some((repo.name, checkout, mirror)))
}

fn shell(command: &str, dir: &Path) -> std::io::Result<std::process::ExitStatus> {
    if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(dir)
            .status()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(dir)
            .status()
    }
}

/// Run `install` in `dir`: through the repository's own mise environment
/// when it uses mise, so its Ruby or Node is the one that runs.
fn install_in(
    install: &str,
    dir: &Path,
    with_mise: bool,
) -> std::io::Result<std::process::ExitStatus> {
    if with_mise && !install.starts_with("mise ") {
        let mut command = Command::new("mise");
        command.args(["exec", "--"]).current_dir(dir);
        if cfg!(windows) {
            command.args(["cmd", "/C", install]);
        } else {
            command.args(["sh", "-c", install]);
        }
        command.status()
    } else {
        shell(install, dir)
    }
}

/// Install what the repository needs, as the person, outside the sandbox,
/// from a clone of the mirror in Desktop's own data folder: never from the
/// checkout in the shared folder, whose install files a bot's commands
/// could change before they run. Toolchains (mise) and packages installed
/// for the whole computer (Ruby gems in mise's Ruby, Cargo's registry) are
/// what carries over; a project's own folders (`node_modules`, `.venv`) are
/// the bot's commands to make in the checkout.
fn toolchain_step(ask: &Asker, paths: &Paths, root: &str, name: &str, mirror: &Path) -> Result<()> {
    let staging = paths
        .data_dir
        .join("staging")
        .join(root)
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(staging.parent().unwrap_or(&staging))?;
    let cloned = Command::new("git")
        .args(["clone", "--quiet", "--no-hardlinks", "--"])
        .arg(mirror)
        .arg(&staging)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status();
    if !cloned.is_ok_and(|s| s.success()) {
        Step::Failed("couldn't make a private copy of the repository to install from".into())
            .print("Toolchain");
        return Ok(());
    }
    let result = install_from(ask, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn install_from(ask: &Asker, staging: &Path) -> Result<()> {
    let needs = detect::needs(staging);
    if needs.is_empty() {
        Step::Ok("nothing to install that it can tell".into()).print("Toolchain");
        return Ok(());
    }
    let with_mise = needs.iter().any(|n| n.program == "mise") && which("mise").is_some();
    let mut missing = Vec::new();
    for need in &needs {
        // mise installs the rest, so its programs may come later.
        let present = which(need.program).is_some();
        if !present && need.program != "mise" && with_mise {
            // Installed by mise above, perhaps.
        } else if !present {
            missing.push(format!("{} needs {}", need.what, need.program));
            continue;
        }
        let Some(install) = &need.install else {
            continue;
        };
        println!(
            "{}: `{install}` runs as you, outside the sandbox, with the repository's own \
             install scripts, in a private copy of the repository from Desktop's mirror.",
            need.what,
        );
        if ask.confirm("Run it?", true)? {
            match install_in(install, staging, with_mise) {
                Ok(s) if s.success() => {}
                _ => missing.push(format!("`{install}` failed")),
            }
        } else {
            missing.push(format!("`{install}` not run"));
        }
    }
    if missing.is_empty() {
        Step::Ok(
            needs
                .iter()
                .map(|n| n.what.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    } else {
        Step::NeedsYou(missing.join("; "))
    }
    .print("Toolchain");
    Ok(())
}

/// Start `command` through the daemon, as the bot's run would start, and
/// wait for it. Answers its state, exit code and the end of its output.
fn check(
    paths: &Paths,
    root: &str,
    dir: &str,
    command: &str,
    bot: &str,
    limit: Duration,
) -> Result<(String, Option<i64>, String)> {
    let started = control::request(
        &paths.socket_path(),
        ControlRequest::CodingCheck {
            root: root.into(),
            dir: dir.into(),
            command: command.into(),
            bot: Some(bot.into()),
        },
    )?
    .context("the background service isn't running")?;
    let id = started["id"].as_str().context("no run")?.to_string();
    let deadline = Instant::now() + limit;
    let mut output = String::new();
    let mut offset = 0u64;
    // Follow its output until it says it's done (the run ended and all it
    // printed was read), then ask how it ended, once.
    loop {
        let mut done = false;
        // Everything printed so far, a chunk at a time.
        for _ in 0..4096 {
            let Some(chunk) = control::request(
                &paths.socket_path(),
                ControlRequest::CommandOutput {
                    run: id.clone(),
                    offset,
                    max: None,
                },
            )?
            else {
                break;
            };
            let next = chunk["next_offset"].as_u64().unwrap_or(offset);
            output.push_str(chunk["text"].as_str().unwrap_or_default());
            keep_tail(&mut output);
            done = chunk["done"] == true;
            if done || next <= offset {
                break;
            }
            offset = next;
        }
        if done {
            break;
        }
        if Instant::now() >= deadline {
            let _ = control::request(
                &paths.socket_path(),
                ControlRequest::CommandStop { run: id.clone() },
            );
            return Ok(("timed_out".into(), None, output));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let list =
        control::request(&paths.socket_path(), ControlRequest::Commands)?.unwrap_or_default();
    let runs = list
        .get("commands")
        .cloned()
        .unwrap_or(list)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let status = runs.iter().find(|r| r["id"] == Value::String(id.clone()));
    let state = status
        .and_then(|s| s["state"].as_str())
        .unwrap_or("exited")
        .to_string();
    let error = status
        .and_then(|s| s["error"].as_str())
        .map(|e| format!("\n{e}"))
        .unwrap_or_default();
    Ok((
        state,
        status.and_then(|s| s["exit_code"].as_i64()),
        output + &error,
    ))
}

/// Keep only the last 256 KiB of what a check printed: only its end shows.
fn keep_tail(output: &mut String) {
    const KEEP: usize = 256 * 1024;
    if output.len() > KEEP {
        let mut cut = output.len() - KEEP;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output.drain(..cut);
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|l| format!("      {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn proof_step(
    paths: &Paths,
    ask: &Asker,
    options: &Options,
    root: &str,
    bot: &str,
    checkout: Option<&(String, PathBuf)>,
) -> Result<()> {
    if let Some((name, path)) = checkout {
        let detected = detect::test_command(path);
        let command = match options.test_command.clone() {
            Some(c) => Some(c),
            None => ask.text(
                "The command that runs its tests (Enter to use it, `-` to skip):",
                detected.as_deref(),
            )?,
        }
        .filter(|c| c != "-");
        match command {
            None => Step::Skipped("no test command".into()).print("Tests"),
            Some(command) => {
                println!("Running `{command}` in {name}, in the sandbox a bot's command gets…");
                let (state, code, output) =
                    check(paths, root, name, &command, bot, Duration::from_secs(1800))
                        .unwrap_or_else(|e| ("failed".into(), None, format!("{e:#}")));
                let step = if state == "exited" && code == Some(0) {
                    Step::Ok(format!("`{command}` passed in the sandbox"))
                } else {
                    Step::Failed(format!(
                        "`{command}` {state}{}\n{}",
                        code.map(|c| format!(" with {c}")).unwrap_or_default(),
                        tail(&output, 15)
                    ))
                };
                step.print("Tests");
            }
        }
    }
    let config = Config::load(paths)?;
    let ready = config
        .root(root)
        .is_some_and(|r| super::claude_state(&config.coding, r, bot) == ClaudeState::Ready);
    if !ready {
        Step::Skipped("Claude Code isn't ready for this bot here".into()).print("Claude Code run");
        return Ok(());
    }
    let dir = checkout.map(|(n, _)| n.as_str()).unwrap_or("");
    println!("Asking Claude Code for one word, in the sandbox, as bot {bot}…");
    let (state, code, output) = check(
        paths,
        root,
        dir,
        "claude -p 'Reply with the word ready.' --max-turns 1",
        bot,
        Duration::from_secs(180),
    )
    .unwrap_or_else(|e| ("failed".into(), None, format!("{e:#}")));
    let step = if state == "exited" && code == Some(0) && output.to_lowercase().contains("ready") {
        Step::Ok("found, signed in, and answering from the sandbox".into())
    } else {
        Step::Failed(format!(
            "{state}{}\n{}",
            code.map(|c| format!(" with {c}")).unwrap_or_default(),
            tail(&output, 10)
        ))
    };
    let failed = step.failed();
    step.print("Claude Code run");
    if failed {
        println!("  The sandbox needs the network for Claude Code; `cww coding status` says more.");
    }
    Ok(())
}

/// What each folder lent to a bot has, as `cww coding status` prints it.
pub fn summary(config: &Config, paths: &Paths, only_bot: Option<&str>) {
    let stored = login(paths).signed_in();
    match &config.coding.claude {
        Some(p) => println!("Claude Code: {}", p.display()),
        None => println!("Claude Code: not set up (`cww coding setup`)"),
    }
    println!(
        "Sign-in: {}",
        if stored {
            "Claude Code is signed in on this computer"
        } else {
            "Claude Code isn't signed in on this computer: run `claude` once and sign in"
        }
    );
    for root in &config.roots {
        for loan in &root.bots {
            if only_bot.is_some_and(|b| b != loan.id) {
                continue;
            }
            let state = effective_state(&config.coding, root, &loan.id, stored);
            println!(
                "Bot {}{} in {}: {}{}",
                loan.id,
                if loan.name.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", loan.name)
                },
                root.label,
                state.sentence(),
                if loan.unattended {
                    ", commands start unattended"
                } else {
                    ""
                }
            );
        }
    }
    for repo in &config.coding.repositories {
        let mirror = repositories::mirror_dir(paths, repo);
        let fetched = repositories::fetched_at(&mirror)
            .and_then(|t| t.elapsed().ok())
            .map_or_else(
                || "never fetched".to_string(),
                |age| format!("fetched {} minutes ago", age.as_secs() / 60),
            );
        let failure = repositories::last_failure(&mirror)
            .map(|f| format!("; the last fetch failed: {f}"))
            .unwrap_or_default();
        println!(
            "Repository {} in {}: {}, {fetched}{failure}",
            repo.name, repo.root, repo.url
        );
    }
}

/// The state as a run would find it: ready only when Claude Code is signed
/// in on this computer, which the config doesn't say.
fn effective_state(
    coding: &crate::config::CodingConfig,
    root: &crate::config::Root,
    bot: &str,
    signed_in: bool,
) -> ClaudeState {
    match super::claude_state(coding, root, bot) {
        ClaudeState::Ready if !signed_in => ClaudeState::NotSignedIn,
        state => state,
    }
}

/// What `cww coding status --json` prints: per loan, the state the server
/// would see, and per repository, its fetch. No token, ever.
pub fn status_json(config: &Config, paths: &Paths, only_bot: Option<&str>) -> Value {
    let stored = login(paths).signed_in();
    let loans: Vec<Value> = config
        .roots
        .iter()
        .flat_map(|root| {
            root.bots
                .iter()
                .filter(move |loan| only_bot.is_none_or(|bot| bot == loan.id))
                .map(move |loan| {
                    serde_json::json!({
                        "root": root.id,
                        "bot": loan.id,
                        "name": loan.name,
                        "unattended": loan.unattended,
                        "claude_code": effective_state(&config.coding, root, &loan.id, stored).name(),
                    })
                })
        })
        .collect();
    let repos: Vec<Value> = config
        .coding
        .repositories
        .iter()
        .map(|repo| {
            let mirror = repositories::mirror_dir(paths, repo);
            serde_json::json!({
                "root": repo.root,
                "name": repo.name,
                "url": repo.url,
                "fetched_at": repositories::fetched_at(&mirror)
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                "fetch_failure": repositories::last_failure(&mirror),
            })
        })
        .collect();
    serde_json::json!({
        "claude": config.coding.claude,
        "signed_in": stored,
        "loans": loans,
        "repositories": repos,
    })
}
