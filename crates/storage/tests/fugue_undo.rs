//! The primitives an app builds undo from. Undo is local: a delete is undone by
//! re-inserting the text as new characters, never by clearing a tombstone.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_storage::address::Id;
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed, TextOp, Undo};
use calimero_storage::collections::FugueText;
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{
    device, edit, entry_bytes, fork, fugue_genesis, fugue_text_in, land, read_with, written_ids,
    Store,
};

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

/// One whole editor change, so its undo is one call too.
fn delta(store: &Store, writer: u8, ops: &[TextOp]) -> (Vec<Undo>, Vec<u8>) {
    act(store, writer, |doc| doc.apply_delta(ops).unwrap())
}

/// Every entity the given deltas wrote, so the two stores compare row by row.
fn touched(deltas: &[&[u8]]) -> BTreeSet<Id> {
    deltas.iter().flat_map(|d| written_ids(d)).collect()
}

/// A delta and its undo restore the text, but never the bytes: tombstones are
/// monotone and an undone delete mints new characters. What must hold is that
/// the undoing replica and a replica that never edited CONVERGE.
#[test]
fn apply_delta__undo_restores_the_text_and_converges_with_a_pristine_replica() {
    let base = fugue_genesis("delta_undo", "hello world");
    let (alice, bob) = (fork(&base), fork(&base));

    let (steps, applied) = delta(
        &alice,
        ALICE,
        &[
            TextOp::Retain(6),
            TextOp::Delete(5),
            TextOp::Insert("there".to_owned()),
        ],
    );
    assert!(
        matches!(steps.as_slice(), [Undo::Removed(_), Undo::Inserted(_)]),
        "the steps must name the delete then the insert: {steps:?}"
    );
    assert_eq!(fugue_text_in(&alice, device(ALICE)), "hello there");

    let (redo, undone) = act(&alice, ALICE, |doc| doc.undo(&steps).unwrap());
    assert_eq!(fugue_text_in(&alice, device(ALICE)), "hello world");
    assert_eq!(redo.len(), 2, "undoing two steps must redo two");

    let ids = touched(&[&applied, &undone]);
    assert_ne!(
        entry_bytes(&alice, &ids),
        entry_bytes(&bob, &ids),
        "an undo that restored the bytes would mean a tombstone was cleared"
    );

    for delta in [&applied, &undone] {
        land(&bob, device(BOB), delta);
    }
    assert_eq!(fugue_text_in(&bob, device(BOB)), "hello world");
    assert_eq!(
        entry_bytes(&alice, &ids),
        entry_bytes(&bob, &ids),
        "the undoing replica and the one that only received must hold equal bytes"
    );
}

/// Undo a whole change while a peer edits inside one of its halves. `inside_insert`
/// puts the peer's characters in the run the change typed, otherwise in the run it
/// deleted, which the peer reaches only by never having seen the change.
fn undo_across_a_remote_edit(inside_insert: bool, remote_first: bool) -> String {
    let base = fugue_genesis("delta_undo_remote", "hello world");
    let (alice, bob) = (fork(&base), fork(&base));

    // One change that deletes "world" and types "there" in its place.
    let change = [
        TextOp::Retain(6),
        TextOp::Delete(5),
        TextOp::Insert("there".to_owned()),
    ];
    let (steps, applied) = delta(&alice, ALICE, &change);
    if inside_insert {
        land(&bob, device(BOB), &applied);
        assert_eq!(fugue_text_in(&bob, device(BOB)), "hello there");
    }

    // Position 8 is inside "there" once the change has landed, inside "world" if not.
    let (_minted, from_bob) = insert(&bob, BOB, 8, "QQ");
    let (_redo, undone) = act(&alice, ALICE, |doc| doc.undo(&steps).unwrap());

    if remote_first {
        land(&alice, device(ALICE), &from_bob);
        land(&bob, device(BOB), &undone);
    } else {
        land(&bob, device(BOB), &undone);
        land(&alice, device(ALICE), &from_bob);
    }
    if !inside_insert {
        land(&bob, device(BOB), &applied);
        land(&bob, device(BOB), &undone);
    }
    let text = fugue_text_in(&alice, device(ALICE));
    assert_eq!(
        text,
        fugue_text_in(&bob, device(BOB)),
        "the replicas disagree after the undo"
    );
    let ids = touched(&[&applied, &from_bob, &undone]);
    assert_eq!(
        entry_bytes(&alice, &ids),
        entry_bytes(&bob, &ids),
        "the replicas read the same text from different bytes"
    );
    text
}

/// The peer's characters survive the undo, whichever half they were typed in and
/// whichever order the deltas arrive. They end up AFTER the restored run: undoing a
/// delete mints new characters at the gap, so a peer's text anchored in the original
/// run stays anchored to the tombstones, which order before the replacement.
#[test]
fn apply_delta__undo_keeps_a_concurrent_edit_made_inside_the_change() {
    for inside_insert in [true, false] {
        let remote_first = undo_across_a_remote_edit(inside_insert, true);
        let undo_first = undo_across_a_remote_edit(inside_insert, false);
        assert_eq!(
            remote_first, undo_first,
            "inside_insert={inside_insert}: the outcome depends on the delivery order"
        );
        assert_eq!(
            remote_first, "hello worldQQ",
            "inside_insert={inside_insert}: the undo must take back exactly the change \
             and keep the peer's characters"
        );
    }
}

/// Positions are Unicode scalar values, so a delta counts astral characters as one.
#[test]
fn apply_delta__undo_round_trips_a_change_over_astral_characters() {
    let base = fugue_genesis("delta_undo_astral", "\u{1F600}\u{1F601}\u{1F602}");
    let store = fork(&base);

    let (steps, _applied) = delta(
        &store,
        ALICE,
        &[
            TextOp::Retain(1),
            TextOp::Delete(1),
            TextOp::Insert("\u{1F680}\u{0301}".to_owned()),
            TextOp::Retain(1),
            TextOp::Insert("!".to_owned()),
        ],
    );
    assert_eq!(
        fugue_text_in(&store, device(ALICE)),
        "\u{1F600}\u{1F680}\u{0301}\u{1F602}!"
    );
    assert_eq!(steps.len(), 3, "one step per op that changed something");

    let (redo, _undone) = act(&store, ALICE, |doc| doc.undo(&steps).unwrap());
    assert_eq!(
        fugue_text_in(&store, device(ALICE)),
        "\u{1F600}\u{1F601}\u{1F602}"
    );

    let _again = act(&store, ALICE, |doc| doc.undo(&redo).unwrap());
    assert_eq!(
        fugue_text_in(&store, device(ALICE)),
        "\u{1F600}\u{1F680}\u{0301}\u{1F602}!",
        "undoing the undo must redo the change"
    );
}
