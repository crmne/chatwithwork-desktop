//! New bot, in a terminal, as the web's (Chat with Work's `docs/bots.md`,
//! "Building a bot in a chat"): "What should your bot do?" over a composer,
//! whose sentence goes straight to a chat that proposes the bot on a card
//! with Make bot and Try it; or a recipe to start from, with its When, the
//! tool for each of its slots, and its project, then Try it and Make bot.
//!
//! Bots are read and made with the computer's own sign-in, as `cww bots`
//! does, through the same endpoints, so the server decides what's offered
//! and what's refused.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::app::{App, Focus, Hit, Hits};
use super::theme::{Signal, Theme};
use super::ui::{BOT, COLUMN, DOT, ellipsize, key_hints, pad, row, spread, tail, wrap};

/// A recipe in New bot's gallery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub key: String,
    pub name: String,
    pub summary: Option<String>,
    pub group: Option<String>,
    /// What starts it: "Every weekday at 9:00", "When an email arrives".
    pub starts: Option<String>,
    pub credits: Option<String>,
}

/// What a bot made from a chat's card, or a recipe's form, should run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// What the recipe starts on.
    Recipe,
    Daily,
    Weekdays,
    Weekly,
    /// Only when someone asks it, or presses Run now.
    Asked,
}

impl When {
    const ALL: [When; 5] = [
        When::Recipe,
        When::Daily,
        When::Weekdays,
        When::Weekly,
        When::Asked,
    ];

    pub fn label(self, at: &str, recipe: Option<&str>) -> String {
        match self {
            When::Recipe => recipe.unwrap_or("As the recipe says").to_string(),
            When::Daily => format!("Every day at {at}"),
            When::Weekdays => format!("Every weekday at {at}"),
            When::Weekly => format!("Every Monday at {at}"),
            When::Asked => "When you ask".into(),
        }
    }

    fn timed(self) -> bool {
        matches!(self, When::Daily | When::Weekdays | When::Weekly)
    }
}

/// A row of a recipe's form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormRow {
    Name,
    When,
    /// A slot of the recipe, by its kind ("crm", "email").
    Slot(String),
    Project,
    Try,
    Make,
}

/// A recipe, opened as New bot's form.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeForm {
    pub key: String,
    /// The server's form: `recipe`, `defaults`, `slots`, `trigger`,
    /// `projects`, `still_needs`, ...
    pub data: Option<Value>,
    pub loading: bool,
    pub name: String,
    pub when: When,
    /// The time a timed When runs at, as typed.
    pub at: String,
    /// The tool picked for each slot, by its kind.
    pub choices: BTreeMap<String, String>,
    /// The project it's shared in, by number; `None` for the maker alone.
    pub project: Option<u64>,
    pub row: usize,
    /// "Trying…" or "Making…" while one runs.
    pub busy: Option<&'static str>,
    /// What a try came back with, or what making it said.
    pub result: Vec<String>,
    pub error: Option<String>,
}

impl RecipeForm {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            data: None,
            loading: true,
            name: String::new(),
            when: When::Recipe,
            at: "09:00".into(),
            choices: BTreeMap::new(),
            project: None,
            row: 0,
            busy: None,
            result: Vec::new(),
            error: None,
        }
    }

    /// The server's form arrived: fill in what the person hasn't changed.
    pub fn take(&mut self, data: Value) {
        if self.name.is_empty() {
            self.name = data["defaults"]["name"]
                .as_str()
                .or(data["recipe"]["name"].as_str())
                .unwrap_or_default()
                .to_string();
        }
        for slot in data["slots"].as_array().into_iter().flatten() {
            let kind = slot["kind"].as_str().unwrap_or_default().to_string();
            if let Some(chosen) = slot["chosen"].as_str() {
                self.choices
                    .entry(kind)
                    .or_insert_with(|| chosen.to_string());
            }
        }
        if !self.recipe_has_trigger(Some(&data)) && self.when == When::Recipe {
            self.when = When::Asked;
        }
        self.data = Some(data);
        self.loading = false;
    }

    fn recipe_has_trigger(&self, data: Option<&Value>) -> bool {
        data.or(self.data.as_ref())
            .is_some_and(|d| d["trigger"].is_object() || d["recipe"]["starts"].is_string())
    }

    pub fn slots(&self) -> Vec<&Value> {
        self.data
            .as_ref()
            .and_then(|d| d["slots"].as_array())
            .map(|s| s.iter().collect())
            .unwrap_or_default()
    }

    pub fn projects(&self) -> Vec<(u64, String)> {
        self.data
            .as_ref()
            .and_then(|d| d["projects"].as_array())
            .map(|p| {
                p.iter()
                    .filter_map(|p| Some((p["number"].as_u64()?, p["name"].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn rows(&self) -> Vec<FormRow> {
        let mut rows = vec![FormRow::Name, FormRow::When];
        for slot in self.slots() {
            if let Some(kind) = slot["kind"].as_str() {
                rows.push(FormRow::Slot(kind.to_string()));
            }
        }
        if !self.projects().is_empty() {
            rows.push(FormRow::Project);
        }
        rows.push(FormRow::Try);
        rows.push(FormRow::Make);
        rows
    }

    pub fn current(&self) -> FormRow {
        let rows = self.rows();
        rows[self.row.min(rows.len() - 1)].clone()
    }

    /// The When choices this recipe offers.
    fn whens(&self) -> Vec<When> {
        When::ALL
            .into_iter()
            .filter(|w| *w != When::Recipe || self.recipe_has_trigger(None))
            .collect()
    }

    /// Left or right on a row with choices.
    pub fn step(&mut self, forward: bool) -> bool {
        let turn = |len: usize, at: usize| {
            if forward {
                (at + 1) % len
            } else {
                (at + len - 1) % len
            }
        };
        match self.current() {
            FormRow::When => {
                let whens = self.whens();
                let at = whens.iter().position(|w| *w == self.when).unwrap_or(0);
                self.when = whens[turn(whens.len(), at)];
                false
            }
            FormRow::Slot(kind) => {
                let Some(slot) = self
                    .slots()
                    .into_iter()
                    .find(|s| s["kind"] == kind.as_str())
                    .cloned()
                else {
                    return false;
                };
                let mut tools: Vec<String> = slot["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["key"].as_str().map(str::to_string))
                    .collect();
                if slot["optional"].as_bool() == Some(true) {
                    tools.insert(0, String::new());
                }
                if tools.is_empty() {
                    return false;
                }
                let current = self.choices.get(&kind).cloned().unwrap_or_default();
                let at = tools.iter().position(|t| *t == current).unwrap_or(0);
                let next = tools[turn(tools.len(), at)].clone();
                if next.is_empty() {
                    self.choices.remove(&kind);
                } else {
                    self.choices.insert(kind, next);
                }
                // The server says again what the pick needs.
                true
            }
            FormRow::Project => {
                let mut choices: Vec<Option<u64>> = vec![None];
                choices.extend(self.projects().into_iter().map(|(n, _)| Some(n)));
                let at = choices.iter().position(|p| *p == self.project).unwrap_or(0);
                self.project = choices[turn(choices.len(), at)];
                false
            }
            _ => false,
        }
    }

    /// The tool picks, as the recipe's page takes them.
    pub fn tools(&self) -> Vec<(String, String)> {
        self.choices
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The request New bot's form would post, for making it or trying it.
    pub fn body(&self) -> anyhow::Result<Value> {
        let default_name = self
            .data
            .as_ref()
            .and_then(|d| d["defaults"]["name"].as_str())
            .unwrap_or_default();
        let mut form = crate::bots::Form {
            recipe: Some(self.key.clone()),
            name: (!self.name.trim().is_empty() && self.name.trim() != default_name)
                .then(|| self.name.trim().to_string()),
            tools: self.tools(),
            project: self.project,
            shared: self.project.is_some(),
            ..crate::bots::Form::default()
        };
        if self.when.timed() {
            form.app = Some("schedule".into());
            form.at = Some(self.at.clone());
            form.frequency = Some(
                match self.when {
                    When::Weekdays => "weekday",
                    When::Weekly => "week",
                    _ => "day",
                }
                .into(),
            );
            if self.when == When::Weekly {
                form.on_weekday = Some("monday".into());
            }
        } else if self.when == When::Asked {
            form.app = Some("when_asked".into());
        }
        form.body(None)
    }

    /// The tool picked for a slot, and whether it's connected.
    pub fn picked(&self, slot: &Value) -> Option<(String, String, bool)> {
        let kind = slot["kind"].as_str()?;
        let key = self.choices.get(kind)?;
        let tool = slot["tools"]
            .as_array()?
            .iter()
            .find(|t| t["key"].as_str() == Some(key))?;
        Some((
            key.clone(),
            tool["name"].as_str().unwrap_or(key).to_string(),
            tool["connected"].as_bool().unwrap_or(false),
        ))
    }
}

/// New bot: the sentence, the recipes, and a recipe's form once one is
/// picked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewBot {
    pub sentence: String,
    pub recipes: Option<Result<Vec<Recipe>, String>>,
    /// 0 is the sentence; then the recipes.
    pub row: usize,
    pub sending: bool,
    pub error: Option<String>,
    pub form: Option<RecipeForm>,
    /// The project New bot was opened in (with a project selected), by
    /// number for a recipe's form.
    pub project: Option<u64>,
    /// The same project, by id, for a sentence's chat.
    pub project_id: Option<u64>,
}

impl NewBot {
    pub fn recipe_list(&self) -> &[Recipe] {
        match &self.recipes {
            Some(Ok(recipes)) => recipes,
            _ => &[],
        }
    }
}

/// The card at the end of a chat that builds a bot, as the web's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BotDraft {
    pub name: Option<String>,
    pub description: Option<String>,
    pub recipe: Option<DraftRecipe>,
    pub when: Vec<String>,
    pub uses: Vec<String>,
    pub does: Option<String>,
    pub ready: bool,
    pub needs: Vec<String>,
    pub connect: Vec<Named>,
    pub try_refusal: Option<String>,
    pub edit_url: Option<String>,
    pub made: Option<Made>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DraftRecipe {
    pub key: String,
    pub name: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Named {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Made {
    pub id: u64,
    pub name: String,
    pub url: Option<String>,
}

/// The make or try request for a chat's card.
pub fn builder_body(chat: u64) -> Value {
    json!({ "builder": chat })
}

/// A try's state in lines, as `cww bots run` says it.
pub fn trial_lines(answer: &Value) -> Vec<String> {
    let trial = &answer["trial"];
    let mut lines = Vec::new();
    match trial["state"].as_str() {
        Some("running") => lines.push(
            trial["status"]
                .as_str()
                .map(|s| format!("Trying… {s}"))
                .unwrap_or_else(|| "Trying…".into()),
        ),
        Some("failed") => lines.push(format!(
            "The try failed: {}",
            trial["failure"].as_str().unwrap_or("it stopped")
        )),
        _ => lines.push("Tried. A try changes nothing outside.".into()),
    }
    if let Some(answer) = trial["answer"].as_str() {
        lines.push(String::new());
        lines.extend(answer.lines().map(str::to_string));
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
    lines
}

/// What making it said: its name, and what to connect next.
pub fn made_lines(answer: &Value) -> Vec<String> {
    let bot = &answer["bot"];
    let mut lines = vec![format!(
        "Made {}. It's in your sidebar.",
        bot["name"].as_str().unwrap_or("the bot")
    )];
    let connect: Vec<&str> = answer["connect_first"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["name"].as_str())
        .collect();
    if !connect.is_empty() {
        lines.push(format!("Connect {} for it to run.", connect.join(", ")));
    }
    if let Some(next) = answer["next"].as_str() {
        lines.push(next.to_string());
    }
    lines
}

// ------------------------------------------------------------------ drawing

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    _now: OffsetDateTime,
    hits: &mut Hits,
) {
    let Some(new_bot) = &app.chat.new_bot else {
        return;
    };
    let area = pad(area, 2, 1);
    let area = Rect {
        width: area.width.min(COLUMN + 8),
        ..area
    };
    match &new_bot.form {
        Some(form) => render_form(frame, area, app, form, theme, hits),
        None => render_start(frame, area, app, new_bot, theme, hits),
    }
}

fn render_start(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    new_bot: &NewBot,
    theme: &Theme,
    hits: &mut Hits,
) {
    let width = area.width as usize;
    let focused = app.focus == Focus::Main && app.modal.is_none();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{BOT} "), theme.strong()),
            Span::styled("What should your bot do?", theme.strong()),
        ])),
        row(area, 0),
    );
    // The composer, as the web's: its edge dim, the sentence in it.
    let input = Rect {
        y: area.y + 2,
        height: 3,
        ..area
    };
    let edge = if new_bot.row == 0 && focused {
        theme.ink()
    } else {
        theme.line()
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(edge);
    let inner = pad(block.inner(input), 1, 0);
    frame.render_widget(block, input);
    let text = if new_bot.sentence.is_empty() {
        Line::from(vec![
            Span::styled("❯ ", theme.strong()),
            Span::styled(
                "Every morning, sum up yesterday's support tickets…",
                theme.faint(),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("❯ ", theme.strong()),
            Span::styled(
                tail(&new_bot.sentence, inner.width.saturating_sub(3) as usize),
                theme.ink(),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(text), inner);
    hits.add(input, Hit::Row(0));
    if new_bot.row == 0 && focused {
        let x = inner.x
            + 2
            + tail(&new_bot.sentence, inner.width.saturating_sub(3) as usize)
                .chars()
                .count() as u16;
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y));
    }
    let mut lines: Vec<(Line<'static>, Option<Hit>)> = Vec::new();
    let status = if new_bot.sending {
        Some(("Starting its chat…".to_string(), Signal::Live))
    } else {
        new_bot
            .error
            .as_ref()
            .map(|e| (e.clone(), Signal::Negative))
    };
    if let Some((text, signal)) = status {
        for part in wrap(&text, width) {
            lines.push((Line::styled(part, theme.signal_ink(signal)), None));
        }
    } else {
        lines.push((
            Line::styled("Enter starts a chat that proposes the bot.", theme.faint()),
            None,
        ));
    }
    lines.push((Line::raw(""), None));
    lines.push((Line::styled("OR START FROM A RECIPE", theme.micro()), None));
    match &new_bot.recipes {
        None => lines.push((Line::styled("Loading…", theme.faint()), None)),
        Some(Err(error)) => {
            for part in wrap(error, width) {
                lines.push((
                    Line::styled(part, theme.signal_ink(Signal::Attention)),
                    None,
                ));
            }
        }
        Some(Ok(recipes)) => {
            let mut group: Option<&str> = None;
            for (i, recipe) in recipes.iter().enumerate() {
                if recipe.group.as_deref() != group {
                    group = recipe.group.as_deref();
                    if let Some(group) = group {
                        lines.push((Line::raw(""), None));
                        lines.push((Line::styled(group.to_string(), theme.muted()), None));
                    }
                }
                let here = new_bot.row == i + 1;
                let marker = if here && focused {
                    Span::styled("▌", theme.ink())
                } else {
                    Span::raw(" ")
                };
                let starts = recipe.starts.clone().unwrap_or_default();
                let mut line = spread(
                    vec![
                        marker,
                        Span::styled(recipe.name.clone(), theme.strong()),
                        Span::styled(
                            format!(
                                "  {}",
                                ellipsize(
                                    recipe.summary.as_deref().unwrap_or_default(),
                                    width.saturating_sub(
                                        recipe.name.chars().count() + starts.chars().count() + 6
                                    )
                                )
                            ),
                            theme.muted(),
                        ),
                    ],
                    vec![Span::styled(starts, theme.faint())],
                    area.width,
                );
                if here && focused {
                    line = line.patch_style(theme.selected());
                }
                lines.push((line, Some(Hit::Row(i + 1))));
            }
        }
    }
    let body = Rect {
        y: area.y + 6,
        height: area.height.saturating_sub(8),
        ..area
    };
    let selected = lines
        .iter()
        .position(|(_, hit)| *hit == Some(Hit::Row(new_bot.row)))
        .unwrap_or(0);
    let start = (selected + 2).saturating_sub(body.height as usize);
    for (i, (line, hit)) in lines
        .into_iter()
        .skip(start)
        .take(body.height as usize)
        .enumerate()
    {
        let at = row(body, i as u16);
        if let Some(hit) = hit {
            hits.add(at, hit);
        }
        frame.render_widget(Paragraph::new(line), at);
    }
    if focused {
        let keys: &[(&'static str, &'static str)] = if new_bot.row == 0 {
            &[
                ("enter", "start its chat"),
                ("↓", "recipes"),
                ("esc", "back"),
            ]
        } else {
            &[
                ("enter", "open the recipe"),
                ("↑ ↓", "choose"),
                ("esc", "back"),
            ]
        };
        frame.render_widget(
            Paragraph::new(key_hints(keys, theme)),
            row(area, area.height.saturating_sub(1)),
        );
    }
}

fn render_form(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    form: &RecipeForm,
    theme: &Theme,
    hits: &mut Hits,
) {
    let width = area.width as usize;
    let focused = app.focus == Focus::Main && app.modal.is_none();
    let data = form.data.as_ref();
    let recipe_name = data
        .and_then(|d| d["recipe"]["name"].as_str())
        .unwrap_or(&form.key)
        .to_string();
    let mut lines: Vec<(Line<'static>, Option<Hit>)> = vec![(
        Line::from(vec![
            Span::styled(format!("{BOT} "), theme.strong()),
            Span::styled(recipe_name, theme.strong()),
        ]),
        None,
    )];
    if let Some(summary) = data.and_then(|d| d["recipe"]["summary"].as_str()) {
        for part in wrap(summary, width) {
            lines.push((Line::styled(part, theme.muted()), None));
        }
    }
    lines.push((Line::raw(""), None));
    if form.loading && data.is_none() {
        lines.push((Line::styled("Loading…", theme.faint()), None));
    }
    let label_width = 10;
    let rows = form.rows();
    let current = form.row.min(rows.len() - 1);
    let recipe_starts = data.and_then(|d| {
        d["trigger"]["name"]
            .as_str()
            .or(d["recipe"]["starts"].as_str())
    });
    for (i, kind) in rows.iter().enumerate() {
        let here = i == current && focused;
        let marker = if here {
            Span::styled("▌", theme.ink())
        } else {
            Span::raw(" ")
        };
        let label = |text: &str| {
            Span::styled(
                format!(
                    "{:<label_width$}",
                    ellipsize(&text.to_uppercase(), label_width - 1)
                ),
                theme.micro(),
            )
        };
        let choice = |text: String| -> Vec<Span<'static>> {
            if here {
                vec![
                    Span::styled("◂ ", theme.faint()),
                    Span::styled(text, theme.strong()),
                    Span::styled(" ▸", theme.faint()),
                ]
            } else {
                vec![Span::styled(text, theme.ink())]
            }
        };
        let mut spans = vec![marker];
        match kind {
            FormRow::Name => {
                spans.push(label("Name"));
                spans.push(Span::styled(form.name.clone(), theme.ink()));
                if here {
                    spans.push(Span::styled("▏", theme.ink()));
                }
            }
            FormRow::When => {
                spans.push(label("When"));
                spans.extend(choice(form.when.label(&form.at, recipe_starts)));
                if here && form.when.timed() {
                    spans.push(Span::styled("  type a time", theme.faint()));
                }
            }
            FormRow::Slot(kind) => {
                let slot = form
                    .slots()
                    .into_iter()
                    .find(|s| s["kind"] == kind.as_str())
                    .cloned()
                    .unwrap_or_default();
                spans.push(label(slot["label"].as_str().unwrap_or(kind)));
                match form.picked(&slot) {
                    Some((_, name, connected)) => {
                        spans.extend(choice(name));
                        if connected {
                            spans.push(Span::styled(
                                format!(" {DOT}"),
                                theme.signal(Signal::Positive),
                            ));
                        } else {
                            spans.push(Span::styled(
                                "  not connected · c connects",
                                theme.signal_ink(Signal::Attention),
                            ));
                        }
                    }
                    None => spans.extend(choice("None".into())),
                }
            }
            FormRow::Project => {
                spans.push(label("Project"));
                let name = match form.project {
                    None => "Just you".to_string(),
                    Some(n) => form
                        .projects()
                        .into_iter()
                        .find(|(p, _)| *p == n)
                        .map(|(_, name)| format!("Shared in {name}"))
                        .unwrap_or_default(),
                };
                spans.extend(choice(name));
            }
            FormRow::Try | FormRow::Make => {
                if *kind == FormRow::Try {
                    lines.push((Line::raw(""), None));
                }
                let (text, style) = if *kind == FormRow::Make {
                    (" Make bot ", theme.strong())
                } else {
                    (" Try it ", theme.ink())
                };
                spans.push(Span::raw(" ".repeat(label_width)));
                spans.push(Span::styled(
                    format!("[{text}]"),
                    if here {
                        style.add_modifier(Modifier::REVERSED)
                    } else {
                        style
                    },
                ));
                if *kind == FormRow::Try {
                    spans.push(Span::styled("  changes nothing outside", theme.faint()));
                }
            }
        }
        lines.push((Line::from(spans), Some(Hit::Row(i))));
    }
    let needs: Vec<String> = data
        .and_then(|d| d["still_needs"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(str::to_string))
        .collect();
    if !needs.is_empty() {
        lines.push((Line::raw(""), None));
        lines.push((Line::styled("STILL NEEDS", theme.micro()), None));
        for need in needs {
            for part in wrap(&need, width) {
                lines.push((
                    Line::styled(part, theme.signal_ink(Signal::Attention)),
                    None,
                ));
            }
        }
    }
    if let Some(busy) = form.busy {
        lines.push((Line::raw(""), None));
        lines.push((Line::styled(busy, theme.signal_ink(Signal::Live)), None));
    }
    if let Some(error) = &form.error {
        lines.push((Line::raw(""), None));
        for part in wrap(error, width) {
            lines.push((Line::styled(part, theme.signal_ink(Signal::Negative)), None));
        }
    }
    if !form.result.is_empty() {
        lines.push((Line::raw(""), None));
        for line in &form.result {
            for part in wrap(line, width) {
                lines.push((Line::styled(part, theme.ink()), None));
            }
        }
    }
    let height = area.height.saturating_sub(2) as usize;
    let selected = lines
        .iter()
        .position(|(_, hit)| *hit == Some(Hit::Row(current)))
        .unwrap_or(0);
    let start = (selected + 2).saturating_sub(height);
    for (i, (line, hit)) in lines.into_iter().skip(start).take(height).enumerate() {
        let at = row(area, i as u16);
        if let Some(hit) = hit {
            hits.add(at, hit);
        }
        frame.render_widget(Paragraph::new(line), at);
    }
    if focused {
        frame.render_widget(
            Paragraph::new(key_hints(
                &[
                    ("↑ ↓", "choose"),
                    ("← →", "change"),
                    ("enter", "try or make"),
                    ("esc", "back"),
                ],
                theme,
            )),
            row(area, area.height.saturating_sub(1)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> RecipeForm {
        let mut form = RecipeForm::new("inbox_triage");
        form.take(json!({
            "recipe": {"key": "inbox_triage", "name": "Inbox triage", "summary": "Sorts new email.", "starts": "When an email arrives"},
            "defaults": {"name": "Inbox triage"},
            "slots": [{"kind": "email", "label": "Email", "optional": false, "chosen": "gmail",
                       "tools": [{"key": "gmail", "name": "Gmail", "connected": true},
                                 {"key": "outlook", "name": "Outlook", "connected": false}]}],
            "trigger": {"key": "gmail", "name": "When an email arrives"},
            "projects": [{"number": 3, "name": "Falcon"}],
            "still_needs": []
        }));
        form
    }

    #[test]
    fn a_recipe_sends_only_what_the_person_changed() {
        let form = form();
        let body = form.body().unwrap();
        assert_eq!(body["recipe"], "inbox_triage");
        assert!(body["bot"].get("name").is_none(), "the recipe's own name");
        assert_eq!(body["bot"]["choices"]["email"], "gmail");
        assert!(body.get("schedule").is_none(), "the recipe's own trigger");
    }

    #[test]
    fn when_can_be_a_time_or_asking() {
        let mut form = form();
        form.row = 1;
        form.step(true);
        assert_eq!(form.when, When::Daily);
        form.at = "08:30".into();
        let body = form.body().unwrap();
        assert_eq!(body["schedule"]["frequency"], "day");
        assert_eq!(body["schedule"]["at"], "08:30");
        assert_eq!(body["schedule"]["trigger_attributes"]["app"], "schedule");
        form.when = When::Asked;
        let body = form.body().unwrap();
        assert_eq!(body["schedule"]["trigger_attributes"]["app"], "when_asked");
    }

    #[test]
    fn a_slot_and_the_project_change_with_the_arrows() {
        let mut form = form();
        form.row = 2;
        assert!(form.step(true), "a slot asks the server again");
        assert_eq!(
            form.choices.get("email").map(String::as_str),
            Some("outlook")
        );
        form.row = 3;
        assert_eq!(form.current(), FormRow::Project);
        form.step(true);
        assert_eq!(form.project, Some(3));
        let body = form.body().unwrap();
        assert_eq!(body["bot"]["project"], 3);
        assert_eq!(body["bot"]["shared"], true);
    }

    #[test]
    fn a_try_reads_as_cww_bots_says_it() {
        let lines = trial_lines(
            &json!({"trial": {"state": "done", "answer": "3 new emails", "would_ask": ["gmail_label"]}}),
        );
        assert_eq!(lines[0], "Tried. A try changes nothing outside.");
        assert!(lines.contains(&"3 new emails".to_string()));
        assert!(lines.contains(&"It would ask you to approve: gmail_label".to_string()));
        let made = made_lines(
            &json!({"bot": {"name": "Inbox triage"}, "connect_first": [{"key": "gmail", "name": "Gmail"}]}),
        );
        assert_eq!(
            made,
            vec![
                "Made Inbox triage. It's in your sidebar.",
                "Connect Gmail for it to run."
            ]
        );
    }
}
