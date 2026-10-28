//! The team space on the real engine: v1's `TestHost` tests, with each storage write
//! policy they exercised now the app's own check.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn space() -> TestApp {
    common::app(Org::new().members(["alice", "bob", "mallory"]))
}

fn ok(r: Result<Value, Failure>) -> Value {
    r.unwrap_or_else(|e| panic!("{e:?}"))
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app error, got {other:?}"),
    }
}

#[test]
fn the_founding_record_and_quorum_are_written_at_init() {
    let mut t = space();
    assert_eq!(
        ok(t.as_("bob").view("founding", json!({}))),
        json!({ "name": "calimero core", "motto": "converge without coordinating" })
    );
    assert_eq!(ok(t.as_("bob").view("quorum", json!({}))), json!("2"));
    assert_eq!(ok(t.as_("bob").view("founder", json!({}))), json!("owner"));
    assert_eq!(
        ok(t.as_("bob").view("moderators", json!({}))),
        json!(["owner"]),
        "whoever ran init founded the space and is the first moderator"
    );
}

#[test]
fn the_owner_founds_the_space_and_moderates_it() {
    let mut t = space();
    ok(t.as_("alice").call(
        "announce",
        json!({ "id": "a1", "text": "x", "sentAt": "1" }),
    ));
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true)
    );
}

#[test]
fn a_message_key_is_per_author_and_final() {
    let mut t = space();
    let post = |t: &mut TestApp, who: &str, text: &str, at: &str| {
        t.as_(who).call(
            "postMessage",
            json!({ "channel": "general", "id": "m1", "text": text, "sentAt": at }),
        )
    };
    assert_eq!(ok(post(&mut t, "alice", "hello", "1")), json!("general/m1"));
    assert_eq!(
        ok(post(&mut t, "bob", "bob's own", "2")),
        json!("general/m1"),
        "the same key is a separate row for another account"
    );
    assert_eq!(
        post(&mut t, "alice", "edited", "3"),
        Err(Failure::App {
            code: "AlreadyWritten".to_owned(),
            fields: json!({ "key": "general/m1" })
        }),
        "its author cannot rewrite it: it was written once"
    );
    let general = ok(t
        .as_("bob")
        .view("channel", json!({ "channel": "general" })));
    assert_eq!(
        general,
        json!([
            { "key": "general/m1", "author": "alice", "text": "hello", "sentAt": "1" },
            { "key": "general/m1", "author": "bob", "text": "bob's own", "sentAt": "2" },
        ]),
        "one row per author, neither touched"
    );
}

#[test]
fn a_channel_is_one_index_range() {
    let mut t = space();
    for (channel, id, who) in [
        ("general", "a", "alice"),
        ("random", "b", "bob"),
        ("general", "c", "bob"),
    ] {
        ok(t.as_(who).call(
            "postMessage",
            json!({ "channel": channel, "id": id, "text": format!("{channel} {id}"),
                    "sentAt": "0" }),
        ));
    }
    let general = ok(t
        .as_("bob")
        .view("channel", json!({ "channel": "general" })));
    let keys: Vec<_> = general
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["key"].clone())
        .collect();
    assert_eq!(keys, [json!("general/a"), json!("general/c")]);
    assert_eq!(general[1]["author"], json!("bob"));
    assert_eq!(
        code(t.as_("owner").call(
            "postMessage",
            json!({ "channel": "gen/eral", "id": "x", "text": "", "sentAt": "0" }),
        )),
        "HasSeparator"
    );
}

#[test]
fn only_a_moderator_removes_an_announcement_not_its_author() {
    let mut t = space();
    ok(t.as_("alice").call(
        "announce",
        json!({ "id": "a1", "text": "offsite friday", "sentAt": "1" }),
    ));
    assert_eq!(
        code(t.as_("alice").call(
            "announce",
            json!({ "id": "a1", "text": "moved", "sentAt": "2" }),
        )),
        "AlreadyWritten",
        "an announcement is never edited"
    );
    assert_eq!(
        code(
            t.as_("alice")
                .call("removeAnnouncement", json!({ "id": "a1" }))
        ),
        "NotModerator",
        "its author is not a moderator"
    );
    assert_eq!(
        code(
            t.as_("bob")
                .call("removeAnnouncement", json!({ "id": "a1" }))
        ),
        "NotModerator"
    );
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true)
    );
    assert_eq!(ok(t.as_("bob").view("announcements", json!({}))), json!([]));
}

#[test]
fn a_moderator_removes_every_author_s_announcement_at_an_id() {
    let mut t = space();
    for (who, text) in [("alice", "from alice"), ("bob", "from bob")] {
        ok(t.as_(who).call(
            "announce",
            json!({ "id": "a1", "text": text, "sentAt": "1" }),
        ));
    }
    assert_eq!(
        ok(t.as_("bob").view("announcements", json!({}))),
        json!([
            { "key": "a1", "author": "alice", "text": "from alice", "sentAt": "1" },
            { "key": "a1", "author": "bob", "text": "from bob", "sentAt": "1" },
        ])
    );
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true)
    );
    assert_eq!(ok(t.as_("bob").view("announcements", json!({}))), json!([]));
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(false),
        "nothing left"
    );
}

#[test]
fn only_a_moderator_appoints_one() {
    let mut t = space();
    assert_eq!(
        code(
            t.as_("bob")
                .call("addModerator", json!({ "account": "bob" }))
        ),
        "NotModerator"
    );
    ok(t.as_("owner")
        .call("addModerator", json!({ "account": "bob" })));
    assert_eq!(
        ok(t.as_("bob").view("moderators", json!({}))),
        json!(["bob", "owner"])
    );
    ok(t.as_("alice").call(
        "announce",
        json!({ "id": "a1", "text": "x", "sentAt": "1" }),
    ));
    assert_eq!(
        ok(t.as_("bob")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true),
        "an appointed moderator removes"
    );
}

#[test]
fn a_page_and_its_revisions_belong_to_its_author() {
    let mut t = space();
    ok(t.as_("alice").call(
        "createPage",
        json!({ "id": "home", "title": "Home", "text": "welcome" }),
    ));
    ok(t.as_("alice").call(
        "addRevision",
        json!({ "id": "home", "revision": "r2", "text": "welcome back" }),
    ));
    ok(t.as_("alice")
        .call("renamePage", json!({ "id": "home", "title": "Start" })));

    assert_eq!(
        code(
            t.as_("bob")
                .call("renamePage", json!({ "id": "home", "title": "Mine" }))
        ),
        "NoPage"
    );
    assert_eq!(
        code(t.as_("bob").call(
            "addRevision",
            json!({ "id": "home", "revision": "r3", "text": "defaced" }),
        )),
        "NoPage",
        "Bob holds no page at `home`, so Alice's revisions are out of reach"
    );
    assert_eq!(
        ok(t.as_("bob").call("deletePage", json!({ "id": "home" }))),
        json!(false),
        "Bob holds no page at `home`"
    );

    // Bob reads Alice's page by naming her; pages of his and Mallory's at the id
    // change nothing about hers.
    ok(t.as_("bob").call(
        "createPage",
        json!({ "id": "home", "title": "Bob's", "text": "his" }),
    ));
    ok(t.as_("bob")
        .call("renamePage", json!({ "id": "home", "title": "Bob's home" })));
    ok(t.as_("mallory").call(
        "createPage",
        json!({ "id": "home", "title": "Hijacked", "text": "spam" }),
    ));
    assert_eq!(
        ok(t.as_("bob")
            .view("page", json!({ "author": "alice", "id": "home" }))),
        json!({ "id": "home", "author": "alice", "title": "Start",
                "revisions": [["r1", "welcome"], ["r2", "welcome back"]] })
    );

    assert_eq!(
        ok(t.as_("alice").call("deletePage", json!({ "id": "home" }))),
        json!(true)
    );
    assert_eq!(
        ok(t.as_("bob")
            .view("page", json!({ "author": "alice", "id": "home" }))),
        json!(null)
    );
    let left = ok(t
        .as_("bob")
        .view("page", json!({ "author": "bob", "id": "home" })));
    assert_eq!(
        (&left["author"], &left["title"]),
        (&json!("bob"), &json!("Bob's home"))
    );
    // A re-created page starts from its first revision again.
    ok(t.as_("alice").call(
        "createPage",
        json!({ "id": "home", "title": "Again", "text": "fresh" }),
    ));
    assert_eq!(
        ok(t.as_("bob")
            .view("page", json!({ "author": "alice", "id": "home" })))["revisions"],
        json!([["r1", "fresh"]])
    );
}

#[test]
fn evidence_is_keyed_by_its_content_and_found_by_kind() {
    let mut t = space();
    let file = |t: &mut TestApp, who: &str, kind: &str, body: &str| {
        ok(t.as_(who)
            .call("fileEvidence", json!({ "kind": kind, "body": body })))
    };
    let log = file(&mut t, "alice", "log", "boot ok");
    let again = file(&mut t, "bob", "log", "boot ok");
    assert_eq!(log, again, "equal content, one row");
    let _photo = file(&mut t, "owner", "photo", "img");
    let _full = file(&mut t, "owner", "log", "disk full");
    let filed = t
        .events_for("bob")
        .iter()
        .filter(|(n, _)| n == "EvidenceFiled")
        .count();
    assert_eq!(filed, 3, "filing equal content again stores nothing");

    let logs = ok(t
        .as_("bob")
        .view("evidenceOfKind", json!({ "kind": "log" })));
    let bodies: Vec<_> = logs
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["body"].clone())
        .collect();
    assert_eq!(bodies, [json!("boot ok"), json!("disk full")]);
    assert_eq!(
        ok(t.as_("bob").view("evidence", json!({ "hash": log })))["body"],
        json!("boot ok")
    );
    assert_eq!(
        code(t.as_("bob").view("evidence", json!({ "hash": "zz" }))),
        "BadHash"
    );
    assert_eq!(
        ok(t.as_("bob")
            .view("evidence", json!({ "hash": "0".repeat(64) }))),
        json!(null)
    );
}
