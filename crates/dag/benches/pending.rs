//! What do the pending-set walks cost when the pending set is big?
//!
//! A node that has been offline, or is behind a peer mid-catch-up, accumulates
//! deltas whose parents have not arrived. `get_missing_parents` is called to
//! build the next sync request, `cleanup_stale` on a timer, `pending_stats` for
//! metrics — all three walk that set. If any of them is superlinear, a node
//! that falls behind gets slower at catching up precisely as it needs to be
//! faster.
//!
//! The brief this bench was written from named the DAG type `Dag` and the
//! parking entry point `restore_applied_delta`. Neither survived contact with
//! the tree: the type is `DagStore<T>`, and `restore_applied_delta` (`:468`)
//! marks a delta as *applied* — it exists to replay already-applied history
//! from storage, not to park anything. Feeding it a delta with a missing
//! parent does not put that delta in the pending set at all, which would have
//! made every number below a measurement of an empty set. The real door is
//! `add_delta_with_outcome` (`:538`): given a delta whose parent is not in the
//! DAG, `can_apply` fails and it lands in `pending` via `insert_pending`.
//! `add_delta_with_outcome` is `async` (the DAG's only async surface,
//! `apply_delta` at `:737`), but it is never actually awaited on the pending
//! path — `can_apply` fails before any `.await` point is reached — so driving
//! it through a bare current-thread `tokio` runtime in the fixture setup adds
//! no meaningful executor overhead, and the setup itself is outside every
//! timed closure below regardless.
//!
//! The brief's fixture built each `CausalDelta` with `CausalDelta::new_test`,
//! which is `#[cfg(any(test, feature = "testing"))]` — unreachable from a
//! bench binary (no `cfg(test)`) without the crate's own `testing` feature
//! turned on. Rather than pull that feature in (which would need either
//! `required-features` on the `[[bench]]`, making a bare `cargo bench -p
//! calimero-dag --bench pending` silently skip the target instead of
//! building it, or a self-referential dev-dependency to force it on), the
//! fixture uses the crate's ungated public constructor,
//! `CausalDelta::new(id, parents, payload, hlc)` (`:135`), passing
//! `HybridTimestamp::default()` for the one field `new_test` filled in for
//! free. That is the only difference between the two constructors — `new_test`
//! is a convenience wrapper around `new`, not a distinct code path — so this
//! fixture exercises the exact same `CausalDelta` shape without needing the
//! feature at all.
//!
//! Each sub-benchmark measures a pending set of exactly the size its `n`
//! claims: `get_missing_parents` and `pending_stats` are read-only and reuse
//! one fixture across all their iterations, but `cleanup_stale` mutates (it
//! evicts), so it rebuilds a fresh `n`-sized pending set per sample via
//! `iter_batched` rather than draining one shared fixture across the batch.
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

        // cleanup_stale mutates (it evicts), so unlike the two read-only
        // benches above it cannot reuse one shared fixture across samples --
        // that would shrink the pending set on every iteration, so only the
        // first sample would actually measure n. Rebuild a fresh n-sized
        // pending set per batch instead, so every measured call sees exactly
        // n pending deltas.
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
