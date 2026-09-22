//! Seeded differential fuzz: several `RichDocument` replicas against the naive
//! document model, edited concurrently and reconciled through real delta bytes.
//!
//! A seeded fuzz that stops reaching its interesting states is decorative, so
//! the run fails if any coverage counter is below `MIN_HITS` - a zero there is
//! reported as "the generator stopped reaching X", not as a pass.
//!
//! The generator never lets two replicas write ONE structure field of one block
//! inside a divergence window. That tie is decided by a wall-clock last-write-
//! wins the model has no way to predict, so fuzzing it would assert a coin toss;
//! `rich_document_blocks.rs` pins each such pair on its own instead.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use calimero_storage::collections::fugue::RawId;
use calimero_storage::collections::{BlockId, BlockView, DefaultMarks, DeltaOp, RichDocument};
use calimero_storage::store::MainStorage;

mod fugue_harness;
mod rich_text_model;

use fugue_harness::{device, edit, fork, genesis, land, read_with, Store};
use rich_text_model::{DocModel, DocView};

const FUZZ_SEED: u64 = 0x_b1_0c_5e_ed; // printed on every failure, so a red run reproduces
const ROUNDS: usize = 1_200; // edits across every replica
const REPLICAS: usize = 3;
const FLUSH_EVERY: usize = 12; // rounds of divergence before deltas are delivered
const MIN_HITS: usize = 5; // per coverage counter, below which the run is decorative
const MAX_BLOCKS: usize = 4; // past this the generator only deletes and merges
const MAX_BODY: usize = 40; // past this a body only shrinks, so the run stays bounded
const KINDS: [&str; 4] = ["paragraph", "heading", "ordered-list-item", "code-block"];
const KEYS: [&str; 4] = ["bold", "italic", "link", "comment:a"]; // After, After, None, None
const ATTRS: [&str; 3] = ["align", "lang", "id"];
const POOL: [&str; 5] = ["a", "bc", "\u{1F600}", "\u{00E9}\u{65E5}", "def"];

type Doc = RichDocument<DefaultMarks, MainStorage>;

/// A window that never raced two replicas on one block, never split under a
/// peer's typing and never carried formatting has not tested this module.
#[derive(Debug, Default)]
struct Coverage {
    raced_one_block: usize,
    split_while_a_peer_edits: usize,
    delete_while_a_peer_edits: usize,
    split_carried_formatting: usize,
    merged_two_blocks: usize,
    moved_a_block: usize,
    removed_an_attribute: usize,
}

impl Coverage {
    fn counters(&self) -> [(&'static str, usize); 7] {
        [
            ("raced_one_block", self.raced_one_block),
            ("split_while_a_peer_edits", self.split_while_a_peer_edits),
            ("delete_while_a_peer_edits", self.delete_while_a_peer_edits),
            ("split_carried_formatting", self.split_carried_formatting),
            ("merged_two_blocks", self.merged_two_blocks),
            ("moved_a_block", self.moved_a_block),
            ("removed_an_attribute", self.removed_an_attribute),
        ]
    }
}

/// One generated operation, in the form both the collection and the model take.
#[derive(Clone, Debug)]
enum Op {
    InsertBlock {
        after: Option<RawId>,
        kind: &'static str,
        depth: u8,
    },
    DeleteBlock(RawId),
    MoveBlock {
        block: RawId,
        after: Option<RawId>,
    },
    SetKind(RawId, &'static str),
    SetDepth(RawId, u8),
    SetAttr(RawId, &'static str, Option<&'static str>),
    Split(RawId, usize),
    Merge(RawId, RawId),
    Type(RawId, usize, &'static str),
    Erase(RawId, usize, usize),
    Mark(RawId, usize, usize, &'static str, Option<&'static str>),
}

impl Op {
    /// The block this operation reads or writes, which is what a race is about.
    const fn block(&self) -> Option<RawId> {
        match *self {
            Self::InsertBlock { after, .. } => after,
            Self::DeleteBlock(block)
            | Self::MoveBlock { block, .. }
            | Self::SetKind(block, _)
            | Self::SetDepth(block, _)
            | Self::SetAttr(block, _, _)
            | Self::Split(block, _)
            | Self::Merge(block, _)
            | Self::Type(block, _, _)
            | Self::Erase(block, _, _)
            | Self::Mark(block, _, _, _, _) => Some(block),
        }
    }

    /// The `(block, field)` pairs a window may only see written once, because
    /// two replicas writing one of them is a tie no model can call.
    fn claims(&self) -> Option<(RawId, String)> {
        match *self {
            Self::MoveBlock { block, .. } => Some((block, "place".to_owned())),
            Self::SetKind(block, _) => Some((block, "kind".to_owned())),
            Self::SetDepth(block, _) => Some((block, "depth".to_owned())),
            Self::SetAttr(block, key, _) => Some((block, format!("attr:{key}"))),
            _ => None,
        }
    }
}

fn pick<'a, T>(rng: &mut StdRng, from: &'a [T]) -> &'a T {
    &from[rng.random_range(..from.len())]
}

/// One operation against what `model` currently holds, or `None` when the roll
/// picked something this replica cannot do right now.
fn generate(rng: &mut StdRng, model: &DocModel) -> Option<Op> {
    let ids = model.live_ids();
    if ids.is_empty() || (ids.len() < MAX_BLOCKS && rng.random_range(..8_usize) == 0) {
        return Some(Op::InsertBlock {
            after: (!ids.is_empty()).then(|| ids[rng.random_range(..ids.len())]),
            kind: pick(rng, &KINDS),
            depth: u8::try_from(rng.random_range(..3_usize)).unwrap(),
        });
    }

    let block = ids[rng.random_range(..ids.len())];
    let len = model.block(block).expect("a live block").body.len();
    if len > MAX_BODY {
        return Some(Op::Erase(block, 0, len / 2));
    }

    Some(match rng.random_range(..11_usize) {
        0 => Op::SetKind(block, pick(rng, &KINDS)),
        1 => Op::SetDepth(block, u8::try_from(rng.random_range(..4_usize)).unwrap()),
        2 => {
            let value = (rng.random_range(..3_usize) != 0).then(|| *pick(rng, &["x", "y"]));
            Op::SetAttr(block, pick(rng, &ATTRS), value)
        }
        10 if ids.len() > 1 => Op::MoveBlock {
            block,
            after: (rng.random_range(..4_usize) != 0)
                .then(|| ids[rng.random_range(..ids.len())])
                .filter(|after| *after != block),
        },
        4 if ids.len() > 2 => Op::DeleteBlock(block),
        3 | 5 if ids.len() > 1 && ids.len() < MAX_BLOCKS => {
            Op::Split(block, rng.random_range(..len + 1))
        }
        6 if ids.len() > 1 => {
            let other = ids[rng.random_range(..ids.len())];
            if other == block {
                return None;
            }
            Op::Merge(block, other)
        }
        7 | 8 if len > 0 => {
            let start = rng.random_range(..len);
            let end = start + 1 + rng.random_range(..len - start);
            let value = (rng.random_range(..3_usize) != 0).then(|| *pick(rng, &["true", "u"]));
            Op::Mark(block, start, end, pick(rng, &KEYS), value)
        }
        9 if len > 1 => {
            let start = rng.random_range(..len);
            Op::Erase(block, start, 1 + rng.random_range(..(len - start).min(3)))
        }
        _ => Op::Type(block, rng.random_range(..len + 1), pick(rng, &POOL)),
    })
}

fn apply_to_doc(doc: &mut Doc, op: &Op) {
    match *op {
        Op::InsertBlock { after, kind, depth } => {
            let _id = doc.insert_block(after.map(BlockId), kind, depth).unwrap();
        }
        Op::DeleteBlock(block) => {
            let _was = doc.delete_block(BlockId(block)).unwrap();
        }
        Op::MoveBlock { block, after } => {
            doc.move_block(BlockId(block), after.map(BlockId)).unwrap()
        }
        Op::SetKind(block, kind) => doc.set_kind(BlockId(block), kind).unwrap(),
        Op::SetDepth(block, depth) => doc.set_depth(BlockId(block), depth).unwrap(),
        Op::SetAttr(block, key, value) => doc.set_attr(BlockId(block), key, value).unwrap(),
        Op::Split(block, at) => {
            let _new = doc.split_block(BlockId(block), at).unwrap();
        }
        Op::Merge(first, second) => doc.merge_blocks(BlockId(first), BlockId(second)).unwrap(),
        Op::Type(block, pos, text) => {
            let _undo = doc
                .apply_delta(
                    BlockId(block),
                    &[
                        retain(pos),
                        DeltaOp::Insert {
                            insert: text.to_owned(),
                            attributes: None,
                        },
                    ],
                )
                .unwrap();
        }
        Op::Erase(block, start, count) => {
            let _undo = doc
                .apply_delta(
                    BlockId(block),
                    &[retain(start), DeltaOp::Delete { delete: count }],
                )
                .unwrap();
        }
        Op::Mark(block, start, end, key, value) => {
            let _id = doc.mark(BlockId(block), start, end, key, value).unwrap();
        }
    }
}

fn apply_to_model(model: &mut DocModel, op: &Op, replica: u64, stamp: u64) {
    match *op {
        Op::InsertBlock { after, kind, depth } => {
            let _id = model.insert_block(after, kind, depth, replica, stamp);
        }
        Op::DeleteBlock(block) => model.delete_block(block, stamp),
        Op::MoveBlock { block, after } => model.move_block(block, after, replica, stamp),
        Op::SetKind(block, kind) => model.set_kind(block, kind, stamp),
        Op::SetDepth(block, depth) => model.set_depth(block, depth, stamp),
        Op::SetAttr(block, key, value) => model.set_attr(block, key, value, stamp),
        Op::Split(block, at) => {
            let _new = model.split_block::<DefaultMarks>(block, at, replica, stamp);
        }
        Op::Merge(first, second) => {
            model.merge_blocks::<DefaultMarks>(first, second, replica, stamp);
        }
        Op::Type(block, pos, text) => {
            let _minted = model.block_mut(block).body.insert(pos, replica, text);
        }
        Op::Erase(block, start, count) => model.block_mut(block).body.delete(start, start + count),
        Op::Mark(block, start, end, key, value) => {
            let _id = model
                .block_mut(block)
                .body
                .mark::<DefaultMarks>(start, end, key, value, replica);
        }
    }
}

const fn retain(count: usize) -> DeltaOp {
    DeltaOp::Retain {
        retain: count,
        attributes: None,
    }
}

fn views_of(store: &Store, writer: u8) -> Vec<BlockView> {
    read_with::<Doc, _>(store, device(writer), |doc| doc.blocks().unwrap())
}

/// The collection's view, in the model's shape, so a failure prints both sides
/// as the same kind of thing.
fn as_model_views(views: Vec<BlockView>) -> Vec<DocView> {
    views
        .into_iter()
        .map(|view| DocView {
            id: view.id.0,
            kind: view.kind,
            depth: view.depth,
            attrs: view.attrs,
            spans: view.spans,
        })
        .collect()
}

#[test]
fn fuzz__rich_document_renders_what_the_naive_model_renders() {
    let base = genesis(|| Doc::new_with_field_name("fuzz"), |_| ());
    let stores: Vec<Store> = (0..REPLICAS).map(|_| fork(&base)).collect();
    let mut models: Vec<DocModel> = (0..REPLICAS).map(|_| DocModel::new()).collect();
    let mut rng = StdRng::seed_from_u64(FUZZ_SEED);
    let mut hits = Coverage::default();

    let mut pending: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut claimed: BTreeSet<(RawId, String)> = BTreeSet::new();
    let mut touched: Vec<BTreeSet<RawId>> = vec![BTreeSet::new(); REPLICAS];
    let mut split_in_window: Vec<BTreeSet<RawId>> = vec![BTreeSet::new(); REPLICAS];
    let mut deleted_in_window: Vec<BTreeSet<RawId>> = vec![BTreeSet::new(); REPLICAS];
    let mut stamp = 0_u64;

    for round in 0..ROUNDS {
        let author = rng.random_range(..REPLICAS);
        let writer = u8::try_from(author).unwrap() + 1;
        let Some(op) = generate(&mut rng, &models[author]) else {
            continue;
        };
        if op.claims().is_some_and(|claim| claimed.contains(&claim)) {
            continue;
        }
        if let Some(claim) = op.claims() {
            let _new = claimed.insert(claim);
        }

        count_window(
            &op,
            author,
            &models[author],
            &touched,
            &split_in_window,
            &deleted_in_window,
            &mut hits,
        );
        if let Op::Split(block, _) = op {
            let _new = split_in_window[author].insert(block);
        }
        if let Op::DeleteBlock(block) = op {
            let _new = deleted_in_window[author].insert(block);
        }
        if let Some(block) = op.block() {
            let _new = touched[author].insert(block);
        }

        stamp += 1;
        let bytes = edit::<Doc>(&stores[author], device(writer), |doc| {
            apply_to_doc(doc, &op);
        });
        apply_to_model(&mut models[author], &op, u64::from(writer), stamp);
        pending.push((author, bytes));
        assert_matches_model(round, author, &stores[author], &models[author]);

        if round % FLUSH_EVERY == FLUSH_EVERY - 1 {
            for (from, bytes) in pending.drain(..) {
                for to in (0..REPLICAS).filter(|to| *to != from) {
                    land(&stores[to], device(u8::try_from(to).unwrap() + 1), &bytes);
                }
            }
            let snapshot = models.clone();
            for (replica, model) in models.iter_mut().enumerate() {
                for other in snapshot
                    .iter()
                    .enumerate()
                    .filter_map(|(index, other)| (index != replica).then_some(other))
                {
                    model.merge(other);
                }
            }
            for replica in 0..REPLICAS {
                assert_matches_model(round, replica, &stores[replica], &models[replica]);
            }
            claimed.clear();
            for seen in touched
                .iter_mut()
                .chain(&mut split_in_window)
                .chain(&mut deleted_in_window)
            {
                seen.clear();
            }
        }
    }

    for (name, count) in hits.counters() {
        assert!(
            count >= MIN_HITS,
            "seed {FUZZ_SEED:#x}: the generator stopped reaching {name} ({count} < {MIN_HITS}); \
             the whole guard reads {hits:?}"
        );
    }
}

/// Count the interesting states BEFORE the operation lands, which is when the
/// window's other replicas still describe what it is racing with.
fn count_window(
    op: &Op,
    author: usize,
    model: &DocModel,
    touched: &[BTreeSet<RawId>],
    split_in_window: &[BTreeSet<RawId>],
    deleted_in_window: &[BTreeSet<RawId>],
    hits: &mut Coverage,
) {
    let peers_touched = |block: RawId| {
        touched
            .iter()
            .enumerate()
            .any(|(replica, seen)| replica != author && seen.contains(&block))
    };
    if op.block().is_some_and(peers_touched) {
        hits.raced_one_block += 1;
    }
    match *op {
        Op::MoveBlock { .. } => hits.moved_a_block += 1,
        Op::Merge(..) => hits.merged_two_blocks += 1,
        Op::SetAttr(_, _, None) => hits.removed_an_attribute += 1,
        Op::Split(block, at) => {
            let body = &model.block(block).expect("a live block").body;
            let tail = rich_text_model::slice_spans(&body.spans(), at, body.len());
            if tail.iter().any(|span| !span.attributes.is_empty()) {
                hits.split_carried_formatting += 1;
            }
        }
        _ => {}
    }
    let by_a_peer = |seen: &[BTreeSet<RawId>], block: RawId| {
        seen.iter()
            .enumerate()
            .any(|(replica, blocks)| replica != author && blocks.contains(&block))
    };
    if let Some(block) = op.block() {
        if matches!(*op, Op::Type(..) | Op::Erase(..) | Op::Mark(..)) {
            if by_a_peer(split_in_window, block) {
                hits.split_while_a_peer_edits += 1;
            }
            if by_a_peer(deleted_in_window, block) {
                hits.delete_while_a_peer_edits += 1;
            }
        }
    }
}

fn assert_matches_model(round: usize, replica: usize, store: &Store, model: &DocModel) {
    let writer = u8::try_from(replica).unwrap() + 1;
    let got = as_model_views(views_of(store, writer));
    assert_eq!(
        got,
        model.views(),
        "seed {FUZZ_SEED:#x} round {round} replica {replica}: the collection and the model \
         disagree"
    );
    let ids: BTreeSet<RawId> = got.iter().map(|view| view.id).collect();
    assert_eq!(
        ids.len(),
        got.len(),
        "seed {FUZZ_SEED:#x} round {round} replica {replica}: a block rendered twice"
    );
}
