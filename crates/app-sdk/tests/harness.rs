//! The test harness on the sample guest: the real component on the real engine.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::files::FileId;
use calimero_app_sdk::testing::{run_scenario, Failure, Org, TestApp};
use serde_json::{json, Value};

fn app_code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

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
        t.as_("bob").view("roomCount", json!({ "roomId": "r1" })),
        Ok(json!("1"))
    );
}

#[test]
fn files_made_in_calls_are_readable_after_they_commit() {
    let mut t = app();
    let id = t
        .as_("bob")
        .call("saveFile", json!({ "data": "aGVsbG8" }))
        .unwrap();
    let id: FileId = serde_json::from_value(id).unwrap();
    assert_eq!(t.file(&id), Some(b"hello".to_vec()));
    assert_eq!(
        t.as_("bob").view("fileLen", json!({ "id": id })),
        Ok(json!("5"))
    );

    let seeded = t.add_file(b"seeded bytes");
    assert_eq!(
        t.as_("bob").view("fileLen", json!({ "id": seeded })),
        Ok(json!("12"))
    );

    t.as_("bob").call("dropFile", json!({ "id": id })).unwrap();
    assert_eq!(t.file(&id), None);
    assert_eq!(
        app_code(t.as_("bob").call("dropFile", json!({ "id": id }))),
        "Files"
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
    // A confirm method is never a view, with the node's code; nor is a timer or `init`
    // reached by a client, as a view or a call (`WrongKind`), nor a reserved name.
    let refused = |code: &str| {
        Err(Failure::App {
            code: code.to_owned(),
            fields: json!({}),
        })
    };
    assert_eq!(
        t.as_("ada").confirmed().view("adminRelease", args),
        refused("ConfirmNotAView")
    );
    for method in ["releaseNoShows", "init"] {
        assert_eq!(t.as_("ada").view(method, json!({})), refused("WrongKind"));
        assert_eq!(t.as_("ada").call(method, json!({})), refused("WrongKind"));
    }
    assert_eq!(
        t.as_("ada").call("__upgrade", json!({})),
        refused("UnknownMethod")
    );
    // Call arguments over the node's 512 KiB are refused before running.
    assert_eq!(
        t.as_("ada")
            .call("adminRelease", json!({ "id": "x".repeat(512 * 1024) })),
        Err(Failure::Refused {
            status: 413,
            code: "ARGS_TOO_LARGE".to_owned()
        })
    );
    // Host-side ABI checks: public and allowSystem.
    assert_eq!(
        t.anonymous()
            .view("availability", json!({ "roomId": "r", "start": "1" })),
        Err(unauthenticated()),
        "anonymous reached a non-public view"
    );
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
        { "as": "@anonymous", "op": "view", "method": "roomCount", "args": { "roomId": "x" }, "expect": { "refused": "UNAUTHENTICATED" } },
        { "as": "ada", "confirmed": true, "op": "call", "method": "adminRelease", "args": { "id": "nope" }, "expect": { "error": "NotFound" } },
        { "op": "advance", "args": 5 },
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    run_scenario(&mut t, &path).unwrap();
}

#[test]
fn scenario_variables_carry_minted_ids() {
    let mut t = app();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scenario-vars.json");
    let steps = json!([
        { "as": "bob", "op": "call", "method": "book", "save": "id",
          "args": { "roomId": "r1", "start": "10", "title": "t" } },
        { "as": "bob", "op": "view", "method": "booking", "args": { "id": "$id" },
          "expect": { "ok": { "id": "$id", "roomId": "r1", "start": "10", "title": "t", "checkedIn": false } } },
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    run_scenario(&mut t, &path).unwrap();

    let steps = json!([
        { "as": "bob", "op": "view", "method": "booking", "args": { "id": "$nope" } },
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    let err = run_scenario(&mut t, &path).unwrap_err();
    assert!(format!("{err:#}").contains("unknown variable $nope"));
}

#[test]
fn scenario_dollar_strings_are_variables_only_when_exactly_a_name() {
    let mut t = app();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scenario-dollar.json");
    let steps = json!([
        { "as": "bob", "op": "call", "method": "book", "save": "id",
          "args": { "roomId": "r1", "start": "10", "title": "$5 lunch" } },
        { "as": "bob", "op": "view", "method": "booking", "args": { "id": "$id" },
          "expect": { "ok": { "id": "$id", "roomId": "r1", "start": "10", "title": "$5 lunch", "checkedIn": false } } },
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    run_scenario(&mut t, &path).unwrap();

    let steps =
        json!([{ "as": "bob", "op": "view", "method": "booking", "args": { "id": "$nope" } }]);
    std::fs::write(&path, steps.to_string()).unwrap();
    let err = run_scenario(&mut t, &path).unwrap_err();
    assert!(format!("{err:#}").contains("unknown variable $nope"));
}

#[test]
fn scenario_save_on_a_step_without_a_result_is_an_error() {
    let mut t = app();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scenario-save.json");
    for step in [
        json!({ "op": "tick", "method": "tickInfo", "save": "x" }),
        json!({ "op": "advance", "args": 5, "save": "x" }),
        json!({ "op": "removeMember", "args": "bob", "save": "x" }),
    ] {
        std::fs::write(&path, json!([step]).to_string()).unwrap();
        let err = run_scenario(&mut t, &path).unwrap_err();
        assert!(format!("{err:#}").contains("save"), "{err:#}");
    }
}

fn roll(seed: u64) -> Value {
    let mut t = app();
    t.seed(seed);
    let a = t.as_("bob").call("roll", json!({ "len": "16" })).unwrap();
    let b = t.as_("bob").call("roll", json!({ "len": "16" })).unwrap();
    assert_ne!(a, b, "two runs draw different bytes");
    json!([a, b])
}

#[test]
fn seed_makes_randomness_repeat() {
    assert_eq!(roll(7), roll(7));
    assert_ne!(roll(7), roll(8));
}

#[test]
fn failed_calls_leave_state_unchanged() {
    let mut t = app();
    book(&mut t, "bob", "10").unwrap();
    let before = t.export_state();
    assert_eq!(
        app_code(t.as_("bob").call("writeThenFail", json!({}))),
        "NotFound"
    );
    assert_eq!(t.export_state(), before, "app error");
    let r = t.as_("bob").call("writeThenTrap", json!({}));
    assert!(matches!(r, Err(Failure::Trap(_))), "{r:?}");
    assert_eq!(t.export_state(), before, "trap");
}

#[test]
fn views_see_the_last_entry_time() {
    let mut t = app();
    let now = |t: &mut TestApp| t.as_("bob").view("now", json!({})).unwrap();
    assert_eq!(now(&mut t), json!(T0.to_string()), "init is the last entry");
    t.set_time(T0 + 500);
    assert_eq!(
        now(&mut t),
        json!(T0.to_string()),
        "set_time alone moves nothing"
    );
    book(&mut t, "bob", "10").unwrap();
    assert_eq!(now(&mut t), json!((T0 + 500).to_string()));
    t.advance(1_000);
    assert_eq!(now(&mut t), json!((T0 + 500).to_string()), "no new entry");
}

#[test]
fn unique_ids_differ_between_calls() {
    let mut t = app();
    let a = t.as_("bob").call("newId", json!({})).unwrap();
    let b = t.as_("bob").call("newId", json!({})).unwrap();
    assert_ne!(a, b);
}

#[test]
fn failed_tick_still_advances_previous() {
    let mut t = app();
    t.set_time(T0 + 60_000);
    assert_eq!(t.tick("flaky"), Ok(json!(format!("{}/-", T0 + 60_000))));
    t.as_("ada")
        .call("putTitle", json!({ "key": "fail", "value": "x" }))
        .unwrap();
    t.advance(60_000);
    assert_eq!(app_code(t.tick("flaky")), "Invalid");
    t.as_("ada")
        .call("clearTitle", json!({ "key": "fail" }))
        .unwrap();
    t.advance(60_000);
    assert_eq!(
        t.tick("flaky"),
        Ok(json!(format!("{}/{}", T0 + 180_000, T0 + 120_000)))
    );
}

#[test]
fn org_events_skip_guests() {
    let mut t = TestApp::new(
        component(),
        Org::new().admins(["ada"]).members(["bob"]).guests(["gus"]),
        T0,
    );
    t.as_("ada").call("announce", json!({ "id": "x" })).unwrap();
    assert_eq!(t.events_for("ada").len(), 1);
    assert_eq!(t.events_for("bob").len(), 1);
    assert_eq!(t.events_for("gus").len(), 0);
}

#[test]
fn team_events_reach_team_members() {
    let mut t = TestApp::new(
        component(),
        Org::new()
            .members(["bob", "cy"])
            .teams([("sales", ["bob"])]),
        T0,
    );
    let args = json!({ "team": "sales", "id": "x" });
    t.as_("cy").call("announceTeam", args).unwrap();
    assert_eq!(t.events_for("bob").len(), 1);
    assert_eq!(t.events_for("cy").len(), 0);
    let unknown = json!({ "team": "nope", "id": "x" });
    assert!(t.as_("cy").call("announceTeam", unknown).is_err());
    t.remove_member("bob");
    t.as_("cy")
        .call("announceTeam", json!({ "team": "sales", "id": "y" }))
        .unwrap();
    assert_eq!(t.events_for("bob").len(), 1, "bob left before the second");
}

/// The harness runs the node's `HostCore`, so an event naming more than 1,000 accounts
/// gets the same soft error as on a node (Ruling 43).
#[test]
fn an_event_names_at_most_a_thousand_accounts() {
    let mut t = app();
    let accounts = |n: usize| (0..n).map(|i| format!("acct-{i}")).collect::<Vec<_>>();
    t.as_("bob")
        .call("notify", json!({ "accounts": accounts(1_000), "id": "x" }))
        .unwrap();
    let r = t
        .as_("bob")
        .call("notify", json!({ "accounts": accounts(1_001), "id": "x" }));
    let Err(Failure::App { code, fields }) = r else {
        panic!("{r:?}");
    };
    assert_eq!(code, "Emit");
    assert_eq!(fields["message"], "too many audience accounts");
}

#[test]
fn caller_checks_come_before_confirmation() {
    let mut t = app();
    let r = t.anonymous().call("adminRelease", json!({ "id": "x" }));
    assert_eq!(r, Err(unauthenticated()));
}

fn unauthenticated() -> Failure {
    Failure::Refused {
        status: 401,
        code: "UNAUTHENTICATED".into(),
    }
}

fn not_found() -> Failure {
    Failure::Refused {
        status: 404,
        code: "NOT_FOUND".into(),
    }
}

/// The platform's refusals come before the app runs, with the node's codes (Rulings 32, 35):
/// the harness app is an org app, so every anonymous caller is `401 UNAUTHENTICATED`, a
/// `public` method included (the node's access gate refuses before the ABI is read); an
/// account the app's org does not hold (never in it, or removed) is `404 NOT_FOUND`. A guest
/// still reaches the app, whose own guard decides.
#[test]
fn the_platform_refuses_before_the_app_with_the_nodes_codes() {
    let mut t = TestApp::new(
        component(),
        Org::new().admins(["ada"]).members(["bob"]).guests(["gus"]),
        T0,
    );
    let args = json!({ "roomId": "r1", "start": "1", "title": "t" });
    assert_eq!(
        t.anonymous().call("book", args.clone()),
        Err(unauthenticated())
    );
    assert_eq!(
        t.anonymous().view("roomCount", json!({ "roomId": "r1" })),
        Err(unauthenticated()),
        "a public method of an org app is not open to anonymous callers"
    );
    assert_eq!(
        t.as_("stranger").call("book", args.clone()),
        Err(not_found())
    );
    assert_eq!(
        t.api_key("k1", "stranger").call("book", args.clone()),
        Err(not_found())
    );
    t.remove_member("bob");
    assert_eq!(
        t.as_("bob").view("roomCount", json!({ "roomId": "r1" })),
        Err(not_found())
    );
    // A system run is the platform's own; the ABI decides it.
    assert_eq!(app_code(t.system().call("bump", json!({}))), "Forbidden");
}

#[test]
fn scenario_expectations() {
    let run = |steps: Value| {
        let mut t = app();
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scenario2.json");
        std::fs::write(&path, steps.to_string()).unwrap();
        run_scenario(&mut t, &path)
    };
    run(json!([
        { "as": "ada", "op": "call", "method": "adminRelease", "args": { "id": "x" },
          "expect": { "confirmRequired": true } },
        { "as": "ada", "confirmed": true, "op": "call", "method": "adminRelease",
          "args": { "id": "x" }, "expect": { "error": "NotFound", "fields": {} } },
    ]))
    .unwrap();
    let wrong = json!([{ "as": "ada", "confirmed": true, "op": "call", "method": "adminRelease",
        "args": { "id": "x" }, "expect": { "error": "NotFound", "fields": { "a": 1 } } }]);
    assert!(run(wrong).is_err());
    let refused = json!([{ "as": "@anonymous", "op": "call", "method": "book",
        "args": { "roomId": "r1", "start": "1", "title": "t" },
        "expect": { "refused": "UNAUTHENTICATED" } }]);
    run(refused).unwrap();
    let not_refused = json!([{ "as": "bob", "op": "view", "method": "roomCount",
        "args": { "roomId": "r1" }, "expect": { "refused": "NOT_FOUND" } }]);
    assert!(run(not_refused).is_err());
    // `@name` in args and expect is that account (in the harness, its name: the title is "bob").
    let marker = json!([
        { "as": "bob", "op": "call", "method": "book",
          "args": { "roomId": "r1", "start": "10", "title": "@bob" }, "save": "b" },
        { "as": "bob", "op": "view", "method": "booking", "args": { "id": "$b" },
          "expect": { "ok": { "id": "$b", "roomId": "r1", "start": "10", "title": "@bob",
                              "checkedIn": false } } },
    ]);
    run(marker).unwrap();
    let tick_as = json!([{ "as": "bob", "op": "tick", "method": "tickInfo" }]);
    assert!(format!("{:#}", run(tick_as).unwrap_err()).contains("always runs as the system"));
}
