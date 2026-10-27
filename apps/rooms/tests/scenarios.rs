//! Rooms on the real engine, driven by the shared scenario (acceptance criterion A1, harness half).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{run_scenario, Failure, Org, TestApp};
use serde_json::json;

/// A slot boundary (a multiple of 15 minutes), so the scenario's `start` values are valid.
const T0: u64 = 1_700_000_100_000;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rooms-guest");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build rooms: {e}"))
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
        T0,
    )
}

#[test]
fn rooms_passes_the_shared_scenario() {
    let mut t = app();
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/rooms.json");
    if let Err(e) = run_scenario(&mut t, &scenario) {
        panic!("{e:#}");
    }
}

/// The fuel of one `book`, for the Task 13 numbers (`cargo test -- --nocapture`).
#[test]
fn book_fuel_is_logged() {
    let mut t = app();
    t.as_("ada")
        .call(
            "addRoom",
            json!({ "id": "r1", "name": "Atlas", "capacity": 8 }),
        )
        .expect("addRoom");
    t.as_("bob")
        .call(
            "book",
            json!({ "roomId": "r1", "start": T0.to_string(), "slotCount": 2, "title": "standup" }),
        )
        .expect("book");
    let usage = t.last_usage();
    println!("rooms book: fuel = {}", usage.fuel);
    assert!(usage.fuel > 0);
}

fn add_rooms(t: &mut TestApp, n: usize) {
    for r in 0..n {
        t.as_("ada")
            .call(
                "addRoom",
                json!({ "id": format!("r{r}"), "name": format!("Room {r}"), "capacity": 4 }),
            )
            .expect("addRoom");
    }
}

fn slot_start(n: u64) -> String {
    (T0 + n * 900_000).to_string()
}

fn code(r: Result<serde_json::Value, Failure>) -> (String, serde_json::Value) {
    match r {
        Err(Failure::App { code, fields }) => (code, fields),
        other => panic!("expected an app failure, got {other:?}"),
    }
}

#[test]
fn a_taken_slot_leaves_the_earlier_slots_of_the_booking_free() {
    let mut t = app();
    add_rooms(&mut t, 1);
    t.as_("bob")
        .call(
            "book",
            json!({ "roomId": "r0", "start": slot_start(2), "slotCount": 1, "title": "held" }),
        )
        .expect("book third slot");
    let r = t.as_("cy").call(
        "book",
        json!({ "roomId": "r0", "start": slot_start(0), "slotCount": 3, "title": "x" }),
    );
    assert_eq!(
        code(r),
        ("Taken".to_owned(), json!({ "start": slot_start(2) }))
    );
    let held = t
        .as_("cy")
        .view(
            "availability",
            json!({ "roomId": "r0", "dayStart": slot_start(0) }),
        )
        .expect("availability");
    let starts: Vec<_> = held
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["start"].clone())
        .collect();
    assert_eq!(starts, vec![json!(slot_start(2))], "slots 0 and 1 are free");
    assert!(t.events_for("cy").iter().all(|(n, _)| n != "Booked"));
}

#[test]
fn the_timer_releases_at_most_500_per_tick_and_carries_the_rest_over() {
    let mut t = app();
    add_rooms(&mut t, 6);
    // 503 unchecked one-slot bookings at distinct (room, slot) pairs, all due after the advance.
    let mut n = 0;
    'fill: for slot in 0..100 {
        for r in 0..6 {
            if n == 503 {
                break 'fill;
            }
            t.as_("bob")
                .call(
                    "book",
                    json!({ "roomId": format!("r{r}"), "start": slot_start(slot),
                            "slotCount": 1, "title": "x" }),
                )
                .expect("book");
            n += 1;
        }
    }
    t.advance(100 * 900_000 + 900_000);
    let released = |t: &TestApp| {
        t.events_for("bob")
            .iter()
            .filter(|(name, _)| name == "BookingReleased")
            .count()
    };
    t.tick("releaseNoShows").expect("first tick");
    assert_eq!(released(&t), 500);
    let left = t.as_("bob").view("myBookings", json!({})).expect("view");
    assert_eq!(left.as_array().unwrap().len(), 3);
    t.advance(60_000);
    t.tick("releaseNoShows").expect("second tick");
    assert_eq!(released(&t), 503);
    let left = t.as_("bob").view("myBookings", json!({})).expect("view");
    assert!(left.as_array().unwrap().is_empty());
}

#[test]
fn event_payloads_carry_no_title_to_the_org_and_the_reason_to_the_holder() {
    let mut t = app();
    add_rooms(&mut t, 1);
    t.as_("bob")
        .call(
            "book",
            json!({ "roomId": "r0", "start": slot_start(0), "slotCount": 2, "title": "secret" }),
        )
        .expect("book");
    let taken = t
        .events_for("cy")
        .into_iter()
        .find(|(n, _)| n == "SlotsTaken")
        .expect("SlotsTaken")
        .1;
    assert_eq!(
        taken,
        json!({ "roomId": "r0", "start": slot_start(0), "end": slot_start(2) })
    );
    t.advance(16 * 60_000);
    t.tick("releaseNoShows").expect("tick");
    let ev = t.events_for("bob");
    let (name, data) = ev.last().expect("an event");
    assert_eq!(name, "BookingReleased");
    assert_eq!(data["reason"], json!("no-show"));
    assert!(t
        .events_for("cy")
        .iter()
        .all(|(n, _)| n != "BookingReleased"));
}
