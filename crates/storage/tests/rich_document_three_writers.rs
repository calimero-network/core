//! Three people typing at one spot, one character per commit, the way an editor
//! sends keystrokes. Reproduces the shape of mero-docs' `rich` e2e (three nodes,
//! the third a late joiner seeded from a snapshot), which diverged on a real rig:
//! one node rendered `…b3a3a3a3` while its peer rendered `…b3b3b3a3a3a3`.
//!
//! Every replica receives every delta, in a seeded random interleaving that keeps
//! each writer's own deltas in order (the causal order the DAG enforces).

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use calimero_storage::collections::{BlockId, DefaultMarks, DeltaOp, RichDocument};
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{device, edit, fork, genesis, land, read_with, root_hash, Store};

type Doc = RichDocument<DefaultMarks, MainStorage>;

const WRITERS: [u8; 3] = [1, 2, 3];
const SEEDS: u64 = 64;

fn text_of(store: &Store, writer: u8) -> String {
    read_with::<Doc, _>(store, device(writer), |doc| {
        let views = doc.blocks().unwrap();
        views[0].spans.iter().map(|s| s.text.as_str()).collect()
    })
}

fn block_of(store: &Store, writer: u8) -> BlockId {
    read_with::<Doc, _>(store, device(writer), |doc| doc.blocks().unwrap()[0].id)
}

/// `run`, typed at the end of the block one character per commit.
fn type_run(store: &Store, writer: u8, run: &str) -> Vec<Vec<u8>> {
    let block = block_of(store, writer);
    run.chars()
        .map(|ch| {
            let at = text_of(store, writer).chars().count();
            edit::<Doc>(store, device(writer), |doc| {
                let _undo = doc
                    .apply_delta(
                        block,
                        &[DeltaOp::retain(at), DeltaOp::insert(&ch.to_string())],
                    )
                    .unwrap();
            })
        })
        .collect()
}

/// Lands every other writer's deltas on `stores[to]`, interleaved at random but
/// each writer's own in order.
fn deliver(rng: &mut StdRng, stores: &[Store], to: usize, streams: &[Vec<Vec<u8>>]) {
    let mut next = vec![0_usize; streams.len()];
    loop {
        let open: Vec<usize> = (0..streams.len())
            .filter(|&from| from != to && next[from] < streams[from].len())
            .collect();
        if open.is_empty() {
            return;
        }
        let from = open[rng.random_range(0..open.len())];
        land(&stores[to], device(WRITERS[to]), &streams[from][next[from]]);
        next[from] += 1;
    }
}

fn whole_orders(runs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in 0..3 {
        for b in 0..3 {
            for c in 0..3 {
                if a != b && b != c && a != c {
                    out.push(format!("{}{}{}", runs[a], runs[b], runs[c]));
                }
            }
        }
    }
    out
}

#[test]
fn three_writers_typing_at_one_spot_converge_and_keep_each_run_whole() {
    for seed in 0..SEEDS {
        let mut rng = StdRng::seed_from_u64(seed);
        let base = genesis(
            || Doc::new_with_field_name("doc"),
            |doc| {
                let id = doc.insert_block(None, "paragraph", 0).unwrap();
                let _undo = doc.apply_delta(id, &[DeltaOp::insert("Look: ")]).unwrap();
            },
        );
        // Two founders; the third joins late from a copy of a founder's rows,
        // which is what a snapshot install leaves on disk.
        let mut stores = vec![fork(&base), fork(&base)];
        let mut late = None;

        for round in 1..=3 {
            if round == 2 {
                stores.push(fork(&stores[0]));
                late = Some(2);
            }
            let typing = stores.len();
            let before = text_of(&stores[0], WRITERS[0]);
            let runs: Vec<String> = ["a", "b", "c"][..typing]
                .iter()
                .map(|l| format!("{l}{round}").repeat(3))
                .collect();
            let streams: Vec<Vec<Vec<u8>>> = (0..typing)
                .map(|i| type_run(&stores[i], WRITERS[i], &runs[i]))
                .collect();
            for to in 0..typing {
                deliver(&mut rng, &stores, to, &streams);
            }

            let texts: Vec<String> = (0..typing)
                .map(|i| text_of(&stores[i], WRITERS[i]))
                .collect();
            for i in 1..typing {
                assert_eq!(
                    texts[0], texts[i],
                    "seed {seed} round {round}: replica {i} (late={late:?}) renders differently"
                );
                assert_eq!(
                    root_hash(&stores[0], WRITERS[0]),
                    root_hash(&stores[i], WRITERS[i]),
                    "seed {seed} round {round}: replica {i} stores a different root"
                );
            }
            let tail = &texts[0][before.len()..];
            if typing == 3 {
                assert!(
                    whole_orders(&runs).iter().any(|o| o == tail),
                    "seed {seed} round {round}: a run was split or lost: {tail:?}"
                );
            } else {
                assert!(
                    tail == format!("{}{}", runs[0], runs[1])
                        || tail == format!("{}{}", runs[1], runs[0]),
                    "seed {seed} round {round}: a run was split or lost: {tail:?}"
                );
            }
        }
    }
}
