//! Seeded differential fuzz: several `RichText` replicas against the naive
//! model, edited concurrently and reconciled through real delta bytes.
//!
//! A seeded fuzz that stops reaching its interesting states is decorative, so
//! the run fails if any coverage counter is below `MIN_HITS` - a zero there is
//! reported as "the generator stopped reaching X", not as a pass.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use calimero_storage::collections::{Attrs, DefaultMarks, DeltaOp, RichText, Span};
use calimero_storage::store::MainStorage;

mod fugue_harness;
mod rich_text_model;

use fugue_harness::{device, edit, fork, genesis, land, read_with, Store};
use rich_text_model::{readable, Model, ModelDelta};

const FUZZ_SEED: u64 = 0x_11_c4_7e_07; // printed on every failure, so a red run reproduces
const ROUNDS: usize = 110; // edits across every replica
const MAX_LEN: usize = 90; // past this the generator only deletes, so the run stays bounded
const REPLICAS: usize = 3;
const FLUSH_EVERY: usize = 4; // rounds of divergence before deltas are delivered
const MIN_HITS: usize = 5; // per coverage counter, below which the run is decorative
const KEYS: [&str; 6] = ["bold", "italic", "link", "code", "comment:a", "comment:b"];
const POOL: [&str; 6] = ["a", "bc", "d", "\u{1F600}", "\u{00E9}\u{65E5}", "ef"];

type Doc = RichText<DefaultMarks, MainStorage>;

/// A run that never produced an overlap, never inherited an attribute at an
/// insert and never suppressed a redundant write has not tested this module.
#[derive(Debug, Default)]
struct Coverage {
    overlapping_same_key: usize,
    overlapping_different_keys: usize,
    insert_at_growing_edge: usize,
    insert_at_non_growing_edge: usize,
    insert_into_tombstone_run: usize,
    redundant_write_suppressed: usize,
    mark_inactive_at_read: usize,
    unmark_of_absent_key: usize,
}

impl Coverage {
    fn counters(&self) -> [(&'static str, usize); 8] {
        [
            ("overlapping_same_key", self.overlapping_same_key),
            (
                "overlapping_different_keys",
                self.overlapping_different_keys,
            ),
            ("insert_at_growing_edge", self.insert_at_growing_edge),
            (
                "insert_at_non_growing_edge",
                self.insert_at_non_growing_edge,
            ),
            ("insert_into_tombstone_run", self.insert_into_tombstone_run),
            (
                "redundant_write_suppressed",
                self.redundant_write_suppressed,
            ),
            ("mark_inactive_at_read", self.mark_inactive_at_read),
            ("unmark_of_absent_key", self.unmark_of_absent_key),
        ]
    }
}

/// One generated edit, as the ops it is made of.
fn generate(rng: &mut StdRng, len: usize) -> Vec<DeltaOp> {
    if len > MAX_LEN {
        return vec![DeltaOp::Delete { delete: len / 2 }];
    }
    let count = if rng.random_range(..5_usize) == 0 {
        2 + rng.random_range(..4_usize)
    } else {
        1
    };
    let mut ops = Vec::new();
    let mut left = len;
    for _ in 0..count {
        let retain = if left == 0 {
            0
        } else {
            rng.random_range(..left + 1)
        };
        left -= retain;
        if retain > 0 {
            ops.push(DeltaOp::Retain {
                retain,
                attributes: None,
            });
        }
        match rng.random_range(..10_usize) {
            0..=4 => ops.push(DeltaOp::Insert {
                insert: POOL[rng.random_range(..POOL.len())].to_owned(),
                attributes: insert_attrs(rng),
            }),
            5..=6 if left > 0 => {
                let delete = 1 + rng.random_range(..left.min(6));
                left -= delete;
                ops.push(DeltaOp::Delete { delete });
            }
            _ if left > 0 => {
                let span = 1 + rng.random_range(..left.min(8));
                left -= span;
                ops.push(DeltaOp::Retain {
                    retain: span,
                    attributes: Some(mark_attrs(rng)),
                });
            }
            _ => {}
        }
    }
    ops
}

fn insert_attrs(rng: &mut StdRng) -> Option<Attrs> {
    match rng.random_range(..4_usize) {
        0 => None,
        1 => Some(Attrs::new()),
        2 => Some(mark_attrs(rng)),
        _ => Some(
            [(KEYS[rng.random_range(..KEYS.len())].to_owned(), None)]
                .into_iter()
                .collect(),
        ),
    }
}

fn mark_attrs(rng: &mut StdRng) -> Attrs {
    let key = KEYS[rng.random_range(..KEYS.len())].to_owned();
    let value = (rng.random_range(..4_usize) != 0).then(|| {
        if key.starts_with("comment") {
            format!("note{}", rng.random_range(..3_usize))
        } else {
            "true".to_owned()
        }
    });
    [(key, value)].into_iter().collect()
}

/// The delta walk, expressed against the model's own primitives. This is the
/// specification written naively: a cursor, one mark per key in map order, and
/// an insert whose requested attributes are the complete desired set.
fn apply_to_model(model: &mut Model, ops: &[DeltaOp], replica: u64, hits: &mut Coverage) {
    let mut pos = 0_usize;
    for op in ops {
        match *op {
            DeltaOp::Retain {
                retain,
                ref attributes,
            } => {
                if let Some(attrs) = attributes {
                    for (key, value) in attrs {
                        note_overlap(model, pos, pos + retain, key, hits);
                        let absent = (pos..pos + retain)
                            .all(|index| model_value(model, index, key).is_none());
                        if model
                            .mark::<DefaultMarks>(pos, pos + retain, key, value.as_deref(), replica)
                            .is_none()
                            && pos < retain + pos
                        {
                            hits.redundant_write_suppressed += 1;
                            if value.is_none() && absent {
                                hits.unmark_of_absent_key += 1;
                            }
                        }
                    }
                }
                pos += retain;
            }
            DeltaOp::Insert {
                ref insert,
                ref attributes,
            } => {
                if model.has_tombstones_at(pos) {
                    hits.insert_into_tombstone_run += 1;
                }
                for grows in model.edges_at(pos) {
                    if grows {
                        hits.insert_at_growing_edge += 1;
                    } else {
                        hits.insert_at_non_growing_edge += 1;
                    }
                }
                let count = insert.chars().count();
                let _minted = model.insert(pos, replica, insert);
                if let Some(requested) = attributes {
                    let inherited = model.attributes_at(pos);
                    let keys: BTreeSet<&String> =
                        requested.keys().chain(inherited.keys()).collect();
                    for key in keys {
                        let want = requested.get(key).cloned().flatten();
                        let _id = model.mark::<DefaultMarks>(
                            pos,
                            pos + count,
                            key,
                            want.as_deref(),
                            replica,
                        );
                    }
                }
                pos += count;
            }
            DeltaOp::Delete { delete } => model.delete(pos, pos + delete),
        }
    }
}

fn model_value(model: &Model, index: usize, key: &str) -> Option<String> {
    model.attributes_at(index).get(key).cloned()
}

fn note_overlap(model: &Model, start: usize, end: usize, key: &str, hits: &mut Coverage) {
    for (from, to, other) in model.active_ranges() {
        if to <= start || from >= end {
            continue;
        }
        if other == key {
            hits.overlapping_same_key += 1;
        } else {
            hits.overlapping_different_keys += 1;
        }
    }
}

fn spans_of(store: &Store, writer: u8) -> Vec<Span> {
    read_with::<Doc, _>(store, device(writer), |doc| doc.to_delta().unwrap())
}

#[test]
fn fuzz__rich_text_renders_what_the_naive_model_renders() {
    let base = genesis(|| Doc::new_with_field_name("fuzz"), |_| ());
    let stores: Vec<Store> = (0..REPLICAS).map(|_| fork(&base)).collect();
    let mut models: Vec<Model> = (0..REPLICAS).map(|_| Model::new()).collect();
    let mut rng = StdRng::seed_from_u64(FUZZ_SEED);
    let mut hits = Coverage::default();
    let mut pending: Vec<(usize, Vec<u8>, ModelDelta)> = Vec::new();

    for round in 0..ROUNDS {
        let author = rng.random_range(..REPLICAS);
        let writer = u8::try_from(author).unwrap() + 1;
        let ops = generate(&mut rng, models[author].len());

        let before = models[author].clone();
        let bytes = edit::<Doc>(&stores[author], device(writer), |doc| {
            doc.apply_delta(&ops).map(drop).unwrap_or_else(|error| {
                panic!("seed {FUZZ_SEED:#x} round {round}: {ops:?} failed: {error}")
            });
        });
        apply_to_model(&mut models[author], &ops, u64::from(writer), &mut hits);
        pending.push((author, bytes, Model::diff(&before, &models[author])));

        hits.mark_inactive_at_read += models[author].inactive_marks().min(1);
        assert_matches_model(round, author, &stores[author], &models[author]);

        if round % FLUSH_EVERY == FLUSH_EVERY - 1 {
            for (from, bytes, delta) in pending.drain(..) {
                for to in 0..REPLICAS {
                    if to == from {
                        continue;
                    }
                    land(&stores[to], device(u8::try_from(to).unwrap() + 1), &bytes);
                    models[to].integrate(&delta);
                }
            }
            for replica in 0..REPLICAS {
                hits.mark_inactive_at_read += models[replica].inactive_marks().min(1);
                assert_matches_model(round, replica, &stores[replica], &models[replica]);
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

fn assert_matches_model(round: usize, replica: usize, store: &Store, model: &Model) {
    let writer = u8::try_from(replica).unwrap() + 1;
    let got = spans_of(store, writer);
    assert_eq!(
        readable(&got),
        readable(&model.spans()),
        "seed {FUZZ_SEED:#x} round {round} replica {replica}: the collection and the model \
         disagree"
    );
    let text: String = got.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(
        text,
        model.text(),
        "seed {FUZZ_SEED:#x} round {round} replica {replica}: the spans do not spell the document"
    );
}
