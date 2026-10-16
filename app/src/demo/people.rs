//! Synthetic people and organization forms, with no outgoing invitations.
use super::State;
use serde_json::{Value, json};
pub(super) fn page() -> Value {
    json!({"ok":true,"account":{"id":"482139075","name":"Northwind"},"email":"alex@example.com","admin":true,"role":"owner","organization":{"name":"Northwind","policies":[
        {"key":"guest_document_titles","label":"Guests see document titles","description":"Show guests the titles and links of the documents behind answers. Off, they read the answers only.","value":false},
        {"key":"connector_changes","label":"Connectors can make changes","description":"Let people set a connector to Read and change, and change files in the folders they allow on their computers, so the assistant can post, create, and edit for them after they approve each change. Off, every connector and computer only reads.","value":true},
        {"key":"web_search","label":"Web search","description":"Let people turn on web search in their chats. The queries the assistant writes go to Google for Gemini models and to Bing for GPT models, outside the EU and outside Chat with Work's data processing terms. Each search costs 0.5 credits.","value":true},
        {"key":"code_execution","label":"Code execution","description":"Let the assistant run code in chats on priced models, so it can calculate exactly and make files: slides, documents, spreadsheets, PDFs, and charts. Its code runs in a sandbox on Chat with Work's servers in the EU, with no network, and sees only the chat's files and your templates. Running code costs 0.1 credits per second.","value":true}
    ]},"people":[
        {"id":1,"name":"Alex Rivera","email":"alex@example.com","role":"owner","self":true,"can_change_role":false,"can_remove":false,"remove_label":"Leave Northwind","remove_confirmation":"Leave Northwind? Your private chats here will be deleted in 30 days."},
        {"id":2,"name":"Sam Lee","email":"sam@example.com","role":"member","self":false,"can_change_role":true,"can_remove":true,"remove_label":"Remove Sam Lee","remove_confirmation":"Remove Sam Lee? Their connectors are disconnected now and their private chats are deleted in 30 days."}
    ],"invitations":[],"projects":[{"id":71,"name":"Launch plan"}],"roles":["admin","member","guest"],"join_link":"https://chatwithwork.com/join/synthetic","new_organization_url":"https://chatwithwork.com/accounts/new","notice":null})
}
pub(super) fn respond(state: &mut State, request: &Value) -> Value {
    let action = request["action"].as_str().unwrap_or("list");
    let account = state.notification_preferences["account"].clone();
    if action != "list" && request["account"] != account["id"] {
        return json!({"ok":false,"code":"stale_account","error":"Your organization changed."});
    }
    if state.people.is_null() {
        state.people = page();
    }
    let page = &mut state.people;
    page["account"] = account;
    page["notice"] = Value::Null;
    match action {
        "organization" => {
            page["organization"]["name"] = request["fields"]["name"].clone();
            page["account"]["name"] = request["fields"]["name"].clone();
            for policy in page["organization"]["policies"].as_array_mut().unwrap() {
                if let Some(value) = request["fields"].get(policy["key"].as_str().unwrap()) {
                    policy["value"] = value.clone();
                }
            }
            state.notification_preferences["account"]["name"] = request["fields"]["name"].clone();
            page["notice"] = json!("Organization updated.");
        }
        "role" => {
            for person in page["people"].as_array_mut().unwrap() {
                if person["id"] == request["id"] {
                    person["role"] = request["role"].clone();
                }
            }
            page["notice"] = json!("Role updated.");
        }
        "remove" => {
            if page["people"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == request["id"] && p["self"] == true)
            {
                return json!({"ok":true,"account":page["account"],"left":true,"notice":"You left Northwind."});
            }
            page["people"]
                .as_array_mut()
                .unwrap()
                .retain(|p| p["id"] != request["id"]);
            page["notice"] = json!("Person removed.");
        }
        "invite" => {
            let fields = &request["fields"];
            if !fields["email"].as_str().unwrap_or("").contains('@')
                || (fields["role"] == "guest"
                    && fields["project_ids"]
                        .as_array()
                        .is_none_or(|p| p.is_empty()))
            {
                return json!({"ok":false,"code":"invalid","error":"Enter an email and choose at least one project for a guest."});
            }
            page["invitations"].as_array_mut().unwrap().push(json!({"id":state.next_id,"email":fields["email"],"role":fields["role"],"inviter":"Alex Rivera","expires_on":"October 30, 2026"}));
            state.next_id += 1;
            page["notice"] = json!("Invitation sent.");
        }
        "withdraw" => {
            page["invitations"]
                .as_array_mut()
                .unwrap()
                .retain(|p| p["id"] != request["id"]);
            page["notice"] = json!("Invitation withdrawn.");
        }
        "reset_join_link" => {
            page["join_link"] = json!("https://chatwithwork.com/join/synthetic-reset");
            page["notice"] = json!("Join link reset.");
        }
        _ => {}
    }
    page.clone()
}
