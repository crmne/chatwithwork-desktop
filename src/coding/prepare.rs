//! A bot's first run on a repository prepares it by itself
//! (docs/coding-computers.md, "The first run"): Desktop makes the mirror with
//! the person's own Git login, and the checkout from it, and installs the
//! toolchain and runs the tests once, in the sandbox, with no agent's sign-in.
//! The person already said yes on this computer, for this bot, to repositories
//! of this owner ([`crate::config::BotLoan::owners`]).
//!
//! Whoever starts commands does this (the supervisor on Linux and macOS, the
//! daemon on Windows), and it trusts nothing it's sent: it reads the config
//! from disk, and the address comes from what the person's Git login reaches,
//! never from the server, which can only name `owner/name` on GitHub.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{Agent, AgentState, Billing, Login, detect, repositories, setup};
use crate::commands::{Request, RunError, Runs};
use crate::config::{Config, Repository, Root};
use crate::paths::Paths;

/// The folder in a shared folder for what outlasts a run once a repository
/// was prepared there: toolchains and package caches. A run's own home goes
/// with it, and nothing else in the folder is outside a checkout.
pub const TOOLS_DIR: &str = ".cww-tools";
/// The file in a mirror that says it's prepared, and how.
const PREPARED: &str = "cww-prepared.json";
/// How long a step waits for room among the commands running here.
const ROOM_WAIT: Duration = Duration::from_secs(600);
/// How much of what a failed step printed goes back, its end.
const DETAIL_CHARS: usize = 2_000;
/// Longest each look at whether a Git login reaches a repository may take,
/// for the app's dialog, which holds up the calls behind it, and for a run.
const LOOK_LIMIT: Duration = Duration::from_secs(5);
const REACH_LIMIT: Duration = Duration::from_secs(60);

/// A GitHub owner's name: letters, digits and hyphens, at most 39.
pub fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `owner/name` on GitHub, split, when it's only that: never an address, a
/// path, or anything Git would read as an option.
pub fn split_repository(repository: &str) -> Option<(&str, &str)> {
    let (owner, name) = repository.split_once('/')?;
    (valid_owner(owner) && repositories::valid_name(name)).then_some((owner, name))
}

/// `owner/name` out of a GitHub address, however it's written.
pub fn github_name(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    split_repository(path).map(|_| path.to_string())
}

/// The folder's place for what outlasts a run, once a run prepared a
/// repository there: other folders, and `cww coding setup`'s repositories,
/// keep their runs as they were.
pub fn tools_dir(config: &Config, root: &str) -> Option<PathBuf> {
    let prepared = config
        .coding
        .repositories
        .iter()
        .any(|repo| repo.root == root && repo.github.is_some());
    if !prepared {
        return None;
    }
    let root = config.roots.iter().find(|r| r.id == root)?;
    Some(crate::roots::canonical(&root.path).ok()?.join(TOOLS_DIR))
}

/// What tells package managers to keep their files in the folder's own place,
/// the only place a run can write that outlasts it.
pub fn tools_env(tools: &Path) -> Vec<(String, String)> {
    let at = |name: &str| tools.join(name).to_string_lossy().into_owned();
    vec![
        ("XDG_CACHE_HOME".to_string(), at("cache")),
        ("BUNDLE_PATH".to_string(), at("bundle")),
        ("CARGO_HOME".to_string(), at("cargo")),
        ("GOPATH".to_string(), at("go")),
        ("GOMODCACHE".to_string(), at("go/pkg/mod")),
        ("npm_config_cache".to_string(), at("npm")),
    ]
}

/// mise's installs, where this folder's fallback made them ([`mise_line`]),
/// for the variables that say so. Nothing where it made none.
pub fn tools_mise_env(tools: &Path) -> Vec<(String, String)> {
    let installs = tools.join("mise").join("installs");
    if !installs.is_dir() {
        return Vec::new();
    }
    let installs = installs.to_string_lossy().into_owned();
    vec![
        ("MISE_INSTALLS_DIR".to_string(), installs.clone()),
        ("MISE_INSTALL_STORE_DIR".to_string(), installs),
    ]
}

/// What goes on a run's `PATH` for them.
pub fn tools_path(tools: &Path) -> Vec<String> {
    vec![
        tools
            .join("cargo")
            .join("bin")
            .to_string_lossy()
            .into_owned(),
    ]
}

/// A step of preparing, as a chat shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub key: String,
    /// While it runs: "Copying the repository".
    pub doing: String,
    /// Once it's done: "Copied the repository".
    pub done: String,
    /// `pending`, `running`, `done`, `skipped` or `failed`.
    pub state: String,
}

impl Step {
    fn new(key: &str, doing: impl Into<String>, done: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            doing: doing.into(),
            done: done.into(),
            state: "pending".into(),
        }
    }
}

/// Where preparing a repository stands, as the server hears it. `record` is
/// the repository as the config keeps it, for the daemon, and never leaves
/// this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// `running`, `ready`, `failed`, or `needs_setup`, with `need` saying
    /// what's missing.
    pub state: String,
    pub repository: String,
    /// `not_allowed`, `no_network`, `not_installed`, `not_signed_in`,
    /// `not_reached` (the person's Git login doesn't reach it) or
    /// `other_owner` (the person hasn't said yes to its owner here), or
    /// `not_available` (the agent can't run in the sandbox on this system).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default)]
    pub billing: Billing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// The checkout's folder, in the shared folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_command: Option<String>,
    /// `passed`, `failed`, `skipped` (the person went on without them) or
    /// `none` (it couldn't tell how to run them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// The step that failed, by the words it was doing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    /// The end of what it printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Repository>,
}

impl View {
    fn needs(repository: &str, need: &str, agent: Option<Agent>) -> Self {
        Self {
            state: "needs_setup".into(),
            repository: repository.into(),
            need: Some(need.into()),
            agent: agent.map(|a| a.key().to_string()),
            ..Default::default()
        }
    }
}

/// What a bot's run asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub bot: String,
    pub repository: String,
    /// The agent its bot uses; absent, the first this loan allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// False once the person chose to go on with its tests failing.
    #[serde(default = "yes")]
    pub tests: bool,
}

fn yes() -> bool {
    true
}

/// The preparing going on, and how each ended, by repository and bot.
#[derive(Clone, Default)]
pub struct Preparations {
    going: Arc<Mutex<HashMap<String, Arc<Mutex<View>>>>>,
    /// Bumped when every run is stopped (the worker restarted, the computer
    /// was signed out): a preparation started before then stops too.
    epoch: Arc<std::sync::atomic::AtomicU64>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the app's dialog shows: what this computer has for each agent, and
/// whether the person's Git login reaches the repository. Nothing here widens
/// anything, and it holds no token. Only a same-user client asks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub agents: Vec<AgentLook>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaches: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLook {
    pub agent: String,
    pub name: String,
    pub installed: bool,
    pub signed_in: bool,
    pub billing: Billing,
    pub sign_in_hint: String,
    /// Why it can't run in a bot's sandbox here, if it can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// Look, for `repository` if the dialog names one.
pub fn look(config: &Config, paths: &Paths, repository: Option<&str>) -> Look {
    let home = paths.person_home.as_deref();
    let agents = Agent::ALL
        .into_iter()
        .map(|agent| {
            let login = home.map(|h| Login::find_for(agent, h)).unwrap_or_default();
            AgentLook {
                agent: agent.key().into(),
                name: agent.name().into(),
                installed: setup::program_for_run(agent, &config.coding, &config.roots).is_some(),
                signed_in: login.signed_in(),
                billing: login.billing,
                sign_in_hint: agent.sign_in_hint().into(),
                unavailable: agent.unavailable().map(String::from),
            }
        })
        .collect();
    let reaches = repository
        .and_then(split_repository)
        .map(|(owner, name)| reachable_url(owner, name, LOOK_LIMIT).is_some());
    Look { agents, reaches }
}

/// The address the person's Git login reaches `owner/name` by, SSH first:
/// a bot's run never names one. Each try gives up within `limit` and asks for
/// nothing.
pub fn reachable_url(owner: &str, name: &str, limit: Duration) -> Option<String> {
    let addresses = [
        format!("git@github.com:{owner}/{name}.git"),
        format!("https://github.com/{owner}/{name}.git"),
    ];
    addresses
        .into_iter()
        .find(|url| repositories::reaches(url, limit))
}

/// The folder and agent `ask` may use, as the config says now, or what's
/// missing. Never a folder not lent to the bot.
fn choose(config: &Config, paths: &Paths, ask: &Ask) -> Result<(Root, Agent), Box<View>> {
    let lent: Vec<&Root> = config
        .roots
        .iter()
        .filter(|root| root.runs_commands() && root.loan(&ask.bot).is_some())
        .collect();
    let named = match ask.agent.as_deref() {
        Some(key) => Some(
            Agent::from_key(key)
                .ok_or_else(|| Box::new(View::needs(&ask.repository, "not_available", None)))?,
        ),
        None => None,
    };
    if let Some(agent) = named.filter(|agent| agent.unavailable().is_some()) {
        return Err(Box::new(View::needs(
            &ask.repository,
            "not_available",
            Some(agent),
        )));
    }
    let wanted: Vec<Agent> = named.map_or_else(
        || {
            Agent::ALL
                .into_iter()
                .filter(|agent| agent.unavailable().is_none())
                .collect()
        },
        |agent| vec![agent],
    );
    // The folder that holds this repository already, else one whose loan
    // names its owner, else any: a ready folder later on is never passed over.
    let owner = split_repository(&ask.repository).map(|(owner, _)| owner);
    let eligible = |root: &Root| {
        recorded(config, &root.id, &ask.repository).is_some()
            || root.loan(&ask.bot).is_some_and(|loan| {
                owner.is_some_and(|owner| loan.owners.iter().any(|o| o.eq_ignore_ascii_case(owner)))
            })
    };
    let ready = |root: &&Root| {
        wanted
            .iter()
            .find(|agent| super::agent_state(**agent, root, &ask.bot) == AgentState::Ready)
            .map(|agent| ((*root).clone(), *agent))
    };
    let found = lent
        .iter()
        .filter(|root| eligible(root))
        .find_map(ready)
        .or_else(|| lent.iter().find_map(ready));
    let Some((root, agent)) = found else {
        let need = if lent.iter().any(|root| {
            wanted
                .iter()
                .any(|agent| super::agent_state(*agent, root, &ask.bot) == AgentState::NoNetwork)
        }) {
            "no_network"
        } else {
            "not_allowed"
        };
        return Err(Box::new(View::needs(&ask.repository, need, named)));
    };
    if setup::program_for_run(agent, &config.coding, &config.roots).is_none() {
        return Err(Box::new(View::needs(
            &ask.repository,
            "not_installed",
            Some(agent),
        )));
    }
    let login = paths
        .person_home
        .as_deref()
        .map(|home| Login::find_for(agent, home))
        .unwrap_or_default();
    if !login.signed_in() {
        return Err(Box::new(View::needs(
            &ask.repository,
            "not_signed_in",
            Some(agent),
        )));
    }
    Ok((root, agent))
}

/// The repository as the config keeps it in `root`, by the name a bot's run
/// gave, whether a run or `cww coding setup` made it.
fn recorded(config: &Config, root: &str, repository: &str) -> Option<Repository> {
    config
        .coding
        .repositories
        .iter()
        .find(|repo| {
            repo.root == root
                && (repo
                    .github
                    .as_deref()
                    .is_some_and(|g| g.eq_ignore_ascii_case(repository))
                    || github_name(&repo.url).is_some_and(|g| g.eq_ignore_ascii_case(repository)))
        })
        .cloned()
}

/// The checkout's name for `owner/name` in `root`: its own name, or, where
/// that's taken by another repository or something of the person's in the
/// folder, `owner-name`. A folder Desktop didn't make is never taken over.
fn checkout_name(config: &Config, root: &Root, owner: &str, name: &str) -> Option<String> {
    let taken = |candidate: &str| {
        config
            .coding
            .repositories
            .iter()
            .any(|repo| repo.root == root.id && repo.name == candidate)
            || std::fs::symlink_metadata(root.path.join(candidate)).is_ok()
    };
    if !taken(name) {
        return Some(name.to_string());
    }
    let both = format!("{owner}-{name}");
    (repositories::valid_name(&both) && !taken(&both)).then_some(both)
}

impl Preparations {
    /// Stop every preparation going on, at its next step.
    pub fn stop_all(&self) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn key(ask: &Ask) -> String {
        format!("{}\n{}", ask.bot, ask.repository.to_ascii_lowercase())
    }

    /// Where it stands, as this process knows it.
    pub fn status(&self, ask: &Ask) -> Option<View> {
        let cell = lock(&self.going).get(&Self::key(ask)).cloned()?;
        let view = lock(&cell).clone();
        Some(view)
    }

    /// Prepare what `ask` names, or say it's prepared or what's missing:
    /// the answer comes at once, and [`Self::status`] follows it.
    pub fn start(
        &self,
        runs: &Runs,
        config: &Config,
        paths: &Paths,
        ask: &Ask,
    ) -> Result<View, RunError> {
        let Some((owner, name)) = split_repository(&ask.repository) else {
            return Err(RunError::new(
                "invalid",
                "a repository is named owner/name, on GitHub",
            ));
        };
        if config.paused {
            return Err(RunError::new(
                "paused",
                "this computer is paused; resume it to run commands",
            ));
        }
        let (root, agent) = match choose(config, paths, ask) {
            Ok(chosen) => chosen,
            Err(view) => return Ok(*view),
        };
        let existing = recorded(config, &root.id, &ask.repository);
        let allowed = root
            .loan(&ask.bot)
            .is_some_and(|loan| loan.owners.iter().any(|o| o.eq_ignore_ascii_case(owner)));
        if existing.is_none() && !allowed {
            return Ok(View::needs(&ask.repository, "other_owner", Some(agent)));
        }
        let billing = folder_billing(config, paths, &root.id, agent).unwrap_or_else(|| {
            paths
                .person_home
                .as_deref()
                .map(|home| Login::find_for(agent, home).billing)
                .unwrap_or_default()
        });
        if let Some(repo) = &existing
            && let Some(view) = already_prepared(paths, repo, &root, ask, agent, billing)
        {
            return Ok(view);
        }
        let key = Self::key(ask);
        let cell = {
            let mut going = lock(&self.going);
            if let Some(cell) = going.get(&key)
                && lock(cell).state == "running"
            {
                let view = lock(cell).clone();
                return Ok(view);
            }
            let cell = Arc::new(Mutex::new(View {
                state: "running".into(),
                repository: ask.repository.clone(),
                agent: Some(agent.key().to_string()),
                billing,
                root: Some(root.id.clone()),
                steps: vec![Step::new(
                    "copy",
                    "Copying the repository",
                    "Copied the repository",
                )],
                ..Default::default()
            }));
            going.insert(key, cell.clone());
            cell
        };
        let view = lock(&cell).clone();
        let work = Work {
            runs: runs.clone(),
            paths: paths.clone(),
            ask: ask.clone(),
            owner: owner.to_string(),
            name: name.to_string(),
            root,
            existing,
            cell,
            epoch: (
                Arc::clone(&self.epoch),
                self.epoch.load(std::sync::atomic::Ordering::SeqCst),
            ),
        };
        std::thread::Builder::new()
            .name("cww-prepare".into())
            .spawn(move || work.run())
            .map_err(|_| RunError::new("unavailable", "couldn't start preparing"))?;
        Ok(view)
    }
}

/// An API key the folder's own environment gives runs (`cww roots env`)
/// takes over from the sign-in, so who pays says so.
fn folder_billing(config: &Config, paths: &Paths, root: &str, agent: Agent) -> Option<Billing> {
    let key = match agent {
        Agent::ClaudeCode => "ANTHROPIC_API_KEY",
        Agent::Codex => "OPENAI_API_KEY",
    };
    crate::commands::env::names_for(config, paths, root)
        .iter()
        .any(|name| name == key)
        .then_some(Billing::ApiKey)
}

/// A repository's answer when it needs no work: one a run prepared before, or
/// one the person set up with `cww coding setup`, whose checkout is there.
fn already_prepared(
    paths: &Paths,
    repo: &Repository,
    root: &Root,
    ask: &Ask,
    agent: Agent,
    billing: Billing,
) -> Option<View> {
    let mirror = repositories::mirror_dir(paths, repo);
    let kept: Option<Prepared> = std::fs::read(mirror.join(PREPARED))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    // A run's own is done only once it said so; the person's needs no proof.
    if repo.github.is_some() && kept.is_none() {
        return None;
    }
    if !mirror.join("HEAD").is_file() {
        return None;
    }
    let folder = crate::roots::canonical(&root.path).ok()?;
    let checkout = crate::roots::canonical(&root.path.join(&repo.name)).ok()?;
    if !checkout.starts_with(&folder) || !checkout.is_dir() {
        return None;
    }
    Some(View {
        state: "ready".into(),
        repository: ask.repository.clone(),
        agent: Some(agent.key().to_string()),
        billing,
        root: Some(root.id.clone()),
        checkout: Some(repo.name.clone()),
        test_command: kept.as_ref().and_then(|k| k.test_command.clone()),
        tests: kept.map(|k| k.tests),
        record: Some(repo.clone()),
        ..Default::default()
    })
}

/// What a mirror keeps of how it was prepared.
#[derive(Debug, Serialize, Deserialize)]
struct Prepared {
    test_command: Option<String>,
    tests: String,
}

struct Work {
    runs: Runs,
    paths: Paths,
    ask: Ask,
    owner: String,
    name: String,
    root: Root,
    existing: Option<Repository>,
    cell: Arc<Mutex<View>>,
    epoch: (Arc<std::sync::atomic::AtomicU64>, u64),
}

impl Work {
    fn run(self) {
        if let Err((step, detail)) = self.prepare() {
            let mut view = lock(&self.cell);
            for each in &mut view.steps {
                if each.key == step {
                    each.state = "failed".into();
                }
            }
            let doing = view
                .steps
                .iter()
                .find(|s| s.key == step)
                .map(|s| s.doing.clone());
            view.state = "failed".into();
            view.failed_step = doing;
            view.detail = Some(end_of(&detail));
            tracing::info!(step = %step, "preparing a repository failed");
        }
    }

    fn set_step(&self, key: &str, state: &str) {
        let mut view = lock(&self.cell);
        if let Some(step) = view.steps.iter_mut().find(|s| s.key == key) {
            step.state = state.into();
        }
    }

    fn prepare(&self) -> Result<(), (String, String)> {
        let copy = "copy".to_string();
        self.set_step(&copy, "running");
        let repo = self.repository().map_err(|e| (copy.clone(), e))?;
        let mirror = {
            let config = self.config().map_err(|e| (copy.clone(), e))?;
            repositories::create_mirror(&self.paths, &repo, &config.roots)
                .map_err(|e| (copy.clone(), format!("{e:#}")))?
        };
        // Kept first: a run reads only the mirrors of the folder's repositories.
        self.record(&repo).map_err(|e| (copy.clone(), e))?;
        lock(&self.cell).checkout = Some(repo.name.clone());
        lock(&self.cell).record = Some(repo.clone());
        // Fresh, for a repository kept from before.
        if let Err(e) = repositories::fetch(&mirror, Some(&repo)) {
            tracing::info!(error = %e, "fetching a repository's mirror failed");
        }
        let exists = std::fs::symlink_metadata(self.root.path.join(&repo.name)).is_ok();
        let line = repositories::checkout_command(&repo.name, &mirror, exists, cfg!(windows));
        self.command("", &line).map_err(|e| (copy.clone(), e))?;
        self.set_step(&copy, "done");

        // What it needs, read from a private copy: never the checkout in the
        // shared folder, whose files a command could have changed.
        let (needs, test_command) = self.read_needs(&repo, &mirror).map_err(|e| (copy, e))?;
        {
            let mut view = lock(&self.cell);
            view.test_command = test_command.clone();
            for need in needs.iter().filter(|need| need.install.is_some()) {
                view.steps.push(Step::new(
                    need.program,
                    format!("Installing {}", need.what),
                    format!("Installed {}", need.what),
                ));
            }
            view.steps.push(Step::new(
                "tests",
                "Running its tests once",
                "Ran its tests once",
            ));
        }
        let with_mise = needs.iter().any(|need| need.program == "mise");
        for need in &needs {
            let Some(install) = &need.install else {
                continue;
            };
            self.set_step(need.program, "running");
            let line = install_line(install, need.program, with_mise, &self.tools());
            self.command(&repo.name, &line)
                .map_err(|e| (need.program.to_string(), e))?;
            self.set_step(need.program, "done");
        }

        let tests = "tests".to_string();
        let outcome = match (&test_command, self.ask.tests) {
            (None, _) => {
                self.set_step(&tests, "skipped");
                "none"
            }
            (Some(_), false) => {
                self.set_step(&tests, "skipped");
                "skipped"
            }
            (Some(command), true) => {
                self.set_step(&tests, "running");
                let line = with_toolchain(command, with_mise);
                self.command(&repo.name, &line)
                    .map_err(|e| (tests.clone(), e))?;
                self.set_step(&tests, "done");
                "passed"
            }
        };
        let kept = Prepared {
            test_command,
            tests: outcome.into(),
        };
        crate::paths::write_private_file(
            &mirror.join(PREPARED),
            &serde_json::to_vec(&kept).unwrap_or_default(),
        )
        .map_err(|e| {
            (
                tests.clone(),
                format!("couldn't keep that it's prepared: {e:#}"),
            )
        })?;
        let mut view = lock(&self.cell);
        view.tests = Some(outcome.into());
        view.state = "ready".into();
        Ok(())
    }

    fn config(&self) -> Result<Config, String> {
        Config::load(&self.paths).map_err(|e| format!("{e:#}"))
    }

    fn tools(&self) -> PathBuf {
        crate::roots::canonical(&self.root.path)
            .unwrap_or_else(|_| self.root.path.clone())
            .join(TOOLS_DIR)
    }

    /// The repository as the config will keep it: its address is what the
    /// person's Git login reaches.
    fn repository(&self) -> Result<Repository, String> {
        if let Some(repo) = &self.existing {
            return Ok(repo.clone());
        }
        let url = reachable_url(&self.owner, &self.name, REACH_LIMIT).ok_or_else(|| {
            format!(
                "your Git login on this computer doesn't reach {}/{}",
                self.owner, self.name
            )
        })?;
        let config = self.config()?;
        let name = checkout_name(&config, &self.root, &self.owner, &self.name)
            .ok_or("the folder has something of that name already")?;
        Ok(Repository {
            root: self.root.id.clone(),
            name,
            url,
            ssh_key: None,
            github: Some(format!("{}/{}", self.owner, self.name)),
        })
    }

    /// Keep the repository, under the settings' lock, and only while the
    /// person still lends this folder to the bot for it.
    fn record(&self, repo: &Repository) -> Result<(), String> {
        Config::edit(&self.paths, |config| {
            self.still_allowed(config).map_err(anyhow::Error::msg)?;
            if !config.coding.repositories.contains(repo) {
                config.coding.repositories.push(repo.clone());
            }
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))
    }

    /// Whether the person still lends the folder to this bot with commands, the
    /// network and an agent, as when it started: checked before every step.
    fn still_allowed(&self, config: &Config) -> Result<(), String> {
        let lent = config
            .roots
            .iter()
            .find(|root| root.id == self.root.id)
            .filter(|root| root.runs_commands() && root.network)
            .and_then(|root| root.loan(&self.ask.bot))
            .is_some_and(|loan| Agent::ALL.into_iter().any(|agent| agent.allowed(loan)));
        if self.epoch.0.load(std::sync::atomic::Ordering::SeqCst) != self.epoch.1 {
            Err("this computer stopped its commands".into())
        } else if config.paused {
            Err("this computer was paused".into())
        } else if lent {
            Ok(())
        } else {
            Err("the folder isn't lent to this bot for that any more".into())
        }
    }

    fn read_needs(
        &self,
        repo: &Repository,
        mirror: &Path,
    ) -> Result<(Vec<detect::Need>, Option<String>), String> {
        let staging = self
            .paths
            .data_dir
            .join("staging")
            .join(&repo.root)
            .join(format!(
                "{}-{}-{}",
                repo.name,
                self.ask.bot,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
        let _ = std::fs::remove_dir_all(&staging);
        let result = repositories::private_copy(mirror, &staging)
            .map(|()| (detect::needs(&staging), detect::test_command(&staging)))
            .map_err(|e| format!("{e:#}"));
        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    /// Run `line` in the sandbox, in `dir` of the folder, as a person's
    /// command, with no agent's sign-in: what a repository's install and
    /// test code never needs. The end of what it printed, if it fails.
    fn command(&self, dir: &str, line: &str) -> Result<(), String> {
        let request = Request {
            root: self.root.id.clone(),
            dir: dir.to_string(),
            command: line.to_string(),
            bot: None,
        };
        let waited = Instant::now();
        let status = loop {
            let config = self.config()?;
            self.still_allowed(&config)?;
            match self.runs.start(&config, &request) {
                Ok(status) => break status,
                Err(e) if e.code == "busy" && waited.elapsed() < ROOM_WAIT => {
                    std::thread::sleep(Duration::from_secs(5));
                }
                Err(e) => return Err(e.message),
            }
        };
        let id = status.id;
        loop {
            let status = self.runs.status(&id).map_err(|e| e.message)?;
            if status.state != "running" {
                let finished = self.runs.finished(&id).map_err(|e| e.message)?;
                if status.state == "exited" && status.exit_code == Some(0) {
                    return Ok(());
                }
                return Err(format!(
                    "{}{}\n{}\n{}",
                    status.state,
                    status
                        .exit_code
                        .map(|c| format!(" with {c}"))
                        .unwrap_or_default(),
                    finished.stderr,
                    finished.stdout
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// The end of what a step printed.
fn end_of(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= DETAIL_CHARS {
        return text.to_string();
    }
    text.chars().skip(count - DETAIL_CHARS).collect()
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `command`, through the repository's own mise environment where it uses
/// mise, so its Ruby or Node is the one that runs.
fn with_toolchain(command: &str, with_mise: bool) -> String {
    if !with_mise || command.starts_with("mise ") {
        return command.to_string();
    }
    if cfg!(windows) {
        format!("mise exec -- cmd /C {command}")
    } else {
        format!("mise exec -- sh -c {}", shell_quote(command))
    }
}

/// The line that installs what `program` is for.
fn install_line(install: &str, program: &str, with_mise: bool, tools: &Path) -> String {
    if program == "mise" {
        return mise_line(tools);
    }
    with_toolchain(install, with_mise)
}

/// `mise install`. What this computer has already installed is enough, and
/// nothing is written. A run can't write mise's installs, so what's missing
/// goes in the folder's own place, beside links to what this computer has,
/// and from then on the folder's runs use those (`tools_mise_env`).
fn mise_line(tools: &Path) -> String {
    if cfg!(windows) {
        return "mise trust --yes; mise install".into();
    }
    let installs = tools.join("mise").join("installs");
    let store = shell_quote(&installs.to_string_lossy());
    format!(
        "mise trust --yes >/dev/null; h=\"$MISE_INSTALLS_DIR\"; if [ -z \"$h\" ] || ! mise install; \
         then l={store}; mkdir -p \"$l\"; for t in \"$h\"/*/; do [ -d \"$t\" ] || continue; \
         t=\"${{t%/}}\"; n=\"${{t##*/}}\"; mkdir -p \"$l/$n\"; ln -sf \"$t\"/* \"$l/$n/\" 2>/dev/null; \
         done; MISE_INSTALLS_DIR=\"$l\" MISE_INSTALL_STORE_DIR=\"$l\" mise install; fi"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BotLoan;

    #[test]
    fn a_repository_is_only_ever_owner_and_name() {
        assert_eq!(split_repository("acme/app"), Some(("acme", "app")));
        for refused in [
            "acme",
            "acme/app/extra",
            "../app",
            "acme/..",
            "-x/app",
            "acme/.git",
            "git@github.com:acme/app",
            "https://github.com/acme/app",
            "acme/app;rm",
            "acme/app name",
            "ext::sh/x",
            "",
        ] {
            assert_eq!(split_repository(refused), None, "{refused}");
        }
    }

    #[test]
    fn github_names_come_from_any_of_its_addresses() {
        for url in [
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app.git",
            "https://github.com/acme/app.git",
            "https://github.com/acme/app",
        ] {
            assert_eq!(github_name(url).as_deref(), Some("acme/app"), "{url}");
        }
        assert_eq!(github_name("git@gitlab.com:acme/app.git"), None);
    }

    fn fixture(owners: &[&str], claude_code: bool) -> (tempfile::TempDir, Config, Paths, Ask) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("bots");
        std::fs::create_dir_all(&folder).unwrap();
        let mut paths = Paths::under(&dir.path().join("cww"));
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaa","subscriptionType":"max"}}"#,
        )
        .unwrap();
        paths.person_home = Some(home);
        let program = dir.path().join("claude");
        std::fs::write(&program, "").unwrap();
        let config = Config {
            coding: crate::config::CodingConfig {
                claude: Some(program),
                ..Default::default()
            },
            roots: vec![Root {
                id: "bots".into(),
                label: "bots".into(),
                path: folder,
                follow_symlinks: false,
                writable: true,
                commands: true,
                network: true,
                read_budget: None,
                bots: vec![BotLoan {
                    id: "42".into(),
                    name: "Fixer".into(),
                    unattended: false,
                    claude_code,
                    codex: false,
                    owners: owners.iter().map(|o| o.to_string()).collect(),
                }],
            }],
            ..Config::default()
        };
        let ask = Ask {
            bot: "42".into(),
            repository: "acme/app".into(),
            agent: None,
            tests: true,
        };
        (dir, config, paths, ask)
    }

    /// The server can name a repository, and that's all: another bot, an agent
    /// not allowed, or an owner the person never said yes to gets nothing,
    /// and nothing is fetched.
    #[test]
    fn only_what_the_person_allowed_here_is_prepared() {
        let (_dir, config, paths, ask) = fixture(&["acme"], true);
        let runs = Runs::new(paths.clone());
        let preparations = Preparations::default();

        let mut other_bot = ask.clone();
        other_bot.bot = "7".into();
        let view = preparations
            .start(&runs, &config, &paths, &other_bot)
            .unwrap();
        assert_eq!(view.need.as_deref(), Some("not_allowed"));

        let mut other_owner = ask.clone();
        other_owner.repository = "someone-else/secrets".into();
        let view = preparations
            .start(&runs, &config, &paths, &other_owner)
            .unwrap();
        assert_eq!(view.state, "needs_setup");
        assert_eq!(view.need.as_deref(), Some("other_owner"));

        let mut codex = ask.clone();
        codex.agent = Some("codex".into());
        let view = preparations.start(&runs, &config, &paths, &codex).unwrap();
        let expected = if cfg!(target_os = "macos") {
            "not_available"
        } else {
            "not_allowed"
        };
        assert_eq!(view.need.as_deref(), Some(expected));

        let (_dir, config, paths, ask) = fixture(&["acme"], false);
        let view = preparations.start(&runs, &config, &paths, &ask).unwrap();
        assert_eq!(view.need.as_deref(), Some("not_allowed"));

        let (_dir, mut config, paths, ask) = fixture(&["acme"], true);
        config.paused = true;
        assert_eq!(
            preparations
                .start(&runs, &config, &paths, &ask)
                .unwrap_err()
                .code,
            "paused"
        );
    }

    #[test]
    fn what_the_computer_lacks_is_said_by_name() {
        let (dir, mut config, mut paths, ask) = fixture(&["acme"], true);
        let runs = Runs::new(paths.clone());
        let preparations = Preparations::default();
        std::fs::remove_file(dir.path().join("home/.claude/.credentials.json")).unwrap();
        let view = preparations.start(&runs, &config, &paths, &ask).unwrap();
        assert_eq!(view.need.as_deref(), Some("not_signed_in"));
        assert_eq!(view.agent.as_deref(), Some("claude_code"));

        config.roots[0].network = false;
        let view = preparations.start(&runs, &config, &paths, &ask).unwrap();
        assert_eq!(view.need.as_deref(), Some("no_network"));
        paths.person_home = None;
        assert!(preparations.status(&ask).is_none());
    }

    #[test]
    fn a_folder_the_person_already_has_is_never_taken_over() {
        let (_dir, mut config, _paths, _ask) = fixture(&[], true);
        let root = config.roots[0].clone();
        assert_eq!(
            checkout_name(&config, &root, "acme", "app").as_deref(),
            Some("app")
        );
        std::fs::create_dir_all(root.path.join("app")).unwrap();
        assert_eq!(
            checkout_name(&config, &root, "acme", "app").as_deref(),
            Some("acme-app")
        );
        config.coding.repositories.push(Repository {
            root: "bots".into(),
            name: "acme-app".into(),
            url: "git@github.com:acme/app.git".into(),
            ssh_key: None,
            github: None,
        });
        assert_eq!(checkout_name(&config, &root, "acme", "app"), None);
    }

    #[test]
    fn nothing_outlasts_a_run_until_a_run_prepared_a_repository() {
        let (_dir, mut config, _paths, _ask) = fixture(&[], true);
        assert_eq!(tools_dir(&config, "bots"), None);
        config.coding.repositories.push(Repository {
            root: "bots".into(),
            name: "app".into(),
            url: "git@github.com:acme/app.git".into(),
            ssh_key: None,
            github: None,
        });
        assert_eq!(tools_dir(&config, "bots"), None, "the person's own setup");
        config.coding.repositories[0].github = Some("acme/app".into());
        let tools = tools_dir(&config, "bots").unwrap();
        assert!(tools.ends_with(TOOLS_DIR));
        assert!(
            tools_env(&tools)
                .iter()
                .any(|(name, _)| name == "BUNDLE_PATH")
        );
        assert!(tools_mise_env(&tools).is_empty());
        std::fs::create_dir_all(tools.join("mise/installs")).unwrap();
        assert_eq!(tools_mise_env(&tools).len(), 2);
    }

    #[test]
    fn a_program_in_the_repositorys_toolchain_runs_through_it() {
        assert_eq!(with_toolchain("cargo test", false), "cargo test");
        assert_eq!(with_toolchain("mise run test", true), "mise run test");
        if !cfg!(windows) {
            assert_eq!(
                with_toolchain("bin/rails test", true),
                "mise exec -- sh -c 'bin/rails test'"
            );
        }
    }
}
