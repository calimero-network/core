//! The store on the real engine (ported from the v1 `TestHost` tests).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // The test's own target dir (`<target>/tmp`'s parent), so the guest shares its deps.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build kv-store-with-user-and-frozen-storage: {e}"))
            .component
    })
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn app() -> TestApp {
    TestApp::new(
        component(),
        Org::new().members(["bob"]).guests(["gus"]),
        1_700_000_000_000,
    )
}

#[test]
fn regular_set_get_entries_remove_clear() {
    let mut t = app();
    t.as_("bob")
        .call("set", json!({ "key": "k", "value": "v" }))
        .unwrap();
    t.as_("bob")
        .call("set", json!({ "key": "k", "value": "w" }))
        .unwrap();
    t.as_("bob")
        .call("set", json!({ "key": "j", "value": "x" }))
        .unwrap();
    assert_eq!(
        t.as_("bob").view("get", json!({ "key": "k" })).unwrap(),
        json!("w")
    );
    assert_eq!(t.as_("bob").view("len", json!({})).unwrap(), json!("2"));
    assert_eq!(
        t.as_("bob").view("entries", json!({})).unwrap(),
        json!({ "j": "x", "k": "w" })
    );
    let names: Vec<String> = t.events_for("bob").into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["Inserted", "Updated", "Inserted"]);
    assert_eq!(
        code(t.as_("bob").view("getResult", json!({ "key": "nope" }))),
        "NotFound"
    );
    assert!(matches!(
        t.as_("bob").view("getUnchecked", json!({ "key": "nope" })),
        Err(Failure::Trap(_))
    ));
    assert_eq!(
        t.as_("bob").call("remove", json!({ "key": "j" })).unwrap(),
        json!("x")
    );
    assert_eq!(
        t.as_("bob").call("remove", json!({ "key": "j" })).unwrap(),
        json!(null)
    );
    t.as_("bob").call("clear", json!({})).unwrap();
    assert_eq!(t.as_("bob").view("len", json!({})).unwrap(), json!("0"));
    assert_eq!(t.as_("bob").view("entries", json!({})).unwrap(), json!({}));
}

#[test]
fn user_storage_is_per_account() {
    let mut t = app();
    t.as_("owner")
        .call("setUserSimple", json!({ "value": "mine" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("getUserSimple", json!({})).unwrap(),
        json!("mine")
    );
    assert_eq!(
        t.as_("bob").view("getUserSimple", json!({})).unwrap(),
        json!(null)
    );
    assert_eq!(
        t.as_("bob")
            .view("getUserSimpleFor", json!({ "userKey": "owner" }))
            .unwrap(),
        json!("mine")
    );
}

#[test]
fn frozen_storage_is_content_addressed() {
    let mut t = app();
    let hash = t
        .as_("bob")
        .call("addFrozen", json!({ "value": "immutable" }))
        .unwrap();
    assert_eq!(
        t.as_("bob")
            .view("getFrozen", json!({ "hashHex": hash }))
            .unwrap(),
        json!("immutable")
    );
    // Same content hashes to the same key (deduplicated).
    let again = t
        .as_("bob")
        .call("addFrozen", json!({ "value": "immutable" }))
        .unwrap();
    assert_eq!(hash, again);
}

#[test]
fn guests_are_refused() {
    let mut t = app();
    assert_eq!(
        code(t.as_("gus").call("setUserSimple", json!({ "value": "x" }))),
        "Forbidden"
    );
}
