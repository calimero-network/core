//! The kv fixture on the real engine: every behaviour the cluster tests lean on.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("kv-guest");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build kv: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(component(), Org::new(), 1_700_000_000_000)
}

#[test]
fn set_get_len_remove() {
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "1" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "a" })).unwrap(),
        json!("1")
    );
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "zz" })).unwrap(),
        json!(null)
    );
    // An overwrite keeps the count.
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "2" }))
        .unwrap();
    t.as_("owner")
        .call("set", json!({ "key": "b", "value": "3" }))
        .unwrap();
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(2));
    assert_eq!(
        t.as_("owner").view("entries", json!({})).unwrap(),
        json!({ "a": "2", "b": "3" })
    );
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "a" }))
            .unwrap(),
        json!("2")
    );
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "a" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(1));
}

#[test]
fn a_missing_key_is_an_app_error() {
    let mut t = app();
    match t.as_("owner").view("getResult", json!({ "key": "nope" })) {
        Err(Failure::App { code, fields }) => {
            assert_eq!(code, "NotFound");
            assert_eq!(fields, json!({ "key": "nope" }));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_emits_one_inserted_event_to_everyone() {
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "1" }))
        .unwrap();
    let events = t.events_for("owner");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "Inserted");
    assert_eq!(events[0].1, json!({ "key": "a", "value": "1" }));
}

#[test]
fn fill_writes_n_keys_of_the_given_size() {
    let mut t = app();
    t.as_("owner")
        .call("fill", json!({ "n": 3, "size": 100 }))
        .unwrap();
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(3));
    let v = t
        .as_("owner")
        .view("get", json!({ "key": "fill-2" }))
        .unwrap();
    assert_eq!(v.as_str().unwrap().len(), 100);
}

#[test]
fn clear_and_wipe_need_a_confirmation() {
    for method in ["clear", "wipe"] {
        let mut t = app();
        t.as_("owner")
            .call("set", json!({ "key": "a", "value": "1" }))
            .unwrap();
        assert!(matches!(
            t.as_("owner").call(method, json!({})),
            Err(Failure::ConfirmRequired)
        ));
        t.as_("owner").confirmed().call(method, json!({})).unwrap();
        assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(0));
        assert_eq!(
            t.as_("owner").view("get", json!({ "key": "a" })).unwrap(),
            json!(null)
        );
    }
}

#[test]
fn stamp_time_now_and_new_id_follow_the_entry() {
    let mut t = app();
    t.set_time(1_700_000_005_000);
    let at = t.as_("owner").call("stamp", json!({})).unwrap();
    assert_eq!(at, json!("1700000005000"));
    t.advance(1_000);
    // A view sees the last logged entry's time, not the clock (D19).
    let seen = t.as_("owner").view("timeNow", json!({})).unwrap();
    assert_eq!(seen, json!("1700000005000"));
    let a = t.as_("owner").call("newId", json!({})).unwrap();
    let b = t.as_("owner").call("newId", json!({})).unwrap();
    assert_ne!(a, b);
    assert!(a.as_str().unwrap().ends_with(".0"), "{a}");
}

#[test]
fn put_writes_without_an_event() {
    let mut t = app();
    t.as_("owner")
        .call("put", json!({ "key": "a", "value": "x".repeat(20_000) }))
        .unwrap();
    assert!(t.events_for("owner").is_empty());
    let v = t.as_("owner").view("get", json!({ "key": "a" })).unwrap();
    assert_eq!(v.as_str().unwrap().len(), 20_000);
}
