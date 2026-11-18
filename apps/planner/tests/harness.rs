//! The planner on the real engine, with the calendar its instance was created with played by
//! `mock_app`: it schedules on that calendar as itself, so the agenda is the planner's own.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use calimero_app_build::build_component;
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build planner: {e}"))
            .component
    })
}

#[test]
fn the_planner_schedules_on_the_calendar_it_was_created_with() {
    let mut t = TestApp::new(
        component(),
        Org::new().members(["ann", "bo"]),
        1_700_000_000_000,
    );
    let no_calendar = t
        .as_("ann")
        .call("schedule", json!({ "title": "retro", "at": "Fri 16:00" }));
    assert!(
        matches!(&no_calendar, Err(Failure::App { code, .. }) if code == "NotLinked"),
        "{no_calendar:?}"
    );

    // What the node does when the planner is created from its release: it creates a calendar
    // and links it under `calendar`.
    t.link("calendar", Some("planner.calendar"));
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let kept = events.clone();
    t.mock_app("planner.calendar", move |c| {
        // `on_behalf_of` is the member whose call this is, but the planner's reference lists no
        // `methods`, so no one can grant it: on a node the calendar runs it as the planner.
        match c.method.as_str() {
            "addEvent" => {
                kept.lock().unwrap().push(json!({
                    "title": c.args["title"], "at": c.args["at"], "private": false,
                }));
                Ok(Value::Null)
            }
            "myEvents" => Ok(Value::Array(kept.lock().unwrap().clone())),
            m => Err(json!(format!("no method {m}"))),
        }
    });
    t.as_("ann")
        .call("schedule", json!({ "title": "retro", "at": "Fri 16:00" }))
        .unwrap();
    let agenda = t.as_("bo").view("agenda", json!({})).unwrap();
    assert_eq!(agenda[0]["title"], "retro", "{agenda}");
    assert_eq!(events.lock().unwrap().len(), 1);
}
