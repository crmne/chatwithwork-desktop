//! Office template settings, scoped to the paired organization and project.
use crate::tui::chat::Named;
use crate::workspace::Layer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    List {
        project: Option<u64>,
    },
    Remove {
        account: String,
        project: Option<u64>,
        template: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub account: String,
    pub project: Option<u64>,
    pub layer: Layer,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    pub account: Named,
    pub email: String,
    pub project: Option<Project>,
    pub groups: Vec<Group>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub id: u64,
    pub number: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Group {
    pub layer: Layer,
    pub name: String,
    pub editable: bool,
    pub templates: Vec<Template>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Template {
    pub kind: String,
    pub label: String,
    pub app: String,
    pub extensions: Vec<String>,
    pub file: Option<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct File {
    pub id: u64,
    pub name: String,
    pub byte_size: u64,
    pub detail: String,
    pub download_path: String,
}
