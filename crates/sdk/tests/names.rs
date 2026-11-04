//! The `app` attributes expand to paths through the name the app depends on the SDK by:
//! `calimero-sdk` (the sample guest, `tests/harness.rs`), the old name `calimero-app-sdk`, or
//! a name the app chose. Each guest uses every attribute and derive, including the paths the
//! macros write as strings (`serde(crate = ..)`, `serde(with = ..)`), and runs on the real
//! engine.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use calimero_app_build::build_component;
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn build(guest: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/guests")
        .join(guest);
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("name-guests");
    build_component(&dir, &target)
        .unwrap_or_else(|e| panic!("build {guest}: {e}"))
        .component
}

fn round_trip(guest: &str) {
    let mut t = TestApp::new(
        &build(guest),
        Org::new().members(["ada"]),
        1_700_000_000_000,
    );
    let ada = || json!({ "id": "a", "size": "7", "data": "AQI" });
    assert_eq!(t.as_("ada").call("put", ada()), Ok(json!("1")), "{guest}");
    assert_eq!(
        t.as_("ada").view("get", json!({ "id": "a" })),
        Ok(json!({ "id": "a", "size": "7", "data": "AQI" })),
        "{guest}",
    );
    assert_eq!(
        t.as_("ada").view("get", json!({ "id": "b" })),
        Err(Failure::App {
            code: "Missing".into(),
            fields: json!({ "id": "b" }),
        }),
        "{guest}",
    );
}

#[test]
fn an_app_on_the_old_name_builds_and_runs() {
    round_trip("old-name");
}

#[test]
fn an_app_that_renamed_the_sdk_builds_and_runs() {
    round_trip("renamed");
}
