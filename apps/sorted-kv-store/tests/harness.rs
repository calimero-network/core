//! The app's own tests, on the real engine.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Org, TestApp};
use serde_json::{json, Value};

fn app() -> TestApp {
    common::app(Org::new())
}

fn names(t: &TestApp) -> Vec<String> {
    t.events_for("owner").into_iter().map(|(n, _)| n).collect()
}

fn set(t: &mut TestApp, key: &str, value: &str) {
    t.as_("owner")
        .call("set", json!({ "key": key, "value": value }))
        .expect("set");
}

#[test]
fn set_get_and_ordered_entries() {
    let mut t = app();
    set(&mut t, "b", "2");
    set(&mut t, "a", "1");
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "a" })).unwrap(),
        json!("1")
    );
    let entries = t.as_("owner").view("entries", json!({})).unwrap();
    let keys: Vec<&String> = entries.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(names(&t), ["Inserted", "Inserted"]);
}

#[test]
fn remove_absent_and_clear_empty_emit_nothing() {
    let mut t = app();
    // Removing a key that was never set must not broadcast a change.
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "missing" }))
            .unwrap(),
        Value::Null
    );
    // Clearing an already-empty store likewise emits nothing.
    t.as_("owner").call("clear", json!({})).unwrap();
    assert!(names(&t).is_empty());

    // A real removal emits exactly one `Removed`, a real clear exactly one `Cleared`.
    set(&mut t, "k", "v");
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "k" }))
            .unwrap(),
        json!("v")
    );
    set(&mut t, "k", "v");
    t.as_("owner").call("clear", json!({})).unwrap();
    assert_eq!(names(&t), ["Inserted", "Removed", "Inserted", "Cleared"]);
}

#[test]
fn reads_past_one_page_and_prefix_bounds() {
    let mut t = app();
    // More than one internal page (1000) of keys under a prefix, between two other keys.
    set(&mut t, "a", "x");
    for i in 0..1005 {
        set(&mut t, &format!("p:{i:04}"), "v");
    }
    set(&mut t, "q", "x");
    let prefix = t
        .as_("owner")
        .view("prefix", json!({ "prefix": "p:" }))
        .unwrap();
    assert_eq!(prefix.as_object().unwrap().len(), 1005);
    assert_eq!(
        t.as_("owner").view("len", json!({})).unwrap(),
        json!("1007")
    );
    let page = t
        .as_("owner")
        .view("page", json!({ "offset": 1004, "limit": 3 }))
        .unwrap();
    assert_eq!(page, json!({ "p:1003": "v", "p:1004": "v", "q": "x" }));
    assert_eq!(
        t.as_("owner")
            .view("range", json!({ "start": "q", "end": "a" }))
            .unwrap(),
        json!({})
    );
}
