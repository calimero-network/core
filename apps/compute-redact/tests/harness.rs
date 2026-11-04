//! compute-redact on the real engine, with an org of an owner, an admin, two members and a guest.
//! The harness runs no compute, so a started run is refused and the job fails: these tests
//! cover who may do what. The runs themselves are covered by the crate's unit tests.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::{build_component, build_compute_component};
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::Engine;
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // The test's own target dir (`<target>/tmp`'s parent), so the guest shares its deps.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build compute-redact: {e}"))
            .component
    })
}

fn compute_component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_compute_component(dir, &target)
            .unwrap_or_else(|e| panic!("build compute-redact's compute component: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(
        component(),
        Org::new()
            .admins(["ada"])
            .members(["bob", "cy"])
            .guests(["gus"]),
        1_700_000_000_000,
    )
}

fn code<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn policy() -> Value {
    json!({ "salt": "pepper", "columns": { "email": "mask", "name": "pseudonym" } })
}

#[test]
fn only_admins_set_the_policy() {
    let mut t = app();
    for who in ["bob", "gus"] {
        let r = t.as_(who).call("setPolicy", json!({ "policy": policy() }));
        assert_eq!(code(r), "AdminsOnly", "{who}");
    }
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    t.as_("owner")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
}

#[test]
fn a_policy_needs_a_salt() {
    let mut t = app();
    let bad = json!({ "salt": "", "columns": {} });
    let r = t.as_("ada").call("setPolicy", json!({ "policy": bad }));
    assert_eq!(code(r), "BadPolicy");
}

#[test]
fn submit_needs_a_policy_a_member_and_a_file() {
    let mut t = app();
    let file = t.upload_blob(b"name,email\nJane,jane@acme.com\n", "text/csv");
    let r = t.as_("bob").call("submit", json!({ "file": file }));
    assert_eq!(code(r), "NoPolicy");

    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let r = t.as_("gus").call("submit", json!({ "file": file }));
    assert_eq!(code(r), "MembersOnly");
    let r = t
        .as_("bob")
        .call("submit", json!({ "file": "ab".repeat(32) }));
    assert_eq!(code(r), "NoSuchFile");
    t.as_("bob")
        .call("submit", json!({ "file": file }))
        .unwrap();
}

#[test]
fn a_job_is_seen_by_its_submitter_and_admins_only() {
    let mut t = app();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let id = t
        .as_("bob")
        .call("submit", json!({ "file": file }))
        .unwrap();
    let id = id.as_str().unwrap().to_owned();

    let job = t.as_("bob").view("job", json!({ "id": id })).unwrap();
    assert_eq!(job["submitter"], "bob");
    assert_eq!(job["policyHash"].as_str().unwrap().len(), 64);
    assert!(!job.to_string().contains("pepper"));
    t.as_("ada").view("job", json!({ "id": id })).unwrap();
    let r = t.as_("cy").view("job", json!({ "id": id }));
    assert_eq!(code(r), "NotFound");

    assert_eq!(
        t.as_("bob")
            .view("jobs", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        t.as_("ada")
            .view("jobs", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(t
        .as_("cy")
        .view("jobs", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_job_is_shared_by_its_submitter_or_an_admin_only() {
    let mut t = app();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let id = t
        .as_("bob")
        .call("submit", json!({ "file": file }))
        .unwrap();
    let id = id.as_str().unwrap().to_owned();

    for who in ["cy", "gus"] {
        let r = t.as_(who).call("share", json!({ "job": id }));
        assert_eq!(code(r), "NotFound", "{who}");
    }
    // The harness runs no compute, so the job never finishes: the submitter and an admin get as
    // far as the job's state.
    for who in ["bob", "ada"] {
        let r = t.as_(who).call("share", json!({ "job": id }));
        assert_eq!(code(r), "NotDone", "{who}");
    }
}

#[test]
fn the_compute_component_imports_only_what_a_job_node_allows() {
    let engine = WasmtimeEngine::for_compute().expect("compute engine");
    engine
        .prepare("compute-redact", compute_component())
        .unwrap_or_else(|e| panic!("a job node refuses the compute component: {e}"));
}
