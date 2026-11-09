//! Trusting the checkouts Desktop made for a coding bot, so Claude Code and
//! Codex read the repository's own settings in a bot's run
//! (docs/coding-computers.md, "Trusting its checkouts").
//!
//! Both keep a workspace's trust in their own settings, keyed by the
//! folder: Claude Code in `~/.claude.json`
//! (`projects["<folder>"].hasTrustDialogAccepted`), Codex in
//! `~/.codex/config.toml` (`[projects."<folder>"] trust_level = "trusted"`).
//! An untrusted one still runs `claude -p` and `codex exec`, but without the
//! repository's `.claude/settings*.json` permissions or `.codex/config.toml`,
//! and Claude Code says so on every run ("this workspace has not been
//! trusted"), which a bot read as something to ask its team about. A git
//! worktree counts as the repository it was made from: Claude Code looks
//! only at the main checkout's entry, so trusting the checkout covers every
//! worktree a bot makes from it.
//!
//! Only a checkout Desktop cloned from its mirror, in the sandbox, as the
//! bot ([`made_by_desktop`]), is trusted. One that was in the folder before,
//! which Desktop only pointed at its mirror, keeps asking as before: that's
//! the person's to trust. Nothing here widens what a run may read, write or
//! reach: those are the sandbox's, whatever the agent's settings say.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{Agent, AgentRun, CONFIG_FILE};
use crate::config::Repository;

/// Codex's settings, in its folder.
pub const CODEX_CONFIG_FILE: &str = "config.toml";
/// Larger settings than this are left alone.
const MOST: u64 = 64 * 1024 * 1024;
/// A reflog line larger than this isn't git's.
const REFLOG_MOST: u64 = 64 * 1024;

/// Whether Desktop made `repo`'s checkout `checkout` from its mirror
/// `mirror`: as recorded since 0.16.1, or, for one recorded before, when
/// the checkout's first reflog entry is git's own for a clone from that
/// mirror. A checkout that was there already starts from somewhere else.
pub fn made_by_desktop(repo: &Repository, checkout: &Path, mirror: &Path) -> bool {
    repo.cloned.unwrap_or_else(|| cloned_from(checkout, mirror))
}

/// Records whether Desktop made the checkout of `root`'s `name`, once the
/// command that makes it or points it at the mirror succeeded: made when
/// there was nothing there (`existed` false), else as before.
pub fn note_checkout(
    repos: &mut [Repository],
    root: &str,
    name: &str,
    existed: bool,
    checkout: &Path,
    mirror: &Path,
) {
    if let Some(repo) = repos.iter_mut().find(|r| r.root == root && r.name == name) {
        repo.cloned = Some(!existed || made_by_desktop(repo, checkout, mirror));
    }
}

/// Whether the checkout's history starts with a clone from `mirror`.
pub fn cloned_from(checkout: &Path, mirror: &Path) -> bool {
    let log = checkout.join(".git").join("logs").join("HEAD");
    // Git's own files, never one a link leads to.
    let plain = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink());
    if !plain(&checkout.join(".git")) || !plain(&log) {
        return false;
    }
    let Ok(file) = std::fs::File::open(&log) else {
        return false;
    };
    use std::io::{BufRead, Read};
    let mut first = String::new();
    if std::io::BufReader::new(file.take(REFLOG_MOST))
        .read_line(&mut first)
        .is_err()
    {
        return false;
    }
    first
        .split_once('\t')
        .and_then(|(_, message)| {
            message
                .trim_end_matches(['\r', '\n'])
                .strip_prefix("clone: from ")
        })
        .is_some_and(|from| Path::new(from) == mirror)
}

/// The checkouts in the folder at `root` that a run there trusts: those
/// Desktop made, where they really are, inside the folder.
pub fn checkouts(root: &Path, repos: &[(PathBuf, Repository)]) -> Vec<PathBuf> {
    let Ok(root) = crate::roots::canonical(root) else {
        return Vec::new();
    };
    repos
        .iter()
        .filter_map(|(mirror, repo)| {
            let checkout = root.join(&repo.name);
            // Never through a link a command left in the folder.
            let real = std::fs::symlink_metadata(&checkout)
                .ok()
                .filter(|m| m.is_dir())
                .and_then(|_| crate::roots::canonical(&checkout).ok())
                .filter(|real| real.parent() == Some(root.as_path()))?;
            made_by_desktop(repo, &real, mirror).then_some(real)
        })
        .collect()
}

/// Before a run: the agents it gets trust the checkouts it may work in.
/// Each settings file is written only when something is missing, and a
/// failure leaves the run as it would have been, untrusted.
pub fn seed(agents: &[AgentRun], checkouts: &[PathBuf]) {
    if checkouts.is_empty() {
        return;
    }
    for run in agents {
        let written = match run.agent {
            Agent::ClaudeCode => claude_file(run).map(|file| trust_in_claude(&file, checkouts)),
            Agent::Codex => run
                .login
                .dir
                .as_ref()
                .map(|dir| trust_in_codex(&dir.join(CODEX_CONFIG_FILE), checkouts)),
            _ => None,
        };
        match written {
            Some(Ok(0)) | None => {}
            Some(Ok(count)) => {
                tracing::info!(agent = run.agent.key(), count, "trusted a bot's checkouts")
            }
            // Without the error's words, which can name the file.
            Some(Err(_)) => {
                tracing::warn!(agent = run.agent.key(), "couldn't trust a bot's checkouts")
            }
        }
    }
}

/// The file the run's Claude Code reads its settings from: `~/.claude.json`,
/// or on Windows, where a run names its folder with `CLAUDE_CONFIG_DIR`,
/// the `.claude.json` inside that folder.
fn claude_file(run: &AgentRun) -> Option<PathBuf> {
    if cfg!(windows) {
        run.login
            .dir
            .as_ref()
            .map(|dir| dir.join(CONFIG_FILE))
            .filter(|file| file.is_file())
    } else {
        run.login.file.clone()
    }
}

/// The keys Claude Code may know `checkout` by: the folder as it is, and on
/// Windows also with forward slashes, as it writes them there.
fn claude_keys(checkout: &Path) -> Vec<String> {
    let native = checkout.to_string_lossy().into_owned();
    let mut keys = vec![native.clone()];
    if cfg!(windows) {
        let forward = native.replace('\\', "/");
        if forward != native {
            keys.push(forward);
        }
    }
    keys
}

/// Marks each checkout trusted in Claude Code's settings `file`, keeping
/// everything else in it. How many entries it marked.
pub fn trust_in_claude(file: &Path, checkouts: &[PathBuf]) -> anyhow::Result<usize> {
    let mut settings: Value = serde_json::from_slice(&read(file)?)?;
    let Some(object) = settings.as_object_mut() else {
        anyhow::bail!("not an object");
    };
    let projects = object
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else {
        anyhow::bail!("projects isn't an object");
    };
    let mut marked = 0;
    for key in checkouts.iter().flat_map(|c| claude_keys(c)) {
        let project = projects
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(project) = project.as_object_mut()
            && project.get("hasTrustDialogAccepted") != Some(&Value::Bool(true))
        {
            project.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            marked += 1;
        }
    }
    if marked > 0 {
        let mut text = serde_json::to_vec_pretty(&settings)?;
        text.push(b'\n');
        replace(file, &text)?;
    }
    Ok(marked)
}

/// Marks each checkout trusted in Codex's settings `file`, adding a table
/// at its end so the person's comments and layout stay. A folder the person
/// set to another trust level ("untrusted") keeps it, and one whose table
/// the file already has without a level is left alone, since a second table
/// of that name isn't TOML. How many it marked.
pub fn trust_in_codex(file: &Path, checkouts: &[PathBuf]) -> anyhow::Result<usize> {
    let mut text = if file.exists() {
        String::from_utf8(read(file)?)?
    } else {
        String::new()
    };
    let settings: toml::Table = toml::from_str(&text)?;
    let projects = settings.get("projects").and_then(|p| p.as_table());
    let mut marked = 0;
    for key in checkouts.iter().map(|c| c.to_string_lossy().into_owned()) {
        if projects.is_some_and(|p| p.contains_key(&key)) {
            continue;
        }
        let mut added = text.clone();
        if !added.is_empty() && !added.ends_with('\n') {
            added.push('\n');
        }
        added.push_str(&format!(
            "\n[projects.{}]\ntrust_level = \"trusted\"\n",
            toml::Value::String(key.clone())
        ));
        // Only what still reads as Codex's settings, with the folder trusted.
        let trusted = toml::from_str::<toml::Table>(&added).is_ok_and(|t| {
            t.get("projects")
                .and_then(|p| p.get(&key))
                .and_then(|p| p.get("trust_level"))
                .and_then(|l| l.as_str())
                == Some("trusted")
        });
        if trusted {
            text = added;
            marked += 1;
        }
    }
    if marked > 0 {
        replace(file, text.as_bytes())?;
    }
    Ok(marked)
}

fn read(file: &Path) -> anyhow::Result<Vec<u8>> {
    let size = std::fs::metadata(file)?.len();
    anyhow::ensure!(size <= MOST, "too large");
    Ok(std::fs::read(file)?)
}

/// Writes `file` whole, beside it first, then over it, so the agent never
/// reads half of it; private to this user, as the agents keep it.
fn replace(file: &Path, contents: &[u8]) -> anyhow::Result<()> {
    crate::paths::write_private_file(file, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(cloned: Option<bool>) -> Repository {
        Repository {
            root: "bots".into(),
            name: "app".into(),
            url: "git@github.com:acme/app.git".into(),
            ssh_key: None,
            github: None,
            cloned,
        }
    }

    fn reflog(checkout: &Path, first: &str) {
        std::fs::create_dir_all(checkout.join(".git/logs")).unwrap();
        std::fs::write(
            checkout.join(".git/logs/HEAD"),
            format!(
                "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 t <t@t> 1 +0000\t{first}\n\
                 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 t <t@t> 2 +0000\tcheckout: moving from main to issue-1\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn only_a_checkout_desktop_cloned_from_its_mirror_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bots");
        let mirror = dir.path().join("data/repositories/bots/app.git");
        let checkout = root.join("app");
        reflog(&checkout, &format!("clone: from {}", mirror.display()));
        let real = crate::roots::canonical(&checkout).unwrap();

        assert!(
            made_by_desktop(&repo(None), &checkout, &mirror),
            "cloned before 0.16.1"
        );
        assert!(made_by_desktop(&repo(Some(true)), &checkout, &mirror));
        assert!(
            !made_by_desktop(&repo(Some(false)), &checkout, &mirror),
            "recorded as kept"
        );
        assert_eq!(
            checkouts(&root, &[(mirror.clone(), repo(None))]),
            vec![real]
        );

        // A checkout that was there already started from somewhere else.
        reflog(&checkout, "clone: from git@github.com:acme/app.git");
        assert!(!made_by_desktop(&repo(None), &checkout, &mirror));
        assert!(checkouts(&root, &[(mirror.clone(), repo(None))]).is_empty());
        reflog(
            &checkout,
            &format!("clone: from {}-other", mirror.display()),
        );
        assert!(!cloned_from(&checkout, &mirror));
        std::fs::remove_dir_all(checkout.join(".git/logs")).unwrap();
        assert!(!cloned_from(&checkout, &mirror), "no history");
    }

    #[test]
    fn a_checkout_is_recorded_as_made_only_when_desktop_cloned_it() {
        let dir = tempfile::tempdir().unwrap();
        let mirror = dir.path().join("app.git");
        let checkout = dir.path().join("bots/app");
        let mut repos = vec![repo(None)];
        note_checkout(&mut repos, "bots", "app", false, &checkout, &mirror);
        assert_eq!(
            repos[0].cloned,
            Some(true),
            "nothing was there: Desktop cloned it"
        );

        // Set up again over the same checkout: still Desktop's.
        note_checkout(&mut repos, "bots", "app", true, &checkout, &mirror);
        assert_eq!(repos[0].cloned, Some(true));

        // One that was there, and isn't a clone of the mirror, is the person's.
        let mut theirs = vec![repo(None)];
        reflog(&checkout, "clone: from git@github.com:acme/app.git");
        note_checkout(&mut theirs, "bots", "app", true, &checkout, &mirror);
        assert_eq!(theirs[0].cloned, Some(false));
        note_checkout(&mut theirs, "bots", "other", false, &checkout, &mirror);
        assert_eq!(theirs[0].cloned, Some(false), "another checkout's");
    }

    #[cfg(unix)]
    #[test]
    fn a_checkout_is_never_trusted_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bots");
        let elsewhere = dir.path().join("elsewhere");
        let mirror = dir.path().join("app.git");
        reflog(&elsewhere, &format!("clone: from {}", mirror.display()));
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("app")).unwrap();
        assert!(checkouts(&root, &[(mirror.clone(), repo(Some(true)))]).is_empty());

        // Nor through a reflog that's a link to one that would say so.
        let checkout = root.join("other");
        std::fs::create_dir_all(checkout.join(".git/logs")).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.join(".git/logs/HEAD"),
            checkout.join(".git/logs/HEAD"),
        )
        .unwrap();
        assert!(!cloned_from(&checkout, &mirror));
    }

    #[test]
    fn claude_code_trusts_the_checkout_and_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CONFIG_FILE);
        let checkout = dir.path().join("bots/app");
        let key = checkout.to_string_lossy().into_owned();
        std::fs::write(
            &file,
            serde_json::json!({
                "oauthAccount": { "emailAddress": "a@example.com" },
                "projects": {
                    "/home/u/own": { "hasTrustDialogAccepted": false, "allowedTools": ["Bash"] },
                    key.clone(): { "hasTrustDialogAccepted": false, "lastCost": 1.5 },
                },
            })
            .to_string(),
        )
        .unwrap();

        assert!(trust_in_claude(&file, std::slice::from_ref(&checkout)).unwrap() >= 1);
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(settings["projects"][&key]["hasTrustDialogAccepted"], true);
        assert_eq!(settings["projects"][&key]["lastCost"], 1.5);
        assert_eq!(
            settings["projects"]["/home/u/own"]["hasTrustDialogAccepted"], false,
            "the person's own"
        );
        assert_eq!(settings["oauthAccount"]["emailAddress"], "a@example.com");

        // Already trusted: nothing is written again.
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(trust_in_claude(&file, &[checkout]).unwrap(), 0);
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn claude_code_settings_without_projects_get_them_and_others_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CONFIG_FILE);
        let checkout = dir.path().join("bots/app");
        std::fs::write(&file, "{}").unwrap();
        assert!(trust_in_claude(&file, std::slice::from_ref(&checkout)).unwrap() >= 1);
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            settings["projects"][checkout.to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
            true
        );

        for broken in ["[]", "{\"projects\": []}", "not json"] {
            std::fs::write(&file, broken).unwrap();
            assert!(trust_in_claude(&file, std::slice::from_ref(&checkout)).is_err());
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                broken,
                "left as it was"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn claude_codes_settings_stay_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CONFIG_FILE);
        std::fs::write(&file, "{}").unwrap();
        trust_in_claude(&file, &[dir.path().join("app")]).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn codex_trusts_the_checkout_and_keeps_the_persons_file_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CODEX_CONFIG_FILE);
        let checkout = dir.path().join("bots/app");
        let mine = "# my settings\nmodel = \"gpt-6.1-sol\"\n\n[projects.\"/home/u/own\"]\ntrust_level = \"untrusted\"\n";
        std::fs::write(&file, mine).unwrap();

        assert!(trust_in_codex(&file, std::slice::from_ref(&checkout)).unwrap() >= 1);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.starts_with(mine),
            "the person's file, comments too, comes first"
        );
        let settings: toml::Table = toml::from_str(&text).unwrap();
        let key = checkout.to_string_lossy().into_owned();
        assert_eq!(
            settings["projects"][&key]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(
            settings["projects"]["/home/u/own"]["trust_level"].as_str(),
            Some("untrusted")
        );

        assert_eq!(
            trust_in_codex(&file, std::slice::from_ref(&checkout)).unwrap(),
            0,
            "once"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }

    #[test]
    fn codex_keeps_a_level_the_person_chose_and_a_file_it_cant_add_to() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CODEX_CONFIG_FILE);
        let checkout = dir.path().join("bots/app");
        let key = toml::Value::String(checkout.to_string_lossy().into_owned());

        // The person said untrusted, or has the folder's table already.
        for theirs in [
            format!("[projects.{key}]\ntrust_level = \"untrusted\"\n"),
            format!("[projects.{key}]\nmodel = \"x\"\n"),
            format!("projects = {{ {key} = {{ trust_level = \"untrusted\" }} }}\n"),
        ] {
            std::fs::write(&file, &theirs).unwrap();
            assert_eq!(
                trust_in_codex(&file, std::slice::from_ref(&checkout)).unwrap(),
                0
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
        }
        // Projects as an inline table: a table added after it isn't TOML.
        let inline = "projects = { \"/home/u/own\" = { trust_level = \"trusted\" } }\n";
        std::fs::write(&file, inline).unwrap();
        assert_eq!(
            trust_in_codex(&file, std::slice::from_ref(&checkout)).unwrap(),
            0
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), inline);

        // No settings yet: Codex's file is made with only that.
        std::fs::remove_file(&file).unwrap();
        assert!(trust_in_codex(&file, std::slice::from_ref(&checkout)).unwrap() >= 1);
        let settings: toml::Table =
            toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(settings.len(), 1);
    }
}
