//! Stable cursors on `FugueText`: an anchor minted on one replica names the same
//! place on every replica once the edits have landed through the apply path.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::rc::Rc;

use calimero_sdk::serde_json::{from_str, to_string};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed, Undo};
use calimero_storage::collections::FugueText;
use calimero_storage::env;
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{
    counting_env_for, device, edit, fork, fugue_genesis, fugue_text_in, land, read_with, Store,
};

type Doc = FugueText<MainStorage>;

const ALICE: u8 = 1;
const BOB: u8 = 2;
const PASTE_LEN: usize = 300; // crosses the 256-node run cap

fn anchor_in(store: &Store, pos: usize, bias: Bias) -> Anchor {
    read_with::<Doc, _>(store, device(ALICE), |doc| {
        doc.anchor_at(pos, bias).unwrap()
    })
}

/// Runs `read` against `store`, returning what it returned and the host reads it cost.
fn counted<R>(store: &Store, read: impl FnOnce(&Doc) -> R) -> (R, usize) {
    let reads = Rc::new(Cell::new(0));
    let out = env::with_runtime_env(counting_env_for(store, device(ALICE), &reads), || {
        let doc = calimero_storage::collections::Root::<Doc>::fetch().unwrap();
        reads.set(0);
        read(&doc)
    });
    (out, reads.get())
}

fn resolve_in(store: &Store, anchor: &Anchor) -> usize {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.resolve(anchor).unwrap())
}

fn insert(store: &Store, writer: u8, pos: usize, text: &str) -> Vec<u8> {
    edit::<Doc>(store, device(writer), |doc| {
        doc.insert_str_with_replica(pos, u64::from(writer), text)
            .unwrap();
    })
}

fn delete(store: &Store, writer: u8, start: usize, end: usize) -> Vec<u8> {
    edit::<Doc>(store, device(writer), |doc| {
        doc.delete_range(start, end).unwrap();
    })
}

/// Lands every delta on both stores and returns the text they agree on.
fn sync(stores: [&Store; 2], deltas: &[Vec<u8>]) -> String {
    for store in stores {
        for delta in deltas {
            land(store, device(ALICE), delta);
        }
    }
    let text = fugue_text_in(stores[0], device(ALICE));
    assert_eq!(text, fugue_text_in(stores[1], device(ALICE)));
    text
}

#[test]
fn anchor__holds_its_place_under_concurrent_inserts_before_and_after() {
    let base = fugue_genesis("anchor_concurrent", "hello world");
    let (alice, bob) = (fork(&base), fork(&base));
    let before_w = anchor_in(&alice, 6, Bias::Before);
    let after_o = anchor_in(&alice, 5, Bias::After);

    let deltas = [
        insert(&alice, ALICE, 0, "ZZ"),
        insert(&bob, BOB, 0, "XX"),
        insert(&bob, BOB, 13, "YY"),
    ];
    let text = sync([&alice, &bob], &deltas);
    assert_eq!(text, "ZZXXhello worldYY");

    for store in [&alice, &bob] {
        assert_eq!(resolve_in(store, &before_w), 10);
        assert_eq!(resolve_in(store, &after_o), 9);
    }
    assert_eq!(text.chars().nth(10), Some('w'));
    assert_eq!(text.chars().nth(8), Some('o'));
}

#[test]
fn anchor__bias_decides_which_side_of_an_insert_at_its_gap_it_lands() {
    let base = fugue_genesis("anchor_bias", "ab");
    let (alice, bob) = (fork(&base), fork(&base));
    let after_a = anchor_in(&alice, 1, Bias::After);
    let before_b = anchor_in(&alice, 1, Bias::Before);

    let text = sync([&alice, &bob], &[insert(&bob, BOB, 1, "X")]);
    assert_eq!(text, "aXb");

    assert_eq!(resolve_in(&alice, &after_a), 1);
    assert_eq!(resolve_in(&alice, &before_b), 2);
}

#[test]
fn anchor__on_a_deleted_character_resolves_to_the_gap_it_left() {
    let base = fugue_genesis("anchor_deleted", "abc");
    let (alice, bob) = (fork(&base), fork(&base));
    let before_b = anchor_in(&alice, 1, Bias::Before);
    let after_b = anchor_in(&alice, 2, Bias::After);

    let deltas = [
        delete(&bob, BOB, 1, 2),
        insert(&alice, ALICE, 1, "P"),
        insert(&alice, ALICE, 3, "Q"),
    ];
    let text = sync([&alice, &bob], &deltas);
    assert_eq!(text, "aPQc");

    for store in [&alice, &bob] {
        assert_eq!(resolve_in(store, &before_b), 2);
        assert_eq!(resolve_in(store, &after_b), 2);
    }
}

#[test]
fn anchor__addresses_nodes_on_both_sides_of_the_run_cap() {
    let base = fugue_genesis("anchor_cap", "");
    let (alice, bob) = (fork(&base), fork(&base));
    let paste: String = ('a'..='z').cycle().take(PASTE_LEN).collect();
    let pasted = insert(&alice, ALICE, 0, &paste);
    land(&bob, device(ALICE), &pasted);

    let last_of_full_run = anchor_in(&alice, 256, Bias::After);
    let first_past_cap = anchor_in(&alice, 256, Bias::Before);
    let replica = u64::from(ALICE);
    assert_eq!(
        last_of_full_run,
        Anchor::Char {
            id: (replica, 255),
            bias: Bias::After
        }
    );
    assert_eq!(
        first_past_cap,
        Anchor::Char {
            id: (replica, 256),
            bias: Bias::Before
        }
    );

    let deltas = [
        insert(&bob, BOB, 0, "0123456789"),
        delete(&bob, BOB, 100, 105),
    ];
    let text = sync([&alice, &bob], &deltas);
    assert_eq!(text.chars().count(), PASTE_LEN + 5);

    for store in [&alice, &bob] {
        assert_eq!(resolve_in(store, &last_of_full_run), 261);
        assert_eq!(resolve_in(store, &first_past_cap), 261);
    }
    assert_eq!(text.chars().nth(261), paste.chars().nth(256));
}

#[test]
fn anchor__document_edges_stay_at_the_edges() {
    let base = fugue_genesis("anchor_edges", "mid");
    let (alice, bob) = (fork(&base), fork(&base));
    let start = anchor_in(&alice, 0, Bias::After);
    let end = anchor_in(&alice, 3, Bias::Before);
    assert_eq!((start, end), (Anchor::Start, Anchor::End));

    let deltas = [insert(&bob, BOB, 0, "<<"), insert(&bob, BOB, 5, ">>")];
    assert_eq!(sync([&alice, &bob], &deltas), "<<mid>>");

    assert_eq!(resolve_in(&alice, &start), 0);
    assert_eq!(resolve_in(&alice, &end), 7);
}

#[test]
fn anchor__rejects_a_position_past_the_end_and_an_unknown_character() {
    let base = fugue_genesis("anchor_errors", "ab");
    read_with::<Doc, _>(&base, device(ALICE), |doc| {
        assert!(doc.anchor_at(3, Bias::Before).is_err());
        assert!(doc.anchor_at(3, Bias::After).is_err());
        let unknown = Anchor::Char {
            id: (99, 0),
            bias: Bias::Before,
        };
        assert!(doc.resolve(&unknown).is_err());
    });
}

/// Apps persist and share anchors, so the encoding is a format.
#[test]
fn anchor__borsh_encoding_is_pinned() {
    let anchor = Anchor::Char {
        id: (1, 2),
        bias: Bias::After,
    };
    let bytes = borsh::to_vec(&anchor).unwrap();
    assert_eq!(bytes, [2, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1]);
    assert_eq!(borsh::from_slice::<Anchor>(&bytes).unwrap(), anchor);
    assert_eq!(borsh::to_vec(&Anchor::Start).unwrap(), [0]);
    assert_eq!(borsh::to_vec(&Anchor::End).unwrap(), [1]);
}

/// Apps ship anchors over JSON-RPC, so the JSON is a format too. A `RawId` is
/// the two-element array `[replica, counter]`.
#[test]
fn editor_types__json_encoding_is_pinned() {
    let anchor = Anchor::Char {
        id: (1, 2),
        bias: Bias::After,
    };
    let range = IdRange {
        start: (1, 2),
        len: 3,
    };
    let removed = Removed {
        text: "ab".to_owned(),
        anchor,
        ids: vec![range],
    };

    let json: Vec<(String, &str)> = vec![
        (to_string(&Bias::Before).unwrap(), r#""Before""#),
        (to_string(&Bias::After).unwrap(), r#""After""#),
        (to_string(&Anchor::Start).unwrap(), r#""Start""#),
        (to_string(&Anchor::End).unwrap(), r#""End""#),
        (
            to_string(&anchor).unwrap(),
            r#"{"Char":{"id":[1,2],"bias":"After"}}"#,
        ),
        (to_string(&range).unwrap(), r#"{"start":[1,2],"len":3}"#),
        (
            to_string(&removed).unwrap(),
            r#"{"text":"ab","anchor":{"Char":{"id":[1,2],"bias":"After"}},"ids":[{"start":[1,2],"len":3}]}"#,
        ),
        (
            to_string(&Undo::Inserted(range)).unwrap(),
            r#"{"Inserted":{"start":[1,2],"len":3}}"#,
        ),
        (
            to_string(&Undo::Removed(removed.clone())).unwrap(),
            r#"{"Removed":{"text":"ab","anchor":{"Char":{"id":[1,2],"bias":"After"}},"ids":[{"start":[1,2],"len":3}]}}"#,
        ),
    ];
    for (got, want) in json {
        assert_eq!(got, want);
    }

    assert_eq!(from_str::<Anchor>(r#""Start""#).unwrap(), Anchor::Start);
    assert_eq!(
        from_str::<Undo>(
            r#"{"Removed":{"text":"ab","anchor":{"Char":{"id":[1,2],"bias":"After"}},"ids":[{"start":[1,2],"len":3}]}}"#
        )
        .unwrap(),
        Undo::Removed(removed)
    );
}

/// Rendering a document resolves every cursor and mark, so the rebuild is paid once.
#[test]
fn resolve_many__equals_resolve_over_the_slice_for_one_rebuild() {
    let base = fugue_genesis("resolve_many", "hello world");
    let store = fork(&base);
    let _deleted = delete(&store, ALICE, 4, 5);
    assert_eq!(fugue_text_in(&store, device(ALICE)), "hell world");

    let tombstoned = Anchor::Char {
        id: (0, 4), // the 'o' the delete took: seeded under replica 0
        bias: Bias::After,
    };
    let anchors = [
        Anchor::Start,
        Anchor::End,
        anchor_in(&store, 2, Bias::Before),
        anchor_in(&store, 7, Bias::After),
        tombstoned,
    ];

    let one_by_one: Vec<usize> = anchors.iter().map(|a| resolve_in(&store, a)).collect();
    let (batched, reads) = counted(&store, |doc| doc.resolve_many(&anchors).unwrap());
    assert_eq!(batched, one_by_one);
    assert_eq!(batched, vec![0, 10, 2, 7, 4]);

    let (_one, single_reads) = counted(&store, |doc| doc.resolve(&Anchor::Start).unwrap());
    assert_eq!(
        reads,
        single_reads,
        "resolving {} anchors must cost what resolving one costs",
        anchors.len()
    );
}

/// An unknown id must fail the batch exactly as it fails a single resolve.
#[test]
fn resolve_many__fails_on_an_unknown_character_like_resolve_does() {
    let base = fugue_genesis("resolve_many_unknown", "ab");
    let unknown = Anchor::Char {
        id: (99, 0),
        bias: Bias::Before,
    };
    read_with::<Doc, _>(&base, device(ALICE), |doc| {
        let single = doc.resolve(&unknown).unwrap_err().to_string();
        let batched = doc
            .resolve_many(&[Anchor::Start, unknown, Anchor::End])
            .unwrap_err()
            .to_string();
        assert_eq!(batched, single);
    });
}

/// An app event is recorded in the delta and re-emitted on the RECEIVING node, so
/// a position the author computed names the wrong place wherever a peer edited
/// first. The ids a write returns do not move.
#[test]
fn an_events_ids_place_correctly_on_a_replica_that_edited_first() {
    let base = fugue_genesis("event_ids", "hello world");
    let (alice, bob) = (fork(&base), fork(&base));

    // Bob types BEFORE everything Alice is about to touch, without telling her.
    let before = insert(&bob, BOB, 0, ">>> ");
    assert_eq!(fugue_text_in(&bob, device(BOB)), ">>> hello world");

    // Alice replaces "world": position 6 on her replica, 10 on Bob's.
    let mut minted = None;
    let typed = edit::<Doc>(&alice, device(ALICE), |doc| {
        let _removed = doc.delete_range(6, 11).unwrap();
        minted = doc
            .insert_str_with_replica(6, u64::from(ALICE), "there")
            .unwrap();
    });
    let minted = minted.unwrap();
    assert_eq!(fugue_text_in(&alice, device(ALICE)), "hello there");

    assert_eq!(sync([&alice, &bob], &[before, typed]), ">>> hello there");

    // The payload a position-based event would have carried.
    const AUTHORS_POSITION: usize = 6;
    let ids = [Anchor::Char {
        id: minted.start,
        bias: Bias::Before,
    }];
    for store in [&alice, &bob] {
        let placed =
            read_with::<Doc, _>(store, device(ALICE), |doc| doc.resolve_many(&ids).unwrap());
        assert_eq!(
            placed,
            vec![10],
            "the typed run sits at 10 on both replicas"
        );
    }
    assert_eq!(
        read_with::<Doc, _>(&bob, device(BOB), |doc| doc
            .text_range(AUTHORS_POSITION, AUTHORS_POSITION + 5)
            .unwrap()),
        "llo t",
        "the author's position names someone else's text on the receiving replica"
    );
}

fn visible_ids_in(store: &Store) -> Vec<IdRange> {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.visible_ids().unwrap())
}

#[test]
fn visible_ids__one_writers_typing_is_one_run() {
    let alice = fugue_genesis("ids_typing", "");
    let _delta = insert(&alice, ALICE, 0, "hello");
    let _delta = insert(&alice, ALICE, 5, " you");
    assert_eq!(
        visible_ids_in(&alice),
        vec![IdRange {
            start: (u64::from(ALICE), 0),
            len: 9
        }]
    );
}

#[test]
fn visible_ids__a_delete_splits_the_run_around_its_gap() {
    let alice = fugue_genesis("ids_delete", "abcdef");
    let _delta = delete(&alice, ALICE, 2, 4);
    assert_eq!(
        visible_ids_in(&alice),
        vec![
            IdRange {
                start: (0, 0),
                len: 2
            },
            IdRange {
                start: (0, 4),
                len: 2
            },
        ]
    );
}

/// Both writers type the same characters, so only the ids say whose is whose.
#[test]
fn visible_ids__agree_across_replicas_and_tell_identical_typing_apart() {
    const SEED: &str = "--";
    const TYPED: &str = "xyx"; // what each writer types, in counter order
    let base = fugue_genesis("ids_concurrent", SEED);
    let (alice, bob) = (fork(&base), fork(&base));
    let deltas = [
        insert(&alice, ALICE, 1, "xy"),
        insert(&bob, BOB, 1, "xy"),
        insert(&alice, ALICE, 0, "x"),
        insert(&bob, BOB, 4, "x"),
        delete(&alice, ALICE, 1, 2),
    ];
    let text = sync([&alice, &bob], &deltas);

    let runs = visible_ids_in(&alice);
    assert_eq!(runs, visible_ids_in(&bob));
    let named: String = runs
        .iter()
        .flat_map(|run| (run.start.1..run.start.1 + run.len).map(move |c| (run.start.0, c)))
        .map(|(replica, counter)| {
            let log = if replica == 0 { SEED } else { TYPED };
            log.chars().nth(counter as usize).unwrap()
        })
        .collect();
    assert_eq!(named, text);
    for writer in [ALICE, BOB] {
        let first_xy = IdRange {
            start: (u64::from(writer), 0),
            len: 2,
        };
        assert!(runs.contains(&first_xy), "{writer}'s xy in {runs:?}");
    }
}
