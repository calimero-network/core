//! The kv fixture on the real engine: every behaviour the cluster tests lean on.

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
        Failure::Trap(calimero_sdk::testing::TrapKind::HostDenied)
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
        Failure::Trap(calimero_sdk::testing::TrapKind::HostDenied)
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
    use calimero_sdk::testing::HttpResponse;
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

/// A delivery runs as the app that sent it, or, as a node delivers a call made with the
/// person's grant, as that person with `env::via()` naming the app. A run started by a person
/// offers that person to every app it calls, by message or synchronously (each call runs as
/// them only with their grant).
#[test]
fn a_delivery_runs_as_the_app_or_as_the_person_through_it() {
    let mut t = TestApp::new(
        component(),
        Org::new().members(["bob", "carol"]),
        1_700_000_000_000,
    );
    let get = |t: &mut TestApp, k: &str| t.as_("owner").view("get", json!({ "key": k })).unwrap();

    t.deliver("app-a", "recordWho", json!({ "key": "plain" }))
        .unwrap();
    assert_eq!(get(&mut t, "plain"), json!("app:app-a"));
    t.deliver_for("app-a", "bob", "recordWho", json!({ "key": "bob" }))
        .unwrap();
    assert_eq!(get(&mut t, "bob"), json!("account:bob via:app-a"));
    t.deliver_for_key("app-a", "k1", "bob", "recordWho", json!({ "key": "key" }))
        .unwrap();
    assert_eq!(get(&mut t, "key"), json!("key:k1 via:app-a"));
    // A person calling this app directly is its caller, through no app.
    assert_eq!(
        t.as_("bob").view("whoView", json!({})).unwrap(),
        json!("account:bob")
    );
    // Through an app as someone the organization does not hold: refused as their own call.
    let e = t
        .deliver_for("app-a", "mallory", "recordWho", json!({ "key": "m" }))
        .unwrap_err();
    assert!(matches!(e, Failure::Refused { status: 404, .. }), "{e:?}");

    // What this app sends for Bob offers Bob.
    t.as_("bob")
        .call("relayWho", json!({ "app": "other", "key": "k" }))
        .unwrap();
    let sent = t.take_xcalls();
    assert_eq!(
        sent[0].on_behalf_of,
        Some(calimero_sdk::env::Caller::Account("bob".into()))
    );

    // Synchronously too: the other app's mock sees whom the call may run as.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&seen);
    t.mock_app("other", move |c| {
        log.lock().unwrap().push(c.on_behalf_of.clone());
        Ok(if c.view { json!("ok") } else { json!(null) })
    });
    t.api_key("k2", "bob")
        .call("callWho", json!({ "app": "other", "key": "k" }))
        .unwrap();
    t.as_("carol")
        .call("askWho", json!({ "app": "other" }))
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            Some(calimero_sdk::env::Caller::ApiKey {
                key_id: "k2".into(),
                account: "bob".into()
            }),
            Some(calimero_sdk::env::Caller::Account("carol".into())),
        ]
    );
}

/// `env::linked`: the app linked under a name, from `init` on, and an attach or detach seen by
/// the next run.
#[test]
fn linked_reads_the_apps_linked_now() {
    let mut t = TestApp::new(
        component(),
        Org::new().link("kv", "kv-app"),
        1_700_000_000_000,
    );
    let linked = |t: &mut TestApp, name: &str| {
        t.as_("owner")
            .view("linked", json!({ "name": name }))
            .unwrap()
    };
    assert_eq!(linked(&mut t, "kv"), json!("kv-app"));
    assert_eq!(linked(&mut t, "calendar"), json!(null));
    t.link("calendar", Some("cal-app"));
    assert_eq!(linked(&mut t, "calendar"), json!("cal-app"));
    t.link("calendar", None);
    assert_eq!(linked(&mut t, "calendar"), json!(null));
}

/// `env::unit_of`: the instance whose unit the app is a member of, none for an app on its own.
#[test]
fn unit_of_names_the_instance_that_created_the_app() {
    let mut member = TestApp::new(
        component(),
        Org::new().unit_of("notes-app"),
        1_700_000_000_000,
    );
    assert_eq!(
        member.as_("owner").view("unitOf", json!({})).unwrap(),
        json!("notes-app")
    );
    let mut alone = app();
    assert_eq!(
        alone.as_("owner").view("unitOf", json!({})).unwrap(),
        json!(null)
    );
}

/// `env::grant_scope`: a person named with a scope sees it, so the app tells a guest kept to
/// one channel from a member who reaches all of it.
#[test]
fn a_scoped_grant_is_told_apart_by_the_app() {
    let mut t = TestApp::new(
        component(),
        Org::new()
            .members(["member", "partner"])
            .scope("partner", r#"{"channel":"partners"}"#),
        1_700_000_000_000,
    );
    assert_eq!(
        t.as_("partner").view("grantScope", json!({})).unwrap(),
        json!(r#"{"channel":"partners"}"#)
    );
    assert_eq!(
        t.as_("member").view("grantScope", json!({})).unwrap(),
        json!(null)
    );
}

#[test]
fn joined_calls_go_to_the_node_in_one_batch() {
    use std::sync::{Arc, Mutex};
    let mut t = app();
    let seen = Arc::new(Mutex::new(Vec::new()));
    for (app, count) in [("a", 7), ("b", 9)] {
        let seen = Arc::clone(&seen);
        t.mock_app(app, move |c| {
            seen.lock()
                .unwrap()
                .push((app, c.method.clone(), c.batch, c.view, c.key.clone()));
            Ok(json!(count))
        });
    }
    let args = json!({ "a": "a", "b": "b", "key": "k", "spins": 1 });
    assert_eq!(
        t.as_("owner").call("bumpBoth", args.clone()).unwrap(),
        json!([7, 9])
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    // Both in the run's first batch, both calls, each under a key of its own.
    for (i, app) in ["a", "b"].into_iter().enumerate() {
        assert_eq!(
            (seen[i].0, seen[i].1.as_str(), seen[i].2, seen[i].3),
            (app, "slowBump", 0, false)
        );
    }
    assert_ne!(seen[0].4, seen[1].4);
    // A run that stalls after its calls times out: their answers came back first.
    let mut stall = args;
    stall["stall"] = json!(true);
    let e = t.as_("owner").call("bumpBoth", stall).unwrap_err();
    assert!(matches!(&e, Failure::Trap(_)), "{e:?}");
}

#[test]
fn slow_bump_counts_under_its_key() {
    let mut t = app();
    for n in 1..=2 {
        assert_eq!(
            t.deliver("other", "slowBump", json!({ "key": "k", "spins": 1 }))
                .unwrap(),
            json!(n)
        );
    }
    assert_eq!(
        t.as_("owner").view("get", json!({ "key": "k" })).unwrap(),
        json!("2")
    );
}

/// A visitor through a public link calls the public methods only, and the app sees the link
/// and its key (`env::Caller::Visitor`); any other method is refused before it runs, and the
/// SDK's guards refuse it as they refuse an anonymous caller.
#[test]
fn a_visitor_calls_only_the_public_methods_and_is_named() {
    let mut t = app();
    let key = "ab".repeat(32);
    let who = t
        .visitor("link1", &key)
        .call("signGuestbook", json!({ "note": "hi" }))
        .unwrap();
    assert_eq!(who, json!(format!("visitor:link1:{key}")));
    assert_eq!(
        t.visitor("link1", &key)
            .view("whoamiPublic", json!({}))
            .unwrap(),
        json!(format!("visitor:link1:{key}"))
    );
    let refused = t
        .visitor("link1", &key)
        .call("set", json!({ "key": "a", "value": "1" }))
        .unwrap_err();
    assert!(
        matches!(refused, Failure::Refused { status: 403, .. }),
        "{refused:?}"
    );
    assert!(t
        .visitor("link1", &key)
        .view("get", json!({ "key": "a" }))
        .is_err());
}

/// C10: a write past the run's 10,000 writes is the typed `RunLimit` the app passes on, with
/// the write's number, not a trap; nothing of the run is kept, on a node and here alike.
#[test]
fn a_write_past_the_runs_writes_is_a_typed_error() {
    let mut t = app();
    let e = t
        .as_("owner")
        .call("fillQuiet", json!({ "n": 10_001, "size": 1 }))
        .unwrap_err();
    assert_eq!(
        e,
        Failure::App {
            code: "RunLimit".into(),
            fields: json!({ "limit": "writes", "max": "10000", "attempted": "10001" }),
        }
    );
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(0));
}

/// C10: past the run's 1 MiB of writes, a write or an event is the typed `RunLimit` with the
/// bytes the run would have reached, not a trap.
#[test]
fn bytes_past_the_runs_mib_are_a_typed_error() {
    let mut t = app();
    let e = t
        .as_("owner")
        .call("fill", json!({ "n": 1000, "size": 1000 }))
        .unwrap_err();
    let Failure::App { code, fields } = e else {
        panic!("not an app error: {e:?}");
    };
    assert_eq!(code, "RunLimit");
    assert_eq!(fields["limit"], "bytes");
    assert_eq!(fields["max"], "1048576");
    let attempted: u64 = fields["attempted"].as_str().unwrap().parse().unwrap();
    assert!(attempted > 1_048_576, "{fields}");
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!(0));
}

/// C10: within the run's 1 MiB, a value whose log entry (JSON, values in base64) passes the
/// log's 1 MiB is refused as a node refuses it: `HostDenied`, nothing kept.
#[test]
fn a_run_whose_log_entry_passes_the_cap_is_host_denied() {
    let mut t = app();
    let e = t
        .as_("owner")
        .call("fillQuiet", json!({ "n": 1, "size": 900 * 1024 }))
        .unwrap_err();
    assert_eq!(
        e,
        Failure::Trap(calimero_sdk::testing::TrapKind::HostDenied)
    );
    assert_eq!(
        t.as_("owner")
            .view("get", json!({ "key": "quiet-0" }))
            .unwrap(),
        json!(null)
    );
    // Its encoding within the cap, the same kind of write commits.
    t.as_("owner")
        .call("fillQuiet", json!({ "n": 1, "size": 600 * 1024 }))
        .unwrap();
}

/// C10: search is the node's ranked word index, not a substring scan: a word matches whole
/// words only, and a note naming it in its title and body ranks first.
#[test]
fn search_matches_words_and_ranks_as_a_node_does() {
    let mut t = app();
    for (id, title, body) in [
        ("1", "Eggplant", "parmesan bake"),
        ("2", "Trip", "pack the eggs"),
        ("3", "Eggs", "eggs, eggs and more eggs"),
    ] {
        t.as_("owner")
            .call("note", json!({ "id": id, "title": title, "body": body }))
            .unwrap();
    }
    assert_eq!(
        t.as_("owner")
            .view("findNotes", json!({ "text": "eggs" }))
            .unwrap(),
        json!(["3", "2"])
    );
    assert_eq!(
        t.as_("owner")
            .view("findNotes", json!({ "text": "egg" }))
            .unwrap(),
        json!([])
    );
}
