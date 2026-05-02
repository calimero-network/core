//! Handler-gating policy: when do application event handlers fire?
//!
//! # Background
//!
//! Each `CausalDelta` may carry an `events: Option<Vec<u8>>` payload — the
//! output of `__calimero_emit` calls inside the WASM app. When a delta is
//! processed on a node, two things can happen:
//!
//! 1. **WASM executes** (`__calimero_sync_next`): actions apply to storage,
//!    state mutates. This is *always* run; it's how the CRDT converges.
//! 2. **Event handlers fire**: the `events` payload is decoded and
//!    dispatched to application-level callbacks. This is a *side effect* —
//!    it doesn't affect on-chain state, only observer logic (UI updates,
//!    external integrations, log sinks, etc).
//!
//! The question is whether step 2 should run unconditionally, or only when
//! the node is "live" — i.e. not catching up from a gap. Firing stale
//! handlers while a node replays hours of history after an outage is
//! usually wrong: every catch-up node would re-trigger integrations,
//! user-facing notifications would fire for events that happened long ago,
//! and external systems would be double-invoked.
//!
//! # The "behind" predicate
//!
//! A node is considered "behind" for a given `(context_id, delta_hlc)` pair
//! at the moment handlers would fire if **either**:
//!
//! * **Sync session is active** for `context_id` — the node is explicitly
//!   catching up from a peer (snapshot pull / bulk sync). Any delta
//!   dispatched during this window is by construction replay.
//!
//! * **HLC staleness**: `max_seen_hlc(context_id) - delta_hlc` exceeds the
//!   configured `handler_staleness_threshold`. The max is the highest HLC
//!   observed for this context across *any* arrival path (pubsub, sync
//!   fetch, cascade). Using max-seen-HLC rather than wall-clock avoids
//!   dependency on clock synchronization with peers.
//!
//! When the predicate returns `true`, handlers are skipped and the DB
//! `events` blob is cleared via `mark_events_executed` — the delta is
//! marked `applied: true, events: None` so startup replay doesn't
//! resurrect it.
//!
//! # Threshold semantics
//!
//! | Threshold | Behavior |
//! | --- | --- |
//! | `0 ms` | Strictest. Handlers fire only when the gap to the frontier rounds to `0 ms`. Sub-millisecond gaps still fire (predicate operates at ms precision); any gap ≥ 1 ms skips. Use when "only live pubsub receipts" is the desired semantic. |
//! | `50–200 ms` | Race-tolerant. Sub-perceptual reorderings still fire; multi-second catch-up does not. Reasonable for UI-style "notify me on live events." |
//! | `5 s` (default) | Generous live window. Tolerates normal network jitter and brief pending-cascade windows. |
//! | `> 30 s` | Loose. Effectively "fire handlers unless we're in a sync session." Approximates pre-gating behavior. |
//!
//! `0` is a legal value, not a sentinel for "disabled." To disable gating
//! entirely, use `Duration::MAX`.
//!
//! # Scenario table
//!
//! All scenarios assume the context has an event-emitting app installed.
//!
//! | # | Scenario | Sync-session? | HLC gap | `is_behind` | Handlers fire? |
//! | --- | --- | --- | --- | --- | --- |
//! | 1 | Live pubsub, parent present, direct-apply | no | `~0` | `false` | **yes** |
//! | 2 | Live pubsub, tiny parent race (<100 ms) | no | `~gap_ms < threshold` | `false` | **yes** |
//! | 3 | Pubsub delta received during active sync session → buffered | yes | n/a | `true` | no |
//! | 4 | Buffered delta replayed after sync completes, HLC recent | no | `< threshold` | `false` | yes (*) |
//! | 5 | Buffered delta replayed after sync completes, HLC stale | no | `> threshold` | `true` | no |
//! | 6 | Pending cascade, parent missing briefly (<1 s) | no | small | `false` | yes |
//! | 7 | Pending cascade, parent missing for minutes | no | large | `true` | no |
//! | 8 | Sync-fetched delta (peer delta-request) | n/a | n/a | n/a | **n/a** — no events on wire |
//! | 9 | Crashed mid-handler on direct-apply, restart, context recent | no | small | `false` | yes (#2185 replay) |
//! | 10 | Crashed mid-handler on direct-apply, restart after long offline | no | large | `true` | no |
//! | 11 | Local-authored delta (we are the author) | no | `~0` | `false` | no (pre-existing author-skip logic in `state_delta/mod.rs`) |
//!
//! (*) Scenario 4 is a boundary case: "buffered during sync" semantically
//! matches "behind," but if the buffer drains while HLCs are still within
//! threshold, the predicate fires handlers. If you want to disable this,
//! lower the threshold or change the buffer-replay site to force-skip.
//!
//! # Invariants the design preserves
//!
//! * **State convergence**: WASM always runs. The CRDT merges correctly on
//!   every node regardless of handler-gating decisions. `is_behind` only
//!   affects the observer callback layer.
//!
//! * **Hash-neutrality**: `delta_id` is a hash of `(parents, actions)`
//!   only (see `calimero_storage::delta::CausalDelta::compute_id`).
//!   `expected_root_hash` is the storage merkle root. `events` is not in
//!   any hash. Flipping `events: Some ↔ None` on a DB row is observably
//!   a no-op for peers.
//!
//! * **Crash-safety for direct-apply (#2185)**: when a delta applies
//!   immediately and handlers *were* selected to fire, the `events` blob
//!   stays on disk until `mark_events_executed` clears it on success. If
//!   the process dies mid-handler, the next `load_persisted_deltas`
//!   surfaces the row via `pending_handler_events` and the handler runs
//!   on restart (re-gated by `is_behind` at replay time).
//!
//! * **No handler re-execution across cascade paths**: for a given
//!   `(context_id, delta_id)` pair, handlers fire at most once. Either at
//!   direct-apply time or at restart-replay (never both). The
//!   `mark_events_executed` call on the skip path clears the blob so a
//!   post-restart retry does not fire them again.
//!
//! # Observation points
//!
//! `observe_hlc(context_id, hlc)` is called at every point where a delta
//! first enters the node:
//!
//! * `state_delta` handler entry (pubsub gossip path)
//! * Sync delta-request response (`sync/delta_request.rs`)
//! * Drainer (`add_local_applied_delta` path — locally-authored)
//! * Buffered-delta replay (post-sync drain)
//! * Cascaded-parent fetch (inside `request_missing_deltas`)
//!
//! Observation runs *before* the `is_behind` check, so a delta that *is*
//! the new frontier records gap `0` at its own dispatch. This is correct:
//! an arriving-now delta cannot be "behind" its own arrival.
//!
//! # Failure modes and mitigations
//!
//! * **Clock skew between peers**: irrelevant. We use max-seen HLC, not
//!   wall clock, so a peer with a fast clock cannot force another peer to
//!   skip its own handlers.
//!
//! * **Adversarial HLC spoofing**: a peer broadcasting a delta with an
//!   artificially-high HLC could push `max_seen_hlc` forward, forcing
//!   subsequent legitimate deltas to be gated as "stale." This is out of
//!   scope for this predicate — HLC validation is a separate concern and
//!   lives in the delta-verification path.
//!
//! * **Isolated / single-peer node**: a node that never observes a newer
//!   delta will have `max_seen_hlc == delta.hlc` for every delta, so gap
//!   is always `0` and handlers always fire. This is correct: a node
//!   with no peers has no way to know it's objectively behind the
//!   network.
//!
//! * **Long-running pending without arrivals**: if delta D goes pending
//!   and no other delta arrives for hours, `max_seen_hlc` remains at
//!   D.hlc. When P arrives later, we `observe_hlc(P.hlc)` first — pushing
//!   max forward — then cascade D. The gap between `max` (now P.hlc) and
//!   `delta.hlc` (D.hlc) determines the outcome. This matches intent.
