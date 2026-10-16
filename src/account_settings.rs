//! Personal profile and copying preference in the selected organization.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyFormat {
    #[default]
    RichText,
    Markdown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Profile {
    pub full_name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    pub account: crate::tui::chat::Named,
    pub email: String,
    pub profile: Profile,
    pub copy_answers_as: CopyFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Profile {
        full_name: String,
        display_name: String,
    },
    CopyFormat {
        copy_answers_as: CopyFormat,
    },
}

impl Change {
    pub fn attributes(&self) -> serde_json::Value {
        match self {
            Self::Profile {
                full_name,
                display_name,
            } => serde_json::json!({"full_name":full_name,"display_name":display_name}),
            Self::CopyFormat { copy_answers_as } => {
                serde_json::json!({"copy_answers_as":copy_answers_as})
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    Read,
    Update { account: String, change: Change },
}
