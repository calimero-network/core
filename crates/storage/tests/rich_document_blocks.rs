//! `RichDocument` block behaviour through the real apply path: every structure
//! operation, every concurrency pair in the design, the split anomaly, and the
//! nested `UnorderedMap<DocId, RichDocument>` shape an app actually stores.
//!
//! Convergence is asserted on three things at once - the rendered blocks, the
//! stored row bytes, and the Merkle root - because a pair that agrees on what it
//! renders and not on what it stores is still diverged.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use calimero_storage::address::Id;
use calimero_storage::collections::{
    BlockId, BlockView, DefaultMarks, DeltaOp, RichDocument, Span, UnorderedMap,
};
use calimero_storage::env;
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{
    device, edit, entry_bytes, env_for, fork, genesis, land, read_with, try_edit, written_ids,
    Store,
};

const ALICE: u8 = 1;
const BOB: u8 = 2;
const PARAGRAPH: &str = "paragraph";
const DOC_KEY: &str = "notes"; // the nested shape's document key
const ABSENT: BlockId = BlockId((0xAB, 3)); // names a spine character nobody minted

type Doc = RichDocument<DefaultMarks, MainStorage>;
type Library = UnorderedMap<String, Doc, MainStorage>;

// ── harness ─────────────────────────────────────────────────────────────

fn empty_doc() -> Store {
    genesis(|| Doc::new_with_field_name("doc"), |_doc| {})
}

/// A document with `kinds.len()` blocks in order, each carrying its own text.
fn seeded(kinds: &[(&str, &str)]) -> (Store, Vec<BlockId>) {
    let store = empty_doc();
    let mut ids = Vec::new();
    for (kind, text) in kinds {
        let previous = ids.last().copied();
        let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
            let id = doc.insert_block(previous, kind, 0).unwrap();
            if !text.is_empty() {
                let _undo = doc.apply_delta(id, &[insert(text)]).unwrap();
            }
            ids.push(id);
        });
    }
    (store, ids)
}

fn blocks_of(store: &Store) -> Vec<BlockView> {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.blocks().unwrap())
}

fn texts_of(store: &Store) -> Vec<String> {
    blocks_of(store)
        .iter()
        .map(|view| view.spans.iter().map(|span| span.text.as_str()).collect())
        .collect()
}

fn root_hash(store: &Store, writer: u8) -> Option<[u8; 32]> {
    env::with_runtime_env(env_for(store, device(writer)), env::root_hash)
}

fn insert(text: &str) -> DeltaOp {
    DeltaOp::Insert {
        insert: text.to_owned(),
        attributes: None,
    }
}

fn retain(count: usize) -> DeltaOp {
    DeltaOp::Retain {
        retain: count,
        attributes: None,
    }
}

/// `(text, attribute pairs)` per span, which is what an expectation reads as.
fn readable(spans: &[Span]) -> Vec<(String, Vec<(String, String)>)> {
    spans
        .iter()
        .map(|span| {
            (
                span.text.clone(),
                span.attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .collect()
}

/// Both replicas receive both deltas, in opposite orders.
fn cross_land(left: &Store, right: &Store, from_left: &[u8], from_right: &[u8]) {
    land(right, device(BOB), from_left);
    land(left, device(ALICE), from_right);
}

/// Everything two converged replicas must agree on.
fn assert_converged(label: &str, left: &Store, right: &Store, ids: &BTreeSet<Id>) {
    assert_eq!(
        blocks_of(left),
        blocks_of(right),
        "{label}: the two replicas render different blocks"
    );
    assert_eq!(
        entry_bytes(left, ids),
        entry_bytes(right, ids),
        "{label}: the two replicas store different row bytes"
    );
    assert_eq!(
        root_hash(left, ALICE),
        root_hash(right, BOB),
        "{label}: the two replicas have different Merkle roots"
    );
}

fn ids_of(deltas: &[&[u8]]) -> BTreeSet<Id> {
    deltas.iter().flat_map(|delta| written_ids(delta)).collect()
}

// ── structure operations ────────────────────────────────────────────────

#[test]
fn insert_block__after_an_unknown_block_is_an_error_that_writes_nothing() {
    let (store, _ids) = seeded(&[(PARAGRAPH, "one")]);
    assert_rejected(&store, "unknown block", |doc| {
        doc.insert_block(Some(ABSENT), PARAGRAPH, 0).map(drop)
    });
}

#[test]
fn move_block__keeps_the_block_id_and_leaves_no_duplicate() {
    let (store, ids) = seeded(&[(PARAGRAPH, "a"), (PARAGRAPH, "b"), (PARAGRAPH, "c")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        doc.move_block(ids[0], Some(ids[2])).unwrap();
    });

    assert_eq!(texts_of(&store), ["b", "c", "a"]);
    assert_eq!(
        blocks_of(&store)
            .iter()
            .map(|view| view.id)
            .collect::<Vec<_>>(),
        vec![ids[1], ids[2], ids[0]],
        "a move must not mint a new block id"
    );
}

/// Both replicas move the same block to different places. Exactly one placement
/// wins, on one register, and the block appears once.
#[test]
fn move_block__concurrent_moves_of_one_block_never_duplicate_it() {
    let (base, ids) = seeded(&[(PARAGRAPH, "a"), (PARAGRAPH, "b"), (PARAGRAPH, "c")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        doc.move_block(ids[0], Some(ids[2])).unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        doc.move_block(ids[0], Some(ids[1])).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        let rendered: Vec<BlockId> = blocks_of(store).iter().map(|view| view.id).collect();
        assert_eq!(rendered.len(), 3, "a concurrent move duplicated a block");
        assert_eq!(
            rendered.iter().collect::<BTreeSet<_>>().len(),
            3,
            "a block was rendered twice"
        );
    }
    assert_converged(
        "concurrent move",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

/// Separate rows, so neither write can be reconciled away by the other.
#[test]
fn set_kind__and_set_depth_concurrently_both_survive() {
    let (base, ids) = seeded(&[(PARAGRAPH, "a")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        doc.set_kind(ids[0], "heading").unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        doc.set_depth(ids[0], 3).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        let view = &blocks_of(store)[0];
        assert_eq!(view.kind, "heading", "the concurrent kind was lost");
        assert_eq!(view.depth, 3, "the concurrent depth was lost");
    }
    assert_converged(
        "kind versus depth",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

#[test]
fn set_kind__concurrent_with_a_body_edit_keeps_both() {
    let (base, ids) = seeded(&[(PARAGRAPH, "hello")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        doc.set_kind(ids[0], "code-block").unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        let _undo = doc.apply_delta(ids[0], &[retain(5), insert("!")]).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        let view = &blocks_of(store)[0];
        assert_eq!(view.kind, "code-block");
        assert_eq!(view.spans[0].text, "hello!");
    }
    assert_converged(
        "kind versus body",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

/// The tombstone is a register nothing else writes, so a delete cannot be
/// undone by a concurrent edit, and the body rows it hides stay readable.
#[test]
fn delete_block__concurrent_with_a_body_edit_deletes_and_keeps_the_rows() {
    let (base, ids) = seeded(&[(PARAGRAPH, "keep"), (PARAGRAPH, "drop")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        let _was = doc.delete_block(ids[1]).unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        let _undo = doc
            .apply_delta(ids[1], &[retain(4), insert("ped")])
            .unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        assert_eq!(texts_of(store), ["keep"], "the delete was undone");
        let hidden =
            read_with::<Doc, _>(store, device(ALICE), |doc| doc.block_delta(ids[1]).unwrap());
        assert_eq!(
            hidden[0].text, "dropped",
            "the concurrent edit's rows must survive the tombstone"
        );
    }
    assert_converged(
        "delete versus body",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

#[test]
fn set_attr__stores_a_value_and_removal_is_its_own_state() {
    let (store, ids) = seeded(&[(PARAGRAPH, "a")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        doc.set_attr(ids[0], "align", Some("center")).unwrap();
        doc.set_attr(ids[0], "id", Some("")).unwrap();
    });
    assert_eq!(
        blocks_of(&store)[0].attrs,
        BTreeMap::from([
            ("align".to_owned(), "center".to_owned()),
            ("id".to_owned(), String::new()),
        ]),
        "an empty string is a value, not an absence"
    );

    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        doc.set_attr(ids[0], "align", None).unwrap();
    });
    assert_eq!(
        blocks_of(&store)[0].attrs,
        BTreeMap::from([("id".to_owned(), String::new())]),
        "removal must drop the key rather than render it empty"
    );
}

#[test]
fn set_attr__concurrent_writes_to_different_keys_both_survive() {
    let (base, ids) = seeded(&[(PARAGRAPH, "a")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        doc.set_attr(ids[0], "align", Some("center")).unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        doc.set_attr(ids[0], "lang", Some("rust")).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        assert_eq!(blocks_of(store)[0].attrs.len(), 2);
    }
    assert_converged(
        "two attributes",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

// ── split and merge ─────────────────────────────────────────────────────

#[test]
fn split_block__carries_the_tail_text_and_its_formatting() {
    let (store, ids) = seeded(&[("heading", "The fox jumped.")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(ids[0], 4, 7, "bold", Some("true")).unwrap();
        let _id = doc.mark(ids[0], 8, 14, "link", Some("u")).unwrap();
        doc.set_attr(ids[0], "align", Some("center")).unwrap();
    });

    let tail = edit::<Doc>(&store, device(ALICE), |doc| {
        let _new = doc.split_block(ids[0], 4).unwrap();
    });
    assert!(!tail.is_empty());

    let blocks = blocks_of(&store);
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        readable(&blocks[0].spans),
        vec![("The ".to_owned(), vec![])]
    );
    assert_eq!(
        readable(&blocks[1].spans),
        vec![
            (
                "fox".to_owned(),
                vec![("bold".to_owned(), "true".to_owned())]
            ),
            (" ".to_owned(), vec![]),
            (
                "jumped".to_owned(),
                vec![("link".to_owned(), "u".to_owned())]
            ),
            (".".to_owned(), vec![]),
        ],
        "the tail's formatting must travel with its text"
    );
    assert_eq!(blocks[1].kind, "heading", "a split inherits the kind");
    assert_eq!(
        blocks[1].attrs,
        BTreeMap::from([("align".to_owned(), "center".to_owned())]),
        "a split inherits the attributes"
    );
}

/// Marks live on the OLD characters, so the new block's formatting is new mark
/// rows over the new characters, one per carried key rather than per span.
#[test]
fn split_block__does_not_move_the_original_marks() {
    let (store, ids) = seeded(&[(PARAGRAPH, "abcdef")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(ids[0], 0, 6, "bold", Some("true")).unwrap();
    });
    let new = pick_new_block(&store, ids[0], 3);

    let head = read_with::<Doc, _>(&store, device(ALICE), |doc| {
        doc.block_delta(ids[0]).unwrap()
    });
    assert_eq!(
        readable(&head),
        vec![(
            "abc".to_owned(),
            vec![("bold".to_owned(), "true".to_owned())]
        )]
    );
    let tail = read_with::<Doc, _>(&store, device(ALICE), |doc| doc.block_delta(new).unwrap());
    assert_eq!(
        readable(&tail),
        vec![(
            "def".to_owned(),
            vec![("bold".to_owned(), "true".to_owned())]
        )]
    );
}

#[test]
fn split_block__at_the_end_touches_no_row_of_the_original() {
    let (store, ids) = seeded(&[(PARAGRAPH, "whole")]);
    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _new = doc.split_block(ids[0], 5).unwrap();
    });
    let touched = rows_written(&store, &delta);

    let body_delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _undo = doc.apply_delta(ids[0], &[retain(5), insert("!")]).unwrap();
    });
    let body_rows = rows_written(&store, &body_delta);
    assert!(
        touched.is_disjoint(&body_rows),
        "the fast path rewrote a row belonging to the original block's body: {:?}",
        touched.intersection(&body_rows).collect::<Vec<_>>()
    );
    assert_eq!(texts_of(&store), ["whole!", ""]);
}

#[test]
fn split_block__past_the_end_is_an_error_that_writes_nothing() {
    let (store, ids) = seeded(&[(PARAGRAPH, "short")]);
    assert_rejected(&store, "out of bounds", |doc| {
        doc.split_block(ids[0], 99).map(drop)
    });
    assert_eq!(
        blocks_of(&store).len(),
        1,
        "a rejected split must not have created the new block first"
    );
}

/// SPECIFIED behaviour, not a defect: a split copies the tail that existed when
/// it ran, and deletes only those characters, so a peer typing in the tail keeps
/// its text in the FIRST block.
#[test]
fn split_block__while_a_peer_types_in_the_tail_keeps_that_text_in_the_first_block() {
    let (base, ids) = seeded(&[(PARAGRAPH, "abcdef")]);
    let (left, right) = (fork(&base), fork(&base));

    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        let _undo = doc.apply_delta(ids[0], &[retain(5), insert("X")]).unwrap();
    });
    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        let _new = doc.split_block(ids[0], 3).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        assert_eq!(
            texts_of(store),
            ["abcX", "def"],
            "the peer's character stays in the first block, by design"
        );
    }
    assert_converged(
        "split versus type",
        &left,
        &right,
        &ids_of(&[&from_left, &from_right]),
    );
}

#[test]
fn merge_blocks__appends_the_body_with_its_formatting_and_tombstones_the_second() {
    let (store, ids) = seeded(&[(PARAGRAPH, "The "), (PARAGRAPH, "fox jumped.")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(ids[1], 0, 3, "bold", Some("true")).unwrap();
        doc.merge_blocks(ids[0], ids[1]).unwrap();
    });

    let blocks = blocks_of(&store);
    assert_eq!(blocks.len(), 1, "the merged block must be tombstoned");
    assert_eq!(blocks[0].id, ids[0]);
    assert_eq!(
        readable(&blocks[0].spans),
        vec![
            ("The ".to_owned(), vec![]),
            (
                "fox".to_owned(),
                vec![("bold".to_owned(), "true".to_owned())]
            ),
            (" jumped.".to_owned(), vec![]),
        ]
    );
}

#[test]
fn merge_blocks__into_itself_is_an_error_that_writes_nothing() {
    let (store, ids) = seeded(&[(PARAGRAPH, "one")]);
    assert_rejected(&store, "cannot merge into itself", |doc| {
        doc.merge_blocks(ids[0], ids[0])
    });
    assert_eq!(texts_of(&store), ["one"]);
}

#[test]
fn merge_blocks__an_unknown_block_is_an_error_that_writes_nothing() {
    let (store, ids) = seeded(&[(PARAGRAPH, "one")]);
    assert_rejected(&store, "unknown block", |doc| {
        doc.merge_blocks(ids[0], ABSENT)
    });
    assert_eq!(texts_of(&store), ["one"]);
}

// ── merge laws ──────────────────────────────────────────────────────────

/// The same set of block operations, authored independently, must land in ANY
/// delivery order to identical blocks, identical row bytes and an identical root.
#[test]
fn merge__every_delivery_order_of_a_block_op_set_converges_byte_for_byte() {
    let (base, ids) = seeded(&[(PARAGRAPH, "one"), (PARAGRAPH, "two"), (PARAGRAPH, "three")]);

    type Step = fn(&mut Doc, &[BlockId]);
    const STEPS: [Step; 5] = [
        |doc, ids| doc.set_kind(ids[0], "heading").unwrap(),
        |doc, ids| doc.set_depth(ids[1], 2).unwrap(),
        |doc, ids| doc.move_block(ids[2], None).unwrap(),
        |doc, ids| {
            let _undo = doc.apply_delta(ids[1], &[retain(3), insert("!")]).unwrap();
        },
        |doc, ids| doc.set_attr(ids[0], "align", Some("center")).unwrap(),
    ];

    // One author per step, each in isolation, so the writes genuinely race.
    let deltas: Vec<Vec<u8>> = STEPS
        .iter()
        .enumerate()
        .map(|(slot, step)| {
            let store = fork(&base);
            edit::<Doc>(&store, device(u8::try_from(slot).unwrap() + 1), |doc| {
                step(doc, &ids);
            })
        })
        .collect();
    let touched = ids_of(&deltas.iter().map(Vec::as_slice).collect::<Vec<_>>());

    let mut reference: Option<Converged> = None;
    for order in permutations(STEPS.len()) {
        let store = fork(&base);
        for index in &order {
            land(&store, device(ALICE), &deltas[*index]);
        }
        let outcome = Converged {
            blocks: blocks_of(&store),
            rows: entry_bytes(&store, &touched),
            root: root_hash(&store, ALICE),
        };
        match &reference {
            None => reference = Some(outcome),
            Some(want) => {
                assert_eq!(
                    outcome.blocks, want.blocks,
                    "order {order:?} rendered differently"
                );
                assert_eq!(
                    outcome.rows, want.rows,
                    "order {order:?} stored different bytes"
                );
                assert_eq!(
                    outcome.root, want.root,
                    "order {order:?} has a different root"
                );
            }
        }
    }
    let converged = reference.expect("at least one delivery order ran");
    assert_eq!(converged.blocks.len(), 3);
    assert_eq!(
        converged.blocks[0].id, ids[2],
        "the move never reached the fixture"
    );
    assert_eq!(
        converged.blocks[1].kind, "heading",
        "the kind never reached the fixture"
    );
    assert_eq!(
        converged.blocks[2].depth, 2,
        "the depth never reached the fixture"
    );
    assert!(!converged.rows.is_empty());
}

/// What every delivery order has to agree on.
struct Converged {
    blocks: Vec<BlockView>,
    rows: BTreeMap<Id, Option<Vec<u8>>>,
    root: Option<[u8; 32]>,
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut current: Vec<usize> = (0..n).collect();
    permute(&mut current, 0, &mut out);
    out
}

fn permute(current: &mut Vec<usize>, at: usize, out: &mut Vec<Vec<usize>>) {
    if at == current.len() {
        out.push(current.clone());
        return;
    }
    for index in at..current.len() {
        current.swap(at, index);
        permute(current, at + 1, out);
        current.swap(at, index);
    }
}

// ── the nested shape an app stores ──────────────────────────────────────

fn library() -> Store {
    genesis(|| Library::new_with_field_name("library"), |_map| {})
}

fn doc_blocks(store: &Store, key: &str) -> Vec<BlockView> {
    read_with::<Library, _>(store, device(ALICE), |map| {
        map.get(&key.to_owned())
            .unwrap()
            .map(|doc| doc.blocks().unwrap())
            .unwrap_or_default()
    })
}

/// Two replicas create the same document key independently and each adds a
/// block. Deterministic ids are what make the two halves merge at all.
#[test]
fn nested__two_replicas_creating_one_doc_key_converge() {
    let base = library();
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Library>(&left, device(ALICE), |map| {
        let mut doc = Doc::new();
        let id = doc.insert_block(None, "heading", 0).unwrap();
        let _undo = doc.apply_delta(id, &[insert("left")]).unwrap();
        let _old = map.insert(DOC_KEY.to_owned(), doc).unwrap();
    });
    let from_right = edit::<Library>(&right, device(BOB), |map| {
        let mut doc = Doc::new();
        let id = doc.insert_block(None, PARAGRAPH, 1).unwrap();
        let _undo = doc.apply_delta(id, &[insert("right")]).unwrap();
        let _old = map.insert(DOC_KEY.to_owned(), doc).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    for store in [&left, &right] {
        let blocks = doc_blocks(store, DOC_KEY);
        assert_eq!(blocks.len(), 2, "one replica's block was lost");
        let texts: BTreeSet<String> = blocks
            .iter()
            .map(|view| view.spans.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            BTreeSet::from(["left".to_owned(), "right".to_owned()])
        );
    }
    assert_eq!(
        root_hash(&left, ALICE),
        root_hash(&right, BOB),
        "the two replicas have different Merkle roots"
    );
}

/// The nested value is an entity of its own, so a document's rows must never
/// reach another document's.
#[test]
fn nested__two_documents_never_bleed_rows() {
    let store = library();
    for (key, text) in [("first", "alpha"), ("second", "beta")] {
        let _delta = edit::<Library>(&store, device(ALICE), |map| {
            let mut doc = Doc::new();
            let id = doc.insert_block(None, PARAGRAPH, 0).unwrap();
            let _undo = doc.apply_delta(id, &[insert(text)]).unwrap();
            let _old = map.insert(key.to_owned(), doc).unwrap();
        });
    }

    for (key, text) in [("first", "alpha"), ("second", "beta")] {
        let blocks = doc_blocks(&store, key);
        assert_eq!(blocks.len(), 1, "{key} sees another document's blocks");
        assert_eq!(blocks[0].spans[0].text, text);
    }
}

/// Removing the map entry while a peer edits a block: the entry is gone on both
/// sides, and the roots still agree.
#[test]
fn nested__removing_a_doc_entry_while_a_peer_edits_a_block_converges() {
    let base = library();
    let mut block = BlockId((0, 0));
    let _seed = edit::<Library>(&base, device(ALICE), |map| {
        let mut doc = Doc::new();
        block = doc.insert_block(None, PARAGRAPH, 0).unwrap();
        let _undo = doc.apply_delta(block, &[insert("body")]).unwrap();
        let _old = map.insert(DOC_KEY.to_owned(), doc).unwrap();
    });
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Library>(&left, device(ALICE), |map| {
        let _old = map.remove(&DOC_KEY.to_owned()).unwrap();
    });
    let from_right = edit::<Library>(&right, device(BOB), |map| {
        let mut doc = map.get_mut(&DOC_KEY.to_owned()).unwrap().unwrap();
        let _undo = doc.apply_delta(block, &[retain(4), insert("!")]).unwrap();
    });
    cross_land(&left, &right, &from_left, &from_right);

    assert_eq!(
        doc_blocks(&left, DOC_KEY),
        doc_blocks(&right, DOC_KEY),
        "the two replicas disagree about the raced document"
    );
    assert_eq!(root_hash(&left, ALICE), root_hash(&right, BOB));
}

// ── the JSON a client receives ──────────────────────────────────────────

#[test]
fn block_view__json_is_the_shape_a_client_reads() {
    let (store, ids) = seeded(&[("heading", "hi")]);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        doc.set_depth(ids[0], 2).unwrap();
        doc.set_attr(ids[0], "align", Some("center")).unwrap();
        let _id = doc.mark(ids[0], 0, 2, "bold", Some("true")).unwrap();
    });

    let view = &blocks_of(&store)[0];
    let json = serde_json::to_value(view).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "id": [view.id.0 .0, view.id.0 .1],
            "kind": "heading",
            "depth": 2,
            "attrs": {"align": "center"},
            "spans": [{"text": "hi", "attributes": {"bold": true}}],
        })
    );
    assert_eq!(
        serde_json::from_value::<BlockView>(json).unwrap(),
        *view,
        "the view must round-trip through its own JSON"
    );
}

/// A view with no attributes must not carry an empty `attrs` object.
#[test]
fn block_view__json_omits_an_empty_attribute_map() {
    let (store, _ids) = seeded(&[(PARAGRAPH, "x")]);
    let json = serde_json::to_value(&blocks_of(&store)[0]).unwrap();
    assert!(
        json.get("attrs").is_none(),
        "an empty attribute map must be absent, not null or empty: {json}"
    );
}

// ── shared helpers ──────────────────────────────────────────────────────

/// The rows a transaction writes beyond the state container, which every commit
/// dirties whether or not a collection changed. An empty id set is therefore not
/// what "wrote no row" looks like, and this is the control it is read against.
fn rows_written(store: &Store, delta: &[u8]) -> BTreeSet<Id> {
    let control: BTreeSet<Id> = written_ids(
        &try_edit::<Doc>(store, device(ALICE), |doc| {
            let _dirty: &mut Doc = doc;
        })
        .expect("dirtying the state container emits a delta"),
    )
    .into_iter()
    .collect();
    written_ids(delta)
        .into_iter()
        .filter(|id| !control.contains(id))
        .collect()
}

/// A rejected operation must report `message` and leave the store exactly as it
/// was: no spine slot, no half-built block, no row on the wire.
fn assert_rejected(
    store: &Store,
    message: &str,
    op: impl FnOnce(&mut Doc) -> Result<(), calimero_storage::collections::error::StoreError>,
) {
    let before = root_hash(store, ALICE);
    let delta = try_edit::<Doc>(store, device(ALICE), |doc| {
        let error = op(doc).unwrap_err().to_string();
        assert!(error.contains(message), "expected {message:?}, got {error}");
    })
    .unwrap_or_default();
    assert!(
        rows_written(store, &delta).is_empty(),
        "a rejected operation wrote rows"
    );
    assert_eq!(
        root_hash(store, ALICE),
        before,
        "a rejected operation changed the Merkle root"
    );
}

fn pick_new_block(store: &Store, block: BlockId, at: usize) -> BlockId {
    let mut new = BlockId((0, 0));
    let _delta = edit::<Doc>(store, device(ALICE), |doc| {
        new = doc.split_block(block, at).unwrap();
    });
    new
}
