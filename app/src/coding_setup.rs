//! Letting a bot code on this computer, in one yes (docs/coding-computers.md,
//! "One yes"). A page of the server asks (`computer.setUpCoding`), and the app
//! shows a window of its own, a bundled page no remote page can see or answer:
//! what it checked for the person, what the yes gives, Allow and Cancel. The
//! page hears only how it ended, never a folder's place or a look at the agents.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use cww::coding::Agent;
use cww::control::ControlRequest;
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State, Webview, WebviewUrl, WebviewWindowBuilder};

use crate::desktop::Desktop;
use crate::windows::{CODING_SETUP, is_local_url};

/// What a page may ask for: a bot, and what its run names. Each is only a
/// suggestion the window checks and shows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub bot: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
}

impl Request {
    /// Whether it's what a page may ask, by shape.
    pub fn check(&self) -> Result<(), String> {
        let bot_ok = !self.bot.is_empty()
            && self.bot.len() <= 20
            && self.bot.bytes().all(|b| b.is_ascii_digit());
        let agent_ok = self
            .agent
            .as_deref()
            .is_none_or(|a| Agent::from_key(a).is_some());
        let repository_ok = self
            .repository
            .as_deref()
            .is_none_or(|r| cww::coding::prepare::split_repository(r).is_some());
        if bot_ok && agent_ok && repository_ok {
            Ok(())
        } else {
            Err("Chat with Work Desktop can't do that.".into())
        }
    }
}

/// The question showing now.
struct Showing {
    request: Request,
    name: String,
    commands_allowed: bool,
    answer: mpsc::Sender<Answer>,
    /// What the window last showed: Allow does only that.
    shown: Option<Value>,
    /// Set once Allow or Cancel was pressed, so only the first counts.
    answered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Allowed,
    Declined,
}

#[derive(Default)]
pub struct CodingSetups {
    showing: Mutex<Option<Showing>>,
}

/// Where the bot would work: the folder already lent to it, or `~/bots`, or,
/// where something of the person's is there already, the next free name. Never
/// a folder that has anything in it that isn't shared yet.
pub fn new_folder(home: &Path, shared: &[PathBuf]) -> Option<PathBuf> {
    // Only a folder that isn't there or is empty, and isn't shared: never
    // another bot's, or the person's own.
    let free = |path: &Path| {
        !shared.iter().any(|s| s == path)
            && std::fs::read_dir(path)
                .map_or(!path.exists(), |mut entries| entries.next().is_none())
    };
    std::iter::once("bots".to_string())
        .chain((2..=9).map(|n| format!("bots-{n}")))
        .map(|name| home.join(name))
        .find(|path| free(path))
}

/// A folder's place, as a person reads it: `~/bots`.
fn shown(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The agent the window offers: the one named, else the first this computer
/// has signed in.
fn chosen<'a>(look: &'a Value, named: Option<&str>) -> Option<&'a Value> {
    let agents = look["agents"].as_array()?;
    match named {
        Some(named) => agents.iter().find(|a| a["agent"] == named),
        None => agents
            .iter()
            .find(|a| {
                a["unavailable"].is_null() && a["installed"] == true && a["signed_in"] == true
            })
            .or_else(|| agents.first()),
    }
}

/// What the window shows, from what the daemon looked at.
pub fn view(
    request: &Request,
    name: &str,
    commands_allowed: bool,
    look: &Value,
    home: &Path,
    shared: &[PathBuf],
) -> Value {
    let mut checks = Vec::new();
    let agent = chosen(look, request.agent.as_deref());
    // The one asked for, by its own name even when this computer didn't
    // list it: an agent the look leaves out isn't here.
    let asked_name = request
        .agent
        .as_deref()
        .and_then(Agent::from_key)
        .map(Agent::name);
    let agent_name = agent
        .and_then(|a| a["name"].as_str())
        .or(asked_name)
        .unwrap_or("A coding agent");
    let ready = agent.is_some_and(|a| {
        a["unavailable"].is_null() && a["installed"] == true && a["signed_in"] == true
    });
    checks.push(match agent {
        Some(a) if a["unavailable"].is_string() => json!({ "ok": false, "text": a["unavailable"] }),
        Some(a) if a["installed"] != true => {
            json!({ "ok": false, "text": format!("{agent_name} isn't installed on this computer") })
        }
        Some(a) if a["signed_in"] != true => json!({ "ok": false, "text": format!(
            "{agent_name} isn't signed in: {}", a["sign_in_hint"].as_str().unwrap_or_default()) }),
        Some(_) => json!({ "ok": true, "text": format!("{agent_name} installed and signed in") }),
        // Not listed: missing here, or a background agent older than the app
        // that doesn't know it, so it says neither.
        None if asked_name.is_some() => {
            json!({ "ok": false, "text": format!("{agent_name} wasn't found on this computer") })
        }
        None => json!({ "ok": false, "text": "No coding agent is installed on this computer" }),
    });
    let reached = match (&request.repository, look["reaches"].as_bool()) {
        (Some(repository), Some(true)) => {
            checks.push(
                json!({ "ok": true, "text": format!("Your Git login reaches {repository}") }),
            );
            true
        }
        (Some(repository), _) => {
            checks.push(json!({ "ok": false, "text": format!("Your Git login doesn't reach {repository}") }));
            false
        }
        (None, _) => true,
    };
    let lent = look["root"]["label"].as_str();
    let folder = (lent.is_none()).then(|| new_folder(home, shared)).flatten();
    match (lent, &folder) {
        (Some(label), _) => checks.push(
            json!({ "ok": true, "text": format!("Works in {label}, already shared with it") }),
        ),
        (None, Some(path)) => checks.push(
            json!({ "ok": true, "text": format!("Works in a new folder, {}", shown(path, home)) }),
        ),
        (None, None) => checks
            .push(json!({ "ok": false, "text": "Your home folder has no free folder named bots" })),
    }
    if look["commands"] == false {
        checks.push(json!({ "ok": false, "text": "Commands can't run on this computer" }));
    }
    // What its code reaches, read from the repository's files where this
    // computer has them already (a first run reads them, and asks again here
    // if the folder doesn't give them).
    let needs = serde_json::from_value::<cww::coding::needs::Needs>(look["needs"].clone()).ok();
    if let Some(sentence) = needs.as_ref().and_then(cww::coding::needs::sentence) {
        checks.push(json!({ "ok": true, "text": sentence }));
    }
    if !commands_allowed {
        checks.push(
            json!({ "ok": false, "text": "Your organization doesn't allow commands on computers" }),
        );
    }
    let owner = request
        .repository
        .as_deref()
        .and_then(|r| r.split_once('/'))
        .map(|(owner, _)| owner);
    json!({
        "title": format!("Let {name} (Bot {}) code here?", request.bot),
        "computer": hostname(),
        "checks": checks,
        "note": format!(
            "Its commands reach this computer's services, the internet, and your {agent_name} sign-in, and can read that sign-in. They can't reach your SSH keys or other projects. {}",
            if look["root"]["unattended"] == true {
                "In this folder they already run without asking."
            } else {
                "You approve each one."
            }
        ),
        "repositories": owner.map(|owner| format!(
            "The first run on each {owner} repository copies it with your Git login, then installs and tests it in the sandbox."
        )),
        "can_allow": ready
            && reached
            && commands_allowed
            && look["commands"] != false
            && (lent.is_some() || folder.is_some()),
    })
}

fn hostname() -> String {
    cww::paths::hostname()
}

fn window_only(webview: &Webview) -> Result<(), String> {
    if webview.label() == CODING_SETUP && webview.url().is_ok_and(|url| is_local_url(&url)) {
        Ok(())
    } else {
        Err("Only the app's own window can ask for this.".into())
    }
}

fn look(desktop: &Desktop, request: &Request) -> Result<Value, String> {
    desktop
        .client
        .request(&ControlRequest::CodingLook {
            bot: request.bot.clone(),
            repository: request.repository.clone(),
        })
        .map_err(|e| e.to_string())
}

fn shared_folders(desktop: &Desktop) -> Vec<PathBuf> {
    desktop
        .client
        .status()
        .map(|status| {
            status
                .roots
                .iter()
                .filter_map(|root| root.local_path.as_ref().map(PathBuf::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Asks in the window, and answers how it ended. A page's request.
pub fn ask(
    app: &AppHandle,
    desktop: &Arc<Desktop>,
    setups: &Arc<CodingSetups>,
    request: Request,
) -> Result<Value, String> {
    request.check()?;
    let Some(_asking) = desktop.asking.start() else {
        return Err("Answer the question Chat with Work Desktop is asking first.".to_string());
    };
    // The bot as the server says it to this device, which also says this
    // person lent it this computer, or refuses.
    let context = desktop
        .client
        .request(&ControlRequest::SettingsContext {
            bot: Some(request.bot.clone()),
        })
        .map_err(|_| "This bot doesn't use this computer.".to_string())?;
    let name = context["bot"]["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("This bot")
        .to_string();
    let (answer, heard) = mpsc::channel();
    *setups.showing.lock().expect("coding setup") = Some(Showing {
        request,
        name,
        commands_allowed: context["commands_allowed"] != false,
        answer,
        shown: None,
        answered: false,
    });
    let opened = show(app);
    if let Err(e) = opened {
        *setups.showing.lock().expect("coding setup") = None;
        return Err(e.to_string());
    }
    let answered = heard.recv().unwrap_or(Answer::Declined);
    *setups.showing.lock().expect("coding setup") = None;
    if let Some(window) = app.get_webview_window(CODING_SETUP) {
        let _ = window.close();
    }
    Ok(match answered {
        Answer::Allowed => json!({ "ready": true }),
        Answer::Declined => json!({ "declined": true }),
    })
}

fn show(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(CODING_SETUP) {
        window.set_focus()?;
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        CODING_SETUP,
        WebviewUrl::App("coding-setup.html".into()),
    )
    .title("Chat with Work Desktop")
    .inner_size(520.0, 460.0)
    .resizable(true)
    .on_navigation(is_local_url)
    .build()?;
    let closing = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let setups = closing.state::<Arc<CodingSetups>>().inner().clone();
            if let Some(showing) = setups.showing.lock().expect("coding setup").as_ref() {
                let _ = showing.answer.send(Answer::Declined);
            }
        }
    });
    window.set_focus()
}

/// What the window shows, looked at again. Only the app's own window.
#[tauri::command]
pub async fn coding_setup_state(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    setups: State<'_, Arc<CodingSetups>>,
) -> Result<Value, String> {
    window_only(&webview)?;
    let (desktop, setups) = (desktop.inner().clone(), setups.inner().clone());
    tauri::async_runtime::spawn_blocking(move || {
        let (request, name, commands_allowed) = {
            let showing = setups.showing.lock().expect("coding setup");
            let showing = showing.as_ref().ok_or("Nothing is waiting.")?;
            (
                showing.request.clone(),
                showing.name.clone(),
                showing.commands_allowed,
            )
        };
        let look = look(&desktop, &request)?;
        let home = cww::paths::home_dir().map_err(|e| e.to_string())?;
        let shown = view(
            &request,
            &name,
            commands_allowed,
            &look,
            &home,
            &shared_folders(&desktop),
        );
        if let Some(showing) = setups.showing.lock().expect("coding setup").as_mut() {
            showing.shown = Some(shown.clone());
        }
        Ok(shown)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The person's answer. Allow does everything the window said, and only what
/// it can still say now.
#[tauri::command]
pub async fn coding_setup_answer(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    setups: State<'_, Arc<CodingSetups>>,
    allow: bool,
) -> Result<Value, String> {
    window_only(&webview)?;
    let (desktop, setups) = (desktop.inner().clone(), setups.inner().clone());
    tauri::async_runtime::spawn_blocking(move || {
        let (request, name, commands_allowed, sender, before) = {
            let mut showing = setups.showing.lock().expect("coding setup");
            let showing = showing.as_mut().ok_or("Nothing is waiting.")?;
            if showing.answered {
                return Err("It's already being answered.".to_string());
            }
            showing.answered = true;
            (
                showing.request.clone(),
                showing.name.clone(),
                showing.commands_allowed,
                showing.answer.clone(),
                showing.shown.clone(),
            )
        };
        let reopen = || {
            if let Some(showing) = setups.showing.lock().expect("coding setup").as_mut() {
                showing.answered = false;
            }
        };
        if !allow {
            let _ = sender.send(Answer::Declined);
            return Ok(json!({ "declined": true }));
        }
        // Whatever stops Allow here leaves the window's buttons to press again.
        let allowed = (|| -> Result<(), String> {
            // The bot again, as the server says it to this device now: still
            // lent this computer, and still allowed commands.
            let context = desktop.client.request(&ControlRequest::SettingsContext {
                bot: Some(request.bot.clone()),
            });
            let still = context.as_ref().is_ok_and(|c| {
                c["bot"]["name"].as_str().map(str::trim) == Some(name.as_str())
                    && c["commands_allowed"] != false
            });
            let look = look(&desktop, &request)?;
            let home = cww::paths::home_dir().map_err(|e| e.to_string())?;
            let shared = shared_folders(&desktop);
            let shown = view(
                &request,
                &name,
                commands_allowed && still,
                &look,
                &home,
                &shared,
            );
            if shown["can_allow"] != true || before.as_ref() != Some(&shown) {
                return Err("Something changed. Check again.".to_string());
            }
            let agent = chosen(&look, request.agent.as_deref())
                .and_then(|a| a["agent"].as_str())
                .and_then(Agent::from_key)
                .ok_or("No coding agent to use.")?;
            let root = look["root"]["id"].as_str().map(str::to_string);
            let folder = match root {
                Some(_) => None,
                None => {
                    let folder = new_folder(&home, &shared).ok_or("No free folder.")?;
                    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    Some(folder)
                }
            };
            desktop
                .client
                .request(&ControlRequest::CodingAllow {
                    root,
                    folder,
                    bot: request.bot.clone(),
                    name: Some(name),
                    agent,
                    owner: request
                        .repository
                        .as_deref()
                        .and_then(|r| r.split_once('/'))
                        .map(|(owner, _)| owner.to_string()),
                    repository: request.repository.clone(),
                })
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = allowed {
            reopen();
            return Err(e);
        }
        let _ = sender.send(Answer::Allowed);
        Ok(json!({ "ready": true }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            bot: "34".into(),
            agent: Some("claude_code".into()),
            repository: Some("acme/app".into()),
        }
    }

    fn look(installed: bool, signed_in: bool, reaches: Option<bool>) -> Value {
        json!({
            "agents": [
                { "agent": "claude_code", "name": "Claude Code", "installed": installed, "signed_in": signed_in, "sign_in_hint": "run `claude` once and sign in" },
                { "agent": "codex", "name": "Codex", "installed": true, "signed_in": true, "sign_in_hint": "run `codex login`" },
            ],
            "reaches": reaches,
            "root": null,
        })
    }

    #[test]
    fn a_page_asks_only_by_a_bots_number_and_a_repositorys_name() {
        assert!(request().check().is_ok());
        let refuse = |change: fn(&mut Request)| {
            let mut asked = request();
            change(&mut asked);
            assert!(asked.check().is_err(), "{asked:?}");
        };
        refuse(|r| r.bot = "../34".into());
        refuse(|r| r.bot = String::new());
        refuse(|r| r.agent = Some("rm -rf".into()));
        refuse(|r| r.repository = Some("git@github.com:acme/app.git".into()));
        refuse(|r| r.repository = Some("../../etc".into()));
        assert!(
            serde_json::from_value::<Request>(json!({ "bot": "34", "folder": "/" })).is_err(),
            "nothing else can be asked"
        );
    }

    #[test]
    fn it_says_what_it_checked_and_allows_only_what_holds() {
        let home = tempfile::tempdir().unwrap();
        let shown = view(
            &request(),
            "Fixer",
            true,
            &look(true, true, Some(true)),
            home.path(),
            &[],
        );
        assert_eq!(shown["title"], "Let Fixer (Bot 34) code here?");
        assert_eq!(shown["can_allow"], true);
        let checks = shown["checks"].to_string();
        assert!(
            checks.contains("Claude Code installed and signed in"),
            "{checks}"
        );
        assert!(
            checks.contains("Your Git login reaches acme/app"),
            "{checks}"
        );
        assert!(checks.contains("Works in a new folder, ~/bots"), "{checks}");
        assert!(
            shown["note"]
                .as_str()
                .unwrap()
                .contains("can read that sign-in")
        );

        for (installed, signed_in, reaches) in [
            (false, false, Some(true)),
            (true, false, Some(true)),
            (true, true, Some(false)),
            (true, true, None),
        ] {
            let shown = view(
                &request(),
                "Fixer",
                true,
                &look(installed, signed_in, reaches),
                home.path(),
                &[],
            );
            assert_eq!(shown["can_allow"], false, "{shown}");
        }
        let shown = view(
            &request(),
            "Fixer",
            false,
            &look(true, true, Some(true)),
            home.path(),
            &[],
        );
        assert_eq!(shown["can_allow"], false, "the organization said no");
    }

    #[test]
    fn an_agent_asked_for_that_isnt_here_is_missing() {
        let home = tempfile::tempdir().unwrap();
        let mut asked = request();
        asked.agent = Some("gemini".into());
        let shown = view(
            &asked,
            "Fixer",
            true,
            &look(true, true, Some(true)),
            home.path(),
            &[],
        );
        assert_eq!(shown["can_allow"], false, "{shown}");
        assert_eq!(
            shown["checks"][0],
            json!({ "ok": false, "text": "Gemini CLI wasn't found on this computer" })
        );

        for (installed, signed_in, text) in [
            (false, false, "Claude Code isn't installed on this computer"),
            (
                true,
                false,
                "Claude Code isn't signed in: run `claude` once and sign in",
            ),
        ] {
            let shown = view(
                &request(),
                "Fixer",
                true,
                &look(installed, signed_in, Some(true)),
                home.path(),
                &[],
            );
            assert_eq!(shown["checks"][0], json!({ "ok": false, "text": text }));
        }

        let mut asked = request();
        asked.agent = None;
        let shown = view(
            &asked,
            "Fixer",
            true,
            &json!({ "agents": [], "reaches": true }),
            home.path(),
            &[],
        );
        assert_eq!(shown["can_allow"], false, "{shown}");
        assert_eq!(
            shown["checks"][0],
            json!({ "ok": false, "text": "No coding agent is installed on this computer" })
        );
    }

    #[test]
    fn with_no_agent_named_it_offers_the_one_signed_in() {
        let home = tempfile::tempdir().unwrap();
        let mut asked = request();
        asked.agent = None;
        let shown = view(
            &asked,
            "Fixer",
            true,
            &look(true, false, Some(true)),
            home.path(),
            &[],
        );
        assert_eq!(shown["can_allow"], true);
        assert!(
            shown["checks"]
                .to_string()
                .contains("Codex installed and signed in")
        );
    }

    #[test]
    fn a_folder_with_something_of_the_persons_in_it_is_never_offered() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(new_folder(home.path(), &[]), Some(home.path().join("bots")));
        std::fs::create_dir_all(home.path().join("bots")).unwrap();
        assert_eq!(
            new_folder(home.path(), &[]),
            Some(home.path().join("bots")),
            "empty"
        );
        std::fs::write(home.path().join("bots/notes.txt"), "mine").unwrap();
        assert_eq!(
            new_folder(home.path(), &[]),
            Some(home.path().join("bots-2"))
        );
        let shared = vec![home.path().join("bots")];
        assert_eq!(
            new_folder(home.path(), &shared),
            Some(home.path().join("bots-2")),
            "never one already shared"
        );
    }
}
