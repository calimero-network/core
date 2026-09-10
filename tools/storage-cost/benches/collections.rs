//! Tier 2: wall-clock throughput. REPORTING ONLY — never a merge gate.
//!
//! Two reasons this does not gate, both load-bearing:
//!
//! 1. It would not have caught core#3602. Against an in-memory store, an O(n)
//!    read pattern costs almost nothing in wall-clock, so the curve reads flat
//!    while real gas explodes. The gate for that is the cost snapshot.
//! 2. Criterion's default significance threshold is ~5%; shared CI runners
//!    routinely exceed that from cache state and neighbouring jobs alone. A
//!    gate that cries wolf gets muted, which is worse than no gate.
//!
//! What it IS good for: throughput curves over `n`, compared against a stored
//! baseline on master, so a real algorithmic win or loss is visible as a trend.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use storage_cost::measure;
use storage_cost::workloads::all;

fn collections(c: &mut Criterion) {
    let mut group = c.benchmark_group("collections");

    for workload in all() {
        // Elements-per-second rather than time-per-iteration: every workload
        // builds `n` entries, so the per-entry rate is the comparable number
        // across sizes. A rate that falls as `n` grows is the signal.
        let _ignored = group.throughput(Throughput::Elements(workload.n as u64));
        let _ignored = group.bench_function(format!("{}/{}", workload.name, workload.n), |b| {
            b.iter_batched(
                || workload.n,
                |n| {
                    let (result, _costs) = measure(|| (workload.run)(n));
                    black_box(result)
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, collections);
criterion_main!(benches);
