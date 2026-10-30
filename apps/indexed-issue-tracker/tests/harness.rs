//! The tracker's lookups on the real engine (ported from the v1 `TestHost` tests).

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
            .unwrap_or_else(|e| panic!("build indexed-issue-tracker: {e}"))
            .component
    })
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn tracker() -> TestApp {
    let mut t = TestApp::new(
        component(),
        Org::new().members(["bob"]).guests(["gus"]),
        1_700_000_000_000,
    );
    for (id, labels, at) in [
        ("t1", vec!["bug", "ui"], 10),
        ("t2", vec!["bug"], 20),
        ("t3", vec!["feature"], 30),
        ("t4", vec![], 40),
    ] {
        t.as_("bob")
            .call(
                "open",
                json!({ "id": id, "title": format!("issue {id}"), "labels": labels,
                        "createdAt": at.to_string() }),
            )
            .expect("open");
    }
    t
}

fn ids(v: Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

fn list(t: &mut TestApp, status: &str, offset: u32, limit: u32) -> Vec<String> {
    ids(t
        .as_("bob")
        .view(
            "listByStatus",
            json!({ "status": status, "offset": offset, "limit": limit }),
        )
        .unwrap())
}

#[test]
fn a_status_column_pages_newest_first() {
    let mut t = tracker();
    assert_eq!(list(&mut t, "open", 0, 2), ["t4", "t3"]);
    assert_eq!(list(&mut t, "open", 2, 2), ["t2", "t1"]);
    assert!(list(&mut t, "open", 4, 2).is_empty());
}

#[test]
fn moving_an_issue_moves_it_between_columns_and_counts() {
    let mut t = tracker();
    t.as_("bob")
        .call("setStatus", json!({ "id": "t3", "status": "closed" }))
        .unwrap();
    assert_eq!(list(&mut t, "open", 0, 10), ["t4", "t2", "t1"]);
    assert_eq!(list(&mut t, "closed", 0, 10), ["t3"]);
    assert_eq!(
        t.as_("bob").view("statusCounts", json!({})).unwrap(),
        json!([
            { "status": "open", "count": "3" },
            { "status": "in_progress", "count": "0" },
            { "status": "closed", "count": "1" }
        ])
    );
    assert_eq!(
        t.as_("bob").view("issueCount", json!({})).unwrap(),
        json!("4")
    );
}

#[test]
fn labels_assignees_and_time_ranges_are_seeks() {
    let mut t = tracker();
    for (id, who) in [
        ("t2", json!("alice")),
        ("t4", json!("alice")),
        ("t4", json!(null)),
    ] {
        t.as_("bob")
            .call("assign", json!({ "id": id, "assignee": who }))
            .unwrap();
    }
    let by = |t: &mut TestApp, m: &str, args: Value| ids(t.as_("bob").view(m, args).unwrap());
    assert_eq!(
        by(&mut t, "listByLabel", json!({ "label": "bug" })),
        ["t1", "t2"]
    );
    assert_eq!(
        by(&mut t, "listByAssignee", json!({ "assignee": "alice" })),
        ["t2"]
    );
    assert_eq!(
        by(
            &mut t,
            "listCreatedBetween",
            json!({ "status": "open", "from": "20", "to": "40" })
        ),
        ["t2", "t3"]
    );
    assert_eq!(
        t.as_("bob").view("get", json!({ "id": "t4" })).unwrap()["assignee"],
        json!(null)
    );
}

#[test]
fn misuse_is_refused() {
    let mut t = tracker();
    let r = t.as_("bob").call(
        "open",
        json!({ "id": "t1", "title": "again", "labels": [], "createdAt": "1" }),
    );
    assert_eq!(code(r), "Exists");
    let r = t
        .as_("bob")
        .call("setStatus", json!({ "id": "t1", "status": "someday" }));
    assert_eq!(code(r), "UnknownStatus");
    let r = t
        .as_("bob")
        .call("setStatus", json!({ "id": "nope", "status": "closed" }));
    assert_eq!(code(r), "NotFound");
    let r = t.as_("gus").view("issueCount", json!({}));
    assert_eq!(code(r), "Forbidden");
}
