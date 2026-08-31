//! How does a child-trie operation scale with the number of children?
//!
//! Every collection write walks this structure (`crates/storage/src/child_trie.rs`),
//! so a per-operation cost that grows with `n` here is a cost that grows for
//! every write in the system — the shape of core#3602.
//!
//! What would change a decision: `insert` or `get` whose per-call time tracks
//! `n` rather than `log n`. `children()` IS expected to be linear (it
//! materialises every child); it is measured so a caller that starts using it
//! on a hot path shows up as a step change downstream.
//!
//! The gate for the same question in operation counts — which is what actually
//! decides gas — is `tools/storage-cost`. This bench is the wall-clock
//! companion and never gates.
//!
//! `ChildTrie` is generic over its `StorageAdaptor` and defaults to
//! `MainStorage`. On a native (non-wasm32) build — which is what a bench
//! compiles as — `MainStorage` itself routes to an in-process thread-local
//! mock (`calimero_storage::env`'s native `imp` module), so no explicit
//! adaptor wiring is needed here: the default type parameter already gives
//! this bench an in-memory backend.

use std::hint::black_box;

use calimero_storage::address::Id;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::entities::{ChildInfo, Metadata};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// Distinct children with fixed metadata; only the id and hash vary, which is
/// all the trie keys on.
fn child(i: u64) -> ChildInfo {
    let mut merkle_hash = [0_u8; 32];
    merkle_hash[..8].copy_from_slice(&i.to_le_bytes());
    merkle_hash[8..16].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_le_bytes());
    ChildInfo::new(
        Id::random(),
        merkle_hash,
        Metadata::new(1_700_000_000, 1_700_000_000),
    )
}

fn populated(n: usize) -> (ChildTrie, Vec<Id>) {
    let trie = ChildTrie::new(Id::random());
    let mut ids = Vec::with_capacity(n);
    for i in 0..n as u64 {
        let c = child(i);
        ids.push(c.id());
        let _hash = trie.insert(c);
    }
    (trie, ids)
}

fn child_trie(c: &mut Criterion) {
    let mut group = c.benchmark_group("child_trie");

    for n in [10_usize, 100, 1_000, 10_000] {
        let (trie, ids) = populated(n);

        group.throughput(Throughput::Elements(1));

        group.bench_with_input(BenchmarkId::new("insert", n), &n, |b, &n| {
            let mut next = n as u64;
            b.iter(|| {
                next += 1;
                black_box(trie.insert(child(next)))
            });
        });

        group.bench_with_input(BenchmarkId::new("get", n), &ids, |b, ids| {
            let mut cursor = 0_usize;
            b.iter(|| {
                cursor = cursor.wrapping_add(1);
                black_box(trie.get(ids[cursor % ids.len()]))
            });
        });

        group.bench_with_input(BenchmarkId::new("root", n), &trie, |b, trie| {
            b.iter(|| black_box(trie.root()));
        });

        // Linear by construction — measured so a caller that puts it on a hot
        // path is visible, not because it is expected to be flat.
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("children", n), &trie, |b, trie| {
            b.iter(|| black_box(trie.children().len()));
        });
    }

    group.finish();
}

criterion_group!(benches, child_trie);
criterion_main!(benches);
