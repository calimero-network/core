//! The noticeboard's rules on the real engine (ported from the v1 `TestHost` tests).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // The test's own target dir (`<target>/tmp`'s parent), so the guest shares its deps.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build authored-sorted-kv-store: {e}"))
            .component
    })
}

fn code(r: Result<Value, Failure>) -> String {
    match r {
        Err(Failure::App { code, .. }) => code,
        other => panic!("expected an app failure, got {other:?}"),
    }
}

fn app() -> TestApp {
    TestApp::new(
        component(),
        Org::new().members(["alice", "bob"]).guests(["gus"]),
        1_700_000_000_000,
    )
}

fn post(t: &mut TestApp, who: &str, topic: &str, seq: u64, text: &str) -> String {
    let key = t
        .as_(who)
        .call(
            "post",
            json!({ "topic": topic, "seq": seq.to_string(), "text": text }),
        )
        .expect("post");
    key.as_str().expect("a key").to_owned()
}

#[test]
fn two_people_post_to_one_topic_without_contending() {
    let mut t = app();
    // The same seq, deliberately: the author in the key keeps the notes on two keys.
    let a = post(&mut t, "alice", "news", 1, "from alice");
    let b = post(&mut t, "bob", "news", 1, "from bob");
    assert_ne!(a, b);
    assert_eq!(a, "news/alice/00000001");
    let notes = t
        .as_("alice")
        .view("readTopic", json!({ "topic": "news" }))
        .unwrap();
    assert_eq!(notes.as_array().unwrap().len(), 2);
    assert!(notes
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n["keyMatchesOwner"] == true));
    assert_eq!(
        t.as_("alice")
            .view("verifiedNotes", json!({ "topic": "news" }))
            .unwrap(),
        json!(2)
    );
}

#[test]
fn another_account_reads_a_note_by_its_key() {
    let mut t = app();
    let key = post(&mut t, "alice", "news", 1, "hers");
    assert_eq!(
        t.as_("bob").view("get", json!({ "key": key })).unwrap(),
        json!("hers")
    );
    assert_eq!(
        t.as_("bob").view("ownerOf", json!({ "key": key })).unwrap(),
        json!("alice")
    );
    let notes = t
        .as_("bob")
        .view("readTopic", json!({ "topic": "news" }))
        .unwrap();
    assert_eq!(notes[0]["author"], "alice");
    assert_eq!(
        t.as_("bob")
            .view("get", json!({ "key": "news/nobody/1" }))
            .unwrap(),
        json!(null)
    );
    assert_eq!(
        t.as_("bob")
            .view("ownerOf", json!({ "key": "news/nobody/1" }))
            .unwrap(),
        json!("")
    );
}

#[test]
fn only_the_author_may_edit_or_retract() {
    let mut t = app();
    let key = post(&mut t, "alice", "news", 1, "mine");
    assert_eq!(
        code(
            t.as_("bob")
                .call("edit", json!({ "key": key, "text": "not yours" }))
        ),
        "NotFound"
    );
    assert_eq!(
        code(t.as_("bob").call("retract", json!({ "key": key }))),
        "NotFound"
    );
    assert_eq!(
        t.as_("alice").view("get", json!({ "key": key })).unwrap(),
        json!("mine")
    );
    t.as_("alice")
        .call("edit", json!({ "key": key, "text": "edited" }))
        .unwrap();
    assert_eq!(
        t.as_("alice")
            .call("retract", json!({ "key": key }))
            .unwrap(),
        json!("edited")
    );
    assert_eq!(
        t.as_("alice").view("get", json!({ "key": key })).unwrap(),
        json!(null)
    );
}

#[test]
fn a_topic_read_is_unmoved_by_another_topic() {
    let mut t = app();
    post(&mut t, "alice", "news", 1, "hello");
    let total = t
        .as_("bob")
        .call("flood", json!({ "topic": "spam", "from": "0", "n": "500" }))
        .unwrap();
    assert_eq!(total, json!(501));
    let notes = t
        .as_("alice")
        .view("readTopic", json!({ "topic": "news" }))
        .unwrap();
    assert_eq!(notes.as_array().unwrap().len(), 1);
    assert_eq!(notes[0]["text"], "hello");
    assert_eq!(
        t.as_("alice").view("countAll", json!({})).unwrap(),
        json!(501)
    );
    let spam = t
        .as_("alice")
        .view("readTopic", json!({ "topic": "spam" }))
        .unwrap();
    assert_eq!(
        spam.as_array().unwrap().len(),
        500,
        "a slice longer than one page"
    );
}

#[test]
fn my_notes_narrows_to_the_caller() {
    let mut t = app();
    post(&mut t, "alice", "news", 1, "a");
    post(&mut t, "alice", "news", 2, "a");
    post(&mut t, "bob", "news", 1, "b");
    let mine = |t: &mut TestApp, who| {
        t.as_(who)
            .view("myNotes", json!({ "topic": "news" }))
            .unwrap()
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(mine(&mut t, "alice"), 2);
    assert_eq!(mine(&mut t, "bob"), 1);
}

#[test]
fn a_topic_cannot_smuggle_a_separator() {
    let mut t = app();
    assert_eq!(
        code(
            t.as_("alice")
                .call("post", json!({ "topic": "a/b", "seq": "1", "text": "x" }))
        ),
        "TopicHasSeparator"
    );
    assert_eq!(
        code(
            t.as_("alice")
                .call("flood", json!({ "topic": "a/b", "from": "0", "n": "1" }))
        ),
        "TopicHasSeparator"
    );
}

#[test]
fn guests_may_not_post() {
    let mut t = app();
    assert_eq!(
        code(
            t.as_("gus")
                .call("post", json!({ "topic": "news", "seq": "1", "text": "x" }))
        ),
        "Forbidden"
    );
}
