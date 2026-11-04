//! compute-redact on the real engine, with an org of an owner, an admin, two members and a guest.
//! The harness runs no compute, so a job fails at once: these tests cover who may do what.

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

/// `who` registers `file` as theirs and submits it; the job's id.
fn submit_own(t: &mut TestApp, who: &str, file: &str) -> Value {
    t.as_(who)
        .call("register", json!({ "file": file }))
        .unwrap();
    t.as_(who).call("submit", json!({ "file": file })).unwrap()
}

/// Every job `who` may see, page by page through `next`; the harness runs no compute, so a
/// submitted job fails at once.
fn all_jobs(t: &mut TestApp, who: &str) -> Vec<Value> {
    let (mut jobs, mut after) = (Vec::new(), Value::Null);
    loop {
        let page = t.as_(who).view("jobs", json!({ "after": after })).unwrap();
        jobs.extend(page["jobs"].as_array().unwrap().iter().cloned());
        after = page["next"].clone();
        if after.is_null() {
            return jobs;
        }
    }
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
    submit_own(&mut t, "bob", &file);
}

#[test]
fn only_the_uploader_or_an_admin_submits_a_file() {
    let mut t = app();
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    let submit = |t: &mut TestApp, who: &str| t.as_(who).call("submit", json!({ "file": file }));

    assert_eq!(code(submit(&mut t, "bob")), "NotYourFile", "not registered");
    t.as_("bob")
        .call("register", json!({ "file": file }))
        .unwrap();
    assert_eq!(
        code(submit(&mut t, "cy")),
        "NotYourFile",
        "another member's file"
    );
    let r = t.as_("cy").call("register", json!({ "file": file }));
    assert_eq!(code(r), "FileTaken", "the first claim stands");
    let r = t.as_("gus").call("register", json!({ "file": file }));
    assert_eq!(code(r), "MembersOnly");
    let r = t
        .as_("bob")
        .call("register", json!({ "file": "ab".repeat(32) }));
    assert_eq!(code(r), "NoSuchFile");
    t.as_("bob")
        .call("register", json!({ "file": file }))
        .unwrap();
    submit(&mut t, "bob").unwrap();
    submit(&mut t, "ada").unwrap();
}

#[test]
fn a_job_is_seen_by_its_submitter_and_admins_only() {
    let mut t = app();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let id = submit_own(&mut t, "bob", &file);
    let id = id.as_str().unwrap().to_owned();

    let job = t.as_("bob").view("job", json!({ "id": id })).unwrap();
    assert_eq!(job["submitter"], "bob");
    assert_eq!(job["policyHash"].as_str().unwrap().len(), 64);
    assert!(!job.to_string().contains("pepper"));
    t.as_("ada").view("job", json!({ "id": id })).unwrap();
    let r = t.as_("cy").view("job", json!({ "id": id }));
    assert_eq!(code(r), "NotFound");

    assert_eq!(all_jobs(&mut t, "bob").len(), 1);
    assert_eq!(all_jobs(&mut t, "ada").len(), 1);
    assert!(all_jobs(&mut t, "cy").is_empty());
}

#[test]
fn a_job_is_shared_by_its_submitter_or_an_admin_only() {
    let mut t = app();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let id = submit_own(&mut t, "bob", &file);
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
fn a_link_is_for_the_submitter_or_an_admin_and_a_published_part() {
    let mut t = app();
    let file = t.upload_blob(b"name\nJane\n", "text/csv");
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let id = submit_own(&mut t, "bob", &file);
    let args = json!({ "job": id, "part": 0, "ttlSecs": 600 });

    let r = t.as_("cy").call("link", args.clone());
    assert_eq!(code(r), "NotFound");
    for who in ["bob", "ada"] {
        let r = t.as_(who).call("link", args.clone());
        assert_eq!(code(r), "NotPublished", "{who}");
    }
    let r = t
        .as_("bob")
        .call("link", json!({ "job": id, "part": 0, "ttlSecs": 0 }));
    assert_eq!(code(r), "BadTtl");
}

#[test]
fn the_compute_component_imports_only_what_a_job_node_allows() {
    let engine = WasmtimeEngine::for_compute().expect("compute engine");
    engine
        .prepare("compute-redact", compute_component())
        .unwrap_or_else(|e| panic!("a job node refuses the compute component: {e}"));
}

#[test]
fn jobs_past_one_page_are_all_reachable_through_the_cursor() {
    let mut t = app();
    t.as_("ada")
        .call("setPolicy", json!({ "policy": policy() }))
        .unwrap();
    let files: Vec<String> = (0..120)
        .map(|n| t.upload_blob(format!("name\n{n}\n").as_bytes(), "text/csv"))
        .collect();
    for (n, file) in files.iter().enumerate() {
        submit_own(&mut t, ["bob", "cy"][n % 2], file);
    }
    let first = t.as_("ada").view("jobs", json!({})).unwrap();
    assert!(!first["next"].is_null(), "more than one page");

    let ids = |jobs: Vec<Value>| -> Vec<String> {
        jobs.iter()
            .map(|j| j["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let everyone = ids(all_jobs(&mut t, "ada"));
    assert_eq!(everyone.len(), 120);
    assert!(
        everyone.windows(2).all(|w| w[0] < w[1]),
        "oldest first, none twice"
    );
    for who in ["bob", "cy"] {
        let own = all_jobs(&mut t, who);
        assert_eq!(own.len(), 60, "{who}");
        assert!(own.iter().all(|j| j["submitter"] == who));
        assert!(
            own.iter().all(|j| j["preview"].is_null()),
            "a list holds no preview rows"
        );
    }
}
