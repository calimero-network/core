//! team-space on the real engine, in an org of an owner, an admin (`ada`), two members
//! (`bob` in team `eng`, `cy`) and a guest (`gus`): standing, owned config, writer sets.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn app() -> TestApp {
    common::app(
        Org::new()
            .admins(["ada"])
            .members(["bob", "cy"])
            .guests(["gus"])
            .teams([("eng", vec!["bob"])]),
    )
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn not_writer(field: &str) -> Result<Value, Failure> {
    Err(Failure::App {
        code: "NotWriter".to_owned(),
        fields: json!({ "field": field }),
    })
}

// Organization standing.

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
fn role_is_whoami_without_the_org() {
    let mut t = app();
    assert_eq!(
        t.as_("owner").view("role", json!({})).unwrap(),
        json!("owner")
    );
    assert_eq!(
        t.as_("ada").view("role", json!({})).unwrap(),
        json!("admin")
    );
    assert_eq!(
        t.as_("gus").view("role", json!({})).unwrap(),
        json!("guest")
    );
    assert_eq!(
        t.as_("owner").view("whoami", json!({})).unwrap()["org"],
        "test-org"
    );
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
    assert_eq!(
        code(t.as_("gus").call("announce", json!({ "text": "x" }))),
        "MembersOnly"
    );
}

// The owned config.

#[test]
fn the_owner_sets_the_config_and_anyone_reads_it() {
    let mut t = app();
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("")
    );
    t.as_("owner")
        .call("setConfig", json!({ "value": "hello" }))
        .unwrap();
    assert_eq!(
        t.as_("bob").view("getConfig", json!({})).unwrap(),
        json!("hello")
    );
    // An org admin is not the config's owner.
    assert_eq!(
        code(t.as_("ada").call("setConfig", json!({ "value": "forged" }))),
        "NotOwner"
    );
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("hello")
    );
}

#[test]
fn transfer_moves_control_to_the_new_owner() {
    let mut t = app();
    t.as_("owner")
        .call("transfer", json!({ "newOwner": "bob" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("owner", json!({})).unwrap(),
        json!("bob")
    );
    t.as_("bob")
        .call("setConfig", json!({ "value": "by-bob" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("getConfig", json!({})).unwrap(),
        json!("by-bob")
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

// Writer sets and their rotation.

#[test]
fn set_and_get_shared() {
    let mut t = app();
    assert_eq!(t.as_("bob").view("getShared", json!({})), Ok(json!("")));
    for v in ["hello", "world"] {
        t.as_("owner")
            .call("setShared", json!({ "value": v }))
            .expect("the owner is the first writer");
        assert_eq!(t.as_("bob").view("getShared", json!({})), Ok(json!(v)));
    }
}

#[test]
fn rotate_writers_emits_event_and_moves_the_value_only() {
    let mut t = app();
    t.as_("owner")
        .call("rotateWriters", json!({ "newWriters": ["bob", "bob"] }))
        .expect("rotate");
    assert_eq!(
        t.events_for("bob"),
        [(
            "WritersRotated".to_owned(),
            json!({ "field": "value", "count": 1 })
        )]
    );
    assert_eq!(
        t.as_("owner").view("writerCount", json!({})),
        Ok(json!("1"))
    );
    t.as_("bob")
        .call("setShared", json!({ "value": "b" }))
        .expect("bob writes the value now");
    assert_eq!(
        t.as_("owner").call("setShared", json!({ "value": "o" })),
        not_writer("value")
    );
    // The map's set and the roles' are their own: still the owner.
    assert_eq!(
        t.as_("bob")
            .call("mapSet", json!({ "key": "k", "value": "v" })),
        not_writer("map")
    );
    assert_eq!(
        t.as_("bob")
            .call("grantRole", json!({ "role": "editor", "who": "bob" })),
        not_writer("roles")
    );
    t.as_("owner")
        .call("mapSet", json!({ "key": "k", "value": "v" }))
        .expect("the owner still writes the map");
}

#[test]
fn rotating_the_roles_set_hands_on_who_grants() {
    let mut t = app();
    t.as_("owner")
        .call(
            "rotateWriters",
            json!({ "field": "roles", "newWriters": ["ada", "cy"] }),
        )
        .expect("rotate the roles' writers");
    assert_eq!(
        t.as_("owner")
            .view("writerCount", json!({ "field": "roles" })),
        Ok(json!("2"))
    );
    t.as_("cy")
        .call("grantRole", json!({ "role": "editor", "who": "bob" }))
        .expect("cy grants roles now");
    assert_eq!(
        t.as_("owner")
            .call("grantRole", json!({ "role": "editor", "who": "gus" })),
        not_writer("roles")
    );
    // The value's set did not move.
    t.as_("owner")
        .call("setShared", json!({ "value": "still mine" }))
        .expect("the owner still writes the value");
}

#[test]
fn a_set_that_does_not_exist_admits_nobody() {
    let mut t = app();
    assert_eq!(
        t.as_("owner").call(
            "rotateWriters",
            json!({ "field": "pinned", "newWriters": ["owner"] })
        ),
        not_writer("pinned")
    );
    assert_eq!(
        t.as_("owner")
            .view("writerCount", json!({ "field": "pinned" })),
        Ok(json!("0"))
    );
}

#[test]
fn a_refused_write_changes_nothing() {
    let mut t = app();
    assert_eq!(
        t.as_("bob")
            .call("mapSet", json!({ "key": "evil", "value": "forged" })),
        not_writer("map")
    );
    assert_eq!(
        t.as_("bob")
            .call("tryRotateWriters", json!({ "newWriters": ["bob"] })),
        Ok(json!(false))
    );
    assert_eq!(
        code(
            t.as_("owner")
                .call("rotateWriters", json!({ "newWriters": [] }))
        ),
        "NoWriters"
    );
    assert_eq!(
        t.as_("owner").view("writerCount", json!({})),
        Ok(json!("1"))
    );
    assert_eq!(
        t.as_("owner").view("mapGet", json!({ "key": "evil" })),
        Ok(json!(null))
    );
    assert!(t.events_for("bob").is_empty());
}
