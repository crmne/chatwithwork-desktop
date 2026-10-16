//! Data shared by the daemon and native workspace screens. These are the
//! website's own service states, suggestions and instruction layers.

use serde::{Deserialize, Serialize};

use crate::tui::chat::Asset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    Connector,
    Mcp,
    Computer,
}

impl ServiceKind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Connector => "connector",
            Self::Mcp => "mcp",
            Self::Computer => "computer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Personal,
    Organization,
    Project,
}

impl Layer {
    pub fn path(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Organization => "organization",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Service {
    pub key: String,
    pub kind: ServiceKind,
    pub name: String,
    pub icon: Option<Asset>,
    pub live: bool,
    pub paused: bool,
    pub label: String,
    pub tone: Option<String>,
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Suggestion {
    pub label: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstructionSummary {
    pub layer: Layer,
    pub owner: String,
    pub written: bool,
    pub editable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NewChat {
    pub services: Vec<Service>,
    pub suggestions: Vec<Suggestion>,
    pub instructions: Vec<InstructionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Instruction {
    pub layer: Layer,
    pub heading: String,
    pub owner: String,
    pub body: String,
    pub tokens: usize,
    pub token_limit: usize,
    pub editable: bool,
    pub editors_note: Option<String>,
    pub placeholder: String,
    pub template: String,
    pub template_topics: String,
    pub revisions: Vec<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Revision {
    pub id: u64,
    pub body: String,
    pub created_at: String,
    pub author: String,
    pub age: String,
    pub tokens: usize,
    pub changes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SavedInstruction {
    pub instruction: Instruction,
    pub notice: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Catalog {
    pub connectors: Vec<Connector>,
    pub categories: Vec<Category>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Category {
    pub key: String,
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Connector {
    pub key: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub icon: Option<Asset>,
    pub status: String,
    pub paused: bool,
    pub changeable: bool,
    pub changes_allowed: bool,
    pub several_accounts: bool,
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Connection {
    pub id: u64,
    pub label: String,
    pub status: String,
    pub paused: bool,
    pub access: String,
    pub needs_grant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstructionChange {
    Edit { body: String },
    Restore { revision_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstructionPage {
    pub instructions: Vec<Instruction>,
    pub memory: Memory,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Memory {
    pub enabled: bool,
    pub entries: Vec<Remembered>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Remembered {
    pub id: u64,
    pub body: String,
    pub date: String,
    pub chat: Option<u64>,
}
