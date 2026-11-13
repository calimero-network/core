//! The `app` attributes expand to paths through the name the app depends on the SDK by:
//! `calimero-sdk` (the sample guest, `tests/harness.rs`), the old name `calimero-app-sdk`, or
//! a name the app chose. Each guest uses every attribute and derive, including the paths the
//! macros write as strings (`serde(crate = ..)`, `serde(with = ..)`), and runs on the real
//! engine, and the two stand in for two releases of one app in the upgrade tests.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use calimero_app_build::build_in_repo_cache;
use calimero_sdk::testing::{run_migration, run_scenario, Failure, Org, TestApp};
use serde_json::json;

fn build(guest: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/guests")
        .join(guest);
    build_in_repo_cache(&dir)
        .unwrap_or_else(|e| panic!("build {guest}: {e}"))
        .app
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

/// The two guests are one app under two SDK names, so one stands in for an earlier release
/// of the other: an upgrade keeps the state the old release wrote, and the new code reads
/// it and carries on from it.
#[test]
fn an_upgrade_switches_the_code_and_keeps_the_state() {
    let (old, new) = (build("old-name"), build("renamed"));
    let mut t = TestApp::new(&old, Org::new().members(["ada"]), 1_700_000_000_000);
    let item = json!({ "id": "a", "size": "7", "data": "AQI" });
    assert_eq!(t.as_("ada").call("put", item.clone()), Ok(json!("1")));
    let before = t.export_state();

    assert_eq!(
        t.upgrade(b"not a component"),
        Err(Failure::Refused {
            status: 400,
            code: "BAD_BUNDLE".into()
        }),
    );
    t.upgrade(&new).expect("upgrade");
    assert_eq!(t.export_state(), before, "an upgrade runs nothing");
    assert_eq!(t.as_("ada").view("get", json!({ "id": "a" })), Ok(item));
    let next = json!({ "id": "b", "size": "1", "data": "" });
    assert_eq!(t.as_("ada").call("put", next), Ok(json!("2")));
    assert_eq!(t.events_for("ada").len(), 2, "events from both releases");
}

fn scenario(name: &str, steps: &serde_json::Value) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).expect("the scratch dir");
    let path = dir.join(name);
    std::fs::write(&path, steps.to_string()).expect("write scenario");
    path
}

#[test]
fn a_migration_scenario_writes_with_the_old_release_and_reads_with_the_new() {
    let (old, new) = (build("old-name"), build("renamed"));
    let org = || Org::new().members(["ada"]);
    let put = json!({ "as": "ada", "op": "call", "method": "put",
        "args": { "id": "a", "size": "7", "data": "AQI" }, "save": "n" });
    let get = json!({ "as": "ada", "op": "view", "method": "get", "args": { "id": "a" },
        "expect": { "ok": { "id": "a", "size": "7", "data": "AQI" } } });
    let path = scenario(
        "migration.json",
        &json!([put, { "op": "upgrade" }, get, { "as": "ada", "op": "call", "method": "put",
            "args": { "id": "b", "size": "1", "data": "" }, "expect": { "ok": "2" } }]),
    );
    let t = run_migration(&old, &new, org(), 1_700_000_000_000, &path).expect("migration scenario");
    assert_eq!(t.events_for("ada").len(), 2);

    let refused = |name: &str, steps: serde_json::Value, why: &str| {
        let path = scenario(name, &steps);
        let err = run_migration(&old, &new, org(), 0, &path)
            .err()
            .unwrap_or_else(|| panic!("{name} ran"));
        assert!(format!("{err:#}").contains(why), "{name}: {err:#}");
    };
    refused("migration-none.json", json!([put]), "has none");
    refused(
        "migration-twice.json",
        json!([{ "op": "upgrade" }, { "op": "upgrade" }]),
        "upgrades once",
    );
    refused(
        "migration-as.json",
        json!([{ "op": "upgrade", "as": "ada" }]),
        "takes no \"as\"",
    );

    let mut t = TestApp::new(&old, org(), 0);
    let path = scenario("not-a-migration.json", &json!([{ "op": "upgrade" }]));
    let err = run_scenario(&mut t, &path).unwrap_err();
    assert!(format!("{err:#}").contains("migration scenario"), "{err:#}");
}
