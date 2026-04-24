//! Micro-benchmarks for the merkle-hash recompute path.
//!
//! Targets issue #2199: the suspicion is that `__calimero_sync_next`'s
//! merge-apply hot path is dominated by `calculate_full_hash_for_children`
//! in `crates/storage/src/index.rs`, which SHA256-loops over every child
//! of the node being updated — typically root. PR #2196's e2e data
//! showed two ~900ms apply outliers where `wasm_ms ≈ hold_ms`, but we
//! couldn't isolate which of the three structural extras on the merge
//! path actually dominated.
//!
//! This bench replicates the exact loop from
//! `Index::calculate_full_hash_for_children` (10 lines, inlined here
//! because the function is `fn`, not `pub`). If the children-count
//! scaling is linear and fast (say <50µs at N=10k), it rules out the
//! merkle rehash as the #2199 culprit and we know to look elsewhere
//! (nested WASM merge callback, extra storage read). If it's slow,
//! incremental-merkle maintenance becomes the obvious fix.
//!
//! # Running
//!
//! ```
//! cargo bench -p calimero-storage --bench merkle_rehash
//! ```
//!
//! Debug builds are ~20× slower than release on SHA256 and would lie
//! about the shape of the curve; criterion defaults to release so no
//! explicit `--release` is needed.

use calimero_storage::entities::{ChildInfo, Metadata};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use sha2::{Digest, Sha256};

/// Build a `Vec<ChildInfo>` of the requested size with deterministic-ish
/// but distinct hashes per child. We don't care about the contents, only
/// that each `merkle_hash()` returns a different 32-byte array — the
/// SHA256 cost of hashing them is what we're measuring.
fn build_children(n: usize) -> Vec<ChildInfo> {
    (0..n)
        .map(|i| {
            // Random Id so the children have plausible distinct identities.
            // The hash loop ignores the id anyway (only `merkle_hash`
            // contributes) but building the ChildInfo requires one.
            let id = calimero_storage::address::Id::random();
            let mut merkle_hash = [0u8; 32];
            merkle_hash[..8].copy_from_slice(&(i as u64).to_le_bytes());
            merkle_hash[8..16].copy_from_slice(&(i as u64).wrapping_mul(2654435761).to_le_bytes());
            let metadata = Metadata::new(1_700_000_000, 1_700_000_000);
            ChildInfo::new(id, merkle_hash, metadata)
        })
        .collect()
}

/// Mirrors `Index::calculate_full_hash_for_children` exactly:
/// SHA256(own_hash || child_hashes_in_order).
#[inline]
fn rehash(own_hash: [u8; 32], children: &[ChildInfo]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(own_hash);
    for child in children {
        hasher.update(child.merkle_hash());
    }
    hasher.finalize().into()
}

fn bench_merkle_rehash(c: &mut Criterion) {
    let mut group = c.benchmark_group("calculate_full_hash_for_children");

    // Sweep across realistic-to-pathological child counts. Anything above
    // ~10k is unlikely in current contexts but worth capturing the curve.
    for n in [1usize, 10, 100, 1_000, 10_000, 100_000] {
        let children = build_children(n);
        let own_hash = [0xABu8; 32];

        // Throughput gives us a per-child rate in addition to total time,
        // which is what tells us whether the scaling is actually linear.
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &children, |b, children| {
            b.iter(|| rehash(black_box(own_hash), black_box(children)));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_merkle_rehash);
criterion_main!(benches);
