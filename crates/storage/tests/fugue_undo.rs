//! The primitives an app builds undo from. Undo is local: a delete is undone by
//! re-inserting the text as new characters, never by clearing a tombstone.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed};
use calimero_storage::collections::FugueText;
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{device, edit, fork, fugue_genesis, fugue_text_in, land, read_with, Store};

type Doc = FugueText<MainStorage>;

const ALICE: u8 = 1;
const BOB: u8 = 2;
const PASTE_LEN: usize = 300; // crosses the 256-node run cap

/// Runs `f` as `writer`, returning what it returned and the delta it committed.
fn act<R>(store: &Store, writer: u8, f: impl FnOnce(&mut Doc) -> R) -> (R, Vec<u8>) {
    let mut out = None;
    let delta = edit::<Doc>(store, device(writer), |doc| out = Some(f(doc)));
    (out.unwrap(), delta)
}

fn insert(store: &Store, writer: u8, pos: usize, text: &str) -> (Option<IdRange>, Vec<u8>) {
    act(store, writer, |doc| {
        doc.insert_str_with_replica(pos, u64::from(writer), text)
            .unwrap()
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
fn undo_insert__removes_exactly_the_minted_characters() {
    let base = fugue_genesis("undo_insert", "ab");
    let (alice, bob) = (fork(&base), fork(&base));

    let (minted, typed) = insert(&alice, ALICE, 1, "XYZ");
    let minted = minted.unwrap();
    assert_eq!(
        minted,
        IdRange {
            start: (u64::from(ALICE), 0),
            len: 3
        }
    );
    land(&bob, device(ALICE), &typed);

    // Bob types inside Alice's text before her undo reaches him.
    let (_, inside) = insert(&bob, BOB, 3, "q");
    let (removed, undone) = act(&alice, ALICE, |doc| doc.delete_ids(&minted).unwrap());
    assert_eq!(removed.unwrap().text, "XYZ");

    assert_eq!(sync([&alice, &bob], &[inside, undone]), "aqb");
}

#[test]
fn undo_delete__reinserts_as_new_characters_where_the_text_was() {
    let base = fugue_genesis("undo_delete", "hello world");
    let (alice, bob) = (fork(&base), fork(&base));

    let (removed, deleted) = act(&alice, ALICE, |doc| doc.delete_range(6, 11).unwrap());
    let removed = removed.unwrap();
    assert_eq!(
        removed,
        Removed {
            text: "world".to_owned(),
            anchor: Anchor::Char {
                id: (0, 6),
                bias: Bias::Before
            },
        }
    );
    let (_, before) = insert(&bob, BOB, 0, "AA");
    assert_eq!(sync([&alice, &bob], &[deleted, before]), "AAhello ");

    let (restored, undone) = act(&alice, ALICE, |doc| {
        doc.insert_str_at(&removed.anchor, &removed.text).unwrap()
    });
    assert_eq!(sync([&alice, &bob], &[undone]), "AAhello world");

    // New ids under the undoing replica: the originals stay tombstoned.
    let restored = restored.unwrap();
    assert_eq!((restored.start.0, restored.len), (u64::from(ALICE), 5));
    let still_dead = IdRange {
        start: (0, 6),
        len: 5,
    };
    let (again, _) = act(&alice, ALICE, |doc| doc.delete_ids(&still_dead).unwrap());
    assert_eq!(again, None);
}

#[test]
fn redo__each_primitive_returns_what_its_inverse_takes() {
    let base = fugue_genesis("undo_redo", "[]");
    let store = fork(&base);

    let (minted, _) = insert(&store, ALICE, 1, "text");
    let (removed, _) = act(&store, ALICE, |doc| {
        doc.delete_ids(&minted.unwrap()).unwrap()
    });
    let removed = removed.unwrap();
    assert_eq!(fugue_text_in(&store, device(ALICE)), "[]");

    let (again, _) = act(&store, ALICE, |doc| {
        doc.insert_str_at(&removed.anchor, &removed.text).unwrap()
    });
    assert_eq!(fugue_text_in(&store, device(ALICE)), "[text]");

    let (removed, _) = act(&store, ALICE, |doc| {
        doc.delete_ids(&again.unwrap()).unwrap()
    });
    assert_eq!(removed.unwrap().text, "text");
    assert_eq!(fugue_text_in(&store, device(ALICE)), "[]");
}

#[test]
fn undo_insert__reaches_across_the_run_cap() {
    let base = fugue_genesis("undo_cap", "[]");
    let store = fork(&base);
    let paste: String = ('a'..='z').cycle().take(PASTE_LEN).collect();

    let (minted, _) = insert(&store, ALICE, 1, &paste);
    let minted = minted.unwrap();
    assert_eq!(minted.len as usize, PASTE_LEN);

    let (removed, _) = act(&store, ALICE, |doc| doc.delete_ids(&minted).unwrap());
    assert_eq!(removed.unwrap().text, paste);
    assert_eq!(fugue_text_in(&store, device(ALICE)), "[]");
}

#[test]
fn edits_that_change_nothing_return_nothing_to_undo() {
    let store = fork(&fugue_genesis("undo_empty", "ab"));
    let unknown = IdRange {
        start: (77, 0),
        len: 4,
    };
    let (nothing, _) = act(&store, ALICE, |doc| {
        let inserted = doc.insert_str_with_replica(1, 1, "").unwrap();
        let ranged = doc.delete_range(1, 1).unwrap();
        let by_id = doc.delete_ids(&unknown).unwrap();
        // The harness requires the commit to emit a delta.
        let _real = doc.insert_str_with_replica(0, 1, "x").unwrap();
        (inserted, ranged, by_id)
    });
    assert_eq!(nothing, (None, None, None));
    read_with::<Doc, _>(&store, device(ALICE), |doc| {
        assert_eq!(doc.get_text().unwrap(), "xab");
    });
}

/// Apps persist undo stacks, so the encoding is a format.
#[test]
fn id_range__borsh_encoding_is_pinned() {
    let range = IdRange {
        start: (1, 2),
        len: 3,
    };
    let bytes = borsh::to_vec(&range).unwrap();
    assert_eq!(bytes, [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
    assert_eq!(borsh::from_slice::<IdRange>(&bytes).unwrap(), range);
}
