# calimero-dag - Causal Delta DAG

Storage- and network-agnostic DAG for ordering state-change deltas by their parent references, applying each only once all its causal ancestors are applied.

## Package Identity

- **Crate**: `calimero-dag`
- **Entry**: `src/lib.rs` (single file; no submodules besides `#[cfg(test)]` ones)
- **Key deps**: `calimero-storage` (`HybridTimestamp` for HLC ordering), `borsh` + `serde` (dual wire formats for `CausalDelta`), `async-trait` (the `DeltaApplier` trait), `thiserror` (`ApplyError`/`DagError`), `tracing` (info/warn on capacity and eviction events)

## Commands

```bash
# Build
cargo build -p calimero-dag

# Test (all)
cargo test -p calimero-dag

# Test a single case
cargo test -p calimero-dag test_dag_out_of_order -- --nocapture
```

The `testing` feature (enabled via `calimero-storage`'s `testing` feature in `[dev-dependencies]`) unlocks `CausalDelta::new_test` and `DagStore::new_with_delta_query_limit`, both also available under `#[cfg(test)]`.

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `CausalDelta<T>` | struct | `id`, `parents: Vec<[u8; 32]>`, generic `payload: T`, `hlc: HybridTimestamp`, `expected_root_hash: [u8; 32]`, `kind: DeltaKind` |
| `CausalDelta::new(id, parents, payload, hlc, expected_root_hash)` | fn | Regular-kind constructor |
| `CausalDelta::checkpoint(id, expected_root_hash)` | fn | Snapshot-boundary delta: genesis parent, `T::default()` payload, `DeltaKind::Checkpoint` |
| `CausalDelta::is_checkpoint()` | fn | `kind == Checkpoint` |
| `CausalDelta::new_test(id, parents, payload)` | fn (test/`testing`) | Convenience ctor with default HLC and zeroed `expected_root_hash` |
| `DeltaKind` | enum | `Regular` \| `Checkpoint` |
| `DeltaApplier<T>` | async trait | `async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>` - dependency-injected application logic |
| `ApplyError` | enum (`#[non_exhaustive]`) | `Application(String)` |
| `DagError` | enum (`#[non_exhaustive]`) | `DuplicateDelta([u8; 32])`, `ApplyFailed(#[from] ApplyError)` |
| `AddDeltaOutcome` | enum | `Applied` \| `Pending` \| `Duplicate`, with `is_applied()`/`is_pending()`/`is_duplicate()` |
| `DagStore<T>` | struct | The DAG itself; see below |
| `DagStats` | struct | `total_deltas`, `applied_deltas`, `pending_deltas`, `head_count` |
| `PendingStats` | struct | `count`, `oldest_age_secs`, `total_missing_parents` |
| `MAX_DELTA_QUERY_LIMIT` | const | `3000` - hard cap on query-method result size |
| `MAX_PENDING_DELTAS` | const | `10_000` - default cap on the pending map |
| `MAX_PRUNED_TRACKED` | const | `100_000` - default cap on the pruned-ancestor tracking set |

### `DagStore<T>` methods

| Method | Purpose |
| --- | --- |
| `new(root)` | Empty DAG seeded with `root` as applied and as the sole head |
| `add_delta(delta, applier) -> Result<bool, DagError>` | Applies immediately, stores pending, or dedupes; `true` only on `Applied` |
| `add_delta_with_outcome(delta, applier) -> Result<AddDeltaOutcome, DagError>` | Same, but distinguishes `Pending` from `Duplicate` |
| `restore_applied_delta(delta) -> bool` | Registers topology for an already-applied delta (e.g. loaded from DB) without re-applying; does **not** cascade pending children |
| `try_process_pending(applier) -> Result<usize, DagError>` | Explicitly re-attempts every currently-pending delta; needed after `restore_applied_delta` |
| `get_heads() -> Vec<[u8; 32]>` | Current DAG tips (no children yet) |
| `get_missing_parents(query_limit) -> Vec<[u8; 32]>` | Parent ids referenced by pending deltas but absent from the DAG entirely - the backfill fetch list |
| `get_deltas_since(ancestor, start_ids, query_limit) -> (Vec<CausalDelta<T>>, Vec<[u8; 32]>)` | BFS from heads (or `start_ids`) back to `ancestor`; returns deltas plus a pagination cursor |
| `get_pending_delta_ids() -> Vec<[u8; 32]>` | Ids currently waiting on parents |
| `cleanup_stale(max_age) -> usize` | Evicts pending deltas older than `max_age` from both `pending` and `deltas` |
| `pending_stats() -> PendingStats` | Snapshot of pending-queue health |
| `has_delta(id) -> bool` / `is_applied(id) -> bool` / `get_delta(id) -> Option<&CausalDelta<T>>` | Lookups |
| `stats() -> DagStats` / `delta_count() -> usize` | Size accounting (the latter feeds compaction eligibility elsewhere in the node) |
| `prune_to_recent(retain_count) -> Vec<[u8; 32]>` | Drops applied history outside a BFS window back from the heads; returns pruned ids for the caller to delete from durable storage |
| `set_delta_query_limit(n)` / `set_max_pending(n)` | Runtime-adjustable caps (`set_max_pending` clamps to a minimum of 1) |

## Mental Model

A `DagStore<T>` tracks three disjoint-ish sets of delta ids: `applied` (causally resolved, in effect), `pending` (seen but blocked on a missing parent), and `deltas` (every delta body ever stored, applied or pending - "genuinely present" is `applied ∪ pending`). `heads` is the frontier: deltas with no children yet, and the parent set a caller should attach a new local delta to.

`add_delta` is the single entry point. `can_apply` decides readiness: every parent must be either the zero-hash genesis, a deliberately-pruned ancestor (see below), or both `applied` and still present in `deltas`. If ready, `apply_delta` calls into the injected `DeltaApplier<T>::apply`, marks the delta applied, and updates `heads` (parents drop out, the new delta becomes a head - so concurrent deltas from the same parent produce multiple heads until a later delta lists both as parents and merges them back to one). If not ready, the delta is stored in `pending` and indexed by `pending_children` (parent id -> waiting child ids).

Applying a delta never recurses into applying its now-unblocked children directly - `apply_delta` intentionally does not trigger a cascade itself (recursion there overflowed the tokio test stack on long chains; see `test_cascade_does_not_grow_stack`). Instead, every caller that can unblock pending deltas (`add_delta_with_outcome`, `try_process_pending`) drives `cascade_ready` with an explicit seed set and an iterative `VecDeque` worklist, so the whole cascade runs at constant stack depth regardless of chain length. `add_delta_with_outcome` seeds it narrowly - only the children of the delta just applied, taken out of `pending_children` entirely rather than cloned - since readiness for anything else in `pending` is unaffected by this one apply.

Out-of-order arrival is core, not an edge case: a delta whose parent hasn't shown up yet simply sits in `pending` until that parent arrives (via `add_delta` or `restore_applied_delta` + `try_process_pending`), at which point the reverse-edge index in `pending_children` lets the cascade visit only the deltas actually unblocked, without rescanning all of `pending`. `restore_applied_delta` is topology-only (used when loading already-applied deltas from persistent storage) and deliberately does not cascade - callers must follow up with `try_process_pending` if pending deltas might now be unblocked.

`get_deltas_since` is the sync-side counterpart: a BFS walk backward from the heads (or a resumed cursor) toward a stop `ancestor`, returning both the deltas found and an unexpanded-frontier cursor for pagination. Because `visited` is local to one call, a diamond-shaped DAG can re-emit the same delta across two pages - wasteful but harmless, since `add_delta` dedups by id on the receiving end.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: types, `DagStore`, and the in-file `basic_tests` module |
| `src/tests.rs` | The bulk of the unit test suite (pagination, pruning, eviction, concurrent branches, stress tests) |
| `src/tests_convergence.rs` | Regression tests replaying a real E2E root-hash-divergence bug: applying the same deltas in different orders on two simulated nodes must converge to the same state |

## Invariants and Gotchas

- **`kind` is a hand-decoded trailing field.** `BorshDeserialize` for `CausalDelta<T>` is written by hand (not derived) so a pre-`kind` delta on disk decodes cleanly as `DeltaKind::Regular` on end-of-input. This only works when the reader is bounded to exactly one delta's bytes (`from_slice`/`try_from_slice` or a length-delimited frame) - never decode a `CausalDelta` embedded mid-stream in a larger borsh aggregate (e.g. as one element of a multi-element `Vec<CausalDelta<T>>`), or trailing bytes get misread as the discriminant.
- **An empty `parents` list is valid and applies immediately.** `can_apply`'s `all()` over an empty iterator is vacuously true - this is intentional for genesis-style ops that implicitly descend from the DAG root, not a bug to "fix" by rejecting empty parents.
- **Genesis (`[0; 32]`) is always considered applied** and is never itself stored in `deltas`; it lives only in the initial `applied`/`heads` sets from `DagStore::new`.
- **Pruned parents count as satisfied, and are never requested for backfill.** `prune_to_recent` drops old applied history outside a BFS retention window from the heads, remembering dropped ids in `pruned` (bounded FIFO via `pruned_order`, capped at `MAX_PRUNED_TRACKED`). `can_apply` treats a pruned parent as satisfied (its ancestry was already applied before pruning) and `get_missing_parents` skips it - requesting it from a peer would be a wasted round trip since the peer likely dropped it too.
- **Zombie deltas are evicted on retry, not treated as duplicates.** A delta can end up in `deltas` without being in `applied` or `pending` if an in-flight apply future is cancelled (e.g. an outer `tokio::time::timeout`) after the insert but before `apply_delta`'s error path rolls it back. `add_delta_with_outcome` checks "genuinely present" (`applied` or `pending`, not just `deltas`) before returning `Duplicate`, and evicts+retries otherwise. The same rollback happens on an ordinary `ApplyError`.
- **`pending`, `pending_order`, and `pending_children` must stay in lockstep.** Always go through `insert_pending`/`remove_pending`/`evict_oldest_pending` rather than touching `self.pending` directly - they keep the arrival-order BTreeMap and the reverse-edge index consistent. Several regression tests exist purely to catch index leaks (`test_pending_order_index_no_leak_on_apply`, `test_pending_children_index_no_leak`, `test_seed_bucket_removed_with_still_blocked_child`).
- **`set_max_pending(0)` is clamped to 1**, not honored literally - a cap of 0 can never be satisfied since `add_delta` must store at least the delta it's currently handling.
- **Query result caps are best-effort, not errors.** `get_missing_parents` and `get_deltas_since` silently truncate to `delta_query_limit` (default `MAX_DELTA_QUERY_LIMIT`) and log a warning rather than erroring; pagination via the returned cursor is the caller's responsibility.

Part of [crates/](../AGENTS.md).
