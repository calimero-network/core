//! Rooms on the real engine, driven by the shared scenario (acceptance criterion A1, harness half).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{run_scenario, Org, TestApp};
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
