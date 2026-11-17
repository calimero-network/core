//! The calendar on the real engine: a person's own events, an app's own events, and a call
//! another app makes as the person (`deliver_for`), which may not add or see private ones.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build calendar: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(component(), Org::new().members(["ann"]), 1_700_000_000_000)
}

fn titles(v: serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn each_caller_keeps_its_own_events() {
    let mut t = app();
    t.as_("ann")
        .call(
            "addEvent",
            json!({ "title": "dentist", "at": "2026-11-20T09:00", "private": true }),
        )
        .unwrap();
    t.deliver(
        "planner-app",
        "addEvent",
        json!({ "title": "standup", "at": "2026-11-20T10:00" }),
    )
    .unwrap();
    assert_eq!(
        titles(t.as_("ann").view("myEvents", json!({})).unwrap()),
        ["dentist"]
    );
    assert_eq!(
        titles(t.as_("owner").view("myEvents", json!({})).unwrap()),
        Vec::<String>::new()
    );
}

#[test]
fn an_app_acting_for_a_person_writes_their_calendar_but_never_private_events() {
    let mut t = app();
    t.as_("ann")
        .call(
            "addEvent",
            json!({ "title": "dentist", "at": "2026-11-20T09:00", "private": true }),
        )
        .unwrap();
    // The booking app, with Ann's grant: the event is Ann's own.
    t.deliver_for(
        "booking-app",
        "ann",
        "addEvent",
        json!({ "title": "call with Bo", "at": "2026-11-21T15:00" }),
    )
    .unwrap();
    let refused = t.deliver_for(
        "booking-app",
        "ann",
        "addEvent",
        json!({ "title": "secret", "at": "2026-11-22T08:00", "private": true }),
    );
    assert!(
        matches!(&refused, Err(Failure::App { code, .. }) if code == "PrivateThroughApp"),
        "{refused:?}"
    );
    assert_eq!(
        titles(t.as_("ann").view("myEvents", json!({})).unwrap()),
        ["dentist", "call with Bo"]
    );
    // Through the app, Ann's private event stays hidden.
    let seen = t
        .deliver_for("booking-app", "ann", "myEvents", json!({}))
        .unwrap();
    assert_eq!(titles(seen), ["call with Bo"]);
}

#[test]
fn events_come_back_in_time_order_and_two_at_one_time_both_stay() {
    let mut t = app();
    for (title, at) in [
        ("late", "2026-11-22T18:00"),
        ("early", "2026-11-20T08:00"),
        ("lunch", "2026-11-21T12:00"),
        ("also lunch", "2026-11-21T12:00"),
    ] {
        t.as_("ann")
            .call("addEvent", json!({ "title": title, "at": at }))
            .unwrap();
    }
    let got = titles(t.as_("ann").view("myEvents", json!({})).unwrap());
    assert_eq!(got.len(), 4);
    assert_eq!(got[0], "early");
    assert_eq!(got[3], "late");
}
