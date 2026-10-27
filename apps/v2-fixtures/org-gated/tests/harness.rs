//! The org-gated fixture on the real engine, with an org of an owner, an admin, a team member.

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
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("org-gated-guest");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build org-gated: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(
        component(),
        Org::new()
            .admins(["ada"])
            .members(["bob", "cy"])
            .guests(["gus"])
            .teams([("eng", vec!["bob"])]),
        1_700_000_000_000,
    )
}

fn code<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

#[test]
fn whoami_reports_role_org_and_the_teams_asked_about() {
    let mut t = app();
    let me = t.as_("owner").view("whoami", json!({})).unwrap();
    assert_eq!(me["role"], "owner");
    assert!(me["org"].is_string());
    let bob = t
        .as_("bob")
        .view("whoami", json!({ "candidates": ["eng", "ops"] }))
        .unwrap();
    assert_eq!(bob["role"], "member");
    assert_eq!(bob["teams"], json!(["eng"]));
    let cy = t
        .as_("cy")
        .view("whoami", json!({ "candidates": ["eng"] }))
        .unwrap();
    assert_eq!(cy["teams"], json!([]));
}

#[test]
fn admins_write_the_pinned_note_members_read_it() {
    let mut t = app();
    assert_eq!(t.as_("bob").view("pinned", json!({})).unwrap(), json!(""));
    t.as_("ada")
        .call("setPinned", json!({ "text": "hello" }))
        .unwrap();
    assert_eq!(
        t.as_("bob").view("pinned", json!({})).unwrap(),
        json!("hello")
    );
    assert_eq!(
        code(t.as_("bob").call("setPinned", json!({ "text": "mine" }))),
        "AdminsOnly"
    );
    t.as_("owner")
        .call("setPinned", json!({ "text": "o" }))
        .unwrap();
    assert_eq!(code(t.as_("gus").view("pinned", json!({}))), "MembersOnly");
}

#[test]
fn a_teams_members_write_that_teams_note() {
    let mut t = app();
    t.as_("bob")
        .call("setTeamNote", json!({ "team": "eng", "text": "ship it" }))
        .unwrap();
    assert_eq!(
        t.as_("cy")
            .view("teamNote", json!({ "team": "eng" }))
            .unwrap(),
        json!("ship it")
    );
    assert_eq!(
        t.as_("cy")
            .view("teamNote", json!({ "team": "ops" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(
        code(
            t.as_("cy")
                .call("setTeamNote", json!({ "team": "eng", "text": "x" }))
        ),
        "TeamOnly"
    );
    // Not even the owner, who is not in the team.
    assert_eq!(
        code(
            t.as_("owner")
                .call("setTeamNote", json!({ "team": "eng", "text": "x" }))
        ),
        "TeamOnly"
    );
}

#[test]
fn announcements_reach_the_orgs_members_only() {
    let mut t = app();
    t.as_("bob")
        .call("announce", json!({ "text": "hi" }))
        .unwrap();
    assert_eq!(t.events_for("cy").len(), 1);
    assert_eq!(t.events_for("ada").len(), 1);
    assert!(t.events_for("gus").is_empty());
    assert_eq!(code(t.as_("gus").call("announce", json!({ "text": "x" }))), "MembersOnly");
}
