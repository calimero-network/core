//! The app's own tests, on the real engine.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

/// How many entries `init` seeds.
const SEEDED: u64 = 3;

fn app() -> TestApp {
    common::app(Org::new())
}

fn len(t: &mut TestApp) -> Value {
    t.as_("owner").view("len", json!({})).unwrap()
}

#[test]
fn init_seeds_default_items() {
    let mut t = app();
    assert_eq!(len(&mut t), json!(SEEDED.to_string()));
    assert_eq!(
        t.as_("owner")
            .view("get", json!({ "key": "welcome" }))
            .unwrap(),
        json!("Welcome to KvStoreInit!")
    );
    // The seed is state, not events.
    assert!(t.events_for("owner").is_empty());
}

#[test]
fn set_get_remove_on_top_of_seed() {
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "k", "value": "v" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        json!("v")
    );
    assert_eq!(len(&mut t), json!((SEEDED + 1).to_string()));
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "k" }))
            .unwrap(),
        json!("v")
    );
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        Value::Null
    );
    assert_eq!(len(&mut t), json!(SEEDED.to_string()));
}

#[test]
fn clear_removes_everything() {
    let mut t = app();
    t.as_("owner").call("clear", json!({})).unwrap();
    assert_eq!(len(&mut t), json!("0"));
}

#[test]
fn get_unchecked_traps_on_a_missing_key() {
    let mut t = app();
    let r = t
        .as_("owner")
        .view("getUnchecked", json!({ "key": "nope" }));
    assert!(matches!(r, Err(Failure::Trap(_))), "{r:?}");
}
