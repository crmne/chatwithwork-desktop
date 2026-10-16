use super::{State, chats};
use serde_json::{Value, json};

pub fn catalog() -> Vec<Value> {
    let mut projects = chats::projects();
    projects.push(chats::project(
        9,
        "Platform partners",
        "users-three",
        false,
        true,
    ));
    for project in &mut projects {
        project["member"] = json!(project["id"] != 9);
        project["archived"] = json!(false);
        project["description"] = json!("A shared place for the team's work.");
        project["members"] = json!([1, 2]);
    }
    projects
}

fn people(project: Option<&Value>) -> Vec<Value> {
    [(1, "Alex", "alex@example.com", "Owner"), (2, "Sam", "sam@example.com", "Member")].into_iter().map(|(id, name, email, role)| {
        let member = project.map_or(id == 1, |p| p["members"].as_array().is_some_and(|ids| ids.contains(&json!(id))));
        json!({"id": id, "name": name, "email": email, "role": role, "me": id == 1, "creator": id == 1, "fixed": id == 1, "member": member})
    }).collect()
}

fn summary(state: &State, project: &Value) -> Value {
    let mut p = project.clone();
    let hq = p["hq"] == true;
    let member = p["member"] == true;
    let archived = p["archived"] == true;
    p["number"] = p["id"].clone();
    p["pinned"] = json!(
        state.pins["projects"]
            .as_array()
            .is_some_and(|ids| ids.contains(&p["id"]))
    );
    p["audience"] = json!(if hq {
        "Everyone in Northwind"
    } else {
        "2 people"
    });
    p["summary"] = json!(if hq {
        "Everyone in Northwind"
    } else if p["all_access"] == true {
        "All-Access · 2 people"
    } else {
        "Invite-only · 2 people"
    });
    p["chat_count"] = if member {
        json!(
            state
                .chats
                .iter()
                .filter(|c| c.summary["project"]["id"] == p["id"])
                .count()
        )
    } else {
        Value::Null
    };
    p["can"] = json!({"edit": true, "delete": !hq, "archive": !hq, "join": !member, "leave": member && !hq, "pin": member, "chat": member && !archived, "instructions": member, "choose_people": !hq});
    p
}

fn page(state: &State, project: &Value) -> Value {
    let items = if project["member"] == true {
        state.chats.iter().filter(|c| c.summary["project"]["id"] == project["id"]).map(|c| json!({"chat": c.summary, "creator": "Alex", "age": "about 1 hour ago", "visible_to_guests": false})).collect::<Vec<_>>()
    } else {
        vec![]
    };
    json!({"ok": true, "owner": true, "project": summary(state, project), "chats": items, "people": people(Some(project)), "next_cursor": null})
}

pub(super) fn request(state: &mut State, request: &Value) -> Value {
    let action = request["action"].as_str().unwrap_or_default();
    if action == "list" {
        return json!({"ok": true, "account": "Northwind", "admin": true, "owner": true,
            "projects": state.project_catalog.iter().filter(|p| p["member"] == true).map(|p| summary(state, p)).collect::<Vec<_>>(),
            "others": state.project_catalog.iter().filter(|p| p["member"] == false && p["archived"] != true).map(|p| summary(state, p)).collect::<Vec<_>>()});
    }
    if action == "new" {
        return json!({"ok": true, "project": null, "people": people(None)});
    }
    if action == "create" {
        let id = state
            .project_catalog
            .iter()
            .filter_map(|p| p["id"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let fields = &request["fields"];
        if fields["name"].as_str().is_none_or(|s| s.trim().is_empty()) {
            return json!({"ok": false, "code": "invalid", "error": "Name can't be blank"});
        }
        let mut project = chats::project(
            id,
            fields["name"].as_str().unwrap(),
            "folder-simple",
            false,
            fields["all_access"] == true,
        );
        project["description"] = fields["description"].clone();
        project["members"] = fields["user_ids"].clone();
        project["member"] = json!(true);
        project["archived"] = json!(false);
        state.project_catalog.push(project.clone());
        state.projects.push(project.clone());
        return page(state, &project);
    }
    let Some(index) = state
        .project_catalog
        .iter()
        .position(|p| p["number"] == request["project"])
    else {
        return json!({"ok": false, "code": "not_found", "error": "No such project"});
    };
    if action == "pin" {
        let id = state.project_catalog[index]["id"].clone();
        let pins = state.pins["projects"].as_array_mut().unwrap();
        pins.retain(|p| p != &id);
        if request["pinned"] == true {
            pins.push(id);
        }
        return json!({"ok": true, "pins": state.pins});
    }
    if action == "edit" {
        return json!({"ok": true, "project": summary(state, &state.project_catalog[index]), "people": people(Some(&state.project_catalog[index]))});
    }
    if action == "delete" {
        let id = state.project_catalog.remove(index)["id"].clone();
        for chat in &mut state.chats {
            if chat.summary["project"]["id"] == id {
                chat.summary["project"] = Value::Null;
            }
        }
        state.projects.retain(|p| p["id"] != id);
        return json!({"ok": true});
    }
    if action == "update" {
        for field in ["name", "description", "all_access"] {
            state.project_catalog[index][field] = request["fields"][field].clone();
        }
        state.project_catalog[index]["members"] = request["fields"]["user_ids"].clone();
    } else if action == "membership" {
        state.project_catalog[index]["member"] = request["join"].clone();
    } else if action == "archive" {
        state.project_catalog[index]["archived"] = request["archived"].clone();
    }
    state.projects = state
        .project_catalog
        .iter()
        .filter(|p| p["member"] == true && p["archived"] == false)
        .cloned()
        .collect();
    if action == "membership" && request["join"] == false {
        return json!({"ok": true});
    }
    page(state, &state.project_catalog[index])
}

pub(super) fn instruction(state: &mut State, id: u64) -> Option<&mut Value> {
    let project = state
        .project_catalog
        .iter_mut()
        .find(|p| p["id"] == id && p["member"] == true)?;
    if !project["instruction"].is_object() {
        let mut instruction = chats::instructions().pop()?;
        instruction["layer"] = json!("project");
        instruction["heading"] = project["name"].clone();
        instruction["owner"] = project["name"].clone();
        instruction["template"] = json!(
            "## This project\n- Goals and important context\n\n## Working together\n- Who does what"
        );
        project["instruction"] = instruction;
    }
    Some(&mut project["instruction"])
}
