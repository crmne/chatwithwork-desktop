//! `cww bots`: makes and manages Chat with Work bots from a terminal, as the
//! person this computer is paired with, for them or for an agent.
//!
//! The server decides everything: what a person may do, which choices a
//! recipe offers, and what it refuses, through the code behind the Bots
//! pages. This is a thin client: it turns flags and a config file into one
//! request, and says what came back, in words or as stable JSON. It holds a
//! token only for the length of a command, like `cww login` holds its key:
//! DPoP-bound, ten minutes, and never printed.
//!
//! A config file holds plain settings only. Nothing that looks like a secret
//! is read from a flag or a file, or sent, and nothing the server answers
//! carries one: a webhook's secret stays on the bot's page.
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::auth::client::AuthClient;
use crate::auth::tokens::Tokens;
use crate::auth::{Credentials, load_credentials, organizations};
use crate::chats::ChatClient;
use crate::config::Config;
use crate::control::Refusal;
use crate::paths::Paths;

const POLL_EVERY: Duration = Duration::from_secs(2);
/// What a field's name may not hold, in a flag or a file. Secrets are added
/// on the bot's page, which never shows them again.
const SECRET_WORDS: &[&str] = &[
    "secret",
    "token",
    "password",
    "passw",
    "api_key",
    "apikey",
    "credential",
    "private_key",
    "access_key",
];
/// The settings `create` and `run` read from a file.
const CREATE_KEYS: &[&str] = &[
    "recipe",
    "name",
    "description",
    "instructions",
    "shared",
    "project",
    "monthly_credits_cap",
    "code",
    "notify",
    "writes",
    "integrations",
    "tools",
    "through_app",
    "github_code",
    "github",
    "schedule",
];
/// What `schedule` in a file may hold. Its trigger's own fields are the
/// server's to check.
const SCHEDULE_KEYS: &[&str] = &[
    "frequency",
    "at",
    "on_weekday",
    "on_day",
    "recurrence",
    "time_zone",
    "repeats",
    "run_at",
    "trigger_attributes",
];
/// The settings `update` reads from a file.
const UPDATE_KEYS: &[&str] = &[
    "name",
    "description",
    "instructions",
    "when",
    "time_zone",
    "add_integrations",
    "remove_integrations",
    "tell",
    "changes",
    "reply_mode",
    "monthly_credits_cap",
    "reads_organization_instructions",
    "writes",
    "policies",
];

/// The API behind the commands, in one organization: the computer's own
/// sign-in, or a saved one (`--org`, as `cww tui --organization`).
pub struct Bots {
    client: ChatClient,
}

impl Bots {
    pub fn connect(paths: &Paths, organization: Option<&str>) -> Result<Self> {
        let credentials = match organization {
            Some(selector) => organizations::resolve(paths, selector)?.credentials(paths)?,
            None => primary_credentials(paths)?,
        };
        let proxy =
            crate::proxy::for_server(Config::load(paths)?.proxy.as_deref(), &credentials.server)?;
        let tokens = Tokens::new(
            Arc::new(AuthClient::new(credentials.server, proxy.as_ref())?),
            Arc::new(credentials.key),
            credentials.store,
        );
        Ok(Self {
            client: ChatClient::new(Arc::new(tokens)),
        })
    }

    /// One call. A computer not allowed yet asks, which waits in Settings,
    /// and says where to answer.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        match self.client.bots_call(method, path, body) {
            Err(error) => {
                let refusal = refusal_of(error);
                Err(self.explained(refusal).into())
            }
            ok => ok,
        }
    }

    fn explained(&self, refusal: Refusal) -> Refusal {
        match refusal.code.as_str() {
            "bots_access_required" => {
                let asked = self
                    .client
                    .bots_call("POST", "desktop/bots_access_request", None)
                    .ok();
                let url = asked
                    .as_ref()
                    .and_then(|answer| answer["approve_url"].as_str())
                    .or(refusal.details["approve_url"].as_str())
                    .unwrap_or("Settings, under Computers")
                    .to_string();
                let message = format!(
                    "This computer can't manage your bots yet. Allow it at {url}, then run this again."
                );
                Refusal::new("bots_access_required", message).with_details(json!({
                    "requested": true, "approve_url": url,
                }))
            }
            "unsupported" => Refusal::new(
                "unsupported",
                "This Chat with Work server doesn't offer bots to the terminal.",
            ),
            _ => refusal,
        }
    }

    pub fn list(&self) -> Result<Value> {
        self.call("GET", "desktop/bots", None)
    }

    pub fn show(&self, id: &str) -> Result<Value> {
        self.call("GET", &format!("desktop/bots/{}", bot_id(id)?), None)
    }

    pub fn recipes(&self) -> Result<Value> {
        self.call("GET", "desktop/bot_recipes", None)
    }

    pub fn recipe(&self, key: &str, tools: &[(String, String)]) -> Result<Value> {
        let mut query = String::new();
        for (kind, tool) in tools {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!(
                "tools%5B{}%5D={}",
                query_word(kind)?,
                query_word(tool)?
            ));
        }
        self.call(
            "GET",
            &format!("desktop/bot_recipes/{}{query}", query_word(key)?),
            None,
        )
    }

    pub fn triggers(&self, query: Option<&str>) -> Result<Value> {
        let path = match query {
            Some(query) => format!("desktop/bot_triggers?q={}", percent(query)),
            None => "desktop/bot_triggers".to_string(),
        };
        self.call("GET", &path, None)
    }

    pub fn trigger(&self, key: &str) -> Result<Value> {
        self.call(
            "GET",
            &format!("desktop/bot_triggers/{}", query_word(key)?),
            None,
        )
    }

    pub fn create(&self, body: &Value) -> Result<Value> {
        self.call("POST", "desktop/bots", Some(body))
    }

    pub fn update(&self, id: &str, body: &Value) -> Result<Value> {
        self.call(
            "PATCH",
            &format!("desktop/bots/{}", bot_id(id)?),
            Some(body),
        )
    }

    pub fn pause(&self, id: &str) -> Result<Value> {
        self.call("POST", &format!("desktop/bots/{}/pause", bot_id(id)?), None)
    }

    pub fn resume(&self, id: &str) -> Result<Value> {
        self.call(
            "DELETE",
            &format!("desktop/bots/{}/pause", bot_id(id)?),
            None,
        )
    }

    pub fn delete(&self, id: &str) -> Result<Value> {
        self.call("DELETE", &format!("desktop/bots/{}", bot_id(id)?), None)
    }

    pub fn start_trial(&self, body: &Value) -> Result<Value> {
        self.call("POST", "desktop/bot_trials", Some(body))
    }

    pub fn trial(&self, id: u64) -> Result<Value> {
        self.call("GET", &format!("desktop/bot_trials/{id}"), None)
    }

    /// Follows a try until it ends, or `limit` passes.
    pub fn wait_for_trial(&self, id: u64, limit: Duration) -> Result<Value> {
        let deadline = Instant::now() + limit;
        loop {
            let answer = self.trial(id)?;
            if answer["trial"]["state"] != "running" || Instant::now() >= deadline {
                return Ok(answer);
            }
            std::thread::sleep(POLL_EVERY);
        }
    }
}

fn primary_credentials(paths: &Paths) -> Result<Credentials> {
    let config = Config::load(paths)?;
    load_credentials(&config, paths)?.ok_or_else(|| {
        Refusal::new(
            "not_paired",
            "This computer isn't signed in. Run cww login --bots first.",
        )
        .into()
    })
}

fn refusal_of(error: anyhow::Error) -> Refusal {
    error
        .downcast::<Refusal>()
        .unwrap_or_else(|error| Refusal::new("failed", format!("{error:#}")))
}

/// The error a failed command prints: `{"error": {"code", "message", ...}}`.
pub fn error_json(error: &anyhow::Error) -> Value {
    let (code, message, details) = match error.downcast_ref::<Refusal>() {
        Some(refusal) => (
            refusal.code.clone(),
            refusal.message.clone(),
            refusal.details.clone(),
        ),
        None => ("failed".to_string(), format!("{error:#}"), json!({})),
    };
    let mut body = Map::new();
    body.insert("code".into(), json!(code));
    body.insert("message".into(), json!(message));
    if let Value::Object(details) = details {
        body.extend(details);
    }
    json!({ "error": body })
}

/// A failure in words, with what to do next where the server said.
pub fn error_lines(error: &anyhow::Error) -> Vec<String> {
    let Some(refusal) = error.downcast_ref::<Refusal>() else {
        return vec![format!("{error:#}")];
    };
    let mut lines = match refusal.details["errors"].as_array() {
        Some(errors) if !errors.is_empty() => errors
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => vec![refusal.message.clone()],
    };
    if refusal.details["trigger_missing"] == true {
        lines.push(
            "Say what starts it with --app (--app schedule --frequency weekday --at 08:30). \
             See cww bots triggers."
                .to_string(),
        );
    }
    if refusal.code == "model_consent" {
        lines.push("Agree on the web, then run this again.".to_string());
    }
    lines
}

/// A bot's ID, checked before it goes into a path.
pub fn bot_id(id: &str) -> Result<&str> {
    if !id.is_empty() && id.len() <= 18 && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(Refusal::new("invalid", "A bot's ID is a number. See cww bots list.").into())
    }
}

fn query_word(word: &str) -> Result<&str> {
    if !word.is_empty()
        && word.len() <= 100
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(word)
    } else {
        Err(Refusal::new("invalid", format!("{word:?} isn't a key.")).into())
    }
}

fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// `kind=value`, as `--tool` and its like take it.
pub fn pair(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
        _ => Err("expected key=value".to_string()),
    }
}

/// What `create` and `run` send but what a file gives: the flags that say
/// what a bot is. Each maps to a field on New bot's page.
#[derive(Debug, Default, Clone)]
pub struct Form {
    pub recipe: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub tools: Vec<(String, String)>,
    pub integrations: Vec<String>,
    pub code: bool,
    pub notify: Option<String>,
    pub writes: Option<String>,
    pub shared: bool,
    pub project: Option<u64>,
    pub cap: Option<u64>,
    pub app: Option<String>,
    pub events: Vec<String>,
    pub trigger_fields: Vec<(String, String)>,
    pub frequency: Option<String>,
    pub at: Option<String>,
    pub on_weekday: Option<String>,
    pub on_day: Option<String>,
    pub once: Option<String>,
    pub recurrence: Option<String>,
    pub time_zone: Option<String>,
}

impl Form {
    /// The request New bot's form would post: a file's settings, with the
    /// flags over them. Refuses what can't be a plain setting.
    pub fn body(&self, file: Option<&Path>) -> Result<Value> {
        let settings = match file {
            Some(path) => read_settings(path, CREATE_KEYS)?,
            None => Map::new(),
        };
        self.body_over(settings)
    }

    fn body_over(&self, mut settings: Map<String, Value>) -> Result<Value> {
        let mut schedule = match settings.remove("schedule") {
            Some(Value::Object(schedule)) => schedule,
            Some(_) => return Err(invalid("schedule in a file is a mapping of its fields")),
            None => Map::new(),
        };
        let recipe = self
            .recipe
            .clone()
            .map(Value::String)
            .or(settings.remove("recipe"));
        let mut bot = settings;
        if let Some(tools) = bot.remove("tools") {
            bot.insert("choices".into(), tools);
        }
        let mut set = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                bot.insert(key.to_string(), value);
            }
        };
        set("name", self.name.clone().map(Value::String));
        set("description", self.description.clone().map(Value::String));
        set("instructions", self.instructions.clone().map(Value::String));
        set("notify", self.notify.clone().map(Value::String));
        set("writes", self.writes.clone().map(Value::String));
        set("code", self.code.then_some(json!(true)));
        set("shared", self.shared.then_some(json!(true)));
        set("project", self.project.map(|number| json!(number)));
        set("monthly_credits_cap", self.cap.map(|cap| json!(cap)));
        if !self.integrations.is_empty() {
            bot.insert("integrations".into(), json!(self.integrations));
        }
        if !self.tools.is_empty() {
            let choices = bot
                .entry("choices")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| invalid("tools in a file map a slot to its tool"))?;
            for (kind, tool) in &self.tools {
                choices.insert(kind.clone(), json!(tool));
            }
        }

        self.fill_timing(&mut schedule);
        self.fill_trigger(&mut schedule, recipe.is_some())?;

        let mut body = Map::new();
        if let Some(recipe) = recipe {
            body.insert("recipe".into(), recipe);
        }
        body.insert("bot".into(), Value::Object(bot));
        if !schedule.is_empty() {
            body.insert("schedule".into(), Value::Object(schedule));
        }
        let body = Value::Object(body);
        refuse_secrets(&body)?;
        Ok(body)
    }

    fn fill_timing(&self, schedule: &mut Map<String, Value>) {
        let mut set = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                schedule.insert(key.to_string(), json!(value));
            }
        };
        set("frequency", &self.frequency);
        set("at", &self.at);
        set("on_weekday", &self.on_weekday);
        set("on_day", &self.on_day);
        set("recurrence", &self.recurrence);
        set("time_zone", &self.time_zone);
        if let Some(run_at) = &self.once {
            schedule.insert("repeats".into(), json!("once"));
            schedule.insert("run_at".into(), json!(run_at));
        } else if self.frequency.is_some() {
            schedule.insert("repeats".into(), json!("repeat"));
        }
    }

    /// When's fields: its app, with its events and fields under its key.
    fn fill_trigger(&self, schedule: &mut Map<String, Value>, recipe: bool) -> Result<()> {
        let attributes = schedule
            .entry("trigger_attributes")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| invalid("trigger_attributes in a file is a mapping"))?;
        let timed = self.frequency.is_some()
            || self.at.is_some()
            || self.once.is_some()
            || self.recurrence.is_some();
        let app = self
            .app
            .clone()
            .or(attributes
                .get("app")
                .and_then(Value::as_str)
                .map(str::to_string))
            .or((timed && !recipe).then(|| "schedule".to_string()));
        if let Some(app) = &app {
            attributes.insert("app".into(), json!(app));
            if !self.events.is_empty() || !self.trigger_fields.is_empty() {
                let fields = attributes
                    .entry(app.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| invalid("an app's fields in a file are a mapping"))?;
                if !self.events.is_empty() {
                    fields.insert("events".into(), json!(self.events));
                }
                for (key, value) in &self.trigger_fields {
                    fields.insert(key.clone(), json!(value));
                }
            }
        } else if !self.events.is_empty() || !self.trigger_fields.is_empty() {
            return Err(invalid("--event and --trigger-field need --app"));
        }
        if attributes.is_empty() {
            schedule.remove("trigger_attributes");
        }
        Ok(())
    }
}

/// What `update` changes: what the bot's page and the chat that builds a bot
/// can change, and no more. Only what's given changes.
#[derive(Debug, Default, Clone)]
pub struct Change {
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub when: Option<String>,
    pub time_zone: Option<String>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub notify: Option<String>,
    pub changes: Option<String>,
    pub reply_mode: Option<String>,
    pub cap: Option<u64>,
    pub writes: Vec<(String, String)>,
    pub policies: Vec<(String, String)>,
    pub allow_destructive: bool,
}

impl Change {
    pub fn body(&self, file: Option<&Path>) -> Result<Value> {
        let mut body = match file {
            Some(path) => read_settings(path, UPDATE_KEYS)?,
            None => Map::new(),
        };
        let mut set = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                body.insert(key.to_string(), value);
            }
        };
        set("name", self.name.clone().map(Value::String));
        set("description", self.description.clone().map(Value::String));
        set("instructions", self.instructions.clone().map(Value::String));
        set("when", self.when.clone().map(Value::String));
        set("time_zone", self.time_zone.clone().map(Value::String));
        set("tell", self.notify.clone().map(Value::String));
        set("changes", self.changes.clone().map(Value::String));
        set("reply_mode", self.reply_mode.clone().map(Value::String));
        set("monthly_credits_cap", self.cap.map(|cap| json!(cap)));
        set(
            "add_integrations",
            (!self.add.is_empty()).then(|| json!(self.add)),
        );
        set(
            "remove_integrations",
            (!self.remove.is_empty()).then(|| json!(self.remove)),
        );
        set(
            "allow_destructive",
            self.allow_destructive.then_some(json!(true)),
        );
        merge_pairs(&mut body, "writes", &self.writes)?;
        merge_pairs(&mut body, "policies", &self.policies)?;
        let body = Value::Object(body);
        refuse_secrets(&body)?;
        Ok(body)
    }
}

fn merge_pairs(body: &mut Map<String, Value>, key: &str, pairs: &[(String, String)]) -> Result<()> {
    if !pairs.is_empty() {
        let entries = body
            .entry(key)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| invalid(&format!("{key} in a file is a mapping")))?;
        for (name, value) in pairs {
            entries.insert(name.clone(), json!(value));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> anyhow::Error {
    Refusal::new("invalid", message).into()
}

/// A config file's settings. YAML, or JSON, which is YAML: only the keys
/// `allowed` names, so a typo says so rather than quietly doing nothing.
fn read_settings(path: &Path, allowed: &[&str]) -> Result<Map<String, Value>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_settings(&text, allowed)
}

fn parse_settings(text: &str, allowed: &[&str]) -> Result<Map<String, Value>> {
    let value: Value = serde_saphyr::from_str(text)
        .map_err(|error| invalid(&format!("That file isn't valid YAML: {error}")))?;
    let Value::Object(settings) = value else {
        return Err(invalid("That file should be a mapping of settings."));
    };
    refuse_secrets(&Value::Object(settings.clone()))?;
    only_known(&settings, allowed)?;
    if let Some(Value::Object(schedule)) = settings.get("schedule") {
        only_known(schedule, SCHEDULE_KEYS)?;
    }
    Ok(settings)
}

fn only_known(settings: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    let unknown: Vec<&str> = settings
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.contains(key))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(invalid(&format!(
            "That file has settings cww doesn't know: {}. It takes {}.",
            unknown.join(", "),
            allowed.join(", ")
        )))
    }
}

/// Secrets stay out of files and flags: refuses a key that names one, at any
/// depth. Nothing is read from its value, and the message doesn't quote it.
fn refuse_secrets(value: &Value) -> Result<()> {
    match value {
        Value::Object(fields) => {
            for (key, nested) in fields {
                let key = key.to_lowercase().replace(['-', '_'], "");
                if SECRET_WORDS
                    .iter()
                    .any(|word| key.contains(&word.replace('_', "")))
                {
                    return Err(invalid(
                        "A secret can't be set from a file or a flag: add it on the bot's page.",
                    ));
                }
                refuse_secrets(nested)?;
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(refuse_secrets),
        _ => Ok(()),
    }
}

/// Asks before something that can't be undone, unless `--yes` said so. Never
/// waits on a program: without a terminal and `--yes` it refuses.
pub fn confirm(question: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(Refusal::new(
            "confirm",
            format!("{question} Pass --yes to confirm.")
        ));
    }
    // Beside the answer, never in it: stdout may be a program's JSON.
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!(Refusal::new("cancelled", "Nothing changed."))
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn row(cells: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    for (cell, width) in cells.iter().zip(widths) {
        line.push_str(&format!("{cell:<width$}  "));
    }
    line.trim_end().to_string()
}

fn table(header: &[&str], rows: Vec<Vec<String>>) -> Vec<String> {
    let mut widths: Vec<usize> = header.iter().map(|cell| cell.chars().count()).collect();
    for cells in &rows {
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let header: Vec<String> = header.iter().map(|cell| cell.to_string()).collect();
    std::iter::once(row(&header, &widths))
        .chain(rows.iter().map(|cells| row(cells, &widths)))
        .collect()
}

pub fn render_list(answer: &Value) -> Vec<String> {
    let bots = answer["bots"].as_array().cloned().unwrap_or_default();
    if bots.is_empty() {
        return vec![format!(
            "No bots in {} yet. Start from cww bots recipes.",
            text(&answer["account"], "name")
        )];
    }
    table(
        &["ID", "NAME", "STATE", "WHERE", "NEXT RUN"],
        bots.iter()
            .map(|bot| {
                vec![
                    bot["id"].to_string(),
                    text(bot, "name").to_string(),
                    text(bot, "state").to_string(),
                    bot["project"]["name"]
                        .as_str()
                        .unwrap_or("Private")
                        .to_string(),
                    text(bot, "next_run").to_string(),
                ]
            })
            .collect(),
    )
}

pub fn render_bot(answer: &Value) -> Vec<String> {
    let bot = &answer["bot"];
    let mut lines = vec![
        format!("{} (bot {})", text(bot, "name"), bot["id"]),
        format!("State: {}", text(bot, "state")),
        format!(
            "Where: {}",
            bot["project"]["name"].as_str().unwrap_or("Private")
        ),
    ];
    if let Some(description) = bot["description"].as_str() {
        lines.push(format!("About: {description}"));
    }
    if let Some(mode) = bot["reply_mode"].as_str() {
        lines.push(format!("Replies: {mode}"));
    }
    if let Some(notify) = bot["notify"].as_str() {
        lines.push(format!("Tells you: {notify}"));
    }
    for trigger in bot["triggers"].as_array().into_iter().flatten() {
        lines.push(format!(
            "When: {}{}",
            text(trigger, "timing"),
            if trigger["paused"] == true {
                " (paused)"
            } else {
                ""
            }
        ));
    }
    for integration in bot["integrations"].as_array().into_iter().flatten() {
        let name = integration["provider"]
            .as_str()
            .unwrap_or(text(integration, "kind"));
        let actor = integration["acts_as"]
            .as_str()
            .map(|who| format!(" as {who}"))
            .unwrap_or_default();
        lines.push(format!(
            "Uses: {name}{actor}, changes {}",
            text(integration, "writes")
        ));
    }
    for missing in bot["missing"].as_array().into_iter().flatten() {
        lines.push(format!("Missing: {}", missing.as_str().unwrap_or_default()));
    }
    if let Some(instructions) = bot["instructions"].as_str() {
        lines.push(String::new());
        lines.push(instructions.to_string());
        lines.push(String::new());
    }
    lines.push(text(bot, "url").to_string());
    for tool in answer["connect_first"].as_array().into_iter().flatten() {
        lines.push(format!(
            "Connect {} on its page, or it works without it.",
            text(tool, "name")
        ));
    }
    if let Some(next) = answer["next"].as_str() {
        lines.push(next.to_string());
    }
    lines
}

pub fn render_recipes(answer: &Value) -> Vec<String> {
    let recipes = answer["recipes"].as_array().cloned().unwrap_or_default();
    table(
        &["RECIPE", "NAME", "STARTS", "GROUP"],
        recipes
            .iter()
            .map(|recipe| {
                vec![
                    text(recipe, "key").to_string(),
                    text(recipe, "name").to_string(),
                    text(recipe, "starts").to_string(),
                    text(recipe, "group").to_string(),
                ]
            })
            .collect(),
    )
}

/// One recipe's page: everything `create` can be told.
pub fn render_recipe(answer: &Value) -> Vec<String> {
    let defaults = &answer["defaults"];
    let mut lines = Vec::new();
    match answer["recipe"].as_object() {
        Some(_) => {
            let recipe = &answer["recipe"];
            lines.push(format!(
                "{} ({})",
                text(recipe, "name"),
                text(recipe, "key")
            ));
            lines.push(text(recipe, "summary").to_string());
            if let Some(credits) = recipe["credits"].as_str() {
                lines.push(credits.to_string());
            }
        }
        None => lines.push("A bot from nothing".to_string()),
    }
    if let Some(trigger) = answer["trigger"].as_object() {
        let unavailable = trigger
            .get("unavailable")
            .and_then(Value::as_str)
            .map(|why| format!(" ({why})"))
            .unwrap_or_default();
        lines.push(format!(
            "Starts: {}{unavailable}",
            trigger
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
    }
    for slot in answer["slots"].as_array().into_iter().flatten() {
        let tools: Vec<String> = slot["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| {
                format!(
                    "{}{}",
                    text(tool, "key"),
                    if tool["connected"] == true {
                        " (connected)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        lines.push(format!(
            "--tool {}=KEY: {}{}, chosen {}: {}",
            slot["kind"].as_str().unwrap_or_default(),
            text(slot, "label"),
            if slot["optional"] == true {
                " (optional)"
            } else {
                ""
            },
            slot["chosen"].as_str().unwrap_or("none"),
            tools.join(", ")
        ));
    }
    for tool in answer["connect_first"].as_array().into_iter().flatten() {
        lines.push(format!("Not connected yet: {}", text(tool, "name")));
    }
    let integrations: Vec<&str> = answer["integrations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|integration| text(integration, "key"))
        .collect();
    if !integrations.is_empty() {
        lines.push(format!("--integration KEY: {}", integrations.join(", ")));
    }
    if answer["can_share"] == true {
        let projects: Vec<String> = answer["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|project| format!("{} ({})", project["number"], text(project, "name")))
            .collect();
        lines.push(format!(
            "--shared --project NUMBER: {}",
            projects.join(", ")
        ));
    }
    lines.push(format!(
        "--notify {} (now {})",
        join(&answer["notify_modes"]),
        text(defaults, "notify")
    ));
    lines.push(format!(
        "--writes {} (now {})",
        join(&answer["writes_modes"]),
        text(defaults, "writes")
    ));
    let needs = answer["still_needs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if needs.is_empty() {
        lines.push("Nothing else is needed.".to_string());
    } else {
        lines.push("Still needed:".to_string());
        lines.extend(
            needs
                .iter()
                .filter_map(Value::as_str)
                .map(|need| format!("  {need}")),
        );
    }
    lines
}

fn join(values: &Value) -> String {
    values
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("|")
}

pub fn render_triggers(answer: &Value) -> Vec<String> {
    let triggers = answer["triggers"].as_array().cloned().unwrap_or_default();
    if triggers.is_empty() {
        return vec!["No trigger matches.".to_string()];
    }
    table(
        &["APP", "NAME", "GROUP", "HEARS"],
        triggers
            .iter()
            .map(|trigger| {
                vec![
                    text(trigger, "key").to_string(),
                    text(trigger, "name").to_string(),
                    text(trigger, "group").to_string(),
                    text(trigger, "hears").to_string(),
                ]
            })
            .collect(),
    )
}

pub fn render_trigger(answer: &Value) -> Vec<String> {
    let trigger = &answer["trigger"];
    let mut lines = vec![
        format!("{} (--app {})", text(trigger, "name"), text(trigger, "key")),
        text(trigger, "hears").to_string(),
    ];
    if let Some(reason) = trigger["unavailable"].as_str() {
        lines.push(format!("Not available: {reason}"));
    }
    for event in trigger["events"].as_array().into_iter().flatten() {
        lines.push(format!(
            "--event {}: {}",
            text(event, "key"),
            text(event, "name")
        ));
    }
    for field in trigger["fields"].as_array().into_iter().flatten() {
        lines.push(format!(
            "--trigger-field {}=VALUE",
            field.as_str().unwrap_or_default()
        ));
    }
    lines
}

pub fn render_trial(answer: &Value) -> Vec<String> {
    let trial = &answer["trial"];
    let mut lines = vec![format!(
        "{} ({})",
        text(trial, "title"),
        text(trial, "state")
    )];
    if let Some(failure) = trial["failure"].as_str() {
        lines.push(failure.to_string());
    }
    if let Some(answer) = trial["answer"].as_str() {
        lines.push(String::new());
        lines.push(answer.to_string());
    }
    if let Some(tell) = trial["would_tell"].as_str() {
        lines.push(format!("It would tell you: {tell}"));
    }
    let asks: Vec<&str> = trial["would_ask"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !asks.is_empty() {
        lines.push(format!("It would ask you to approve: {}", asks.join(", ")));
    }
    if trial["state"] == "running" {
        lines.push(format!(
            "Still running. Follow it with cww bots run --follow {}.",
            trial["id"]
        ));
    } else {
        lines.push("A try, so nothing changed.".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        Form::default()
    }

    #[test]
    fn a_recipe_sends_only_what_differs_from_the_page() {
        let body = Form {
            recipe: Some("morning_briefing".into()),
            tools: vec![("calendar".into(), "outlook".into())],
            ..form()
        }
        .body(None)
        .unwrap();
        assert_eq!(
            body,
            json!({ "recipe": "morning_briefing", "bot": { "choices": { "calendar": "outlook" } } })
        );
    }

    #[test]
    fn timing_flags_start_on_a_schedule() {
        let body = Form {
            name: Some("Morning".into()),
            instructions: Some("Say hello.".into()),
            frequency: Some("weekday".into()),
            at: Some("08:30".into()),
            ..form()
        }
        .body(None)
        .unwrap();
        assert_eq!(
            body["schedule"],
            json!({ "frequency": "weekday", "at": "08:30", "repeats": "repeat", "trigger_attributes": { "app": "schedule" } })
        );
    }

    #[test]
    fn a_once_run_says_so_and_a_recipes_app_stays_its_own() {
        let body = Form {
            once: Some("2026-12-01T09:00".into()),
            ..form()
        }
        .body(None)
        .unwrap();
        assert_eq!(body["schedule"]["repeats"], "once");
        assert_eq!(body["schedule"]["run_at"], "2026-12-01T09:00");

        let body = Form {
            recipe: Some("r".into()),
            at: Some("09:00".into()),
            ..form()
        }
        .body(None)
        .unwrap();
        assert!(body["schedule"].get("trigger_attributes").is_none());
    }

    #[test]
    fn events_and_fields_go_under_their_app() {
        let body = Form {
            app: Some("github".into()),
            events: vec!["issue_opened".into()],
            trigger_fields: vec![("repository".into(), "crmne/cww".into())],
            ..form()
        }
        .body(None)
        .unwrap();
        assert_eq!(
            body["schedule"]["trigger_attributes"],
            json!({ "app": "github", "github": { "events": ["issue_opened"], "repository": "crmne/cww" } })
        );
    }

    #[test]
    fn events_without_an_app_are_refused() {
        let error = Form {
            events: vec!["x".into()],
            ..form()
        }
        .body(None)
        .unwrap_err();
        assert!(error.to_string().contains("--app"), "{error}");
    }

    #[test]
    fn a_file_gives_the_same_form_with_flags_over_it() {
        let settings = parse_settings(
            "recipe: lead_triage\nname: From the file\ntools:\n  crm: hubspot\nmonthly_credits_cap: 500\n\
             shared: true\nproject: 12\nschedule:\n  frequency: day\n  at: '09:00'\n  trigger_attributes:\n    app: schedule\n",
            CREATE_KEYS,
        )
        .unwrap();
        let body = Form {
            name: Some("From the flag".into()),
            tools: vec![("mail".into(), "outlook".into())],
            ..form()
        }
        .body_over(settings)
        .unwrap();
        assert_eq!(body["recipe"], "lead_triage");
        assert_eq!(body["bot"]["name"], "From the flag");
        assert_eq!(
            body["bot"]["choices"],
            json!({ "crm": "hubspot", "mail": "outlook" })
        );
        assert_eq!(body["bot"]["project"], 12);
        assert_eq!(body["schedule"]["trigger_attributes"]["app"], "schedule");
        assert!(body["bot"].get("schedule").is_none());
        assert!(body["bot"].get("recipe").is_none());
    }

    #[test]
    fn a_files_unknown_setting_is_named() {
        let error = parse_settings("nmae: Typo\n", CREATE_KEYS).unwrap_err();
        assert!(error.to_string().contains("nmae"), "{error}");
    }

    #[test]
    fn a_file_or_a_flag_never_carries_a_secret() {
        for text in [
            "api_key: abc123\n",
            "schedule:\n  trigger_attributes:\n    webhook:\n      signing-secret: abc123\n",
            "github:\n  access_token: abc123\n",
            "integrations:\n  - password: abc123\n",
        ] {
            let error = parse_settings(text, CREATE_KEYS).unwrap_err();
            let error = error.to_string();
            assert!(error.contains("secret"), "{error}");
            assert!(!error.contains("abc123"), "{error}");
        }
        let error = Form {
            app: Some("webhook".into()),
            trigger_fields: vec![("secret".into(), "abc123".into())],
            ..form()
        }
        .body(None)
        .unwrap_err();
        assert!(!error.to_string().contains("abc123"), "{error}");
    }

    #[test]
    fn update_sends_only_whats_given() {
        let body = Change {
            name: Some("New".into()),
            notify: Some("never".into()),
            changes: Some("automatic".into()),
            writes: vec![("slack".into(), "ask".into())],
            policies: vec![("calendar_delete".into(), "automatic".into())],
            allow_destructive: true,
            ..Change::default()
        }
        .body(None)
        .unwrap();
        assert_eq!(
            body,
            json!({ "name": "New", "tell": "never", "changes": "automatic", "writes": { "slack": "ask" },
                "policies": { "calendar_delete": "automatic" }, "allow_destructive": true })
        );
    }

    #[test]
    fn ids_and_keys_are_checked_before_they_reach_a_path() {
        assert_eq!(bot_id("42").unwrap(), "42");
        for id in ["", "4/2", "../1", "x", "1234567890123456789"] {
            assert!(bot_id(id).is_err(), "{id}");
        }
        assert!(query_word("issue_triage").is_ok());
        assert!(query_word("a/b").is_err());
        assert_eq!(percent("a b&c"), "a%20b%26c");
    }

    #[test]
    fn pairs_need_a_key() {
        assert_eq!(pair("a=b=c").unwrap(), ("a".into(), "b=c".into()));
        assert!(pair("=b").is_err());
        assert!(pair("ab").is_err());
    }

    #[test]
    fn a_refusal_prints_stable_json_and_every_reason() {
        let error: anyhow::Error = Refusal::new("invalid", "Name can't be blank and more")
            .with_details(json!({ "errors": ["Name can't be blank", "Choose what starts it"], "trigger_missing": true, "status": 422 }))
            .into();
        let body = error_json(&error);
        assert_eq!(body["error"]["code"], "invalid");
        assert_eq!(body["error"]["status"], 422);
        let lines = error_lines(&error);
        assert_eq!(lines[0], "Name can't be blank");
        assert!(lines[2].contains("--app"));
    }

    #[test]
    fn a_secret_is_refused_however_its_name_is_written() {
        for key in [
            "privateKey",
            "accessKey",
            "api-key",
            "ApiKey",
            "client_secret",
        ] {
            let error = refuse_secrets(&json!({ "webhook": { key: "x" } })).unwrap_err();
            assert!(error.to_string().contains("secret"), "{key}");
        }
        assert!(refuse_secrets(&json!({ "keywords": "x", "monkey": 1 })).is_ok());
    }

    #[test]
    fn a_misspelled_schedule_setting_is_refused_before_anything_is_sent() {
        let error = parse_settings("schedule:\n  frequncy: weekday\n", CREATE_KEYS).unwrap_err();
        assert!(error.to_string().contains("frequncy"), "{error}");
        let good = "schedule:\n  frequency: weekday\n  trigger_attributes:\n    app: schedule\n";
        assert!(parse_settings(good, CREATE_KEYS).is_ok());
    }

    #[test]
    fn a_time_keeps_the_trigger_of_a_recipe_from_a_file() {
        let form = Form {
            at: Some("09:00".into()),
            ..Form::default()
        };
        let file = parse_settings("recipe: triage\n", CREATE_KEYS).unwrap();
        let body = form.body_over(file).unwrap();
        assert!(
            body.pointer("/bot/schedule/trigger_attributes/app")
                .is_none(),
            "{body}"
        );
        assert!(
            body.to_string().find("\"app\":\"schedule\"").is_none(),
            "{body}"
        );
    }

    #[test]
    fn confirming_never_waits_on_a_program() {
        assert!(confirm("Delete?", true).is_ok());
    }

    #[test]
    fn a_list_reads_like_a_table() {
        let lines = render_list(&json!({ "account": { "name": "Plenty" }, "bots": [
            { "id": 7, "name": "Triage", "state": "next", "project": null, "next_run": "2026-11-02T08:30:00Z" }
        ] }));
        assert!(lines[1].starts_with("7"), "{lines:?}");
        assert!(lines[1].contains("Private"), "{lines:?}");
        assert!(
            render_list(&json!({ "account": { "name": "Plenty" }, "bots": [] }))[0]
                .contains("cww bots recipes")
        );
    }

    #[test]
    fn a_recipe_shows_what_its_page_asks() {
        let lines = render_recipe(&json!({
            "recipe": { "key": "lead_triage", "name": "Lead triage", "summary": "Sort leads" },
            "defaults": { "notify": "when_needed", "writes": "ask" },
            "slots": [{ "kind": "crm", "label": "CRM", "optional": false, "chosen": "hubspot", "tools": [{ "key": "hubspot", "connected": true }] }],
            "notify_modes": ["when_needed", "always", "never"], "writes_modes": ["ask", "automatic"],
            "still_needs": ["Instructions can't be empty"],
        }));
        let text = lines.join("\n");
        assert!(
            text.contains("--tool crm=KEY: CRM, chosen hubspot: hubspot (connected)"),
            "{text}"
        );
        assert!(text.contains("--writes ask|automatic (now ask)"), "{text}");
        assert!(text.contains("Instructions can't be empty"), "{text}");
    }
}
