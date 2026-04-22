//! Micro-benchmark for `merge::merge_root_state`.
//!
//! Targets issue #2199 suspect (b) from PR #2196's investigation: the
//! nested `merge_root_state` WASM callback invoked from inside
//! `try_merge_data` during a merge-scenario apply. Each call does:
//! 1. Borsh-deserialize `existing` and `incoming` payloads.
//! 2. Call the registered `Mergeable::merge`.
//! 3. Borsh-serialize the result.
//!
//! Scaling with payload size is what we want to know. If merge_root_state
//! is fast at realistic payload sizes (hundreds of entries), we've
//! ruled out another #2199 suspect and the investigation narrows to the
//! extra storage read in `try_merge_data` (suspect a). If it's slow,
//! the fix candidate becomes "reduce deserialization/serialization cost
//! on the merge path" — e.g., structural sharing, avoid full
//! round-trip.
//!
//! # Why a custom test type
//!
//! `merge_root_state` needs a registered merge function (via
//! `register_crdt_merge::<T>()`) to do anything useful — unregistered
//! types return `NoMergeFunctionRegistered`. We register a minimal
//! `BenchState { values: Vec<u64> }` with a trivial "take union" merge,
//! which is representative of set-shaped CRDT merges and cheap enough
//! that the bench measures the framework cost (dispatch + borsh), not
//! the merge logic itself.
//!
//! # Running
//!
//! ```
//! cargo bench -p calimero-storage --bench merge_root_state
//! ```

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::Mergeable;
use calimero_storage::merge::{merge_root_state, register_crdt_merge};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Once;

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct BenchState {
    values: Vec<u64>,
}

impl Mergeable for BenchState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Intentionally O(|other|) — we want the bench to measure
        // framework overhead (borsh deserialize → dispatch → borsh
        // serialize), NOT the inner merge's dedup cost. A naive
        // `contains`-based union is O(n*m) and would drown out the
        // framework cost at large N. Real CRDTs use HashSet-style
        // lookup or sorted structures; either way the merge itself is
        // app-owned. This bench answers: "how much does the framework
        // add on top of whatever the app's merge does?"
        self.values.extend_from_slice(&other.values);
        Ok(())
    }
}

static REGISTER: Once = Once::new();

fn ensure_registered() {
    REGISTER.call_once(|| {
        register_crdt_merge::<BenchState>();
    });
}

fn encode_state(n: usize) -> Vec<u8> {
    let state = BenchState {
        values: (0..n as u64).collect(),
    };
    borsh::to_vec(&state).expect("bench state should serialize")
}

fn bench_merge_root_state(c: &mut Criterion) {
    ensure_registered();

    let mut group = c.benchmark_group("merge_root_state");

    // Sweep payload complexity. Production contexts hold hundreds to
    // low-thousands of items in a typical root state, so N in that
    // range is the most operationally relevant; the 10k/100k tail
    // tells us about the scaling shape.
    for n in [1usize, 10, 100, 1_000, 10_000] {
        let existing = encode_state(n);
        // Incoming has a small (~10%) overlap with existing to exercise
        // the Mergeable::merge path without making it purely O(n^2).
        let incoming_state = BenchState {
            values: (n as u64 / 2..n as u64 + n as u64 / 2).collect(),
        };
        let incoming = borsh::to_vec(&incoming_state).expect("bench state should serialize");

        group.throughput(Throughput::Bytes(existing.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(existing, incoming),
            |b, (existing, incoming)| {
                b.iter(|| {
                    let merged = merge_root_state(
                        black_box(existing),
                        black_box(incoming),
                        black_box(1_700_000_000),
                        black_box(1_700_000_001),
                    )
                    .expect("registered merge should succeed");
                    black_box(merged);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_merge_root_state);
criterion_main!(benches);
