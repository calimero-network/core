//! What do the pending-set walks cost when the pending set is big?
//!
//! A node that has been offline, or is behind a peer mid-catch-up, accumulates
//! deltas whose parents have not arrived. `get_missing_parents` is called to
//! build the next sync request, `cleanup_stale` on a timer, `pending_stats` for
//! metrics, and all three walk that set. If any of them is superlinear, a node
//! that falls behind gets slower at catching up precisely as it needs to be
//! faster.
//!
//! The pending set is filled through `add_delta_with_outcome`, not
//! `restore_applied_delta`: the latter marks a delta APPLIED, so a delta with
//! a missing parent never reaches `pending` and every number here would be a
//! measurement of an empty set. `add_delta_with_outcome` is `async`, but
//! `can_apply` fails before any `.await` on the pending path, so the
//! current-thread runtime in the fixture adds no executor overhead, and the
//! fixture is outside every timed closure regardless.
//!
//! Deltas are built with the ungated `CausalDelta::new` rather than
//! `new_test`, which needs the crate's `testing` feature: reaching that would
//! mean `required-features` on the `[[bench]]`, and a bare `cargo bench` then
//! silently SKIPS the target instead of building it. The two constructors
//! differ only in the default HLC, which the fixture passes explicitly.
//!
//! Each sub-benchmark sees a pending set of exactly its `n`. The two read-only
//! walks share one fixture; `cleanup_stale` evicts, so it rebuilds per sample
//! rather than draining one shared fixture across the batch.
//!
//! What would change a decision: a walk that grows faster than the set, which
//! would make an index over pending parents worth building.

use std::hint::black_box;
use std::time::Duration;

use calimero_dag::{AddDeltaOutcome, ApplyError, CausalDelta, DagStore};
use calimero_storage::logical_clock::HybridTimestamp;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

/// Applier that is never actually invoked: every delta built by
/// [`pending_dag`] names a parent that is not in the DAG, so
/// `add_delta_with_outcome` always takes the pending path and `apply` is
/// never called. Exists only to satisfy `DeltaApplier`'s bound.
struct NeverApplied;

#[async_trait::async_trait]
impl calimero_dag::DeltaApplier<Vec<u8>> for NeverApplied {
    async fn apply(&self, _delta: &CausalDelta<Vec<u8>>) -> Result<(), ApplyError> {
        unreachable!("fixture deltas never satisfy can_apply, so apply() is never reached")
    }
}

/// A pending set of `n` deltas, each depending on a parent that never
/// arrives, so nothing applies and everything stays pending.
fn pending_dag(n: usize) -> DagStore<Vec<u8>> {
    let mut dag = DagStore::new([0_u8; 32]);
    let applier = NeverApplied;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("building a current-thread runtime cannot fail");

    for i in 0..n as u64 {
        let mut id = [0_u8; 32];
        id[..8].copy_from_slice(&i.to_le_bytes());
        let mut parent = [0xFF_u8; 32];
        parent[..8].copy_from_slice(&i.to_le_bytes());
        let delta = CausalDelta::new(id, vec![parent], vec![0_u8; 64], HybridTimestamp::default());
        let outcome = rt.block_on(dag.add_delta_with_outcome(delta, &applier));
        assert!(
            matches!(outcome, Ok(AddDeltaOutcome::Pending)),
            "fixture delta did not land in the pending set"
        );
    }
    dag
}

fn pending(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag");

    for n in [10_usize, 100, 1_000, 10_000] {
        let dag = pending_dag(n);

        // Proof the fixture is real: pending_stats must report exactly n
        // pending deltas, not zero. See module docs above.
        assert_eq!(dag.pending_stats().count, n, "fixture pending count != n");

        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(
            BenchmarkId::new("get_missing_parents", n),
            &dag,
            |b, dag| {
                b.iter(|| black_box(dag.get_missing_parents(black_box(128)).len()));
            },
        );

        group.bench_with_input(BenchmarkId::new("pending_stats", n), &dag, |b, dag| {
            b.iter(|| black_box(dag.pending_stats()));
        });

        // A shared fixture would shrink on every iteration, leaving only the
        // first sample measuring n, so rebuild an n-sized set per batch.
        group.bench_with_input(BenchmarkId::new("cleanup_stale", n), &n, |b, &n| {
            b.iter_batched(
                || pending_dag(n),
                |mut dag| black_box(dag.cleanup_stale(Duration::from_secs(0))),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, pending);
criterion_main!(benches);
