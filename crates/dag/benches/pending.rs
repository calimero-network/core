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
//!
//! # `get_missing_parents` is bounded work, not `n`-proportional work
//!
//! `get_missing_parents(query_limit)` breaks out of its walk over
//! `self.pending` as soon as it has collected
//! `min(query_limit, self.delta_query_limit)` missing parent ids (`:861` —
//! `if missing_ids.len() >= delta_query_limit { break 'outer; }`). This bench
//! calls it with `query_limit = 128` against a `DagStore` built with the
//! default `delta_query_limit` (`MAX_DELTA_QUERY_LIMIT = 3000`), so the walk
//! is capped at 128 missing-parent ids REGARDLESS of how big the pending set
//! is. At `n = 10_000` the reported ~515 Melem/s is `n` divided by a runtime
//! that is actually governed by the 128-id cap, not by `n` — it is not a
//! throughput number at all, it is "how fast can 128 ids be collected",
//! relabelled using an `n` the call never walks in full. It only becomes an
//! honest throughput figure at `n <= 128`, where the cap has not yet
//! engaged. The group's `group.throughput(Throughput::Elements(n as u64))`
//! is overridden below for this one sub-benchmark to declare
//! `min(n, query_limit)` instead — the actual bounded element count the call
//! does work over — so the reported elem/s stops silently flattening at the
//! cap and instead reads as what it is: roughly flat past `n = 128`, because
//! the work IS flat past `n = 128`.
//!
//! `pending_stats` and `cleanup_stale` are NOT bounded this way — both walk
//! (or drain) the full pending set — so `Throughput::Elements(n)` stays
//! correct for them and is left alone.
//!
//! # The more useful measurement: sweep `query_limit`, not `n`
//!
//! Since the real independent variable behind `get_missing_parents`' cost is
//! `min(query_limit, delta_query_limit)`, not `n`, `get_missing_parents_by_limit`
//! below fixes the pending set at a single large size (`n = 10_000`, already
//! deep in the capped regime for every `query_limit` this sweeps) and varies
//! `query_limit` instead. That is the sweep that actually answers "what does
//! this call cost", where the `n`-sweep above (at a fixed `query_limit =
//! 128`) mostly answers "is 10,000 pending entries slower to WALK THE FIRST
//! 128 IDS OF than 10 pending entries is" — a much narrower question, and
//! one this bench already answers by n=128 in the sweep above.

use std::hint::black_box;
use std::time::Duration;

use calimero_dag::{AddDeltaOutcome, ApplyError, CausalDelta, DagStore};
use calimero_storage::logical_clock::HybridTimestamp;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

/// `query_limit` this bench passes to `get_missing_parents` in the `n`-sweep
/// above — the number that actually bounds its work once `n` exceeds it. See
/// the module doc's "bounded work, not n-proportional work" section.
const GET_MISSING_PARENTS_QUERY_LIMIT: usize = 128;

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

        // `get_missing_parents` walks at most `min(n, GET_MISSING_PARENTS_QUERY_LIMIT)`
        // pending entries — see the module doc's "bounded work, not
        // n-proportional work" section — so its throughput is declared over
        // that bounded count, not over `n`.
        group.throughput(Throughput::Elements(
            n.min(GET_MISSING_PARENTS_QUERY_LIMIT) as u64
        ));
        group.bench_with_input(
            BenchmarkId::new("get_missing_parents", n),
            &dag,
            |b, dag| {
                b.iter(|| {
                    black_box(
                        dag.get_missing_parents(black_box(GET_MISSING_PARENTS_QUERY_LIMIT))
                            .len(),
                    )
                });
            },
        );

        // `pending_stats` and `cleanup_stale` (below) both walk/drain the
        // FULL pending set, so `n` is their real throughput denominator.
        group.throughput(Throughput::Elements(n as u64));
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

/// The more useful sweep for `get_missing_parents`: fix the pending set at a
/// single large `n` and vary `query_limit`, the parameter that actually
/// bounds the call's work — see the module doc's "the more useful
/// measurement" section.
fn get_missing_parents_by_limit(c: &mut Criterion) {
    const N: usize = 10_000;
    let dag = pending_dag(N);
    assert_eq!(dag.pending_stats().count, N, "fixture pending count != N");

    let mut group = c.benchmark_group("dag_get_missing_parents_by_query_limit");
    for query_limit in [10_usize, 128, 500, 1_000, 3_000] {
        group.throughput(Throughput::Elements(query_limit.min(N) as u64));
        group.bench_with_input(
            BenchmarkId::new("get_missing_parents", query_limit),
            &dag,
            |b, dag| {
                b.iter(|| black_box(dag.get_missing_parents(black_box(query_limit)).len()));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, pending, get_missing_parents_by_limit);
criterion_main!(benches);
