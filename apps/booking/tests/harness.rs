//! booking on the real engine, with the attached calendar played by `mock_app`: the harness
//! hands the mock the person each call would run as (`on_behalf_of`), which is what the node
//! uses, with the person's grant, to run it on the calendar as them.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use calimero_app_build::build_component;
use calimero_sdk::env::Caller;
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build booking: {e}"))
            .component
    })
}

fn code<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

/// A calendar that keeps each caller's times, as `apps/calendar` does.
fn attach_calendar(t: &mut TestApp) -> Arc<Mutex<Vec<(String, String)>>> {
    let kept: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = kept.clone();
    t.link("calendar", Some("cal-app"));
    t.mock_app("cal-app", move |c| {
        let who = match &c.on_behalf_of {
            Some(Caller::Account(a)) => format!("account:{a}"),
            _ => "app:booking".to_owned(),
        };
        let mut kept = seen.lock().unwrap();
        match c.method.as_str() {
            // As `apps/calendar` answers: a private event through an app is refused with its
            // own error, which the node passes back as `XcallFailed { code, fields }`.
            "addEvent" if c.args["private"] == json!(true) && who != "app:booking" => {
                Err(json!({ "code": "PrivateThroughApp", "fields": {} }))
            }
            "addEvent" if c.args["title"] == "who am I" => {
                Err(json!({ "code": "UnknownCaller", "fields": {} }))
            }
            "addEvent" => {
                kept.push((who, c.args["at"].as_str().unwrap().to_owned()));
                Ok(Value::Null)
            }
            "myEvents" => Ok(Value::Array(
                kept.iter()
                    .filter(|(w, _)| *w == who)
                    .map(|(_, at)| json!({ "title": "x", "at": at, "private": false }))
                    .collect(),
            )),
            m => Err(json!(format!("no method {m}"))),
        }
    });
    kept
}

#[test]
fn without_a_calendar_nothing_is_booked() {
    let mut t = TestApp::new(component(), Org::new().members(["ann"]), 1_700_000_000_000);
    t.as_("owner")
        .call("setSlots", json!({ "slots": ["10:00"] }))
        .unwrap();
    assert_eq!(
        code(
            t.as_("ann")
                .call("book", json!({ "at": "10:00", "title": "call" }))
        ),
        "NoCalendar"
    );
}

#[test]
fn a_booking_lands_in_the_persons_own_calendar() {
    let mut t = TestApp::new(
        component(),
        Org::new().members(["ann", "bo"]),
        1_700_000_000_000,
    );
    let kept = attach_calendar(&mut t);
    t.as_("owner")
        .call("setSlots", json!({ "slots": ["10:00", "11:00"] }))
        .unwrap();
    assert_eq!(
        code(t.as_("ann").call("setSlots", json!({ "slots": [] }))),
        "OwnerOnly"
    );

    t.as_("ann")
        .call(
            "book",
            json!({ "at": "10:00", "title": "call with the owner" }),
        )
        .unwrap();
    // Ann's call offers Ann to the calendar: the event is hers, so she is busy at 10:00 and
    // Bo is not.
    assert_eq!(
        kept.lock().unwrap().clone(),
        [("account:ann".to_owned(), "10:00".to_owned())]
    );
    assert_eq!(
        t.as_("ann").view("freeSlots", json!({})).unwrap(),
        json!(["11:00"])
    );
    assert_eq!(
        t.as_("bo").view("freeSlots", json!({})).unwrap(),
        json!(["10:00", "11:00"])
    );
    assert_eq!(
        code(
            t.as_("ann")
                .call("book", json!({ "at": "10:00", "title": "again" }))
        ),
        "SlotTaken"
    );
    assert_eq!(
        code(
            t.as_("ann")
                .call("book", json!({ "at": "12:00", "title": "not offered" }))
        ),
        "SlotTaken"
    );
}

#[test]
fn the_calendars_own_errors_come_back_typed() {
    let mut t = TestApp::new(component(), Org::new().members(["ann"]), 1_700_000_000_000);
    let kept = attach_calendar(&mut t);
    t.as_("owner")
        .call("setSlots", json!({ "slots": ["10:00"] }))
        .unwrap();
    // The calendar refuses a private event through an app; booking matches that error
    // (`uses::calendar::Error::PrivateThroughApp`) and answers with its own.
    assert_eq!(
        code(t.as_("ann").call(
            "book",
            json!({ "at": "10:00", "title": "secret", "private": true })
        )),
        "PrivateNotAllowed"
    );
    // One booking does not match passes on as the calendar answered it.
    let passed = t
        .as_("ann")
        .call("book", json!({ "at": "10:00", "title": "who am I" }));
    assert!(
        matches!(&passed, Err(Failure::App { code, fields })
            if code == "XcallFailed" && fields["code"] == "UnknownCaller"),
        "{passed:?}"
    );
    assert!(kept.lock().unwrap().is_empty());
}
