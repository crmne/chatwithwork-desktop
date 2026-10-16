//! Synthetic template settings, including stateful replacements and removals.
use super::State;
use serde_json::{Value, json};

pub(super) fn respond(state: &mut State, cmd: &str, request: &Value) -> Value {
    let action = if cmd == "template_upload" {
        "upload"
    } else {
        request["request"]["action"].as_str().unwrap_or("list")
    };
    let scope = if cmd == "template_upload" {
        &request["target"]
    } else {
        &request["request"]
    };
    let account = state.notification_preferences["account"].clone();
    if action != "list" && scope["account"] != account["id"] {
        return json!({"ok":false,"code":"stale_account","error":"Your organization changed."});
    }
    let project = scope["project"].as_u64();
    let mut notice = Value::Null;
    if action == "upload" {
        let kind = scope["kind"].as_str().unwrap_or("");
        let layer = scope["layer"].as_str().unwrap_or("");
        state.templates.retain(|f| {
            !(f["project"].as_u64() == project && f["kind"] == kind && f["layer"] == layer)
        });
        let id = state.next_id;
        state.next_id += 1;
        state.templates.push(json!({"id":id,"project":project,"layer":layer,"kind":kind,"name":request["filename"],"byte_size":2048,"detail":"2 KB · Alex Morgan, less than a minute ago","download_path":"/rails/active_storage/blobs/proxy/synthetic/template.docx?disposition=attachment"}));
        notice = json!("Template saved.");
    } else if action == "remove" {
        state
            .templates
            .retain(|f| f["id"] != scope["template"] || f["project"].as_u64() != project);
        notice = json!("Template removed.");
    }
    let project_value =
        project.map(|number| json!({"id":700+number,"number":number,"name":"Launch plan"}));
    let groups:Vec<_> = if project.is_some() {vec!["project"]} else {vec!["personal","organization"]}.into_iter().map(|layer| {
        let templates:Vec<_> = [("presentation","Presentation","PowerPoint",vec!["pptx","potx"]),("document","Document","Word",vec!["docx","dotx"]),("spreadsheet","Spreadsheet","Excel",vec!["xlsx","xltx"])].into_iter().map(|(kind,label,app,extensions)| {
            let file=state.templates.iter().find(|f| f["project"].as_u64()==project && f["layer"]==layer && f["kind"]==kind);
            json!({"kind":kind,"label":label,"app":app,"extensions":extensions,"file":file})
        }).collect();
        json!({"layer":layer,"name":match layer {"personal"=>"Your templates".to_owned(),"project"=>"Launch plan's templates".into(),_=>format!("{}'s templates",account["name"].as_str().unwrap())},"editable":true,"templates":templates})
    }).collect();
    json!({"ok":true,"account":account,"email":"alex@example.com","project":project_value,"groups":groups,"notice":notice})
}
