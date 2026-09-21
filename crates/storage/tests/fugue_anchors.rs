//! Stable cursors on `FugueText`: an anchor minted on one replica names the same
//! place on every replica once the edits have landed through the apply path.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use calimero_storage::collections::fugue_text::{Anchor, Bias};
use calimero_storage::collections::FugueText;
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{device, edit, fork, fugue_genesis, fugue_text_in, land, read_with, Store};

type Doc = FugueText<MainStorage>;

const ALICE: u8 = 1;
const BOB: u8 = 2;
const PASTE_LEN: usize = 300; // crosses the 256-node run cap

fn anchor_in(store: &Store, pos: usize, bias: Bias) -> Anchor {
    read_with::<Doc, _>(store, device(ALICE), |doc| {
        doc.anchor_at(pos, bias).unwrap()
    })
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
