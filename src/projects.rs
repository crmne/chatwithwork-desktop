//! Native project pages, using the paired organization's website permissions.
use serde::{Deserialize, Serialize};

use crate::tui::chat::{ChatSummary, Pins};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    List,
    Show {
        project: u64,
        before: Option<String>,
    },
    New,
    Edit {
        project: u64,
    },
    Create {
        fields: Fields,
    },
    Update {
        project: u64,
        fields: Fields,
    },
    Delete {
        project: u64,
    },
    Membership {
        project: u64,
        join: bool,
    },
    Pin {
        project: u64,
        pinned: bool,
    },
    Archive {
        project: u64,
        archived: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fields {
    pub name: String,
    pub description: String,
    pub all_access: bool,
    pub user_ids: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    List(Index),
    Page(Page),
    Form(Form),
    Pins(Pins),
    Removed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Index {
    pub account: String,
    pub admin: bool,
    pub owner: bool,
    pub projects: Vec<Project>,
    pub others: Vec<Project>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub id: u64,
    pub number: u64,
    pub name: String,
    pub description: Option<String>,
    pub icon: String,
    pub hq: bool,
    pub all_access: bool,
    pub archived: bool,
    pub member: bool,
    pub pinned: bool,
    pub audience: String,
    pub summary: String,
    pub chat_count: Option<u64>,
    pub can: Can,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Can {
    pub edit: bool,
    pub delete: bool,
    pub archive: bool,
    pub join: bool,
    pub leave: bool,
    pub pin: bool,
    pub chat: bool,
    pub instructions: bool,
    pub choose_people: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Page {
    pub project: Project,
    pub owner: bool,
    pub chats: Vec<Chat>,
    pub people: Vec<Person>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Chat {
    pub chat: ChatSummary,
    pub creator: String,
    pub age: String,
    pub visible_to_guests: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Form {
    pub project: Option<Project>,
    pub people: Vec<Person>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Person {
    pub id: u64,
    pub name: String,
    pub role: String,
    pub me: bool,
    pub creator: bool,
    pub email: Option<String>,
    pub member: bool,
    pub fixed: bool,
}
