use std::path::{Path, PathBuf};

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
        codex: false,
        owners: Vec::new(),
    }
}

fn ready_coding() -> CodingConfig {
    CodingConfig {
        claude: Some(PathBuf::from("/home/u/.local/bin/claude")),
        ..Default::default()
    }
}

/// A home folder signed in to Claude Code as Claude Code leaves it on
/// Linux and Windows: `~/.claude/.credentials.json` with its tokens, and
/// `~/.claude.json`.
fn signed_in_home(home: &Path) {
    std::fs::create_dir_all(home.join(CONFIG_DIR)).unwrap();
    std::fs::write(
        home.join(CONFIG_DIR).join(CREDENTIALS_FILE),
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": TOKEN,
                "refreshToken": "sk-ant-ort01-refreshREFRESH0123456789",
                "expiresAt": 1,
                "scopes": ["user:inference"],
                "subscriptionType": "max",
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(home.join(CONFIG_FILE), "{}").unwrap();
}

#[test]
fn the_login_is_found_where_claude_code_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let none = Login::find(&home);
    assert!(!none.signed_in());
    assert_eq!(none.paths().count(), 0);

    signed_in_home(&home);
    let login = Login::find(&home);
    assert!(login.signed_in());
    let real = crate::roots::canonical(&home).unwrap();
    assert_eq!(login.dir, Some(real.join(CONFIG_DIR)));
    assert_eq!(login.file, Some(real.join(CONFIG_FILE)));
    let secrets: Vec<&str> = login.secrets.iter().map(Secret::expose).collect();
    assert!(secrets.contains(&TOKEN), "the access token");
    assert!(
        secrets.contains(&"sk-ant-ort01-refreshREFRESH0123456789"),
        "the refresh token"
    );
    assert_eq!(secrets.len(), 2, "never the other fields");
    assert!(!format!("{login:?}").contains("oat01"));

    // A `~/.claude` that leads to a folder not named so is left out.
    #[cfg(unix)]
    {
        let other = dir.path().join("home2");
        std::fs::create_dir_all(other.join(".ssh")).unwrap();
        std::os::unix::fs::symlink(other.join(".ssh"), other.join(CONFIG_DIR)).unwrap();
        assert_eq!(Login::find(&other).dir, None);
    }

    // Folders Claude Code keeps elsewhere aren't the computer's sign-in.
    std::fs::remove_file(home.join(CONFIG_DIR).join(CREDENTIALS_FILE)).unwrap();
    assert!(!Login::find(&home).signed_in());
}

#[test]
fn a_secret_never_prints() {
    let secret = Secret::new(TOKEN.into());
    assert!(!format!("{secret:?}").contains("oat01"));
    let claude = AgentRun {
        agent: Agent::ClaudeCode,
        program: "/x/claude".into(),
        real: "/y/claude".into(),
        read: "/y".into(),
        entry: None,
        login: Login {
            secrets: vec![secret],
            ..Default::default()
        },
    };
    assert!(!format!("{claude:?}").contains("oat01"));
}

#[test]
fn claude_is_ready_only_for_a_bot_allowed_it_where_commands_reach_the_network() {
    assert_eq!(
        agent_state(Agent::ClaudeCode, &root(Some(loan(true))), "42"),
        AgentState::Ready
    );
    assert_eq!(
        agent_state(Agent::ClaudeCode, &root(Some(loan(false))), "42"),
        AgentState::NotAllowed
    );
    assert_eq!(
        agent_state(Agent::ClaudeCode, &root(Some(loan(true))), "7"),
        AgentState::NotAllowed
    );
    let mut offline = root(Some(loan(true)));
    offline.network = false;
    assert_eq!(
        agent_state(Agent::ClaudeCode, &offline, "42"),
        AgentState::NoNetwork
    );
    let mut no_commands = root(Some(loan(true)));
    no_commands.commands = false;
    assert_eq!(
        agent_state(Agent::ClaudeCode, &no_commands, "42"),
        AgentState::NotAllowed
    );
    // No setup needed: a run finds `claude` itself.
    assert_eq!(
        agent_state(Agent::ClaudeCode, &root(Some(loan(true))), "42"),
        AgentState::Ready
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
        github: None,
    });
    let value = coding_for_bot(&coding, &root(Some(loan(true))), "42");
    assert_eq!(value["claude_code"], "ready");
    assert_eq!(value["repositories"], serde_json::json!(["app"]));
    let text = value.to_string();
    assert!(!text.contains("github.com") && !text.contains("/home/u"));
}

#[test]
fn only_an_allowed_bot_run_gets_the_login() {
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
    let home = dir.path().join("home");
    signed_in_home(&home);
    let login = |agent: Agent| Login::find_for(agent, &home);

    let person = for_run(&config, &paths, "bots", None, |_| {
        panic!("read for a person")
    });
    assert_eq!(person.agents, Vec::new());
    let other = for_run(&config, &paths, "bots", Some("7"), |_| {
        panic!("read for bot 7")
    });
    assert_eq!(other.agents, Vec::new());
    let bot = for_run(&config, &paths, "bots", Some("42"), login);
    let claude = bot.agents.first().expect("the bot's run gets Claude Code");
    assert_eq!(bot.agents.len(), 1, "only the agent its loan allows");
    assert_eq!(claude.program, program);
    assert!(claude.login.signed_in());
    assert_eq!(claude.login.paths().count(), 2);

    config.roots[0].bots[0].claude_code = false;
    let withdrawn = for_run(&config, &paths, "bots", Some("42"), |_| {
        panic!("read when withdrawn")
    });
    assert_eq!(withdrawn.agents, Vec::new());
}

#[test]
fn output_loses_the_token_across_chunks() {
    let secret = Secret::new(TOKEN.into());
    let text = format!("before {TOKEN} middle {TOKEN}after\n");
    for split in [1, 3, 7, 13, 40, 64, text.len()] {
        let mut redactor = Redactor::new([(&secret, REDACTED.to_string())]);
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
    let mut redactor = Redactor::new([(&secret, REDACTED.to_string())]);
    let mut out = redactor.push(b"sk-ant-oat01-abc");
    assert!(out.is_empty(), "a possible start waits");
    out.extend(redactor.push(b" and more"));
    out.extend(redactor.finish());
    assert_eq!(String::from_utf8(out).unwrap(), "sk-ant-oat01-abc and more");
    let mut none = Redactor::new([]);
    assert_eq!(none.push(b"anything"), b"anything");
}

#[test]
fn output_loses_every_secret_across_chunks() {
    let token = Secret::new(TOKEN.into());
    let long = Secret::new("ops_testSERVICEaccount0123".into());
    // One secret that starts another: the longer is never left half shown.
    let short = Secret::new("ops_test".into());
    let text = "a sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ_-xyz b \
                ops_testSERVICEaccount0123 c ops_test d ops_testSERVICEaccount0123";
    let expected = format!("a {REDACTED} b [LONG] c [SHORT] d [LONG]");
    for split in [1, 2, 5, 9, 31, text.len()] {
        let mut redactor = Redactor::new([
            (&token, REDACTED.to_string()),
            (&long, "[LONG]".to_string()),
            (&short, "[SHORT]".to_string()),
        ]);
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(split) {
            out.extend(redactor.push(chunk));
        }
        out.extend(redactor.finish());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            expected,
            "chunks of {split}"
        );
    }
    // A shorter one held at the end, as the start of a longer, still goes.
    let mut redactor = Redactor::new([
        (&long, "[LONG]".to_string()),
        (&short, "[SHORT]".to_string()),
    ]);
    let mut out = redactor.push(b"x ops_testSERV");
    assert_eq!(out, b"x ", "a possible start of the longer waits");
    out.extend(redactor.finish());
    assert_eq!(String::from_utf8(out).unwrap(), "x [SHORT]SERV");
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

#[cfg(unix)]
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
        github: None,
    };
    let mirror = repositories::create_mirror(&paths, &repo, &[]).unwrap();
    assert!(mirror.starts_with(&paths.data_dir));
    let folder = dir.path().join("bots");
    std::fs::create_dir(&folder).unwrap();
    let checkout = in_sandboxs_place(&folder, "app", &mirror);
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

#[cfg(unix)]
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
        github: None,
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
    in_sandboxs_place(&folder, "app", &mirror);
    assert_eq!(
        PathBuf::from(git(&["remote", "get-url", "origin"], &checkout)),
        mirror
    );
    assert_eq!(
        PathBuf::from(git(&["remote", "get-url", "--push", "origin"], &checkout)),
        mirror,
        "no other place to push to"
    );
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

/// Run the command setup runs in the sandbox to make or adopt a checkout,
/// here with `sh`, and answer the checkout.
#[cfg(unix)]
fn in_sandboxs_place(folder: &std::path::Path, name: &str, mirror: &std::path::Path) -> PathBuf {
    let exists = folder.join(name).exists();
    let line = repositories::checkout_command(name, mirror, exists, false);
    let out = std::process::Command::new("sh")
        .args(["-c", &line])
        .current_dir(folder)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{line}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    folder.join(name)
}

#[test]
fn checkout_commands_quote_what_they_name() {
    let mirror = PathBuf::from("/data/it's here/app.git");
    let sh = repositories::checkout_command("app", &mirror, false, false);
    assert_eq!(
        sh,
        "git clone --quiet --no-hardlinks -- '/data/it'\\''s here/app.git' 'app'"
    );
    let ps = repositories::checkout_command("app", &mirror, true, true);
    assert!(
        ps.starts_with("git -C 'app' remote set-url origin '/data/it''s here/app.git'; "),
        "{ps}"
    );
}

/// `cww coding setup` picks a bot by name: an ID or a whole name finds one,
/// and letters in order find every bot whose name, project and organization
/// hold them.
#[test]
fn a_bot_is_picked_by_name() {
    let answer = serde_json::json!({
        "account": { "name": "Plenty" },
        "bots": [
            { "id": 24, "name": "Solco issue fixer", "project": { "name": "Solco" } },
            { "id": 34, "name": "Issue fixer", "project": { "name": "Chat with Work" } },
            { "id": 7, "name": "Weekly digest", "project": null },
        ]
    });
    let choices = super::setup::bot_choices(&answer);
    assert_eq!(choices.len(), 3);
    assert_eq!(choices[2].project, "Private");
    assert_eq!(choices[0].organization, "Plenty");
    let ids = |query: &str| -> Vec<String> {
        super::setup::matching(&choices, query)
            .iter()
            .map(|c| c.id.clone())
            .collect()
    };
    assert_eq!(ids("34"), ["34"]);
    assert_eq!(ids("#24"), ["24"]);
    assert_eq!(ids("issue fixer"), ["34"], "a whole name is that bot");
    assert_eq!(ids("isfx"), ["24", "34"]);
    assert_eq!(ids("solco"), ["24"]);
    assert_eq!(ids("cww isfx"), Vec::<String>::new());
    assert_eq!(ids("chat isfx"), Vec::<String>::new(), "in the order shown");
    assert_eq!(ids("isfx chat"), ["34"]);
    assert_eq!(ids("digest"), ["7"]);
}

/// A `~/.claude` that holds Desktop's own folders is never lent to a run:
/// from its control socket, a command could change what it's allowed.
#[test]
fn a_login_folder_holding_desktops_own_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let login = crate::roots::canonical(dir.path())
        .unwrap()
        .join(CONFIG_DIR);
    let runtime = login.join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    let mut paths = crate::paths::Paths::under(&dir.path().join("cww"));
    paths.runtime_dir = runtime;
    let found = Login {
        dir: Some(login),
        ..Default::default()
    };
    assert_eq!(found.narrowed(&Config::default(), &paths).dir, None);
}

/// A recorded `claude` that's gone isn't handed to a run: it finds the one
/// installed instead, never one in a shared folder that allows changes.
#[test]
fn a_run_uses_the_recorded_claude_only_while_its_there() {
    let dir = tempfile::tempdir().unwrap();
    let recorded = dir.path().join("claude");
    std::fs::write(&recorded, "").unwrap();
    let coding = CodingConfig {
        claude: Some(recorded.clone()),
        ..Default::default()
    };
    assert_eq!(
        super::setup::program_for_run(Agent::ClaudeCode, &coding, &[]),
        Some(recorded.clone())
    );
    std::fs::remove_file(&recorded).unwrap();
    assert_ne!(
        super::setup::program_for_run(Agent::ClaudeCode, &coding, &[]),
        Some(recorded)
    );
}

/// Codex as it leaves itself on any system: `~/.codex/auth.json`.
fn codex_home(home: &Path, auth: serde_json::Value) {
    std::fs::create_dir_all(home.join(CODEX_DIR)).unwrap();
    std::fs::write(home.join(CODEX_DIR).join(CODEX_AUTH_FILE), auth.to_string()).unwrap();
}

const CODEX_ACCESS: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6IjEyMyJ9.access.token";
const CODEX_REFRESH: &str = "rt_refreshREFRESHrefreshREFRESH0123456789";

#[test]
fn codexs_sign_in_is_its_own_folder_and_every_token_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    codex_home(
        &home,
        serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "eyJpZCI6InRva2VuIn0.id.tokenTOKENtoken",
                "access_token": CODEX_ACCESS,
                "refresh_token": CODEX_REFRESH,
                "account_id": "0123456789abcdef0123456789abcdef",
            },
            "last_refresh": "2026-11-05T21:29:45Z",
        }),
    );
    let login = Login::find_for(Agent::Codex, &home);
    assert!(login.signed_in());
    assert_eq!(login.billing, Billing::Plan);
    assert_eq!(login.file, None, "only its folder");
    assert_eq!(login.paths().count(), 1);
    let secrets: Vec<&str> = login.secrets.iter().map(Secret::expose).collect();
    assert!(secrets.contains(&CODEX_ACCESS) && secrets.contains(&CODEX_REFRESH));
    assert!(
        !secrets.contains(&"0123456789abcdef0123456789abcdef"),
        "an id isn't a secret"
    );
    assert!(!format!("{login:?}").contains(CODEX_REFRESH));

    codex_home(
        &home,
        serde_json::json!({ "OPENAI_API_KEY": "sk-proj-abcdefghijklmnopqrstuvwxyz" }),
    );
    let login = Login::find_for(Agent::Codex, &home);
    assert!(login.signed_in());
    assert_eq!(login.billing, Billing::ApiKey);

    codex_home(&home, serde_json::json!({}));
    assert!(!Login::find_for(Agent::Codex, &home).signed_in());
}

#[cfg(unix)]
#[test]
fn codexs_folder_is_never_one_a_link_makes_something_else() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::os::unix::fs::symlink(home.join(".ssh"), home.join(CODEX_DIR)).unwrap();
    assert_eq!(Login::find_for(Agent::Codex, &home).dir, None);
}

#[test]
fn claudes_plan_or_api_account_is_read_from_its_credentials() {
    assert_eq!(
        claude_billing(r#"{"claudeAiOauth":{"subscriptionType":"max"}}"#),
        Billing::Plan
    );
    assert_eq!(
        claude_billing(r#"{"claudeAiOauth":{"accessToken":"x"}}"#),
        Billing::ApiKey
    );
    assert_eq!(claude_billing("not json"), Billing::Unknown);
}

#[test]
fn each_agent_has_its_own_switch_on_a_loan() {
    let dir = tempfile::tempdir().unwrap();
    let codex = dir.path().join("codex");
    std::fs::write(&codex, "").unwrap();
    let claude = dir.path().join("claude");
    std::fs::write(&claude, "").unwrap();
    let paths = crate::paths::Paths::under(dir.path());
    let mut lent = loan(false);
    lent.codex = true;
    let mut config = Config {
        coding: CodingConfig {
            claude: Some(claude),
            codex: Some(codex.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    config.roots.push(root(Some(lent)));
    assert_eq!(
        agent_state(Agent::Codex, &config.roots[0], "42"),
        AgentState::Ready
    );
    assert_eq!(
        agent_state(Agent::ClaudeCode, &config.roots[0], "42"),
        AgentState::NotAllowed
    );

    let home = dir.path().join("home");
    codex_home(
        &home,
        serde_json::json!({ "tokens": { "access_token": CODEX_ACCESS } }),
    );
    let run = for_run(&config, &paths, "bots", Some("42"), |agent| {
        Login::find_for(agent, &home)
    });
    if cfg!(target_os = "macos") {
        assert!(run.agents.is_empty(), "Codex can't run in Seatbelt yet");
        return;
    }
    assert_eq!(run.agents.len(), 1, "only Codex");
    assert_eq!(run.agents[0].agent, Agent::Codex);
    assert_eq!(run.agents[0].program, codex);

    let value = coding_for_bot(&config.coding, &config.roots[0], "42");
    assert_eq!(value["agents"]["codex"], "ready");
    assert_eq!(value["agents"]["claude_code"], "not_allowed");
    assert_eq!(
        value["claude_code"], "not_allowed",
        "for servers that know only this"
    );
    assert_eq!(value["prepares"], true);

    let mut off = config.clone();
    crate::roots::set_commands(&mut off, "bots", false, false).unwrap();
    assert!(!off.roots[0].bots[0].codex, "commands off ends it");
    let mut read_only = config.clone();
    crate::roots::set_writable(&mut read_only, "bots", false, false).unwrap();
    assert!(!read_only.roots[0].bots[0].codex);
    assert!(
        crate::roots::set_bot(&mut config, "bots", "42", None, true, false)
            .unwrap()
            .bots[0]
            .codex,
        "lending again keeps it"
    );
}

#[test]
fn only_this_computer_names_the_owners_a_bots_runs_may_prepare() {
    let mut config = Config::default();
    config.roots.push(root(Some(loan(true))));
    assert!(crate::roots::add_bot_owner(&mut config, "bots", "42", "../x").is_err());
    crate::roots::add_bot_owner(&mut config, "bots", "42", "acme").unwrap();
    crate::roots::add_bot_owner(&mut config, "bots", "42", "ACME").unwrap();
    assert_eq!(config.roots[0].bots[0].owners, ["acme"]);
    assert!(crate::roots::add_bot_owner(&mut config, "bots", "7", "acme").is_err());
    let saved = toml::to_string(&config.roots[0].bots[0]).unwrap();
    assert!(saved.contains("repository_owners"), "{saved}");
}

/// Always allow on a card turns on only that, only for a loan with commands.
#[test]
fn a_cards_always_allow_widens_only_a_loan_with_commands() {
    use crate::roots::allow_bot_unattended_from_server as allow;
    let mut config = Config::default();
    config.roots.push(root(Some(loan(false))));
    let before = config.roots[0].clone();

    assert!(allow(&mut config, "bots", "7").is_err(), "not lent to it");
    assert!(allow(&mut config, "elsewhere", "42").is_err(), "not shared");
    let mut no_commands = config.clone();
    crate::roots::set_commands(&mut no_commands, "bots", false, false).unwrap();
    assert!(
        allow(&mut no_commands, "bots", "42").is_err(),
        "commands off"
    );
    assert!(!no_commands.roots[0].bots[0].unattended);

    allow(&mut config, "bots", "42").unwrap();
    let mut expected = before;
    expected.bots[0].unattended = true;
    assert_eq!(config.roots[0], expected, "nothing else changed");
}
