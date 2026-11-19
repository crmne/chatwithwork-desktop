//! The sidebar as the web draws it (Chat with Work's `docs/ui/chat.md`, "The
//! sidebar"): New bot and New chat, Decisions with its count, the search,
//! Pinned, then the person's projects as a tree under "Projects", each with
//! its newest chats and its bots (one row each, never a bot's own chats),
//! then "Recent" for the chats and bots in no project. The All, Chats and
//! Bots filter picks rows of both lists.
//!
//! The server sends the tree with the chat list (`sidebar`). An older server
//! doesn't, and then the projects are a flat list with their chats.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::app::{ChatPane, SideItem, parse_ts};
use super::chat::ChatSummary;

/// Top-level projects shown before "N more", as on the web.
pub const SHOWN: usize = 6;
/// Chats a project shows before "N more".
pub const ROWS: usize = 5;
/// Chats each press of "N more" adds.
pub const PAGE: usize = 50;

/// What the server says about the sidebar, beside the chats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub decisions: DecisionCount,
    pub can: Can,
    /// The person's projects in their tree's order, depth first.
    pub projects: Vec<TreeProject>,
    /// Every bot they read, most recently active first.
    pub bots: Vec<BotRow>,
    pub waiting: Waiting,
    pub pins: BotPins,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DecisionCount {
    /// What waits for the person now, snoozed ones left out.
    pub count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Can {
    pub new_bot: bool,
    pub new_chat: bool,
    pub new_project: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TreeProject {
    pub id: u64,
    pub number: Option<u64>,
    pub name: String,
    pub icon: Option<String>,
    pub hq: bool,
    /// Only the person is on it: the web draws a lock.
    pub solo: bool,
    pub depth: u16,
    /// The project it's drawn under.
    pub parent: Option<u64>,
    /// Closed, as the person left it on the web.
    pub collapsed: bool,
    /// The newest chats the web shows under it, by number.
    pub chats: Vec<u64>,
    /// How many more of its chats there are.
    pub more: u64,
    /// Its bots, by id.
    pub bots: Vec<u64>,
    /// Decisions waiting for the person in it and inside it.
    pub waiting: u64,
    /// A chat in it answers, or a bot in it works.
    pub working: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BotRow {
    pub id: u64,
    pub name: String,
    /// The project it sits in, in the person's tree; none for Recent.
    pub project: Option<u64>,
    pub personal: bool,
    pub state: String,
    /// "Waiting for you", "Working now", "Paused", ...
    pub state_label: Option<String>,
    pub waiting: u64,
    pub working: bool,
    pub paused: bool,
    /// One of the bot mark's faces, or none for its blinking eyes.
    pub face: Option<String>,
    /// The dot at the row's end.
    pub dot: Option<Dot>,
    /// What it's up to, the web row's tooltip.
    pub line: Option<String>,
    pub active_at: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Dot {
    /// `attention`, `negative`, `live` or `positive`.
    pub tone: Option<String>,
    pub number: Option<serde_json::Value>,
    /// The number as the web shows it: "2", "99+".
    pub number_label: Option<String>,
    /// The dot's words, as screen readers get them on the web.
    pub label: Option<String>,
}

impl Dot {
    /// "2", "99+", or nothing.
    pub fn number(&self) -> Option<String> {
        if let Some(label) = self
            .number_label
            .as_ref()
            .filter(|l| !l.is_empty() && *l != "0")
        {
            return Some(label.clone());
        }
        match self.number.as_ref()? {
            serde_json::Value::Number(n) if n.as_u64().is_some_and(|n| n > 0) => {
                Some(n.to_string())
            }
            serde_json::Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Waiting {
    /// Decisions waiting for the person, by chat number.
    pub chats: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BotPins {
    /// Pinned bots' ids, in pin order.
    pub bots: Vec<u64>,
}

/// Which rows the lists show, the person's own choice, kept between runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Chats,
    Bots,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Chats, Filter::Bots];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Chats => "Chats",
            Filter::Bots => "Bots",
        }
    }

    pub fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Chats,
            Filter::Chats => Filter::Bots,
            Filter::Bots => Filter::All,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Chats => "chats",
            Filter::Bots => "bots",
        }
    }

    fn parse(text: &str) -> Option<Filter> {
        Filter::ALL.into_iter().find(|f| f.key() == text.trim())
    }

    fn chats(self) -> bool {
        self != Filter::Bots
    }

    fn bots(self) -> bool {
        self != Filter::Chats
    }
}

/// Where the filter is kept: beside the questions' history, one line.
pub fn filter_path(history: &Path) -> PathBuf {
    history.with_file_name("sidebar")
}

pub fn load_filter(path: &Path) -> Filter {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Filter::parse(&text))
        .unwrap_or_default()
}

pub fn save_filter(path: &Path, filter: Filter) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{}\n", filter.key()))
}

/// What the person opened and closed here, over what the server says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub filter: Filter,
    /// Projects opened (true) or closed (false) in this run.
    pub open: HashMap<u64, bool>,
    /// Chats shown under a project past the first few, after "N more".
    pub limit: HashMap<u64, usize>,
    /// Every top-level project shown, after "N more" under them.
    pub all_projects: bool,
    /// Where a project's chats read from the server go on from.
    pub cursors: HashMap<u64, String>,
}

/// A line of the sidebar's scrolling list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Label(Label),
    Item { item: SideItem, depth: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Pinned,
    /// "Projects", with the filter at its end.
    Projects,
    /// "Recent": the heading of the lower list, or the only one for
    /// someone on no project, with the filter then.
    Recent,
    /// Nothing the filter or the search leaves.
    Empty,
}

/// A project as drawn: from the server's tree, or, from an older server,
/// one of its projects at the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node<'a> {
    pub id: u64,
    pub name: &'a str,
    pub hq: bool,
    pub solo: bool,
    pub depth: u16,
    pub parent: Option<u64>,
    pub collapsed: bool,
    pub total: Option<u64>,
    pub bots: &'a [u64],
    pub waiting: u64,
    pub working: bool,
}

impl ChatPane {
    pub fn sidebar(&self) -> Option<&Sidebar> {
        self.list.sidebar.as_ref()
    }

    /// The projects as their tree, depth first.
    pub fn nodes(&self) -> Vec<Node<'_>> {
        match self.sidebar() {
            Some(sidebar) => sidebar
                .projects
                .iter()
                .map(|p| Node {
                    id: p.id,
                    name: &p.name,
                    hq: p.hq,
                    solo: p.solo,
                    depth: p.depth,
                    parent: p.parent,
                    collapsed: p.collapsed,
                    total: Some(p.chats.len() as u64 + p.more),
                    bots: &p.bots,
                    waiting: p.waiting,
                    working: p.working,
                })
                .collect(),
            None => self
                .list
                .projects
                .iter()
                .map(|p| Node {
                    id: p.id,
                    name: &p.name,
                    hq: p.hq,
                    solo: false,
                    depth: 0,
                    parent: None,
                    collapsed: true,
                    total: None,
                    bots: &[],
                    waiting: 0,
                    working: false,
                })
                .collect(),
        }
    }

    pub fn node(&self, id: u64) -> Option<Node<'_>> {
        self.nodes().into_iter().find(|n| n.id == id)
    }

    pub fn bot(&self, id: u64) -> Option<&BotRow> {
        self.sidebar()?.bots.iter().find(|b| b.id == id)
    }

    pub fn chat(&self, number: u64) -> Option<&ChatSummary> {
        self.list.chats.iter().find(|c| c.number == number)
    }

    /// Whether a project shows what's inside it.
    pub fn is_open(&self, node: &Node) -> bool {
        self.tree
            .open
            .get(&node.id)
            .copied()
            .unwrap_or(!node.collapsed)
    }

    /// Decisions waiting for the person in a chat.
    pub fn waiting_in(&self, number: u64) -> u64 {
        self.sidebar()
            .and_then(|s| s.waiting.chats.get(&number.to_string()).copied())
            .unwrap_or(0)
    }

    pub fn decisions_count(&self) -> u64 {
        self.decisions
            .as_ref()
            .map(|d| d.count)
            .or(self.sidebar().map(|s| s.decisions.count))
            .unwrap_or(0)
    }

    pub fn can_make_bots(&self) -> bool {
        self.sidebar().is_some_and(|s| s.can.new_bot)
    }

    /// The rows at the top that stay put: New bot (for those who make
    /// bots), New chat, and Decisions (from a server that has them).
    pub fn top_items(&self) -> Vec<SideItem> {
        let mut items = Vec::new();
        if self.can_make_bots() {
            items.push(SideItem::NewBot);
        }
        items.push(SideItem::NewChat);
        if self.sidebar().is_some() {
            items.push(SideItem::Decisions);
        }
        items
    }

    /// The scrolling list under the search.
    pub fn rows(&self) -> Vec<Row> {
        if let Some(query) = self.query() {
            return self.search_rows(&query);
        }
        let filter = self.tree.filter;
        let nodes = self.nodes();
        let in_tree: HashSet<u64> = nodes.iter().map(|n| n.id).collect();
        let mut rows = Vec::new();

        let pinned_chats: Vec<u64> = self
            .list
            .pins
            .chats
            .iter()
            .copied()
            .filter(|n| self.chat(*n).is_some())
            .collect();
        let pinned_bots: Vec<u64> = self
            .sidebar()
            .map(|s| s.pins.bots.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|id| self.bot(*id).is_some())
            .collect();
        if !pinned_chats.is_empty() || !pinned_bots.is_empty() {
            rows.push(Row::Label(Label::Pinned));
            for number in pinned_chats {
                rows.push(item(SideItem::PinnedChat(number), 0));
            }
            for id in pinned_bots {
                rows.push(item(SideItem::PinnedBot(id), 0));
            }
        }

        if !nodes.is_empty() {
            rows.push(Row::Label(Label::Projects));
            let children = |parent: Option<u64>| -> Vec<&Node> {
                nodes
                    .iter()
                    .filter(|n| match parent {
                        Some(p) => n.parent == Some(p),
                        None => n.parent.is_none_or(|p| !in_tree.contains(&p)),
                    })
                    .collect()
            };
            let roots = children(None);
            let shown = if self.tree.all_projects {
                roots.len()
            } else {
                roots.len().min(SHOWN)
            };
            fn walk<'a>(
                pane: &ChatPane,
                node: &Node<'a>,
                children: &dyn Fn(Option<u64>) -> Vec<&'a Node<'a>>,
                rows: &mut Vec<Row>,
            ) {
                rows.push(item(SideItem::Project(node.id), node.depth));
                if !pane.is_open(node) {
                    return;
                }
                for child in children(Some(node.id)) {
                    walk(pane, child, children, rows);
                }
                let depth = node.depth + 1;
                for entry in pane.project_rows(node) {
                    rows.push(item(entry, depth));
                }
            }
            for node in &roots[..shown] {
                walk(self, node, &children, &mut rows);
            }
            if roots.len() > SHOWN {
                rows.push(item(SideItem::MoreProjects, 0));
            }
        }

        let recent = self.recent(&in_tree, filter);
        if !recent.is_empty() || nodes.is_empty() {
            rows.push(Row::Label(Label::Recent));
        }
        if recent.is_empty() && nodes.is_empty() {
            rows.push(Row::Label(Label::Empty));
        }
        rows.extend(recent.into_iter().map(|i| item(i, 0)));
        rows
    }

    /// A project's chats and bots, newest first, the chats cut at its
    /// limit with "N more" and "Show fewer" after them.
    pub fn project_rows(&self, node: &Node) -> Vec<SideItem> {
        let filter = self.tree.filter;
        let mut timed: Vec<(Option<time::OffsetDateTime>, SideItem)> = Vec::new();
        let chats: Vec<&ChatSummary> = self
            .list
            .chats
            .iter()
            .filter(|c| c.project.as_ref().is_some_and(|p| p.id == node.id))
            .collect();
        let limit = self.tree.limit.get(&node.id).copied().unwrap_or(ROWS);
        if filter.chats() {
            for chat in chats.iter().take(limit) {
                timed.push((parse_ts(chat.asked_at()), SideItem::Chat(chat.number)));
            }
        }
        if filter.bots() {
            for id in node.bots {
                if let Some(bot) = self.bot(*id) {
                    let at = bot.active_at.as_deref().and_then(parse_ts);
                    timed.push((at, SideItem::Bot(bot.id)));
                }
            }
        }
        timed.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let mut items: Vec<SideItem> = timed.into_iter().map(|(_, i)| i).collect();
        if filter.chats() {
            let shown = chats.len().min(limit) as u64;
            let total = node.total.unwrap_or(chats.len() as u64);
            if total > shown {
                items.push(SideItem::More(node.id));
            }
            if limit > ROWS && shown > ROWS as u64 {
                items.push(SideItem::Fewer(node.id));
            }
        }
        items
    }

    /// How many of a project's chats "N more" stands for.
    pub fn more_in(&self, id: u64) -> u64 {
        let Some(node) = self.node(id) else { return 0 };
        let chats = self
            .list
            .chats
            .iter()
            .filter(|c| c.project.as_ref().is_some_and(|p| p.id == id))
            .count();
        let limit = self.tree.limit.get(&id).copied().unwrap_or(ROWS);
        let shown = chats.min(limit) as u64;
        node.total.unwrap_or(chats as u64).saturating_sub(shown)
    }

    /// How many top-level projects wait behind "N more".
    pub fn more_projects(&self) -> usize {
        let nodes = self.nodes();
        let ids: HashSet<u64> = nodes.iter().map(|n| n.id).collect();
        let roots = nodes
            .iter()
            .filter(|n| n.parent.is_none_or(|p| !ids.contains(&p)))
            .count();
        roots.saturating_sub(SHOWN)
    }

    /// Chats and bots in no project of the tree, newest first.
    fn recent(&self, in_tree: &HashSet<u64>, filter: Filter) -> Vec<SideItem> {
        let mut timed: Vec<(Option<time::OffsetDateTime>, SideItem)> = Vec::new();
        if filter.chats() {
            for chat in &self.list.chats {
                if chat
                    .project
                    .as_ref()
                    .is_none_or(|p| !in_tree.contains(&p.id))
                {
                    timed.push((parse_ts(chat.asked_at()), SideItem::Chat(chat.number)));
                }
            }
        }
        if filter.bots()
            && let Some(sidebar) = self.sidebar()
        {
            for bot in &sidebar.bots {
                if bot.project.is_none_or(|p| !in_tree.contains(&p)) {
                    let at = bot.active_at.as_deref().and_then(parse_ts);
                    timed.push((at, SideItem::Bot(bot.id)));
                }
            }
        }
        // Stable: chats keep the server's order among themselves.
        timed.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        timed.into_iter().map(|(_, i)| i).collect()
    }

    fn query(&self) -> Option<String> {
        self.search
            .as_deref()
            .map(|q| q.trim().trim_start_matches('#').to_lowercase())
            .filter(|q| !q.is_empty())
    }

    /// While searching: every chat and bot whose name matches, newest
    /// first, wherever it sits.
    fn search_rows(&self, query: &str) -> Vec<Row> {
        let filter = self.tree.filter;
        let mut rows = Vec::new();
        if filter.chats() {
            for chat in &self.list.chats {
                let project = chat.project.as_ref().map(|p| p.name.to_lowercase());
                if chat.title.to_lowercase().contains(query)
                    || chat.number.to_string() == query
                    || project.is_some_and(|p| p.contains(query))
                {
                    rows.push(item(SideItem::Chat(chat.number), 0));
                }
            }
        }
        if filter.bots()
            && let Some(sidebar) = self.sidebar()
        {
            for bot in &sidebar.bots {
                if bot.name.to_lowercase().contains(query) {
                    rows.push(item(SideItem::Bot(bot.id), 0));
                }
            }
        }
        if rows.is_empty() {
            rows.push(Row::Label(Label::Empty));
        }
        rows
    }
}

fn item(item: SideItem, depth: u16) -> Row {
    Row::Item { item, depth }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_is_kept_as_a_word() {
        let dir = tempfile::tempdir().unwrap();
        let path = filter_path(&dir.path().join("history"));
        assert_eq!(load_filter(&path), Filter::All, "nothing kept yet");
        save_filter(&path, Filter::Bots).unwrap();
        assert_eq!(load_filter(&path), Filter::Bots);
        std::fs::write(&path, "nonsense").unwrap();
        assert_eq!(load_filter(&path), Filter::All);
    }

    #[test]
    fn a_dot_shows_its_number_only_above_zero() {
        let dot = |n: serde_json::Value| Dot {
            number: Some(n),
            ..Dot::default()
        };
        assert_eq!(dot(serde_json::json!(2)).number().as_deref(), Some("2"));
        assert_eq!(dot(serde_json::json!(0)).number(), None);
        assert_eq!(
            dot(serde_json::json!("99+")).number().as_deref(),
            Some("99+")
        );
        assert_eq!(Dot::default().number(), None);
    }
}
