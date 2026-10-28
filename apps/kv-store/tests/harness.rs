//! The store on the real engine: v1's `TestHost` tests, method for method.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn app() -> TestApp {
    common::app(Org::new().members(["bob"]))
}

fn call(t: &mut TestApp, method: &str, args: Value) -> Value {
    t.as_("owner")
        .call(method, args)
        .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

fn view(t: &mut TestApp, method: &str, args: Value) -> Value {
    t.as_("owner")
        .view(method, args)
        .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

fn names(t: &TestApp) -> Vec<String> {
    t.events_for("bob").into_iter().map(|(n, _)| n).collect()
}

#[test]
fn set_get_len_remove() {
    let mut t = app();
    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    assert_eq!(view(&mut t, "get", json!({ "key": "k" })), json!("v"));
    assert_eq!(view(&mut t, "len", json!({})), json!("1"));
    assert_eq!(call(&mut t, "remove", json!({ "key": "k" })), json!("v"));
    assert_eq!(view(&mut t, "get", json!({ "key": "k" })), json!(null));
    assert_eq!(view(&mut t, "len", json!({})), json!("0"));
}

#[test]
fn entries_and_clear() {
    let mut t = app();
    call(&mut t, "set", json!({ "key": "b", "value": "2" }));
    call(&mut t, "set", json!({ "key": "a", "value": "1" }));
    assert_eq!(
        view(&mut t, "entries", json!({})),
        json!({ "a": "1", "b": "2" })
    );
    assert_eq!(
        t.as_("owner").call("clear", json!({})),
        Err(Failure::ConfirmRequired)
    );
    t.as_("owner")
        .confirmed()
        .call("clear", json!({}))
        .expect("clear");
    assert_eq!(view(&mut t, "len", json!({})), json!("0"));
    assert_eq!(view(&mut t, "entries", json!({})), json!({}));
}

#[test]
fn update_if_exists_and_get_or_insert() {
    let mut t = app();
    let args = json!({ "key": "k", "value": "v" });
    assert_eq!(call(&mut t, "updateIfExists", args.clone()), json!(false));
    assert_eq!(view(&mut t, "get", json!({ "key": "k" })), json!(null));
    assert_eq!(
        call(
            &mut t,
            "getOrInsert",
            json!({ "key": "k", "value": "first" })
        ),
        json!("first")
    );
    assert_eq!(
        call(
            &mut t,
            "getOrInsert",
            json!({ "key": "k", "value": "second" })
        ),
        json!("first")
    );
    assert_eq!(call(&mut t, "updateIfExists", args), json!(true));
    assert_eq!(view(&mut t, "get", json!({ "key": "k" })), json!("v"));
    assert_eq!(view(&mut t, "len", json!({})), json!("1"));
    assert_eq!(names(&t), ["Inserted", "Updated"]);
}

#[test]
fn set_emits_inserted_then_updated() {
    let mut t = app();
    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    call(&mut t, "set", json!({ "key": "k", "value": "w" }));
    assert_eq!(
        t.events_for("bob"),
        [
            ("Inserted".to_owned(), json!({ "key": "k", "value": "v" })),
            ("Updated".to_owned(), json!({ "key": "k", "value": "w" })),
        ]
    );
    assert_eq!(view(&mut t, "len", json!({})), json!("1"));
}

#[test]
fn remove_absent_and_clear_empty_emit_nothing() {
    let mut t = app();
    assert_eq!(
        call(&mut t, "remove", json!({ "key": "missing" })),
        json!(null)
    );
    t.as_("owner")
        .confirmed()
        .call("clear", json!({}))
        .expect("clear");
    assert!(names(&t).is_empty());

    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    assert_eq!(call(&mut t, "remove", json!({ "key": "k" })), json!("v"));
    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    t.as_("owner")
        .confirmed()
        .call("clear", json!({}))
        .expect("clear");
    assert_eq!(names(&t), ["Inserted", "Removed", "Inserted", "Cleared"]);
}

#[test]
fn get_result_and_get_unchecked_on_a_missing_key() {
    let mut t = app();
    assert_eq!(
        t.as_("owner").view("getResult", json!({ "key": "nope" })),
        Err(Failure::App {
            code: "NotFound".to_owned(),
            fields: json!({ "key": "nope" })
        })
    );
    assert!(matches!(
        t.as_("owner")
            .view("getUnchecked", json!({ "key": "nope" })),
        Err(Failure::Trap(_))
    ));
    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    assert_eq!(view(&mut t, "getResult", json!({ "key": "k" })), json!("v"));
    assert_eq!(
        view(&mut t, "getUnchecked", json!({ "key": "k" })),
        json!("v")
    );
}

/// v1's `converge.rs`: every write a member makes survives. Here the leader orders them,
/// so three members' writes to distinct keys are three keys.
#[test]
fn distinct_keys_from_several_members_all_survive() {
    let mut t = common::app(Org::new().members(["bob", "cy"]));
    for (who, key) in [("owner", "a"), ("bob", "b"), ("cy", "c")] {
        t.as_(who)
            .call("set", json!({ "key": key, "value": who }))
            .expect("set");
    }
    assert_eq!(view(&mut t, "len", json!({})), json!("3"));
    assert_eq!(
        view(&mut t, "entries", json!({})),
        json!({ "a": "owner", "b": "bob", "c": "cy" })
    );
}
