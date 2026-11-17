//! The forum on the real engine: v1's `TestHost` tests (the forum's, and the noticeboard's
//! and team space's it absorbed), with the storage policies they exercised now the app's
//! own checks.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

/// The id `createPost` mints for `who`'s post `id`.
fn pid(who: &str, id: &str) -> String {
    format!("{who}-{id}")
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

/// Alice files four posts in `dev` and one in `ops`; the owner founded the forum, and
/// `gus` is a guest.
fn forum() -> TestApp {
    let mut t = common::app(
        Org::new()
            .members(["alice", "bob", "mallory"])
            .guests(["gus"]),
    );
    for (id, board, tags, at) in [
        ("p1", "dev", json!(["rust", "wasm"]), 10),
        ("p2", "dev", json!(["rust"]), 20),
        ("p3", "dev", json!(["js"]), 30),
        ("p4", "dev", json!([]), 40),
        ("o1", "ops", json!(["rust"]), 50),
    ] {
        let minted = ok(t.as_("alice").call(
            "createPost",
            json!({ "id": id, "board": board, "title": format!("post {id}"), "body": "",
                    "tags": tags, "createdAt": at.to_string() }),
        ));
        assert_eq!(minted, json!(pid("alice", id)));
    }
    t
}

/// The caller-chosen part of each view's id.
fn ids(views: Value) -> Vec<String> {
    views
        .as_array()
        .expect("a list")
        .iter()
        .map(|v| {
            let id = v["id"].as_str().expect("id");
            id.split_once('-').expect("minted id").1.to_owned()
        })
        .collect()
}

#[test]
fn a_board_pages_newest_first_and_polls_by_time() {
    let mut t = forum();
    let mut page = |offset: u32, limit: u32| {
        ids(ok(t.as_("bob").view(
            "boardFeed",
            json!({ "board": "dev", "offset": offset, "limit": limit }),
        )))
    };
    assert_eq!(page(0, 2), ["p4", "p3"]);
    assert_eq!(page(2, 2), ["p2", "p1"]);
    assert!(page(4, 2).is_empty());
    let since = ids(ok(t
        .as_("bob")
        .view("boardSince", json!({ "board": "dev", "after": "20" }))));
    assert_eq!(since, ["p3", "p4"]);
}

#[test]
fn a_tag_feed_is_scoped_to_its_board() {
    let mut t = forum();
    let rust = |t: &mut TestApp| {
        ids(ok(t.as_("bob").view(
            "boardTagFeed",
            json!({ "board": "dev", "tag": "rust", "offset": 0, "limit": 10 }),
        )))
    };
    // `o1` is tagged `rust` too, but lives in `ops`.
    assert_eq!(rust(&mut t), ["p2", "p1"]);
    ok(t.as_("alice").call(
        "retag",
        json!({ "id": pid("alice", "p3"), "tags": ["rust", "js"] }),
    ));
    ok(t.as_("alice").call(
        "retag",
        json!({ "id": pid("alice", "p1"), "tags": ["wasm"] }),
    ));
    assert_eq!(rust(&mut t), ["p3", "p2"]);
}

#[test]
fn pins_and_counts_come_from_the_index() {
    let mut t = forum();
    ok(t.as_("alice")
        .call("pin", json!({ "id": pid("alice", "p1"), "at": "100" })));
    ok(t.as_("alice")
        .call("pin", json!({ "id": pid("alice", "p3"), "at": "200" })));
    assert_eq!(
        ids(ok(t.as_("bob").view("pinned", json!({ "board": "dev" })))),
        ["p3", "p1"]
    );
    ok(t.as_("alice")
        .call("unpin", json!({ "id": pid("alice", "p3") })));
    ok(t.as_("alice")
        .call("deletePost", json!({ "id": pid("alice", "p4") })));
    assert_eq!(
        ok(t.as_("bob").view("boardStats", json!({ "board": "dev" }))),
        json!({ "posts": "3", "pinned": "1" })
    );
    assert_eq!(
        ids(ok(t.as_("bob").view(
            "authorFeed",
            json!({ "author": "alice", "limit": 2 })
        ))),
        ["o1", "p3"]
    );
}

#[test]
fn the_charter_is_written_at_init_and_the_owner_moderates() {
    let mut t = forum();
    assert_eq!(
        ok(t.as_("bob").view("charter", json!({}))),
        json!({ "name": "calimero forum", "rules": "be kind and stay on topic" })
    );
    assert_eq!(
        ok(t.as_("bob").view("moderators", json!({}))),
        json!(["owner"]),
        "whoever ran init founded the forum and is the first moderator"
    );
}

#[test]
fn only_a_moderator_appoints_one_and_the_appointee_moderates() {
    let mut t = forum();
    assert_eq!(
        t.as_("bob")
            .call("addModerator", json!({ "account": "bob" })),
        Err(Failure::App {
            code: "NotModerator".to_owned(),
            fields: json!({ "id": "bob" })
        })
    );
    ok(t.as_("owner")
        .call("addModerator", json!({ "account": "bob" })));
    assert_eq!(
        ok(t.as_("alice").view("moderators", json!({}))),
        json!(["bob", "owner"])
    );
    ok(t.as_("bob")
        .call("moderatePost", json!({ "id": pid("alice", "p1") })));
    ok(t.as_("alice").call(
        "announce",
        json!({ "id": "a1", "text": "x", "sentAt": "1" }),
    ));
    assert_eq!(
        ok(t.as_("bob")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true),
        "an appointed moderator takes announcements down too"
    );
}

#[test]
fn guests_read_but_may_not_write() {
    let mut t = forum();
    let p1 = pid("alice", "p1");
    for (method, args) in [
        (
            "createPost",
            json!({ "id": "g1", "board": "dev", "title": "", "body": "", "tags": [],
                    "createdAt": "1" }),
        ),
        ("vote", json!({ "post": p1 })),
        (
            "comment",
            json!({ "post": p1, "id": "c1", "body": "", "createdAt": "1" }),
        ),
        (
            "postMessage",
            json!({ "board": "dev", "id": "m1", "text": "", "sentAt": "1" }),
        ),
        ("announce", json!({ "id": "a1", "text": "", "sentAt": "1" })),
    ] {
        assert_eq!(
            code(t.as_("gus").call(method, args)),
            "Forbidden",
            "{method}"
        );
    }
    assert_eq!(
        ok(t.as_("gus").view("getPost", json!({ "id": p1 })))["author"],
        json!("alice")
    );
}

/// A member may fill a board, and nobody else may empty it; another board's read is one
/// range of the index all the same, so the flood costs its readers nothing.
#[test]
fn a_board_read_is_unmoved_by_a_flood_in_another_board() {
    let mut t = forum();
    for i in 0..300 {
        ok(t.as_("mallory").call(
            "createPost",
            json!({ "id": format!("s{i}"), "board": "spam", "title": "", "body": "",
                    "tags": [], "createdAt": i.to_string() }),
        ));
    }
    assert_eq!(
        ok(t.as_("bob").view("boardStats", json!({ "board": "spam" })))["posts"],
        json!("300")
    );
    assert_eq!(
        ids(ok(t.as_("bob").view(
            "boardFeed",
            json!({ "board": "ops", "offset": 0, "limit": 10 })
        ))),
        ["o1"]
    );
    assert_eq!(
        code(
            t.as_("bob")
                .call("deletePost", json!({ "id": pid("mallory", "s0") }))
        ),
        "NotAuthor",
        "only its author or a moderator removes it"
    );
}

#[test]
fn a_moderator_removes_anyone_s_post_and_nobody_else_can() {
    let mut t = forum();
    let p1 = pid("alice", "p1");
    assert_eq!(
        code(t.as_("bob").call("moderatePost", json!({ "id": p1 }))),
        "NotModerator"
    );
    assert_eq!(
        code(t.as_("alice").call("moderatePost", json!({ "id": p1 }))),
        "NotModerator",
        "authoring a post does not make you a moderator"
    );
    ok(t.as_("owner").call("moderatePost", json!({ "id": p1 })));
    assert_eq!(
        ok(t.as_("bob").view("getPost", json!({ "id": p1 }))),
        json!(null)
    );
    assert_eq!(
        ok(t.as_("bob").view("boardStats", json!({ "board": "dev" })))["posts"],
        json!("3"),
        "the board's count follows the removal"
    );
    assert_eq!(
        code(t.as_("owner").call("moderatePost", json!({ "id": p1 }))),
        "NoPost"
    );
}

#[test]
fn only_the_author_may_change_a_post() {
    let mut t = forum();
    let p1 = pid("alice", "p1");
    for (method, args) in [
        ("pin", json!({ "id": p1, "at": "1" })),
        ("retag", json!({ "id": p1, "tags": [] })),
        ("editPost", json!({ "id": p1, "title": "mine", "body": "" })),
        ("deletePost", json!({ "id": p1 })),
    ] {
        assert_eq!(
            code(t.as_("bob").call(method, args)),
            "NotAuthor",
            "{method}"
        );
    }
    assert_eq!(
        t.as_("alice").call(
            "createPost",
            json!({ "id": "p1", "board": "dev", "title": "again", "body": "", "tags": [],
                    "createdAt": "1" }),
        ),
        Err(Failure::App {
            code: "Exists".to_owned(),
            fields: json!({ "id": p1 })
        })
    );
    assert_eq!(
        code(t.as_("alice").call(
            "createPost",
            json!({ "id": "a/b", "board": "dev", "title": "", "body": "", "tags": [],
                    "createdAt": "1" }),
        )),
        "HasSeparator"
    );
    assert_eq!(
        ok(t.as_("bob").view("boardStats", json!({ "board": "dev" }))),
        json!({ "posts": "4", "pinned": "0" })
    );
    ok(t.as_("alice").call(
        "editPost",
        json!({ "id": p1, "title": "edited", "body": "b" }),
    ));
    let post = ok(t.as_("bob").view("getPost", json!({ "id": p1 })));
    assert_eq!(
        (&post["title"], &post["body"]),
        (&json!("edited"), &json!("b"))
    );
}

/// Bob's `p1` is his own post beside Alice's, and he reads, votes on and comments on hers.
#[test]
fn another_account_reads_and_joins_someone_s_post() {
    let mut t = forum();
    let p1 = pid("alice", "p1");
    let bobs = ok(t.as_("bob").call(
        "createPost",
        json!({ "id": "p1", "board": "dev", "title": "bob's own", "body": "", "tags": [],
                "createdAt": "1" }),
    ));
    assert_eq!(bobs, json!(pid("bob", "p1")));
    let post = ok(t.as_("bob").view("getPost", json!({ "id": p1 })));
    assert_eq!(
        (&post["author"], &post["title"]),
        (&json!("alice"), &json!("post p1"))
    );
    assert_eq!(
        ok(t.as_("bob")
            .view("authorFeed", json!({ "author": "alice", "limit": 10 })))
        .as_array()
        .map(Vec::len),
        Some(5)
    );
    assert_eq!(
        ok(t.as_("bob").call("vote", json!({ "post": p1 }))),
        json!("1")
    );
    ok(t.as_("bob").call(
        "comment",
        json!({ "post": p1, "id": "c1", "body": "nice", "createdAt": "5" }),
    ));
    let thread = ok(t
        .as_("alice")
        .view("comments", json!({ "post": p1, "offset": 0, "limit": 10 })));
    assert_eq!(thread[0]["author"], json!("bob"));
    assert_eq!(thread.as_array().map(Vec::len), Some(1));
}

#[test]
fn a_vote_is_its_voter_s_own_row() {
    let mut t = forum();
    let p1 = pid("alice", "p1");
    let mut vote = |who: &str, method: &str| ok(t.as_(who).call(method, json!({ "post": p1 })));
    assert_eq!(vote("alice", "vote"), json!("1"));
    assert_eq!(
        vote("alice", "vote"),
        json!("1"),
        "voting twice is one vote"
    );
    assert_eq!(vote("bob", "vote"), json!("2"));
    assert_eq!(vote("alice", "unvote"), json!("1"));
    assert_eq!(vote("alice", "vote"), json!("2"));
    assert_eq!(
        vote("bob", "unvote"),
        json!("1"),
        "bob takes back his own only"
    );
    assert_eq!(
        code(t.as_("bob").call("vote", json!({ "post": "nope" }))),
        "NoPost"
    );
    let post = ok(t.as_("alice").view("getPost", json!({ "id": p1 })));
    assert_eq!(
        (&post["votes"], &post["votedByMe"]),
        (&json!("1"), &json!(true))
    );
    let post = ok(t.as_("bob").view("getPost", json!({ "id": p1 })));
    assert_eq!(post["votedByMe"], json!(false));
}

#[test]
fn a_thread_is_one_post_s_comments_in_time_order() {
    let mut t = forum();
    let (p1, p2) = (pid("alice", "p1"), pid("alice", "p2"));
    let late = ok(t.as_("bob").call(
        "comment",
        json!({ "post": p1, "id": "c1", "body": "second", "createdAt": "200" }),
    ));
    ok(t.as_("alice").call(
        "comment",
        json!({ "post": p1, "id": "c1", "body": "first", "createdAt": "100" }),
    ));
    ok(t.as_("alice").call(
        "comment",
        json!({ "post": p2, "id": "c1", "body": "elsewhere", "createdAt": "150" }),
    ));
    assert_eq!(
        code(t.as_("alice").call(
            "comment",
            json!({ "post": "nope", "id": "c1", "body": "lost", "createdAt": "1" }),
        )),
        "NoPost"
    );
    let thread = ok(t
        .as_("bob")
        .view("comments", json!({ "post": p1, "offset": 0, "limit": 10 })));
    let bodies: Vec<_> = thread
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].clone())
        .collect();
    assert_eq!(bodies, [json!("first"), json!("second")]);
    assert_eq!(thread[1]["author"], json!("bob"));
    assert_eq!(thread[1]["createdAt"], json!("200"));
    assert_eq!(
        ok(t.as_("bob").view("getPost", json!({ "id": p1 })))["comments"],
        json!("2")
    );

    // Alice cannot touch Bob's comment; Bob can.
    let none = Err(Failure::App {
        code: "NoComment".to_owned(),
        fields: json!({ "key": late }),
    });
    assert_eq!(
        t.as_("alice")
            .call("editComment", json!({ "key": late, "body": "mine now" })),
        none
    );
    assert_eq!(
        t.as_("alice").call("deleteComment", json!({ "key": late })),
        none
    );
    ok(t.as_("bob")
        .call("editComment", json!({ "key": late, "body": "edited" })));
    ok(t.as_("bob").call("deleteComment", json!({ "key": late })));
    assert_eq!(
        ok(t.as_("bob")
            .view("comments", json!({ "post": p1, "offset": 0, "limit": 10 }),))
        .as_array()
        .map(Vec::len),
        Some(1)
    );
}

#[test]
fn a_chat_message_is_per_author_and_final() {
    let mut t = forum();
    let say = |t: &mut TestApp, who: &str, text: &str, at: &str| {
        t.as_(who).call(
            "postMessage",
            json!({ "board": "dev", "id": "m1", "text": text, "sentAt": at }),
        )
    };
    assert_eq!(ok(say(&mut t, "alice", "hello", "1")), json!("dev/m1"));
    assert_eq!(
        ok(say(&mut t, "bob", "bob's own", "2")),
        json!("dev/m1"),
        "the same key is a separate row for another account"
    );
    assert_eq!(
        say(&mut t, "alice", "edited", "3"),
        Err(Failure::App {
            code: "Exists".to_owned(),
            fields: json!({ "id": "dev/m1" })
        }),
        "its author cannot rewrite it: it was written once"
    );
    ok(t.as_("owner").call(
        "postMessage",
        json!({ "board": "ops", "id": "m2", "text": "elsewhere", "sentAt": "0" }),
    ));
    assert_eq!(
        code(t.as_("owner").call(
            "postMessage",
            json!({ "board": "d/ev", "id": "x", "text": "", "sentAt": "0" }),
        )),
        "HasSeparator"
    );
    assert_eq!(
        ok(t.as_("bob").view("messages", json!({ "board": "dev" }))),
        json!([
            { "key": "dev/m1", "author": "alice", "text": "hello", "sentAt": "1" },
            { "key": "dev/m1", "author": "bob", "text": "bob's own", "sentAt": "2" },
        ]),
        "one board's range, in time order, neither row touched"
    );
}

#[test]
fn only_a_moderator_removes_an_announcement_not_its_author() {
    let mut t = forum();
    for (who, text) in [("alice", "offsite friday"), ("bob", "from bob")] {
        ok(t.as_(who).call(
            "announce",
            json!({ "id": "a1", "text": text, "sentAt": "1" }),
        ));
    }
    assert_eq!(
        code(t.as_("alice").call(
            "announce",
            json!({ "id": "a1", "text": "moved", "sentAt": "2" }),
        )),
        "Exists",
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
        ok(t.as_("bob").view("announcements", json!({}))),
        json!([
            { "key": "a1", "author": "alice", "text": "offsite friday", "sentAt": "1" },
            { "key": "a1", "author": "bob", "text": "from bob", "sentAt": "1" },
        ])
    );
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(true),
        "every author's announcement at the id goes"
    );
    assert_eq!(ok(t.as_("bob").view("announcements", json!({}))), json!([]));
    assert_eq!(
        ok(t.as_("owner")
            .call("removeAnnouncement", json!({ "id": "a1" }))),
        json!(false),
        "nothing left"
    );
}
