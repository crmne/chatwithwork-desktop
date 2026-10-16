//! Personal schedules, using the server's timing rules and rendered labels.
use crate::tui::chat::Named;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    pub account: Named,
    pub email: String,
    pub active_count: usize,
    pub limit: usize,
    pub schedules: Vec<Schedule>,
    pub form: Option<Form>,
    pub notice: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Schedule {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub tone: Option<String>,
    pub attention: bool,
    pub timing: String,
    pub location: String,
    pub time_zone: String,
    pub next_run: Option<String>,
    pub last_run: Option<LastRun>,
    pub skipped: Option<String>,
    pub paused: bool,
    pub active: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LastRun {
    pub chat: u64,
    pub time: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Form {
    pub id: Option<u64>,
    pub fields: Fields,
    pub projects: Vec<Choice>,
    pub models: Vec<Choice>,
    pub time_zones: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Choice {
    pub id: u64,
    pub name: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Fields {
    pub name: String,
    pub prompt: String,
    pub repeats: String,
    pub run_at: String,
    pub frequency: String,
    pub at: String,
    pub on_weekday: String,
    pub on_day: String,
    pub recurrence: String,
    pub time_zone: String,
    pub project_id: Option<u64>,
    pub model_setting_id: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Reply {
    Page(Box<Page>),
    Run { account: Named, chat: u64 },
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    List,
    New,
    Edit {
        id: u64,
    },
    Create {
        account: String,
        fields: Fields,
    },
    Update {
        account: String,
        id: u64,
        fields: Fields,
    },
    Pause {
        account: String,
        id: u64,
    },
    Resume {
        account: String,
        id: u64,
    },
    Delete {
        account: String,
        id: u64,
    },
    Run {
        account: String,
        id: u64,
    },
}
