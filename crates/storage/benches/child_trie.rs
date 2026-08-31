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
//!
//! # What `{n}` means for each id — read this before reading the numbers
//!
//! `get/{n}`, `root/{n}` and `children/{n}` each build **one fresh trie of
//! exactly `n` children** and only then start the timed loop, so every read
//! in that group is against a trie whose population is exactly `n` — none of
//! them mutate the trie they read.
//!
//! `insert/{n}` is inherently mutating, so it cannot share that trie: instead
//! it uses `iter_batched` with `BatchSize::PerIteration`, which builds a
//! freshly made `n`-child trie AND the `ChildInfo` to be inserted
//! (`Id::random()` + `Metadata::new()`) in the (unmeasured) setup closure
//! before every single timed call, and the timed closure does nothing but
//! the `insert` itself. So `insert/{n}` measures "the cost of the `n+1`th
//! insert into a trie that already holds `n` children" — not the cost of
//! minting the child to insert, and not contaminated by any other insert in
//! the same run.
//!
//! An earlier version of this bench built one trie per `n` and reused it,
//! unguarded, across `insert`/`get`/`root`/`children`. Because `insert`'s own
//! timed closure runs thousands of times during criterion's
//! calibration+sampling, that shared trie kept growing *during* the `insert`
//! sub-benchmark — so by the time `get`/`root`/`children` ran against it,
//! their actual population was `n` plus however many extra inserts criterion
//! happened to perform, not `n`. That produced a `children/{n}` curve with no
//! relationship to `n` (`children/10` and `children/1000` came out the same
//! order of magnitude). See `task-2-3-report.md`'s fix section for the
//! contaminated numbers next to the corrected ones — the difference is itself
//! evidence for how much a shared, mutated fixture can lie.

use std::hint::black_box;

use calimero_storage::address::Id;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::entities::{ChildInfo, Metadata};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

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

/// A trie with exactly `n` children, and the ids of every one of them.
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
        group.throughput(Throughput::Elements(1));

        // `insert/{n}`: cost of one insert into a trie that already holds
        // exactly `n` children. `PerIteration` puts a fresh `n`-child build
        // AND the `ChildInfo` to be inserted in the unmeasured setup closure
        // before every single timed call, so neither the trie build nor
        // `child()`'s `Id::random()` + `Metadata::new()` land in the timed
        // path — only the `insert` call itself does — and the measured
        // trie's population never drifts above `n` the way a shared, reused
        // trie would.
        group.bench_with_input(BenchmarkId::new("insert", n), &n, |b, &n| {
            b.iter_batched(
                || (populated(n).0, child(n as u64 + 1)),
                |(trie, c)| black_box(trie.insert(c)),
                BatchSize::PerIteration,
            );
        });

        // `get/{n}`, `root/{n}`, `children/{n}`: all read-only, so one fresh
        // `n`-child trie built once (outside the timed loop) can be shared
        // safely across all three — none of them mutates it, so its
        // population stays exactly `n` for the whole group.
        let (trie, ids) = populated(n);

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
