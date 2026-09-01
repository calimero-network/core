//! What does the merge framework cost per applied delta, independent of what
//! the app's own merge does?
//!
//! `merge::merge_root_state` (`crates/storage/src/merge.rs:200`) is on the
//! path of every delta a node applies, but its own dispatch step
//! (`try_merge_registered`, and the `register_crdt_merge` needed to seed it)
//! is compiled in only under `#[cfg(any(target_arch = "wasm32", test, feature
//! = "testing"))]` — a plain `cargo bench` (no `--features testing`) builds
//! neither, so calling `merge_root_state` itself here would just measure the
//! `NoFunctionsRegistered` bootstrap/error shortcut, not a merge.
//!
//! This benches `merge_root_state_typed` (`crates/storage/src/merge.rs:125`)
//! instead. It is NOT a function `merge_root_state` calls: when the registry
//! dispatch path IS compiled in, `merge_root_state` -> `try_merge_registered`
//! -> the `merge_fn` closure built by `register_crdt_merge`
//! (`crates/storage/src/merge/registry.rs:224-249`), which is an
//! independent, hand-duplicated decode -> `with_merge_mode(merge)` -> encode
//! implementation — it never calls `merge_root_state_typed`
//! (`grep -rn merge_root_state_typed crates/storage/src/` turns up only test
//! call sites and a doc comment about the WASM-side macro export).
//! `merge_root_state_typed` is instead the shape the WASM-side
//! `#[app::state]`-generated `__calimero_merge_root_state` export calls, and
//! is *functionally equivalent* to what the registry closure duplicates —
//! same decode -> `Mergeable::merge` -> encode steps, same
//! `existing_created_at == existing_ts` bootstrap shortcut — without needing
//! the registry, `TypeId`, or any feature flag to reach it. So what is
//! measured here is the framework's encode/decode overhead around one
//! `Mergeable::merge` call: there is no `TypeId` lookup or trial-deserialize
//! in the timed path, so registry-lookup cost is NOT included, and an
//! optimization scoped from this number should not assume it is.
//!
//! `merge_root_state_typed`'s real signature (read from the source before
//! writing this bench — it differs from an earlier draft that assumed
//! `Option<&[u8]>` existing state and a two-argument call):
//!
//! ```ignore
//! pub fn merge_root_state_typed<T>(
//!     existing: &[u8],
//!     incoming: &[u8],
//!     existing_created_at: u64,
//!     existing_ts: u64,
//!     _incoming_ts: u64,
//! ) -> Result<Vec<u8>, MergeError>
//! where
//!     T: BorshSerialize + BorshDeserialize + Mergeable;
//! ```
//!
//! `existing_created_at` is set different from `existing_ts` below, so every
//! call takes the real typed-merge branch, not the `existing_created_at ==
//! existing_ts` bootstrap fast path that just clones `incoming` verbatim.
//!
//! `T` needs a `Mergeable` impl to call `merge` on (there is no
//! registry involved here — `merge_root_state_typed` is generic and calls
//! `T::merge` directly), so this bench defines a minimal local `BenchState`
//! with an O(n) `merge`: a linear zip of `existing`/`incoming`'s identically
//! ordered, identically sized key sets, standing in for a real CRDT's own
//! O(n) merge (e.g. `Counter::merge`, `Set::merge`). What is being measured
//! is the framework's overhead around that call, never a real app's merge
//! logic.
//!
//! What would change a decision: framework cost that is a material fraction
//! of a real apply (say >1ms at 1k items) would make the encode/decode round
//! trip worth attacking. #2203 measured ~18us at n=1000 on the pre-trie tree
//! — this bench re-establishes that number on the current one.

use std::hint::black_box;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::{MergeError, Mergeable};
use calimero_storage::collections::rekey::RekeyTarget;
use calimero_storage::merge::merge_root_state_typed;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// A borsh payload of `items` string-keyed entries — stands in for a root
/// state of that size without depending on any particular collection's
/// encoding.
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
    // No nested collection ids to re-key — every field is a plain byte blob.
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

impl Mergeable for BenchState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // `existing`/`incoming` in this bench always carry the same,
        // identically ordered key set (see `BenchState::new`), so a single
        // linear zip is a faithful O(n) stand-in for a real CRDT merge (e.g.
        // `Counter::merge` summing, `Set::merge` unioning) — not a claim
        // about this being anyone's actual merge semantics.
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
