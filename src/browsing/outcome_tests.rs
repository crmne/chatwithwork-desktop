//! How the bundled engine's failures read when the extension says how the
//! operation ended (`engine_cancel`'s `outcome`): certain when nothing was
//! sent, what the page looks like when something was, and the old
//! "uncertain" answer when an older extension says nothing.

use serde_json::json;

use super::engine::outcome_error;
use crate::error::ErrorCode;

#[test]
fn nothing_sent_is_said_plainly_with_the_extensions_reason() {
    let error = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": false, "reason": "the element didn't become ready in time (that element is disabled)" } }),
    )
    .unwrap();
    assert_eq!(error.code, ErrorCode::PageError);
    assert!(
        error.message.starts_with(
            "Nothing was clicked: the element didn't become ready in time (that element is disabled)."
        ),
        "{}",
        error.message
    );
    assert!(error.message.contains("that element is disabled"));
    assert!(error.message.contains("Nothing was sent to the page"));
    assert!(!error.message.contains("uncertain"));
    let typed = outcome_error("type", &json!({ "outcome": { "dispatched": false } })).unwrap();
    assert!(
        typed
            .message
            .starts_with("Nothing was typed. Nothing was sent"),
        "{}",
        typed.message
    );
}

#[test]
fn a_reason_is_trimmed_and_stripped_of_control_characters() {
    let long = format!("covered\n{}", "x".repeat(1_000));
    let error = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": false, "reason": long } }),
    )
    .unwrap();
    assert!(!error.message.contains('\n'));
    assert!(error.message.len() < 600);
}

#[test]
fn something_sent_says_what_the_page_looks_like_afterwards() {
    let same = outcome_error("click", &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
        "responding": true, "navigated": false, "address_changed": false, "element_present": true } } }))
    .unwrap();
    assert!(
        same.message.starts_with("The click was sent"),
        "{}",
        same.message
    );
    assert!(same.message.contains(
        "the page stayed on the same address and document, and the element is still there"
    ));
    let moved = outcome_error("click", &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
        "responding": true, "navigated": true, "address_changed": true, "element_present": false } } }))
    .unwrap();
    assert!(
        moved
            .message
            .contains("the tab went to another page, and the element is gone")
    );
    let dialog = outcome_error("click", &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
        "responding": true, "navigated": false, "address_changed": false, "dialog": true, "element_present": true } } }))
    .unwrap();
    assert!(dialog.message.contains("same address and document, and a dialog opened and was dismissed, and the element is still there"), "{}", dialog.message);
    let unread = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": true, "observed": { "responding": true, "address_changed": false } } }),
    )
    .unwrap();
    assert!(
        unread
            .message
            .contains("the page's address stayed the same."),
        "{}",
        unread.message
    );
    assert!(!unread.message.contains("document"));
    let hung = outcome_error(
        "type",
        &json!({ "outcome": { "dispatched": true, "complete": true, "observed": { "responding": false } } }),
    )
    .unwrap();
    assert!(hung.message.starts_with("The text was sent"));
    assert!(hung.message.contains("stopped responding"));
    let closed = outcome_error("select", &json!({ "outcome": { "dispatched": true, "observed": { "responding": true, "closed": true } } })).unwrap();
    assert!(closed.message.contains("tab was closed"));
}

#[test]
fn an_older_extension_or_an_unreadable_outcome_keeps_the_uncertain_answer() {
    assert!(outcome_error("click", &json!({})).is_none());
    assert!(outcome_error("click", &json!({ "outcome": { "dispatched": "no" } })).is_none());
    // Something was sent but the extension couldn't look: nothing to say.
    assert!(outcome_error("click", &json!({ "outcome": { "dispatched": true } })).is_none());
}

#[test]
fn partial_input_and_a_pointer_move_are_said_as_they_were() {
    let press = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": true, "complete": false, "observed": {
            "responding": true, "navigated": false, "address_changed": false, "element_present": true } } }),
    )
    .unwrap();
    assert!(
        press
            .message
            .starts_with("The mouse press was sent, but not its release"),
        "{}",
        press.message
    );
    let keys = outcome_error(
        "type",
        &json!({ "outcome": { "dispatched": true, "complete": false, "observed": { "responding": true, "closed": true } } }),
    )
    .unwrap();
    assert!(
        keys.message
            .starts_with("Input was sent, but whether all of the text arrived is unconfirmed"),
        "{}",
        keys.message
    );
    let hover = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": false, "moved": true, "reason": "the browser was stopped" } }),
    )
    .unwrap();
    assert!(
        hover
            .message
            .contains("The pointer moved over the element, but nothing was pressed"),
        "{}",
        hover.message
    );
    assert!(!hover.message.contains("Nothing was sent"));
}

#[test]
fn a_replaced_document_is_a_reload_only_when_the_address_is_known_to_be_the_same() {
    let reloaded = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
            "responding": true, "navigated": true, "address_changed": false } } }),
    )
    .unwrap();
    assert!(
        reloaded.message.contains("the page reloaded"),
        "{}",
        reloaded.message
    );
    let replaced = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
            "responding": true, "navigated": true } } }),
    )
    .unwrap();
    assert!(
        replaced
            .message
            .contains("the page's document was replaced"),
        "{}",
        replaced.message
    );
    assert!(!replaced.message.contains("reloaded"));
    let moving = outcome_error(
        "click",
        &json!({ "outcome": { "dispatched": true, "complete": true, "observed": {
            "responding": true, "address_changed": true } } }),
    )
    .unwrap();
    assert!(
        moving.message.contains("the page's address changed."),
        "{}",
        moving.message
    );
}

#[test]
fn an_observation_with_nothing_in_it_keeps_the_uncertain_answer() {
    assert!(
        outcome_error(
            "click",
            &json!({ "outcome": { "dispatched": true, "observed": {} } })
        )
        .is_none()
    );
    assert!(
        outcome_error(
            "click",
            &json!({ "outcome": { "dispatched": true, "observed": { "responding": true } } })
        )
        .is_none()
    );
    assert!(
        outcome_error(
            "click",
            &json!({ "outcome": { "dispatched": true, "observed": null } })
        )
        .is_none()
    );
}
