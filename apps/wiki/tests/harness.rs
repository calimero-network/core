//! The wiki on the real engine: each rule of `src/lib.rs`, with the refusals.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

/// What `sha256sum` prints for `hello`.
const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn wiki() -> TestApp {
    common::app(
        Org::new()
            .members(["alice", "bob", "carol"])
            .guests(["gus"]),
    )
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

fn events(t: &TestApp, who: &str) -> Vec<String> {
    t.events_for(who).into_iter().map(|(n, _)| n).collect()
}

fn create(t: &mut TestApp, who: &str, title: &str, body: &str) -> Value {
    ok(t.as_(who)
        .call("createPage", json!({ "title": title, "body": body })))
}

#[test]
fn init_seeds_the_home_page_as_the_owner() {
    let mut t = wiki();
    let home = ok(t.as_("bob").view("page", json!({ "slug": "home" })));
    assert_eq!(home["author"], json!("owner"), "init runs as the owner");
    assert_eq!(home["title"], json!("Home"));
    assert_eq!(home["revision"], json!(1));
    assert_eq!(
        ok(t.as_("bob").view("pages", json!({}))),
        json!([{ "slug": "home", "title": "Home", "author": "owner" }])
    );
    assert!(
        events(&t, "bob").is_empty(),
        "the seed is state, not events"
    );
    assert_eq!(
        code(
            t.as_("bob")
                .call("revisePage", json!({ "slug": "home", "body": "mine now" }))
        ),
        "NotAuthor",
        "the seeded page is the owner's like any other"
    );
    assert_eq!(
        ok(t.as_("owner")
            .call("revisePage", json!({ "slug": "home", "body": "hello" }))),
        json!(2)
    );
}

#[test]
fn titles_that_differ_in_case_or_spacing_name_one_slug() {
    let mut t = wiki();
    assert_eq!(
        create(&mut t, "alice", "  Getting   Started ", "step one"),
        json!("getting-started")
    );
    for spelling in ["getting-started", "GETTING STARTED", " Getting Started"] {
        let page = ok(t.as_("bob").view("page", json!({ "slug": spelling })));
        assert_eq!(page["body"], json!("step one"), "{spelling}");
    }
    assert_eq!(
        t.as_("bob").call(
            "createPage",
            json!({ "title": "getting started", "body": "x" })
        ),
        Err(Failure::App {
            code: "PageExists".to_owned(),
            fields: json!({ "slug": "getting-started" })
        })
    );
    assert_eq!(
        code(
            t.as_("bob")
                .call("createPage", json!({ "title": "   ", "body": "x" }))
        ),
        "BadTitle"
    );
    assert_eq!(
        ok(t.as_("bob").view("page", json!({ "slug": "nowhere" }))),
        json!(null)
    );
}

#[test]
fn only_the_author_revises_renames_tags_attaches_or_deletes() {
    let mut t = wiki();
    create(&mut t, "alice", "Runbook", "restart the node");
    let refused = |t: &mut TestApp, method: &str, args: Value| {
        assert_eq!(
            t.as_("bob").call(method, args),
            Err(Failure::App {
                code: "NotAuthor".to_owned(),
                fields: json!({ "slug": "runbook" })
            }),
            "{method}"
        );
    };
    refused(
        &mut t,
        "revisePage",
        json!({ "slug": "runbook", "body": "defaced" }),
    );
    refused(
        &mut t,
        "renamePage",
        json!({ "slug": "runbook", "title": "Mine" }),
    );
    refused(
        &mut t,
        "tagPage",
        json!({ "slug": "runbook", "tag": "ops" }),
    );
    refused(
        &mut t,
        "attach",
        json!({ "slug": "runbook", "content": "x" }),
    );
    refused(&mut t, "deletePage", json!({ "slug": "runbook" }));
    assert_eq!(
        code(
            t.as_("bob")
                .call("revisePage", json!({ "slug": "nope", "body": "x" }))
        ),
        "NoPage"
    );

    ok(t.as_("alice").call(
        "revisePage",
        json!({ "slug": "runbook", "body": "drain, then restart" }),
    ));
    ok(t.as_("alice").call(
        "renamePage",
        json!({ "slug": "runbook", "title": "Ops runbook" }),
    ));
    let page = ok(t.as_("bob").view("page", json!({ "slug": "runbook" })));
    assert_eq!(
        (
            &page["slug"],
            &page["title"],
            &page["revision"],
            &page["body"]
        ),
        (
            &json!("runbook"),
            &json!("Ops runbook"),
            &json!(2),
            &json!("drain, then restart")
        ),
        "a rename keeps the slug"
    );
    assert_eq!(
        ok(t.as_("bob").view("history", json!({ "slug": "runbook" }))),
        json!([
            { "revision": 1, "body": "restart the node" },
            { "revision": 2, "body": "drain, then restart" },
        ])
    );
}

#[test]
fn deleting_a_page_drops_its_revisions_and_tags_but_keeps_attachments() {
    let mut t = wiki();
    create(&mut t, "alice", "Old", "v1");
    ok(t.as_("alice")
        .call("revisePage", json!({ "slug": "old", "body": "v2" })));
    ok(t.as_("alice")
        .call("tagPage", json!({ "slug": "old", "tag": "legacy" })));
    let hash = ok(t
        .as_("alice")
        .call("attach", json!({ "slug": "old", "content": "hello" })));
    assert_eq!(
        ok(t.as_("alice").call("deletePage", json!({ "slug": "old" }))),
        json!(true)
    );
    assert_eq!(
        ok(t.as_("alice").call("deletePage", json!({ "slug": "old" }))),
        json!(false)
    );
    assert_eq!(
        ok(t.as_("bob").view("history", json!({ "slug": "old" }))),
        json!([])
    );
    assert_eq!(
        ok(t.as_("bob").view("tagStats", json!({}))),
        json!({ "count": "0", "first": null, "last": null }),
        "a tag leaves the set with its last page"
    );
    assert_eq!(
        ok(t.as_("bob").view("attachment", json!({ "hash": hash }))),
        json!("hello"),
        "content is never removed"
    );
    // A re-created page starts from revision 1 again.
    create(&mut t, "bob", "Old", "fresh");
    assert_eq!(
        ok(t.as_("bob").view("history", json!({ "slug": "old" }))),
        json!([{ "revision": 1, "body": "fresh" }])
    );
}

#[test]
fn the_tag_set_reads_in_order_by_prefix_range_and_page() {
    let mut t = wiki();
    for (who, title) in [("alice", "Deploy"), ("bob", "Onboarding")] {
        create(&mut t, who, title, "");
    }
    for (who, slug, tag) in [
        ("alice", "deploy", "ops"),
        ("alice", "deploy", "API"),
        ("alice", "deploy", " apple "),
        ("bob", "onboarding", "apricot"),
        ("bob", "onboarding", "ops"),
        ("bob", "onboarding", "wasm"),
    ] {
        assert_eq!(
            ok(t.as_(who)
                .call("tagPage", json!({ "slug": slug, "tag": tag }))),
            json!(true)
        );
    }
    assert_eq!(
        ok(t.as_("alice")
            .call("tagPage", json!({ "slug": "deploy", "tag": "OPS" }))),
        json!(false),
        "a tag the page has is a no-op"
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tags", json!({ "offset": 0, "limit": 100 }))),
        json!(["api", "apple", "apricot", "ops", "wasm"])
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tagsStartingWith", json!({ "prefix": "ap" }))),
        json!(["api", "apple", "apricot"])
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tagsBetween", json!({ "start": "apple", "end": "ops" }))),
        json!(["apple", "apricot"])
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tagsBetween", json!({ "start": "z", "end": "a" }))),
        json!([])
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tags", json!({ "offset": 3, "limit": 5 }))),
        json!(["ops", "wasm"])
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("tags", json!({ "offset": 5, "limit": 5 }))),
        json!([])
    );
    assert_eq!(
        ok(t.as_("carol").view("tagStats", json!({}))),
        json!({ "count": "5", "first": "api", "last": "wasm" })
    );
    assert_eq!(
        ok(t.as_("carol").view("pagesTagged", json!({ "tag": "ops" }))),
        json!(["deploy", "onboarding"])
    );
    assert_eq!(
        ok(t.as_("carol").view("page", json!({ "slug": "deploy" })))["tags"],
        json!(["api", "apple", "ops"])
    );
    assert_eq!(
        code(
            t.as_("alice")
                .call("tagPage", json!({ "slug": "deploy", "tag": "  " }))
        ),
        "BadTag"
    );

    // `ops` stays while another page holds it; `wasm` goes with its only page.
    assert_eq!(
        ok(t.as_("alice")
            .call("untagPage", json!({ "slug": "deploy", "tag": "ops" }))),
        json!(true)
    );
    assert_eq!(
        ok(t.as_("alice")
            .call("untagPage", json!({ "slug": "deploy", "tag": "ops" }))),
        json!(false)
    );
    ok(t.as_("bob")
        .call("untagPage", json!({ "slug": "onboarding", "tag": "wasm" })));
    assert_eq!(
        ok(t.as_("carol").view("tagStats", json!({}))),
        json!({ "count": "4", "first": "api", "last": "ops" })
    );
    assert_eq!(
        events(&t, "carol").len(),
        2 + 6 + 2,
        "two creates, six tags and two untags; the no-op tag and untag emit nothing"
    );
}

#[test]
fn each_account_writes_only_its_own_draft() {
    let mut t = wiki();
    ok(t.as_("alice").call(
        "saveDraft",
        json!({ "title": "Retro", "body": "went well" }),
    ));
    assert_eq!(
        ok(t.as_("alice").view("myDraft", json!({}))),
        json!({ "title": "Retro", "body": "went well" })
    );
    assert_eq!(ok(t.as_("bob").view("myDraft", json!({}))), json!(null));
    assert_eq!(
        ok(t.as_("bob").view("draftOf", json!({ "account": "alice" })))["body"],
        json!("went well"),
        "anyone reads a draft by account"
    );
    // Bob's save lands under Bob: there is no argument naming Alice.
    ok(t.as_("bob").call(
        "saveDraft",
        json!({ "title": "Retro", "body": "bob's take" }),
    ));
    assert_eq!(
        ok(t.as_("alice").view("myDraft", json!({})))["body"],
        json!("went well")
    );

    assert_eq!(
        ok(t.as_("alice").call("publishDraft", json!({}))),
        json!("retro")
    );
    assert_eq!(ok(t.as_("alice").view("myDraft", json!({}))), json!(null));
    assert_eq!(
        code(t.as_("bob").call("publishDraft", json!({}))),
        "NotAuthor",
        "Bob's draft names Alice's page now"
    );
    assert_eq!(
        ok(t.as_("bob").call("discardDraft", json!({}))),
        json!(true)
    );
    assert_eq!(
        ok(t.as_("bob").call("discardDraft", json!({}))),
        json!(false)
    );
    assert_eq!(
        code(t.as_("bob").call("publishDraft", json!({}))),
        "NoDraft"
    );

    // Publishing onto one's own page adds a revision.
    ok(t.as_("alice").call(
        "saveDraft",
        json!({ "title": "retro", "body": "and actions" }),
    ));
    ok(t.as_("alice").call("publishDraft", json!({})));
    let page = ok(t.as_("bob").view("page", json!({ "slug": "retro" })));
    assert_eq!(
        (&page["revision"], &page["body"]),
        (&json!(2), &json!("and actions"))
    );
}

#[test]
fn attachments_are_content_addressed_and_stored_once() {
    let mut t = wiki();
    create(&mut t, "alice", "A", "");
    create(&mut t, "bob", "B", "");
    let hash = ok(t
        .as_("alice")
        .call("attach", json!({ "slug": "a", "content": "hello" })));
    assert_eq!(hash, json!(HELLO_SHA256), "the SHA-256 of the bytes");
    let again = ok(t
        .as_("alice")
        .call("attach", json!({ "slug": "a", "content": "hello" })));
    let elsewhere = ok(t
        .as_("bob")
        .call("attach", json!({ "slug": "b", "content": "hello" })));
    assert_eq!((&again, &elsewhere), (&hash, &hash));
    let stored = events(&t, "carol")
        .into_iter()
        .filter(|n| n == "AttachmentStored")
        .count();
    assert_eq!(stored, 1, "equal content stores nothing");
    assert_eq!(
        ok(t.as_("carol").view("page", json!({ "slug": "a" })))["attachments"],
        json!([HELLO_SHA256]),
        "attaching to the same page twice links it once"
    );
    assert_eq!(
        ok(t.as_("carol")
            .view("attachment", json!({ "hash": HELLO_SHA256.to_uppercase() }))),
        json!("hello")
    );
    assert_eq!(
        code(t.as_("carol").view("attachment", json!({ "hash": "zz" }))),
        "BadHash"
    );
    assert_eq!(
        t.as_("carol")
            .view("attachment", json!({ "hash": "0".repeat(64) })),
        Err(Failure::App {
            code: "NoAttachment".to_owned(),
            fields: json!({ "hash": "0".repeat(64) })
        })
    );
}

#[test]
fn guests_read_but_never_write() {
    let mut t = wiki();
    assert_eq!(
        ok(t.as_("gus").view("page", json!({ "slug": "home" })))["title"],
        json!("Home")
    );
    for (method, args) in [
        ("createPage", json!({ "title": "G", "body": "" })),
        ("saveDraft", json!({ "title": "G", "body": "" })),
        ("deletePage", json!({ "slug": "home" })),
    ] {
        assert_eq!(
            code(t.as_("gus").call(method, args)),
            "Forbidden",
            "{method}"
        );
    }
}
