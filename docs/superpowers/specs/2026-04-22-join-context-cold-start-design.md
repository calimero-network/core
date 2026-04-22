# Design: fix cold-start `join_context` failure with queued gossip deltas

- **Issue:** calimero-network/core#2198
- **Date:** 2026-04-22
- **Scope:** `crates/node/src/sync/manager/mod.rs`

## Problem

`join_context` fails deterministically on the first cold-start attempt of a
subgroup-owned context when gossip deltas have already arrived before the
joining node can sync. Two symptoms fire together, both within ~30ms of the
`join_context` call:

1. **Deltas stuck pending.** The joining node has persisted N gossip deltas
   (N=19 in the mero-drive repro) whose parents are not in the local DB.
   `delta_store::load_persisted_deltas` logs `remaining_count=N loaded_count=0`
   at `crates/node/src/delta_store.rs:619-625`. The sync session returns
   without pulling those parents.
2. **Stream bail on unrelated context.** A peer opens an inbound sync stream
   whose `StreamMessage::Init.context_id` is neither the registry context nor
   the docs context being joined — it is a third ID (a subgroup/app-internal
   context). The receiver bails at
   `crates/node/src/sync/manager/mod.rs:2207-2209`
   (`bail!("context not found: {}", context_id)`), which tears down the
   stream and surfaces an error on the active sync path.

Today, the `mero-drive` e2e workflow flakes on its first attempt because of
this issue. Fixing both symptoms is expected to make the e2e deterministic.

## Non-goals

- Restructuring how `join_context` materializes subgroup hierarchies before
  subscribing. If the inbound-stream fix in §C is insufficient, this is the
  follow-up — tracked separately.
- Changing `join_context`'s API contract. On success, the context remains
  fully usable (DAG applied, reads return consistent state).
- Changing gossip broadcast semantics.

## Fix B — aggressive cross-peer parent pull

**Where:** `crates/node/src/sync/manager/mod.rs`, the
`request_dag_heads_and_sync` path (around lines 1819-1860) and the sync
session driver.

**Current behaviour.** After requesting DAG heads, the code calls
`delta_store_ref.get_missing_parents().await` and, if any are missing, calls
`self.request_missing_deltas(...)` against the **single** peer that sourced
the DAG heads. If that peer doesn't hold the missing parents, the fetch
returns `Ok(())` (see `request_missing_deltas` in
`crates/node/src/sync/delta_request.rs`) and the outer function returns
`Ok(SyncProtocol::DeltaSync { missing_delta_ids: vec![] })` — reporting
success even though `delta_store` still has pending deltas.

**Target behaviour.** The sync session reports success only when
`delta_store.get_pending_delta_ids()` is empty for the context, or reports a
**typed failure** when a bounded retry budget is exhausted.

**Algorithm.**

1. After the current `request_missing_deltas` call, re-check
   `delta_store.get_missing_parents().await`.
2. If `missing_ids` is non-empty, iterate **other known mesh peers** for the
   context (via the same peer-enumeration path used by peer-discovery
   fallback in #2170) and call `request_missing_deltas` against each,
   re-checking `get_missing_parents` after each attempt.
3. Budget: up to 3 additional peers, bounded by a total wall-clock deadline
   (default 10s, configurable via `SyncManager` constant). The existing
   single-peer attempt counts as one within the budget.
4. Exit conditions:
   - `missing_ids` becomes empty → return
     `SyncProtocol::DeltaSync { missing_delta_ids: vec![] }` (success).
   - Budget exhausted with `missing_ids` still non-empty → return
     `Err(SyncError::PendingParentsUnresolved { context_id, remaining })`.
     The sync session driver surfaces this as a real failure to the caller
     of `node_client.sync(...)`, so `join_context` fails fast with an
     actionable error instead of silently succeeding on a partially-applied
     DAG.

**Peer enumeration.** Reuse whatever `peer-discovery` already exposes for
the namespace-root fallback introduced in #2170 — do not duplicate its peer
store or discovery logic. If that helper is not in
`sync/manager/mod.rs`, thread it through the existing `NodeClients`.

**No `join_context` changes.** `join_context` keeps its current sequence
(`subscribe` → `sync`). The improvement is entirely inside sync.

## Fix C (surgical) — don't tear down the session on unrelated streams

**Where:** `crates/node/src/sync/manager/mod.rs:2207-2209`, inside
`internal_handle_opened_stream`.

**Current behaviour.**

```rust
let Some(context) = self.context_client.get_context(&context_id)? else {
    bail!("context not found: {}", context_id);
};
```

A single unknown-context stream failing propagates as a full session
error, which is what surfaces on the `join_context` return path within
~30ms.

**Target behaviour.** The stream is closed cleanly; the active sync is
unaffected.

**Change.**

1. Replace the `bail!` with:
   - `warn!` including `context_id`, the peer, and the stream-local
     fields already in scope, with a short message like
     `inbound stream for unknown context, closing`.
   - Send `StreamMessage::OpaqueError` to the peer (same helper used at
     `mod.rs:2151-2156`), so the peer knows the stream is dead.
   - `return Ok(None)` from `internal_handle_opened_stream`, matching the
     existing "no message" exit at `mod.rs:2163-2165` — this is the signal
     the dispatcher treats as "stream ended, move on."
2. Do **not** touch the existing stream path for known contexts. No
   membership-check changes. No `sync_context_config` on receive.

**Why this is safe.** The stream is peer-initiated. Closing it does not
affect our in-flight outbound sync for the docs context, and does not
affect delta_store. The peer can re-open the stream later if the context
becomes known.

## Failure semantics

| Scenario | Before | After |
|---|---|---|
| Peer has all parents | Sync succeeds | Same |
| Peer lacks parents, another peer has them | Sync succeeds silently; `delta_store` still has pending deltas | Sync pulls from other peer; succeeds with pending=0 |
| No peer has parents within budget | Sync reports success; `join_context` returns success; later reads hit unapplied DAG | Sync returns typed error; `join_context` returns a real error; caller can retry |
| Inbound stream for unknown context | Session fails; `join_context` errors | Stream closed; active sync unaffected |

## Testing

**Rust integration test** — added in `crates/node`, wired via
`cargo test -p calimero-node`:

1. `test_cold_start_pending_parents_pull_from_second_peer`
   - Stage node A with a ≥20-delta DAG for a context.
   - Stage node B fresh. Pre-seed B's `delta_store` with the 19 child
     deltas only (parent missing), mirroring the gossip-before-sync state.
   - Stage node C with the full DAG (acts as the second peer).
   - Trigger B's sync with A as the initial peer (A holds only the tip,
     not the missing parents — simulated by restricting A's response).
   - Assert: `delta_store.get_pending_delta_ids(ctx).is_empty()` before
     the sync session reports success.
   - Assert: the session calls `request_missing_deltas` against C.

2. `test_cold_start_pending_parents_budget_exhausted`
   - Same staging as (1), but no peer holds the missing parents.
   - Assert: sync returns the typed error
     `SyncError::PendingParentsUnresolved`, not `Ok(SyncProtocol::…)`.
   - Assert: total wall-clock does not exceed the configured deadline.

3. `test_inbound_stream_unknown_context_does_not_kill_session`
   - Node B has context `docs_ctx` and a concurrent inbound sync in
     flight for it.
   - Inject a second inbound stream whose `StreamMessage::Init.context_id`
     is a random ID unknown to B.
   - Assert: the unknown-context stream handler returns `Ok(None)` and
     sends `OpaqueError`.
   - Assert: the concurrent legitimate sync for `docs_ctx` completes
     successfully.

**mero-drive e2e** — secondary smoke test, not a gate. Expected to pass
deterministically on first attempt after this fix; if it still flakes,
the remaining variance is out of scope for this spec and reopens the
question of whether §C needs the broader materialization-order change.

## Out of scope (deferred)

- **Subgroup-hierarchy pre-materialization in `join_context`.** If the
  surgical §C is not enough and the `context not found` error still drives
  flakes, the follow-up is: in
  `crates/context/src/handlers/join_context.rs:182-183`, resolve and
  `sync_context_config` for the joining node's subgroup-internal contexts
  *before* `node_client.subscribe(...)` so no gossip window exists where a
  stream can arrive for an un-materialized context. Not done now — ship B
  + surgical C first, measure, then decide.
- **Delta-store-level self-healing.** A background task that re-requests
  missing parents independent of sync sessions is a larger design
  question. Current sync-session-bounded pulling is sufficient for the
  reported failure.

## Rollout

- Single PR touching only `crates/node/src/sync/manager/mod.rs` (+ a
  typed error in the existing sync error enum).
- No config migration. No protocol-level changes. No API-surface changes.
- Revertable as a single commit.
