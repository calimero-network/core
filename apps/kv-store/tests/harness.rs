//! The store on the real engine: what the scenario files do not check (event payloads,
//! misses, ordering, paging).

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_sdk::testing::{Failure, Org, TestApp};
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
fn entries_and_clear() {
    let mut t = app();
    call(&mut t, "set", json!({ "key": "b", "value": "2" }));
    call(&mut t, "set", json!({ "key": "a", "value": "1" }));
    assert_eq!(
        view(&mut t, "entries", json!({})),
        json!({ "a": "1", "b": "2" })
    );
    // Any member may clear, with no passkey confirmation.
    t.as_("owner").call("clear", json!({})).expect("clear");
    assert_eq!(view(&mut t, "len", json!({})), json!("0"));
    assert_eq!(view(&mut t, "entries", json!({})), json!({}));
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
    t.as_("owner").call("clear", json!({})).expect("clear");
    assert!(names(&t).is_empty());

    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    assert_eq!(call(&mut t, "remove", json!({ "key": "k" })), json!("v"));
    call(&mut t, "set", json!({ "key": "k", "value": "v" }));
    t.as_("owner").call("clear", json!({})).expect("clear");
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

#[test]
fn ordered_reads_first_last_and_is_empty() {
    let mut t = app();
    assert_eq!(view(&mut t, "isEmpty", json!({})), json!(true));
    assert_eq!(view(&mut t, "first", json!({})), Value::Null);
    let _ = call(&mut t, "set", json!({ "key": "b", "value": "2" }));
    let _ = call(&mut t, "set", json!({ "key": "a", "value": "1" }));
    let _ = call(&mut t, "set", json!({ "key": "c", "value": "3" }));
    let entries = view(&mut t, "entries", json!({}));
    let keys: Vec<&String> = entries.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["a", "b", "c"]);
    assert_eq!(view(&mut t, "isEmpty", json!({})), json!(false));
    assert_eq!(
        view(&mut t, "first", json!({})),
        json!({ "key": "a", "value": "1" })
    );
    assert_eq!(
        view(&mut t, "last", json!({})),
        json!({ "key": "c", "value": "3" })
    );
    assert_eq!(
        view(&mut t, "range", json!({ "start": "a", "end": "c" })),
        json!({ "a": "1", "b": "2" })
    );
}

#[test]
fn reads_past_one_page_and_prefix_bounds() {
    let mut t = app();
    // More than one internal page (1000) of keys under a prefix, between two other keys.
    let _ = call(&mut t, "set", json!({ "key": "a", "value": "x" }));
    for i in 0..1005 {
        let _ = call(
            &mut t,
            "set",
            json!({ "key": format!("p:{i:04}"), "value": "v" }),
        );
    }
    let _ = call(&mut t, "set", json!({ "key": "q", "value": "x" }));
    let prefix = view(&mut t, "prefix", json!({ "prefix": "p:" }));
    assert_eq!(prefix.as_object().unwrap().len(), 1005);
    assert_eq!(view(&mut t, "len", json!({})), json!("1007"));
    assert_eq!(
        view(&mut t, "page", json!({ "offset": 1004, "limit": 3 })),
        json!({ "p:1003": "v", "p:1004": "v", "q": "x" })
    );
    assert_eq!(
        view(&mut t, "page", json!({ "offset": 0, "limit": 0 })),
        json!({})
    );
    assert_eq!(
        view(&mut t, "range", json!({ "start": "q", "end": "a" })),
        json!({})
    );
}
