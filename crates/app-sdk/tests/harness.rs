//! The test harness on the sample guest: the real component on the real engine.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{run_scenario, Failure, Org, TestApp};
use serde_json::{json, Value};

const T0: u64 = 1_700_000_000_000;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/sample");
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("harness-guests");
        build_component(&dir, &target)
            .unwrap_or_else(|e| panic!("build sample: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(component(), Org::new().admins(["ada"]).members(["bob"]), T0)
}

fn book(t: &mut TestApp, who: &str, start: &str) -> Result<Value, Failure> {
    t.as_(who).call(
        "book",
        json!({ "roomId": "r1", "start": start, "title": "t" }),
    )
}

#[test]
fn harness_runs_the_real_component() {
    let mut t = app();
    let id = book(&mut t, "bob", "10").expect("book");
    assert!(id.is_string());
    let free = t
        .as_("bob")
        .view("availability", json!({ "roomId": "r1", "start": "10" }));
    assert_eq!(free, Ok(json!(false)));
    let err = book(&mut t, "bob", "10").unwrap_err();
    assert_eq!(
        err,
        Failure::App {
            code: "Taken".into(),
            fields: json!({ "start": "10" })
        }
    );
    assert!(t.last_usage().fuel > 0);
    assert_eq!(
        t.anonymous().view("roomCount", json!({ "roomId": "r1" })),
        Ok(json!("1"))
    );
}

#[test]
fn confirm_needs_confirmed() {
    let mut t = app();
    let id = book(&mut t, "bob", "10").unwrap();
    let args = json!({ "id": id });
    assert_eq!(
        t.as_("ada").call("adminRelease", args.clone()),
        Err(Failure::ConfirmRequired)
    );
    assert_eq!(
        t.as_("ada").confirmed().call("adminRelease", args.clone()),
        Ok(Value::Null)
    );
    // A confirm method is never a view.
    let Err(Failure::App { code, .. }) = t.as_("ada").confirmed().view("adminRelease", args) else {
        panic!("confirm method ran as a view");
    };
    assert_eq!(code, "NotAView");
    // Host-side ABI checks: public and allowSystem.
    let Err(Failure::App { code, .. }) = t
        .anonymous()
        .view("availability", json!({ "roomId": "r", "start": "1" }))
    else {
        panic!("anonymous reached a non-public view");
    };
    assert_eq!(code, "LoginRequired");
    let Err(Failure::App { code, .. }) = t.system().call("bump", json!({})) else {
        panic!("system reached a method without allow(system)");
    };
    assert_eq!(code, "Forbidden");
}

#[test]
fn tick_carries_previous_slot() {
    let mut t = app();
    t.set_time(T0 + 60_000);
    assert_eq!(t.tick("tickInfo"), Ok(json!(format!("{}/-", T0 + 60_000))));
    t.advance(60_000);
    assert_eq!(
        t.tick("tickInfo"),
        Ok(json!(format!("{}/{}", T0 + 120_000, T0 + 60_000)))
    );
}

#[test]
fn events_resolve_audience_at_delivery() {
    let mut t = app();
    t.as_("ada")
        .call("announce", json!({ "id": "one" }))
        .unwrap();
    assert_eq!(t.events_for("bob").len(), 1);
    assert_eq!(t.events_for("cy").len(), 0, "cy was never in the org");
    t.remove_member("bob");
    t.as_("ada")
        .call("announce", json!({ "id": "two" }))
        .unwrap();
    let bob: Vec<_> = t
        .events_for("bob")
        .into_iter()
        .map(|(_, d)| d["id"].clone())
        .collect();
    assert_eq!(bob, [json!("one")], "bob sees the first only");
    assert_eq!(t.events_for("ada").len(), 2);
}

#[test]
fn export_state_is_stable_across_runs() {
    let run = || {
        let mut t = app();
        t.seed(42);
        book(&mut t, "bob", "10").unwrap();
        book(&mut t, "ada", "20").unwrap();
        t.export_state()
    };
    let a = run();
    assert!(!a.is_empty());
    assert_eq!(a, run());
}

#[test]
fn scenario_file_runs() {
    let mut t = app();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scenario.json");
    let steps = json!([
        { "as": "bob", "op": "call", "method": "bump", "args": {}, "expect": { "ok": "1" } },
        { "as": "@anonymous", "op": "view", "method": "roomCount", "args": { "roomId": "x" }, "expect": { "ok": "0" } },
        { "as": "ada", "confirmed": true, "op": "call", "method": "adminRelease", "args": { "id": "nope" }, "expect": { "error": "NotFound" } },
        { "op": "advance", "args": 5 },
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    run_scenario(&mut t, &path).unwrap();
}
