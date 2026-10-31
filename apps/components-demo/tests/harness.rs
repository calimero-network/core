//! The three access-control patterns on the real engine (ported from the v1 `TestHost` tests).

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
            .unwrap_or_else(|e| panic!("build components-demo: {e}"))
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
        Org::new().members(["other", "third"]).guests(["gus"]),
        1_700_000_000_000,
    )
}

#[test]
fn owner_can_set_and_read_config() {
    let mut t = app();
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("")
    );
    t.as_("owner")
        .call("setConfig", json!({ "value": "hello" }))
        .unwrap();
    assert_eq!(
        t.as_("other").view("getConfig", json!({})).unwrap(),
        json!("hello")
    );
}

#[test]
fn non_owner_cannot_set_config() {
    let mut t = app();
    assert_eq!(
        code(
            t.as_("other")
                .call("setConfig", json!({ "value": "forged" }))
        ),
        "NotOwner"
    );
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("")
    );
}

#[test]
fn transfer_moves_control_to_new_owner() {
    let mut t = app();
    t.as_("owner")
        .call("transfer", json!({ "newOwner": "other" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("owner", json!({})).unwrap(),
        json!("other")
    );
    t.as_("other")
        .call("setConfig", json!({ "value": "by-other" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("by-other")
    );
    assert_eq!(
        code(t.as_("owner").call("setConfig", json!({ "value": "nope" }))),
        "NotOwner"
    );
    assert_eq!(
        code(
            t.as_("owner")
                .call("transfer", json!({ "newOwner": "owner" }))
        ),
        "NotOwner"
    );
}

#[test]
fn admin_grants_role_non_admin_cannot() {
    let mut t = app();
    t.as_("owner")
        .call("grantRole", json!({ "role": "editor", "who": "other" }))
        .unwrap();
    assert_eq!(
        t.as_("other")
            .view("hasRole", json!({ "role": "editor", "who": "other" }))
            .unwrap(),
        json!(true)
    );
    // An editor is not an admin.
    assert_eq!(
        code(
            t.as_("other")
                .call("grantRole", json!({ "role": "editor", "who": "third" }))
        ),
        "NotAdmin"
    );
    assert_eq!(
        t.as_("other")
            .call("tryGrantRole", json!({ "role": "editor", "who": "third" }))
            .unwrap(),
        json!(false)
    );
    t.as_("owner")
        .call("revokeRole", json!({ "role": "editor", "who": "other" }))
        .unwrap();
    assert_eq!(
        t.as_("other")
            .view("hasRole", json!({ "role": "editor", "who": "other" }))
            .unwrap(),
        json!(false)
    );
}

#[test]
fn settings_map_roundtrip_and_writer_set() {
    let mut t = app();
    t.as_("owner")
        .call("settingsSet", json!({ "key": "k", "value": "v" }))
        .unwrap();
    assert_eq!(
        t.as_("other")
            .view("settingsGet", json!({ "key": "k" }))
            .unwrap(),
        json!("v")
    );
    assert_eq!(
        t.as_("other")
            .view("settingsGet", json!({ "key": "absent" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(
        code(
            t.as_("other")
                .call("settingsSet", json!({ "key": "k", "value": "x" }))
        ),
        "NotWriter"
    );
    assert_eq!(
        code(t.as_("gus").view("settingsGet", json!({ "key": "k" }))),
        "Forbidden"
    );
}
