//! Synthetic schedules for the interactive demo and native UI checks.
use super::State;
use serde_json::{Value, json};

pub(super) fn respond(state: &mut State, request: &Value) -> Value {
    let action = request["action"].as_str().unwrap_or("list");
    let account = state.notification_preferences["account"].clone();
    if !matches!(action, "list" | "new" | "edit") && request["account"] != account["id"] {
        return json!({"ok":false,"code":"stale_account","error":"Your organization changed."});
    }
    let id = request["id"].as_u64().unwrap_or(0);
    let mut notice = Value::Null;
    match action {
        "create" | "update" => {
            let fields = &request["fields"];
            if fields["name"].as_str().unwrap_or("").is_empty()
                || fields["prompt"].as_str().unwrap_or("").is_empty()
            {
                return json!({"ok":false,"code":"invalid","error":"Give the schedule a name and write what to ask."});
            }
            let row_id = if action == "create" {
                let next = state.next_id;
                state.next_id += 1;
                next
            } else {
                id
            };
            state.schedules.retain(|s| s["id"] != row_id);
            state.schedules.push(json!({"id":row_id,"name":fields["name"],"state":"Active","tone":"positive","attention":false,"timing":"Every weekday at 8:30","location":"Private","time_zone":fields["time_zone"],"next_run":"Fri 16 Oct, 08:30 CEST","last_run":null,"skipped":null,"paused":false,"active":true,"fields":fields}));
            notice = json!("Schedule saved.");
        }
        "delete" => {
            state.schedules.retain(|s| s["id"] != id);
            notice = json!("Schedule deleted. The chats it ran stay.");
        }
        "pause" | "resume" => {
            if let Some(row) = state.schedules.iter_mut().find(|s| s["id"] == id) {
                let paused = action == "pause";
                row["paused"] = json!(paused);
                row["active"] = json!(!paused);
                row["state"] = json!(if paused { "Paused" } else { "Active" });
                row["tone"] = if paused {
                    Value::Null
                } else {
                    json!("positive")
                };
                row["next_run"] = if paused {
                    Value::Null
                } else {
                    json!("Fri 16 Oct, 08:30 CEST")
                };
            }
        }
        "run" => return json!({"ok":true,"account":account,"chat":11}),
        _ => {}
    }
    let form = if matches!(action, "new" | "edit") {
        let fields = if action == "edit" {
            let Some(row) = state.schedules.iter().find(|s| s["id"] == id) else {
                return json!({"ok":false,"code":"not_found","error":"No such schedule"});
            };
            row["fields"].clone()
        } else {
            json!({"name":"","prompt":"","repeats":"repeat","run_at":"","frequency":"weekday","at":"08:00","on_weekday":"monday","on_day":"1","recurrence":"","time_zone":"Europe/Berlin","project_id":null,"model_setting_id":null})
        };
        json!({"id":if action=="edit" {Some(id)} else {None},"fields":fields,"projects":[{"id":71,"name":"Launch plan"}],"models":[{"id":12,"name":"GPT-6 Luna · About 2 credits per answer"},{"id":13,"name":"GPT-6.1 Sol · About 10 credits per answer"}],"time_zones":["Europe/Berlin","Europe/London","UTC"]})
    } else {
        Value::Null
    };
    json!({"ok":true,"account":account,"email":"alex@example.com","active_count":state.schedules.iter().filter(|s|s["active"]==true).count(),"limit":10,"schedules":state.schedules,"form":form,"notice":notice})
}
