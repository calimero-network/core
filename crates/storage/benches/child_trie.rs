//! How does a child-trie operation scale with the number of children?
//!
//! The operation-count gate for the same question is `tools/storage-cost`.

use std::hint::black_box;

use calimero_storage::address::Id;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::entities::{ChildInfo, Metadata};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

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
        group.throughput(Throughput::Elements(1));

        // `PerIteration` keeps the build out of the timed path and the trie at `n`.
        group.bench_with_input(BenchmarkId::new("insert", n), &n, |b, &n| {
            b.iter_batched(
                || (populated(n).0, child(n as u64 + 1)),
                |(trie, c)| black_box(trie.insert(c)),
                BatchSize::PerIteration,
            );
        });

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

        // `children()` is linear by construction, hence per-child throughput.
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("children", n), &trie, |b, trie| {
            b.iter(|| black_box(trie.children().len()));
        });
    }

    group.finish();
}

criterion_group!(benches, child_trie);
criterion_main!(benches);
