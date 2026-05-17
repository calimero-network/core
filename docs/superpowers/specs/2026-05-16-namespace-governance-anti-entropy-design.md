# Namespace Governance Anti-Entropy — Design

**Date:** 2026-05-16
**Status:** Approved (brainstorming complete, ready for implementation plan)
**Supersedes:** PR #2369 (publisher-side outbox — abandoned, see "Why not the outbox" below)
**Tracks:** #2367 scope item 1 · unblocks mero-drive PR #32 · resolves PR #2368 "Bug 3"

## Context

A node that misses a namespace governance op broadcast never recovers. Concretely:

- A node creates a context (or any governance op) and broadcasts `ContextRegistered`
  (etc.) on the `ns/<hex>` gossipsub topic. When the per-namespace mesh is cold or
  still forming, the broadcast reaches zero subscribers.
- `publish_and_await_ack_namespace` with `min_acks == 0` (the publisher has no known
  subscribers) returns `Ok` immediately — the op is delivered to no one and is **never
  retried**. It lives only on the creator's local governance DAG.
- A peer that is **already a member** of the namespace has no recovery trigger: the
  join flow does not re-run for it, so it stays permanently unaware of the op.

This produces two observed failures:

1. **mero-drive PR #32 CI** — node-2 is already in the namespace; node-1 creates a
   context; the `ContextRegistered` broadcast is dropped on the cold per-context mesh;
   node-2 never learns the context exists, so the `auto_follow` handler never fires.
2. **PR #2368 "Bug 3"** — node-2's `MemberJoined` op does not reach node-3 before
   `MemberJoinedOpen` arrives. node-3 buffers `MemberJoinedOpen` (missing parent) and
   never receives `MemberJoined`, so the buffer never drains.

Both are the same root cause: a governance op dropped on a cold mesh, with no
receiver-side anti-entropy to detect and repair the gap.

### Why not the outbox (PR #2369)

PR #2369 attempted a network-layer publish outbox: queue a publish that hits
`NoPeersSubscribedToTopic`, replay it on the next `Subscribed` event. It failed CI four
times, each with a new failure mode (stale state-delta replay → `Cannot change
StorageType`; API-signature ripple; topic-scoping; governance-snapshot race that drops
`ContextRegistered` apply side-effects, losing `service_name`).

Root cause of the outbox's unfixability: it re-publishes governance ops as **delayed
raw gossip**, decoupled from their DAG context. Governance ops are DAG-ordered and have
apply-time side-effects; blind re-broadcast races the governance DAG sync protocol and
corrupts derived state. The outbox is the wrong abstraction. PR #2369 is closed.

## Approach

Close the gap on the **receiver side**, working with the existing #2237 readiness
system rather than against it.

The recovery signal **already exists and already flows**: `ReadinessBeacon` is emitted
by every Ready node every ~5s (`crates/node/src/readiness.rs`, `emit_periodic_beacons`,
`beacon_interval` default 5s) and carries `applied_through` + `dag_head`. It is already
received and verified (`crates/node/src/handlers/network_event/readiness.rs`). The
receiver simply inserts it into `ReadinessCache` and does **not act on divergence**.

The fix is two targeted patches that make the receiver act on a signal it already gets.

### Why this is correct where the outbox was not

- **Receiver-pull, not publisher-replay.** A spurious sync is wasted work, never wrong
  state. The outbox's failure mode (corrupting state by replaying stale ops) is
  structurally impossible here.
- **Syncs through the real governance DAG sync protocol** (`sync_namespace_from_peer`)
  — ops applied in DAG order with their side-effects (`set_context_service_name`,
  `OpEvent::ContextRegistered`). No snapshot-path corruption.
- **No wire-format change** — the beacon already carries `applied_through` + `dag_head`.
- **No publisher change** — the op lives on the creator's DAG; the beacon advertises it;
  peers pull. `publish_and_await_ack(min_acks=0)` is left untouched.
- **Aligned with #2237.** The `NamespaceStateHeartbeat` was *deliberately* made
  liveness-only (Phase 11.2) because the readiness beacon was meant to be the divergence
  signal. This change finishes that intent — it does not revert it.

## Components

### Patch 1 — beacon-divergence sync trigger

**File:** `crates/node/src/handlers/network_event/readiness.rs` — the `ReadinessBeacon`
receive handler (currently ~lines 25–56).

After the existing `readiness_cache.insert(&beacon)` + `readiness_notify.notify(...)`:

1. Read the local namespace governance head for `beacon.namespace_id` via the
   `calimero_store::key::NamespaceGovHead` store key (the same accessor
   `handle_namespace_state_heartbeat` uses), or `NamespaceGovernance::read_head`.
2. Compute "peer ahead": `beacon.dag_head` is not present in the local namespace DAG
   (we are missing the peer's head) **or** `beacon.applied_through` exceeds the local
   applied-through sequence.
3. If peer-ahead, enqueue `beacon.namespace_id` into the existing `ns_sync_rx` channel
   (the same channel the join flow uses) → `sync_namespace_from_peer`.
4. **Debounce:** a per-namespace `HashMap<[u8; 32], Instant>` on the `NodeManager` /
   `ReadinessManager`. Skip the trigger if a sync for this namespace was already
   triggered within the debounce window (~5s, i.e. one beacon interval). Beacons arrive
   every ~5s from every peer; without the debounce a behind-node would fire one sync per
   beacon per peer.

Sync peer: `sync_namespace_from_peer` already selects a mesh peer. The beacon's `source`
peer is demonstrably ahead and in the mesh; if `sync_namespace_from_peer` accepts a peer
hint, pass `source`. If not, the existing peer-pick is acceptable (the implementation
plan will confirm and decide).

### Patch 2 — on-subscribe out-of-cycle beacon

**File:** `crates/node/src/handlers/network_event/subscriptions.rs` — the
`NetworkEvent::Subscribed` handler.

The handler already special-cases `group/<hex>` topics (triggers `sync_group` +
`broadcast_group_local_state`) and context topics. Add an `ns/<hex>` arm:

- Decode `namespace_id` from the topic hash (`ns/` + hex).
- Send the existing `EmitOutOfCycleBeacon` actor message to the `ReadinessManager` for
  that namespace — the same path the `ReadinessProbe` receive handler already uses
  (`readiness.rs`, the `EmitOutOfCycleBeacon` message, rate-limited to `BEACON_INTERVAL/2`).

Effect: a newly-subscribed peer receives a beacon within ~1s instead of waiting up to
the full ~5s periodic interval. Patch 1 alone already bounds recovery at ~5s; Patch 2
shaves the cold-start case to ~1s.

## Data flow

**mero-drive cold-start**

1. node-2 is already in the namespace. node-1 creates a context → `ContextRegistered`
   op. Broadcast on `ns/<hex>`; cold mesh → node-2 misses it.
2. node-1 is Ready → emits a `ReadinessBeacon` every ~5s; `dag_head` now includes the
   `ContextRegistered` op.
3. node-2 receives node-1's beacon → Patch 1: `beacon.dag_head` is absent from node-2's
   local namespace DAG → trigger `sync_namespace_from_peer`.
4. node-2 syncs the namespace governance DAG from node-1 → applies `ContextRegistered`
   in DAG order; side-effects run (context registered in the group tree,
   `set_context_service_name`, `OpEvent::ContextRegistered` notified).
5. The `auto_follow` handler fires on `OpEvent::ContextRegistered` and joins the context.

**PR #2368 "Bug 3"**

1. node-3 misses node-2's `MemberJoined` op on the cold mesh.
2. `MemberJoinedOpen` arrives at node-3 → buffered in the governance pending buffer
   (missing parent).
3. node-2 emits a `ReadinessBeacon` → node-3 receives it → `dag_head`/`applied_through`
   ahead of local → node-3 triggers a namespace sync → applies `MemberJoined`.
4. The buffered `MemberJoinedOpen` drains once its parent is present.

**On-subscribe fast path**

1. A new peer subscribes to `ns/<hex>`.
2. Existing members observe `NetworkEvent::Subscribed` → Patch 2 → emit an out-of-cycle
   beacon.
3. The new peer receives a beacon within ~1s → Patch 1 → fast sync.

## Error handling

- **`read_head` failure** — log and skip the divergence check (do not trigger a sync on
  unknown local state). The next beacon (~5s) retries.
- **`sync_namespace_from_peer` failure** — handled by the existing namespace-sync error
  path (exponential backoff). The next beacon re-triggers regardless.
- **Concurrent heads** (peer and local each hold an op the other lacks) — the
  "peer-ahead" predicate still fires; triggering a sync is correct (the sync converges
  both DAGs). Harmless.
- **Peer actually behind us** — the predicate is strictly "peer ahead"; no trigger fires.
- **Sync storm** — the per-namespace debounce caps the trigger rate to ~1 sync per
  namespace per beacon interval.
- **Untrusted input** — the beacon is already signature-verified and size-bounded in the
  existing receive handler; Patch 1 runs after that verification.

## Testing

1. **Rust unit tests** — the divergence predicate: peer-ahead (`dag_head ∉ local`) →
   trigger; `∈ local` → no trigger; zero head → no trigger; **no local namespace state
   (still bootstrapping / mid-join) → no trigger** (the join flow owns the initial sync;
   firing here races the join handshake — see "join-race gate" below). Plus the debounce
   window: a second beacon inside the window does not re-trigger, re-allowed after it,
   per-namespace isolation.
2. **`group-metadata` e2e** (existing `e2e-rust-apps` matrix) — exercises the
   namespace-governance join + `GroupMetadataSet` path end-to-end. This is the e2e proof:
   a draft of this change regressed it (the beacon-divergence sync raced the join
   handshake, pulling governance ops before key delivery → opaque skeletons), and the
   join-race gate turns it green again.
   *Note:* an earlier draft added a Phase 6 to `opaque-leaf-regression.yml` as a
   dedicated sentinel; that was dropped — the workflow's Round-2 interleaved-write step
   is a pre-existing #2293 mesh-starvation flake, so coupling a sentinel behind it is
   unreliable. This change does not touch the sync crate, so `sync-regression.yml` does
   not apply to it.
3. **PR #2368** — re-trigger its CI on top of this change; the `MemberJoined` /
   `MemberJoinedOpen` ordering scenario should pass without a #2368-specific patch.
4. **mero-drive PR #32** — post-merge, once `merod:edge` rebuilds, re-trigger its
   `E2E (main)` workflow; the "Sync Registry after nested-folder registration" step
   should stop flaking.

### Join-race gate

`beacon_indicates_divergence` requires the node to already hold a non-empty local
namespace governance DAG head before triggering a sync. A node still bootstrapping or
mid-join has no head: the join flow owns the initial governance sync, and a
beacon-triggered sync firing concurrently races the join handshake — it can pull
governance ops before the namespace key is delivered, leaving them as undecryptable
opaque skeletons that the post-key join sync then skips as duplicates. An established
member (the scenario this anti-entropy targets) always has a non-empty head.

## Out of scope

- **`publish_and_await_ack_namespace(min_acks == 0)` retry** — not needed. The op
  persists on the creator's DAG; the beacon advertises it; receivers pull. The publisher
  is left untouched.
- **`NamespaceStateHeartbeat`** — stays liveness-only, preserving the #2237 Phase 11.2
  intent.
- **`ReadinessProbe` join-path** — unchanged.
- **The PR #2369 outbox** — fully reverted; PR #2369 closed with a comment documenting
  the four-failure diagnosis so the dead end is recorded.

## Disposition of PR #2369

Close PR #2369. Post a closing comment summarizing the outbox diagnosis (four CI
failures, each a distinct failure mode, root cause = re-publishing governance ops as
delayed raw gossip races the DAG sync protocol). Implementation of this design proceeds
on a new branch `fix/2367-namespace-governance-anti-entropy`.
