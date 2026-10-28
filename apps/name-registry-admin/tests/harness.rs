//! The registry's rules on the real engine (ported from the v1 `TestHost` tests; the
//! scenario in `scenarios/` covers the claim, release and rotation flow).

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
            .unwrap_or_else(|e| panic!("build name-registry-admin: {e}"))
            .component
    })
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn registry() -> TestApp {
    TestApp::new(
        component(),
        Org::new().members(["ann", "bob"]).guests(["gus"]),
        1_700_000_000_000,
    )
}

fn claim(t: &mut TestApp, who: &str, name: &str) {
    t.as_(who)
        .call("claim", json!({ "name": name, "display": name }))
        .unwrap();
}

fn state(t: &mut TestApp, who: &str, name: &str) -> Value {
    t.as_(who).view("status", json!({ "name": name })).unwrap()["state"].clone()
}

#[test]
fn the_creator_is_the_first_admin() {
    let mut t = registry();
    assert_eq!(
        t.as_("ann").view("admins", json!({})).unwrap(),
        json!(["owner"])
    );
}

#[test]
fn resolving_a_decided_name_changes_nothing() {
    let mut t = registry();
    claim(&mut t, "ann", "ann");
    assert_eq!(
        t.as_("owner")
            .call("resolve", json!({ "name": "ann" }))
            .unwrap(),
        json!("ann")
    );
    let before = t.export_state();
    // An admin's resolve of a decided name writes nothing (the event aside).
    assert_eq!(
        t.as_("owner")
            .call("resolve", json!({ "name": "ann" }))
            .unwrap(),
        json!("ann")
    );
    assert_eq!(t.export_state(), before);
}

#[test]
fn a_pending_claim_can_be_withdrawn() {
    let mut t = registry();
    claim(&mut t, "ann", "rose");
    assert_eq!(
        t.as_("ann")
            .call("letGo", json!({ "name": "rose" }))
            .unwrap(),
        json!("withdrawn")
    );
    assert_eq!(
        t.as_("owner")
            .call("resolve", json!({ "name": "rose" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(state(&mut t, "ann", "rose"), json!("free"));
}

#[test]
fn a_member_cannot_resolve_and_nothing_changes() {
    let mut t = registry();
    claim(&mut t, "ann", "iris");
    let before = t.export_state();
    for who in ["ann", "bob"] {
        assert_eq!(
            code(t.as_(who).call("resolve", json!({ "name": "iris" }))),
            "NotAdmin"
        );
    }
    assert_eq!(t.export_state(), before);
    assert_eq!(state(&mut t, "ann", "iris"), json!("pending"));
}

#[test]
fn a_username_is_checked() {
    let mut t = registry();
    for bad in ["ab", "Ann", "a-b-c", &"x".repeat(33)] {
        let r = t
            .as_("ann")
            .call("claim", json!({ "name": bad, "display": "x" }));
        assert_eq!(code(r), "BadName", "{bad}");
    }
}

#[test]
fn guests_may_not_claim() {
    let mut t = registry();
    let r = t
        .as_("gus")
        .call("claim", json!({ "name": "gus", "display": "x" }));
    assert_eq!(code(r), "Forbidden");
}
