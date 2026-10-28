//! The writer sets on the real engine: v1's `TestHost` tests, and the checks that replace
//! `SharedStorage`'s.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn app() -> TestApp {
    common::app(Org::new().members(["bob"]))
}

fn not_writer(field: &str) -> Result<serde_json::Value, Failure> {
    Err(Failure::App {
        code: "NotWriter".to_owned(),
        fields: json!({ "field": field }),
    })
}

#[test]
fn set_and_get_shared() {
    let mut t = app();
    assert_eq!(t.as_("bob").view("getShared", json!({})), Ok(json!("")));
    for v in ["hello", "world"] {
        t.as_("owner")
            .call("setShared", json!({ "value": v }))
            .expect("the owner is the first writer");
        assert_eq!(t.as_("bob").view("getShared", json!({})), Ok(json!(v)));
    }
}

#[test]
fn rotate_writers_emits_event_and_moves_the_value_only() {
    let mut t = app();
    t.as_("owner")
        .call("rotateWriters", json!({ "newWriters": ["bob", "bob"] }))
        .expect("rotate");
    assert_eq!(
        t.events_for("bob"),
        [("WritersRotated".to_owned(), json!({ "count": 1 }))]
    );
    assert_eq!(
        t.as_("owner").view("writerCount", json!({})),
        Ok(json!("1"))
    );
    t.as_("bob")
        .call("setShared", json!({ "value": "b" }))
        .expect("bob writes the value now");
    assert_eq!(
        t.as_("owner").call("setShared", json!({ "value": "o" })),
        not_writer("value")
    );
    // The map's set is its own: still the owner.
    assert_eq!(
        t.as_("bob")
            .call("mapSet", json!({ "key": "k", "value": "v" })),
        not_writer("map")
    );
    t.as_("owner")
        .call("mapSet", json!({ "key": "k", "value": "v" }))
        .expect("the owner still writes the map");
}

#[test]
fn map_set_and_get() {
    let mut t = app();
    t.as_("owner")
        .call("mapSet", json!({ "key": "greeting", "value": "hello" }))
        .expect("mapSet");
    assert_eq!(
        t.as_("bob").view("mapGet", json!({ "key": "greeting" })),
        Ok(json!("hello"))
    );
    assert_eq!(
        t.as_("bob").view("mapGet", json!({ "key": "absent" })),
        Ok(json!(null))
    );
}

#[test]
fn a_refused_write_changes_nothing() {
    let mut t = app();
    assert_eq!(
        t.as_("bob")
            .call("mapSet", json!({ "key": "evil", "value": "forged" })),
        not_writer("map")
    );
    assert_eq!(
        t.as_("bob")
            .call("tryRotateWriters", json!({ "newWriters": ["bob"] })),
        Ok(json!(false))
    );
    assert_eq!(
        t.as_("owner").view("writerCount", json!({})),
        Ok(json!("1"))
    );
    assert_eq!(
        t.as_("owner").view("mapGet", json!({ "key": "evil" })),
        Ok(json!(null))
    );
    assert!(t.events_for("bob").is_empty());
}
