use std::path::PathBuf;

use super::repositories::{check_url, name_from_url};
use super::*;
use crate::config::{BotLoan, Repository, Root};

const TOKEN: &str = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ_-xyz";

fn root(bot: Option<BotLoan>) -> Root {
    Root {
        id: "bots".into(),
        label: "bots".into(),
        path: PathBuf::from("/home/u/bots"),
        follow_symlinks: false,
        writable: true,
        commands: true,
        network: true,
        bots: bot.into_iter().collect(),
        read_budget: None,
    }
}

fn loan(claude_code: bool) -> BotLoan {
    BotLoan {
        id: "42".into(),
        name: "Issue fixer".into(),
        unattended: false,
        claude_code,
    }
}

fn ready_coding() -> CodingConfig {
    CodingConfig {
        claude: Some(PathBuf::from("/home/u/.local/bin/claude")),
        claude_signed_in: true,
        repositories: vec![],
    }
}

#[test]
fn tokens_look_like_setup_tokens() {
    assert!(valid_token(TOKEN));
    assert!(!valid_token(
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF"
    ));
    assert!(!valid_token("sk-ant-oat01-short"));
    assert!(!valid_token(&format!("{TOKEN} rm -rf")));
    assert!(!valid_token(&format!("{TOKEN}\n")));
}

#[test]
fn a_secret_never_prints() {
    let secret = Secret::new(TOKEN.into());
    assert!(!format!("{secret:?}").contains("oat01"));
    let claude = Claude {
        program: "/x/claude".into(),
        real: "/y/claude".into(),
        read: "/y".into(),
        token: secret,
    };
    assert!(!format!("{claude:?}").contains("oat01"));
}

#[test]
fn claude_is_ready_only_for_a_bot_allowed_it_where_commands_reach_the_network() {
    let coding = ready_coding();
    assert_eq!(
        claude_state(&coding, &root(Some(loan(true))), "42"),
        ClaudeState::Ready
    );
    assert_eq!(
        claude_state(&coding, &root(Some(loan(false))), "42"),
        ClaudeState::NotAllowed
    );
    assert_eq!(
        claude_state(&coding, &root(Some(loan(true))), "7"),
        ClaudeState::NotAllowed
    );
    let mut offline = root(Some(loan(true)));
    offline.network = false;
    assert_eq!(
        claude_state(&coding, &offline, "42"),
        ClaudeState::NoNetwork
    );
    let mut no_commands = root(Some(loan(true)));
    no_commands.commands = false;
    assert_eq!(
        claude_state(&coding, &no_commands, "42"),
        ClaudeState::NotAllowed
    );
    let signed_out = CodingConfig {
        claude_signed_in: false,
        ..ready_coding()
    };
    assert_eq!(
        claude_state(&signed_out, &root(Some(loan(true))), "42"),
        ClaudeState::NotSignedIn
    );
    assert_eq!(
        claude_state(&CodingConfig::default(), &root(Some(loan(true))), "42"),
        ClaudeState::NotInstalled
    );
}

#[test]
fn a_bot_sees_whether_claude_is_ready_and_repository_names_only() {
    let mut coding = ready_coding();
    coding.repositories.push(Repository {
        root: "bots".into(),
        name: "app".into(),
        url: "git@github.com:acme/app.git".into(),
        ssh_key: None,
    });
    let value = coding_for_bot(&coding, &root(Some(loan(true))), "42");
    assert_eq!(value["claude_code"], "ready");
    assert_eq!(value["repositories"], serde_json::json!(["app"]));
    let text = value.to_string();
    assert!(!text.contains("github.com") && !text.contains("/home/u"));
}

#[test]
fn only_an_allowed_bot_run_reads_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("claude");
    std::fs::write(&program, "").unwrap();
    let paths = crate::paths::Paths::under(dir.path());
    let mut config = Config {
        coding: CodingConfig {
            claude: Some(program.clone()),
            ..ready_coding()
        },
        ..Default::default()
    };
    config.roots.push(root(Some(loan(true))));
    let token = || Some(Secret::new(TOKEN.into()));

    let person = for_run(&config, &paths, "bots", None, || {
        panic!("read for a person")
    });
    assert_eq!(person.claude, None);
    let other = for_run(&config, &paths, "bots", Some("7"), || {
        panic!("read for bot 7")
    });
    assert_eq!(other.claude, None);
    let bot = for_run(&config, &paths, "bots", Some("42"), token);
    let claude = bot.claude.expect("the bot's run gets Claude Code");
    assert_eq!(claude.program, program);
    assert_eq!(claude.token.expose(), TOKEN);

    config.roots[0].bots[0].claude_code = false;
    let withdrawn = for_run(&config, &paths, "bots", Some("42"), || {
        panic!("read when withdrawn")
    });
    assert_eq!(withdrawn.claude, None);
}

#[test]
fn output_loses_the_token_across_chunks() {
    let secret = Secret::new(TOKEN.into());
    let text = format!("before {TOKEN} middle {TOKEN}after\n");
    for split in [1, 3, 7, 13, 40, 64, text.len()] {
        let mut redactor = Redactor::new(Some(&secret));
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(split) {
            out.extend(redactor.push(chunk));
        }
        out.extend(redactor.finish());
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            format!("before {REDACTED} middle {REDACTED}after\n"),
            "chunks of {split}"
        );
    }
}

#[test]
fn output_keeps_what_only_starts_like_the_token() {
    let secret = Secret::new(TOKEN.into());
    let mut redactor = Redactor::new(Some(&secret));
    let mut out = redactor.push(b"sk-ant-oat01-abc");
    assert!(out.is_empty(), "a possible start waits");
    out.extend(redactor.push(b" and more"));
    out.extend(redactor.finish());
    assert_eq!(String::from_utf8(out).unwrap(), "sk-ant-oat01-abc and more");
    let mut none = Redactor::new(None);
    assert_eq!(none.push(b"anything"), b"anything");
}

#[test]
fn repository_addresses_never_hold_a_token() {
    assert!(check_url("git@github.com:acme/app.git").is_ok());
    assert!(check_url("https://github.com/acme/app.git").is_ok());
    assert!(check_url("ssh://git@host/acme/app").is_ok());
    assert!(check_url("https://x-access-token:ghp_abc@github.com/acme/app.git").is_err());
    assert!(check_url("https://user@github.com/acme/app.git").is_err());
    assert!(check_url("--upload-pack=evil").is_err());
    assert!(check_url("ext::sh -c evil").is_err());
    assert!(check_url("/home/u/app").is_err());
    assert!(check_url("ssh://user:password@host/acme/app").is_err());
    assert!(check_url("user:token@github.com:acme/app.git").is_err());
    assert!(check_url("ssh://git@host:2222/acme/app").is_ok());
    assert!(check_url("ftp://alice:secret@example.com/app.git").is_err());
    assert!(check_url("git+ssh://alice:secret@example.com/app.git").is_err());
    assert_eq!(
        name_from_url("git@github.com:acme/app.git").as_deref(),
        Some("app")
    );
    assert_eq!(
        name_from_url("https://github.com/acme/chat-with-work/").as_deref(),
        Some("chat-with-work")
    );
    assert_eq!(name_from_url("https://github.com/acme/..").as_deref(), None);
}

#[test]
fn checkouts_fetch_from_a_mirror_they_cant_push_to() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    let git = |args: &[&str], cwd: &std::path::Path| {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
    };
    std::fs::create_dir(&upstream).unwrap();
    git(&["init", "-q"], &upstream);
    std::fs::write(upstream.join("a"), "1").unwrap();
    git(&["add", "a"], &upstream);
    git(&["commit", "-qm", "one"], &upstream);

    let paths = crate::paths::Paths::under(&dir.path().join("cww"));
    // A file:// address stands in for GitHub.
    let repo = Repository {
        root: "bots".into(),
        name: "app".into(),
        url: format!("file://{}", upstream.display()),
        ssh_key: None,
    };
    let mirror = repositories::create_mirror(&paths, &repo, &[]).unwrap();
    assert!(mirror.starts_with(&paths.data_dir));
    let folder = dir.path().join("bots");
    std::fs::create_dir(&folder).unwrap();
    let checkout = repositories::create_checkout(&folder, "app", &mirror).unwrap();
    assert!(checkout.join("a").is_file());

    std::fs::write(upstream.join("a"), "2").unwrap();
    git(&["commit", "-qam", "two"], &upstream);
    repositories::fetch(&mirror, Some(&repo)).unwrap();
    assert!(repositories::fetched_at(&mirror).is_some());
    git(&["fetch", "-q", "origin"], &checkout);
    git(&["merge", "-q", "--ff-only", "origin/main"], &checkout);
    assert_eq!(std::fs::read_to_string(checkout.join("a")).unwrap(), "2");

    // The checkout's origin is the mirror, never GitHub: no credentials,
    // and in the sandbox, which only reads the mirror, no push.
    let remote = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&checkout)
        .output()
        .unwrap();
    assert_eq!(
        PathBuf::from(String::from_utf8_lossy(&remote.stdout).trim()),
        mirror
    );
}

#[test]
fn an_adopted_checkout_fetches_and_pushes_only_through_the_mirror() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    std::fs::create_dir(&upstream).unwrap();
    let git = |args: &[&str], cwd: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"], &upstream);
    std::fs::write(upstream.join("a"), "1").unwrap();
    git(&["add", "a"], &upstream);
    git(&["commit", "-qm", "one"], &upstream);
    let paths = crate::paths::Paths::under(&dir.path().join("cww"));
    let repo = Repository {
        root: "bots".into(),
        name: "app".into(),
        url: format!("file://{}", upstream.display()),
        ssh_key: None,
    };
    let mirror = repositories::create_mirror(&paths, &repo, &[]).unwrap();

    // Another address under the same name never reuses this mirror.
    let other = Repository {
        url: "git@github.com:acme/other.git".into(),
        ..repo.clone()
    };
    assert!(repositories::create_mirror(&paths, &other, &[]).is_err());

    // A checkout from before, with a push address of its own.
    let folder = dir.path().join("bots");
    std::fs::create_dir(&folder).unwrap();
    git(&["clone", "-q", upstream.to_str().unwrap(), "app"], &folder);
    let checkout = folder.join("app");
    git(
        &[
            "config",
            "remote.origin.pushurl",
            "git@github.com:acme/app.git",
        ],
        &checkout,
    );
    repositories::create_checkout(&folder, "app", &mirror).unwrap();
    assert_eq!(
        PathBuf::from(git(&["remote", "get-url", "origin"], &checkout)),
        mirror
    );
    assert_eq!(
        PathBuf::from(git(&["remote", "get-url", "--push", "origin"], &checkout)),
        mirror,
        "no other place to push to"
    );

    // A checkout that is a link is refused.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&checkout, folder.join("linked")).unwrap();
        assert!(repositories::create_checkout(&folder, "linked", &mirror).is_err());
    }
}

#[test]
fn claude_inside_a_toolchain_gets_its_own_folder_even_under_a_broad_one() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::roots::canonical(dir.path()).unwrap();
    let paths = crate::paths::Paths::under(&base.join("cww"));
    let toolchain = base.join("mise");
    let install = toolchain.join("installs").join("claude").join("2.1.316");
    std::fs::create_dir_all(&install).unwrap();
    let program = install.join("claude.exe");
    std::fs::write(&program, "").unwrap();
    let config = Config::default();
    assert_eq!(
        grant_with(&program, &config, &paths, std::slice::from_ref(&toolchain)),
        Some(install.clone())
    );
    // Never the toolchain's own top, and never a folder holding a shared one.
    let mut shared = Config::default();
    shared.roots.push(Root {
        path: install.join("work"),
        ..root(None)
    });
    std::fs::create_dir_all(install.join("work")).unwrap();
    assert_ne!(
        grant_with(&program, &shared, &paths, std::slice::from_ref(&toolchain)),
        Some(install)
    );
}
