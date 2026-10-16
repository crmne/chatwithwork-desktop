//! Organization settings and people, always bound to a single device grant.
use crate::tui::chat::Named;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    pub account: Named,
    pub email: String,
    pub admin: bool,
    pub role: String,
    pub organization: Organization,
    pub people: Vec<Person>,
    pub invitations: Option<Vec<Invitation>>,
    pub projects: Option<Vec<Project>>,
    pub roles: Vec<String>,
    pub join_link: String,
    pub new_organization_url: Option<String>,
    pub notice: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Organization {
    pub name: String,
    pub policies: Vec<Policy>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Policy {
    pub key: String,
    pub label: String,
    pub description: String,
    pub value: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Person {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub role: String,
    #[serde(rename = "self")]
    pub is_self: bool,
    pub can_change_role: bool,
    pub can_remove: bool,
    pub remove_label: String,
    pub remove_confirmation: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Invitation {
    pub id: u64,
    pub email: String,
    pub role: String,
    pub inviter: String,
    pub expires_on: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub id: u64,
    pub name: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct OrganizationFields {
    pub name: String,
    #[serde(flatten)]
    pub policies: BTreeMap<String, bool>,
}
impl From<&Organization> for OrganizationFields {
    fn from(organization: &Organization) -> Self {
        Self {
            name: organization.name.clone(),
            policies: organization
                .policies
                .iter()
                .map(|p| (p.key.clone(), p.value))
                .collect(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct InvitationFields {
    pub email: String,
    pub role: String,
    pub project_ids: Vec<u64>,
}
impl Default for InvitationFields {
    fn default() -> Self {
        Self {
            email: String::new(),
            role: "member".into(),
            project_ids: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Reply {
    Page(Box<Page>),
    Left {
        account: Named,
        left: bool,
        notice: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    List,
    Organization {
        account: String,
        fields: OrganizationFields,
    },
    Role {
        account: String,
        id: u64,
        role: String,
    },
    Remove {
        account: String,
        id: u64,
    },
    Invite {
        account: String,
        fields: InvitationFields,
    },
    Withdraw {
        account: String,
        id: u64,
    },
    ResetJoinLink {
        account: String,
    },
}
