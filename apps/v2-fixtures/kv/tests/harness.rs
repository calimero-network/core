//! The kv fixture on the real engine: every behaviour the cluster tests lean on.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("kv-guest");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build kv: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(component(), Org::new(), 1_700_000_000_000)
}

#[test]
fn set_get_len_remove() {
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "1" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "a" })).unwrap(),
        json!("1")
    );
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "zz" })).unwrap(),
        json!(null)
    );
    // An overwrite keeps the count.
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "2" }))
        .unwrap();
    t.as_("owner")
        .call("set", json!({ "key": "b", "value": "3" }))
        .unwrap();
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(2));
    assert_eq!(
        t.as_("owner").view("entries", json!({})).unwrap(),
        json!({ "a": "2", "b": "3" })
    );
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "a" }))
            .unwrap(),
        json!("2")
    );
    assert_eq!(
        t.as_("owner")
            .call("remove", json!({ "key": "a" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(1));
}

#[test]
fn a_missing_key_is_an_app_error() {
    let mut t = app();
    match t.as_("owner").view("getResult", json!({ "key": "nope" })) {
        Err(Failure::App { code, fields }) => {
            assert_eq!(code, "NotFound");
            assert_eq!(fields, json!({ "key": "nope" }));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_emits_one_inserted_event_to_everyone() {
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "a", "value": "1" }))
        .unwrap();
    let events = t.events_for("owner");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "Inserted");
    assert_eq!(events[0].1, json!({ "key": "a", "value": "1" }));
}

#[test]
fn fill_writes_n_keys_of_the_given_size() {
    let mut t = app();
    t.as_("owner")
        .call("fill", json!({ "n": 3, "size": 100 }))
        .unwrap();
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(3));
    let v = t
        .as_("owner")
        .view("get", json!({ "key": "fill-2" }))
        .unwrap();
    assert_eq!(v.as_str().unwrap().len(), 100);
}

#[test]
fn clear_and_wipe_need_a_confirmation() {
    for method in ["clear", "wipe"] {
        let mut t = app();
        t.as_("owner")
            .call("set", json!({ "key": "a", "value": "1" }))
            .unwrap();
        assert!(matches!(
            t.as_("owner").call(method, json!({})),
            Err(Failure::ConfirmRequired)
        ));
        t.as_("owner").confirmed().call(method, json!({})).unwrap();
        assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(0));
        assert_eq!(
            t.as_("owner").view("get", json!({ "key": "a" })).unwrap(),
            json!(null)
        );
    }
}

#[test]
fn stamp_time_now_and_new_id_follow_the_entry() {
    let mut t = app();
    t.set_time(1_700_000_005_000);
    let at = t.as_("owner").call("stamp", json!({})).unwrap();
    assert_eq!(at, json!("1700000005000"));
    t.advance(1_000);
    // A view sees the last logged entry's time, not the clock (D19).
    let seen = t.as_("owner").view("timeNow", json!({})).unwrap();
    assert_eq!(seen, json!("1700000005000"));
    let a = t.as_("owner").call("newId", json!({})).unwrap();
    let b = t.as_("owner").call("newId", json!({})).unwrap();
    assert_ne!(a, b);
    assert!(a.as_str().unwrap().ends_with(".0"), "{a}");
}

#[test]
fn put_writes_without_an_event() {
    let mut t = app();
    t.as_("owner")
        .call("put", json!({ "key": "a", "value": "x".repeat(20_000) }))
        .unwrap();
    assert!(t.events_for("owner").is_empty());
    let v = t.as_("owner").view("get", json!({ "key": "a" })).unwrap();
    assert_eq!(v.as_str().unwrap().len(), 20_000);
}

#[test]
fn beat_records_its_slot_and_fails_inside_a_fail_window() {
    const HOUR: u64 = 3_600_000;
    let mut t = app();
    t.advance(HOUR);
    t.tick("beat").unwrap();
    t.as_("owner").call("failBeats", json!({ "n": 1 })).unwrap();
    t.advance(HOUR);
    assert!(matches!(t.tick("beat"), Err(Failure::App { .. })));
    t.advance(HOUR);
    t.tick("beat").unwrap();
    let beats = t.as_("owner").view("beats", json!({})).unwrap();
    let start = 1_700_000_000_000 + HOUR;
    assert_eq!(
        beats,
        json!([
            { "scheduled": start.to_string(), "previous": null, "at": start.to_string() },
            {
                "scheduled": (start + 2 * HOUR).to_string(),
                "previous": (start + HOUR).to_string(),
                "at": (start + 2 * HOUR).to_string(),
            },
        ])
    );
}

#[test]
fn blobs_are_referenced_from_state_and_granted() {
    let mut t = app();
    let id = t.upload_blob(b"hello", "text/plain");
    t.as_("owner")
        .call("attach", json!({ "name": "greeting", "blob": id }))
        .unwrap();
    assert_eq!(t.blob_refs(&id), 1);
    let file = t
        .as_("owner")
        .view("file", json!({ "name": "greeting" }))
        .unwrap();
    assert_eq!(file, json!({ "id": id, "size": "5", "mime": "text/plain" }));
    let grant = t
        .as_("owner")
        .view("share", json!({ "name": "greeting", "ttl": 60 }))
        .unwrap();
    assert!(grant.as_str().unwrap().starts_with(&id), "{grant}");
    // A blob the app does not hold cannot be referenced.
    let e = t
        .as_("owner")
        .call("attach", json!({ "name": "x", "blob": "00".repeat(32) }))
        .unwrap_err();
    assert_eq!(
        e,
        Failure::Trap(calimero_app_sdk::testing::TrapKind::HostDenied)
    );
    t.as_("owner")
        .call("detach", json!({ "name": "greeting" }))
        .unwrap();
    assert_eq!(t.blob_refs(&id), 0);
}

#[test]
fn notes_are_found_by_their_declared_fields() {
    let mut t = app();
    for (id, title, body) in [
        ("1", "Groceries", "milk and eggs"),
        ("2", "Trip", "pack the eggs carefully"),
        ("3", "Work", "secret plans"),
    ] {
        t.as_("owner")
            .call("note", json!({ "id": id, "title": title, "body": body }))
            .unwrap();
    }
    let mut found = t
        .as_("owner")
        .view("findNotes", json!({ "text": "eggs" }))
        .unwrap();
    found.as_array_mut().unwrap().sort_by_key(|v| v.to_string());
    assert_eq!(found, json!(["1", "2"]));
    // Search is for views: the same method run as a call traps.
    assert_eq!(
        t.as_("owner")
            .call("findNotes", json!({ "text": "eggs" }))
            .unwrap_err(),
        Failure::Trap(calimero_app_sdk::testing::TrapKind::HostDenied)
    );
    t.as_("owner")
        .call("dropNote", json!({ "id": "1" }))
        .unwrap();
    assert_eq!(
        t.as_("owner")
            .view("findNotes", json!({ "text": "eggs" }))
            .unwrap(),
        json!(["2"])
    );
}

#[test]
fn xcalls_leave_only_with_an_ok_run_and_reach_only_methods_that_allow_the_source() {
    let mut t = app();
    t.as_("owner")
        .call("relay", json!({ "app": "other", "key": "k", "value": "v" }))
        .unwrap();
    let sent = t.take_xcalls();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].app, "other");
    assert_eq!(sent[0].method, "setFromApp");
    assert_eq!(sent[0].args, json!({ "key": "k", "value": "v" }));
    // Delivered here as if from another app: `allow(app = "*")` takes it.
    t.deliver("app-a", "setFromApp", json!({ "key": "k", "value": "v" }))
        .unwrap();
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        json!("v")
    );
    // `set` allows no app.
    let e = t
        .deliver("app-a", "set", json!({ "key": "k", "value": "w" }))
        .unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "Forbidden"),
        "{e:?}"
    );
}

#[test]
fn an_action_reads_through_views_fetches_and_writes_through_its_calls() {
    use calimero_app_sdk::testing::HttpResponse;
    let mut t = app();
    t.as_("owner")
        .call("set", json!({ "key": "acme", "value": "acme.com" }))
        .unwrap();
    t.mock_http("echo.outbound.test", |req| HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: format!("saw {}", req.url).into_bytes(),
    });
    t.as_("owner")
        .action("enrich", json!({ "key": "acme" }))
        .unwrap();
    assert_eq!(
        t.as_("owner")
            .view("enrichment", json!({ "key": "acme" }))
            .unwrap(),
        json!("200 saw https://echo.outbound.test/v1/enrich?v=acme.com")
    );
    // Only the app's actions reach an `allow(action)` method.
    let e = t
        .as_("owner")
        .call(
            "applyEnrichment",
            json!({ "key": "acme", "value": "forged" }),
        )
        .unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "Forbidden"),
        "{e:?}"
    );
    // Started from a call, run later as its caller.
    let started = t
        .as_("owner")
        .call("startEnrich", json!({ "key": "acme" }))
        .unwrap();
    assert!(started.as_str().unwrap().starts_with("a."), "{started}");
    let ran = t.run_started_actions();
    assert_eq!(ran.len(), 1);
    assert!(ran[0].is_ok(), "{ran:?}");
    // An app error fails an action at once (D22).
    let e = t.as_("owner").action("explode", json!({})).unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "BeatFailed"),
        "{e:?}"
    );
    // A host with no mock: the fetch fails inside the action.
    let mut bare = app();
    let e = bare
        .as_("owner")
        .action("enrich", json!({ "key": "acme" }))
        .unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "Fetch"),
        "{e:?}"
    );
}

#[test]
fn a_captcha_service_answers_in_the_same_call() {
    let mut t = app();
    t.mock_app("captcha", |c| {
        Ok(match c.method.as_str() {
            "verify" => {
                assert!(!c.view && c.key.starts_with("xc:"), "{c:?}");
                json!(c.args["answer"] == "7")
            }
            _ => json!("open"),
        })
    });
    let human = |answer: &str| json!({ "key": "k", "value": "v", "captcha": "captcha", "challenge": "c1", "answer": answer });
    let e = t.as_("owner").call("setHuman", human("8")).unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "CaptchaFailed"),
        "{e:?}"
    );
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        json!(null)
    );
    t.as_("owner").call("setHuman", human("7")).unwrap();
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        json!("v")
    );
    assert_eq!(
        t.as_("owner")
            .view(
                "captchaStatus",
                json!({ "captcha": "captcha", "challenge": "c1" })
            )
            .unwrap(),
        json!("open")
    );
    // An app without a mock fails the xcall, and the call with it.
    let e = t
        .as_("owner")
        .call("setHuman", json!({ "key": "k", "value": "v", "captcha": "other", "challenge": "c1", "answer": "7" }))
        .unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "XcallFailed"),
        "{e:?}"
    );
}
