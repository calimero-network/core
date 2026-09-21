//! What does the merge framework cost per applied delta, independent of what
//! the app's own merge does?
//!
//! It benches `merge_root_state_typed`, not `merge_root_state`: the latter's
//! registry dispatch is compiled in only under `wasm32`/`test`/`testing`, so a
//! plain `cargo bench` would measure the `NoFunctionsRegistered` shortcut
//! rather than a merge. The typed function is the shape the WASM-side
//! `#[app::state]` export calls, and is functionally equivalent to the closure
//! `register_crdt_merge` builds - same decode -> `Mergeable::merge` -> encode,
//! same bootstrap shortcut - but reachable without the registry. So what is
//! measured is the framework's encode/decode overhead around one
//! `Mergeable::merge` call, with no `TypeId` lookup in the timed path: an
//! optimization scoped from this number must not assume registry-lookup cost
//! is included.
//!
//! `existing_created_at` is passed different from `existing_ts` so every call
//! takes the real typed-merge branch rather than the bootstrap fast path that
//! clones `incoming` verbatim. `BenchState`'s linear `merge` stands in for a
//! real CRDT's own O(n) merge; the framework's overhead around it is the
//! subject, never a real app's merge logic.
//!
//! What would change a decision: framework cost that is a material fraction
//! of a real apply (say >1ms at 1k items) would make the encode/decode round
//! trip worth attacking. Framework overhead measured ~18us at n=1000 on the
//! pre-trie tree - this bench re-establishes that number on the current one.

use std::hint::black_box;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::{MergeError, MergeStrategy, Mergeable};
use calimero_storage::collections::rekey::RekeyTarget;
use calimero_storage::merge::merge_root_state_typed;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// A borsh payload of `items` string-keyed entries: a root state of that size
/// without depending on any particular collection's encoding.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
struct BenchState(Vec<(String, Vec<u8>)>);

impl BenchState {
    fn new(items: usize, salt: u8) -> Self {
        Self(
            (0..items)
                .map(|i| (format!("key{i:08}"), vec![salt; 32]))
                .collect(),
        )
    }
}

impl RekeyTarget for BenchState {
    // No nested collection ids to re-key: every field is a plain byte blob.
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

// A bench fixture with a hand-written rule, not an app type: the merge is
// fixed here, so it is not dispatched through a `CustomTypeId`.
impl MergeStrategy for BenchState {
    const DISPATCHED: bool = false;
}

impl Mergeable for BenchState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Both sides always carry the same, identically ordered key set, so a
        // single linear zip is a faithful O(n) stand-in for a real CRDT merge.
        for (mine, theirs) in self.0.iter_mut().zip(other.0.iter()) {
            mine.1.clone_from(&theirs.1);
        }
        Ok(())
    }
}

fn payload(items: usize, salt: u8) -> Vec<u8> {
    to_vec(&BenchState::new(items, salt)).expect("borsh encode of BenchState cannot fail")
}

fn merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_root_state");

    for items in [10_usize, 100, 1_000, 10_000] {
        let existing = payload(items, 1);
        let incoming = payload(items, 2);

        group.throughput(Throughput::Elements(items as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(items),
            &(existing, incoming),
            |b, (existing, incoming)| {
                b.iter(|| {
                    black_box(merge_root_state_typed::<BenchState>(
                        black_box(existing),
                        black_box(incoming),
                        // created_at != existing_ts: take the real typed-merge
                        // branch, not the bootstrap fast path.
                        1,
                        2,
                        3,
                    ))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, merge);
criterion_main!(benches);
