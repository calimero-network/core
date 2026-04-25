# Governance three-phase contract + peer-readiness handshake — design

- **Status**: Design + implementation plan complete; pending implementation
- **Date**: 2026-04-25
- **Tracking issue**: [#2237](https://github.com/calimero-network/core/issues/2237)
- **Supersedes / related**: [#2198](https://github.com/calimero-network/core/issues/2198), [#2236](https://github.com/calimero-network/core/issues/2236)
- **Related merged PRs**: #2225 (cold-start parent-pull), #2252 (unknown-peer governance catch-up), #2146 (KeyDelivery DAG unfreeze)
- **Scope**: Full #2237 (three-phase contract for every governance endpoint + KeyDelivery + member-list propagation on join) + a new peer-level readiness handshake for sync-partner selection (beyond #2237)

---

## 1. Problem

Every governance endpoint and `KeyDelivery` today uses fire-and-forget gossipsub publish: the caller returns `Ok` the moment the message is handed to the local gossipsub instance, with **no guarantee any recipient actually received it**. This is the root cause of a cluster of production and CI symptoms:

- **#2198** cold-start `join_context` races (the umbrella flake).
- **`execute/mod.rs:180` silent `InternalError`** — 100% repro on `fuzzy-handlers-node-4`, 356/356 requests failing. Root cause: a group joiner that published its `KeyDelivery` into a cold mesh never received the other members' keys, so `load_current_group_key(gid) == None` and `identity.sender_key == None`, and every subsequent request returns `InternalError` without a log line.
- **`fuzzy-kv-store` "last-joined node never converges"** — node-4 stuck 30+ seconds at pre-seed root hash while nodes 2 and 3 converged in ~2s, because existing members' identities never reached node-4, so node-4 rejected incoming sync streams as "unknown context member".
- **Workaround sprawl**: `type: wait, seconds: 5` steps in `group-subgroup-queued-deltas.yml` / `group-subgroup-cold-sync.yml`; `max_attempts=2` + `merobox nuke --force` at the workflow-runner level; PR #2225's cross-peer parent-pull retry loop; 30-second `NamespaceStateHeartbeat` reconciliation. All masking symptoms rather than closing the root cause.

This is not a mesh-timing bug. It is a **delivery-semantics bug**: the transport is correct, the API contract is wrong.

A complementary problem the existing #2237 proposal does not address: even with mesh-ready publishing, **a joiner cannot tell who is actually able to serve them**. A cloud node that has just appeared in the gossipsub mesh may still be mid-backfill and unable to authoritatively answer "who are the members" or "what's the DAG head". Picking such a peer as a sync source produces the "weird edge cases where nodes never sync". This requires a peer-level readiness handshake on top of the #2237 mechanism.

## 2. Goals

1. Turn every governance op into an acked, typed-outcome operation — publisher learns whether the op propagated.
2. Make `join_namespace`, `join_group`, and `KeyDelivery` success-atomic: if the API returns `Ok`, the local state is usable.
3. Give sync-partner selection a freshness signal so a fresh joiner picks a peer that is actually caught up, not just one that is on the topic.
4. Break the cold-start fleet deadlock where nobody is ready, so nobody becomes ready.
5. Delete the workaround scaffolding (waits, retry loops, reconciliation paths) whose purpose is compensating for silent drops.
6. Preserve gossipsub as the transport; preserve CRDT eventual-consistency semantics for user data-deltas unchanged.

## 3. Non-goals

- Changing CRDT data-delta sync semantics — those are genuinely eventually consistent by design, and the contract here does **not** apply to user-space writes.
- Replacing gossipsub. The contract adds delivery confirmation on top, not a different transport.
- Consensus / quorum semantics for governance ops — at-least-one-ack is propagation evidence, not consensus. The op commits to the publisher's local DAG the moment it is signed; acks are informational for correctness.
- A DHT-backed member directory (#2236's proposal) — useful for other concerns (ex-member isolation, admission control) but not required here.
- Rolling-upgrade compatibility with older nodes. This is a coordinated wire change, pre-1.0.

## 4. Design overview

### 4.1 Architecture at a glance

```
┌────────────────────────────────────────────────────────────────────┐
│ PHASE 1 — Readiness gate (R1, transport-ready)                     │
│  mesh_peers(topic).len() >= min(mesh_n_low, known_subscribers)     │
│  Reject fast with NamespaceNotReady. Client retries.               │
├────────────────────────────────────────────────────────────────────┤
│ PHASE 2 — Publish + collect acks                                   │
│  apply_locally(op) → publish(topic, op) → await_acks(deadline, N)  │
│  Acks are signed by members; verified against governance state.    │
├────────────────────────────────────────────────────────────────────┤
│ PHASE 3 — Typed Outcome                                            │
│  Ok(DeliveryReport { acked_by })                                   │
│  Err(NamespaceNotReady) | Err(NoAckReceived)                       │
└────────────────────────────────────────────────────────────────────┘
```

In parallel, every node maintains a per-namespace readiness state machine (R2, tip-fresh) and emits signed `ReadinessBeacon` messages so that sync-partner selection (especially at join time) picks a peer that is actually caught up, not just one on the topic.

### 4.2 Two readiness layers, one transport

| Layer | Checked where | Guarantees |
|---|---|---|
| **R1** (transport-ready) | Publisher-side, Phase 1 | The local mesh has ≥ `min(mesh_n_low, known_subscribers)` peers for the topic. Cheap. Already implemented as `mesh_peers(topic)`. |
| **R2** (tip-fresh, peer-validated) | Sync-partner selection, especially `join_namespace` step 4 | Peer has no pending ops, has applied through at least the max `applied_through` it has seen recently, and has observed a peer heartbeat within TTL. Carried by a signed `ReadinessBeacon` message. |

R1 is cheap and immediate. R2 is the richer claim that supports the three-step "hi / avail / sync" join dance.

### 4.3 Join flow (J6)

The join API is split into two explicit calls and a convenience aggregate, so callers can choose between fast UX feedback and atomic block-until-ready:

```
join_namespace(invitation) ─────── steps 1–4 ─────►  Ok(JoinStarted { sync_partner, partner_head })
                                   (fast: "joined, syncing…")

await_namespace_ready(ns, deadline) ── steps 5–8 ──►  Ok(ReadyReport { final_head, acked_by, ms })
                                   (correctness: "ready to play")

join_and_wait_ready(invitation, deadline) = sequential wrapper for headless callers
```

Steps 1-4 (in `join_namespace`): mark pending-membership locally → subscribe to namespace topic → publish `ReadinessProbe` (active) → collect first signed `ReadinessBeacon`.

Steps 5-8 (in `await_namespace_ready`): pick partner by `(strong desc, applied_through desc, ts desc)` → run existing `NamespaceBackfill` protocol until local head matches → publish joiner's `MemberJoined` op through the full three-phase contract → local FSM transitions to `PeerValidatedReady` and begins emitting its own beacons.

## 5. Wire protocol

### 5.1 Topic envelope

The namespace topic `ns/<namespace_id>` currently carries a bare borsh-encoded `SignedNamespaceOp`. Replace with a discriminated enum. Same pattern on the group topic.

```rust
// crates/context-client/src/local_governance/wire.rs (new)
#[derive(BorshSerialize, BorshDeserialize)]
pub enum NamespaceTopicMsg {
    Op(SignedNamespaceOp),              // existing payload, unchanged
    Ack(SignedAck),                     // new — phase 2 ack
    ReadinessBeacon(SignedReadinessBeacon),  // new — R2 beacon
    ReadinessProbe(ReadinessProbe),     // new — joiner probe, unsigned
}

#[derive(BorshSerialize, BorshDeserialize)]
pub enum GroupTopicMsg {
    Op(SignedGroupOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
}
```

### 5.2 Per-kind shapes

```rust
pub struct SignedAck {
    pub op_hash:       [u8; 32],    // hash_scoped(topic, &op)
    pub signer_pubkey: PublicKey,   // namespace or group identity of acker
    pub signature:     [u8; 64],    // sign_by(signer_sk, op_hash)
}

pub struct SignedReadinessBeacon {
    pub namespace_id:    [u8; 32],
    pub peer_pubkey:     PublicKey,
    pub dag_head:        [u8; 32],
    pub applied_through: u64,       // highest op sequence applied — tip yardstick
    pub ts_millis:       u64,       // publisher clock; used for TTL, not ordering
    pub strong:          bool,      // true = PeerValidatedReady, false = LocallyReady (boot-grace fallback)
    pub signature:       [u8; 64],
}

pub struct ReadinessProbe {
    pub namespace_id:    [u8; 32],
    pub nonce:           [u8; 16],  // joiner may be unsigned (no ns identity yet); nonce prevents reflection loops
}
```

### 5.3 `op_hash` scoping

```
op_hash = blake3(topic_id || borsh(SignedOp))
```

Topic-scoping in the preimage prevents a cross-namespace replay where an ack for op X on namespace A would count as an ack for (coincidentally identical) op X on namespace B.

### 5.4 Signature verification

- **Ack**: (a) ed25519 verify over `op_hash`, (b) `signer_pubkey` must appear in `current_governance_members(namespace_id)` at verifier's local DAG.
- **ReadinessBeacon**: (a) ed25519 verify over the full field sequence including `strong`, (b) `peer_pubkey` must be a current member.
- **ReadinessProbe**: unsigned (joiner has no namespace identity at probe time). Defended only by gossipsub peer scoring + rate limiting. The only effect of a probe is to prompt an out-of-cycle beacon from ready peers; abuse cost is bounded by beacon volume.

### 5.5 Message sizes

All new kinds are <200 bytes. Gossipsub default max is 64 KiB. No fragmentation concern.

### 5.6 Wire versioning

Pre-1.0, single wire version. New nodes on the enum'd wire against old-wire nodes will borsh-fail on bare-`SignedNamespaceOp` inputs and drop silently — coordinated cluster-wide upgrade, not rolling. Document in release notes.

## 6. Three-phase contract (core mechanism)

### 6.1 Phase 1 — Readiness gate

```rust
// crates/context/src/governance_broadcast.rs (new)
async fn assert_transport_ready(
    net: &NetworkClient,
    topic: TopicHash,
    known_subscribers: usize,
) -> Result<(), GovernanceBroadcastError> {
    let mesh = net.mesh_peer_count(topic).await;        // existing primitive — see crates/network/primitives/src/client.rs:140
    let required = min(MESH_N_LOW, known_subscribers);
    if mesh < required {
        return Err(GovernanceBroadcastError::NamespaceNotReady { mesh, required });
    }
    Ok(())
}
```

- `MESH_N_LOW` comes from gossipsub config.
- `known_subscribers` is maintained by an extended `handle_subscribed` in `crates/node/src/handlers/network_event/subscriptions.rs` as `HashMap<TopicHash, HashSet<PeerId>>` — bounding the required threshold by "peers we have ever observed subscribing to this topic" avoids demanding `MESH_N_LOW=4` in a 1-peer-namespace.
- Solo-node case: `known_subscribers = 0` ⟹ `required = 0` ⟹ publish freely. Preserves existing single-node test workflows.
- On `NamespaceNotReady`, no publish attempt, no side effects. Caller retries.
- **Contract requirement:** wrappers that combine `sign + apply_signed_op + publish_and_await_ack` (e.g. `sign_apply_and_publish_namespace_op`) MUST run `assert_transport_ready` *before* `apply_signed_op`, not after. Applying first and gating second leaves the op durably committed to the local DAG when the gate rejects, breaking the "no side effects on `NamespaceNotReady`" invariant — and a retry would sign+apply a *different* op (new `op_hash`), producing duplicate DAG entries on every rejection. Phase 1 is a precondition, not a post-condition.

### 6.2 Phase 2 — Publish and collect acks

```rust
// NOTE: `publish_and_await_ack` does NOT apply the op locally. Local apply
// happens at the caller (e.g. `sign_apply_and_publish_namespace_op`) AFTER
// `assert_transport_ready` (Phase 1) passes. See §6.1 Contract requirement.
//
// Dependencies are passed narrowly (rather than via a fat `&ContextClient`):
// `ack_router` for subscription, `store` + `namespace_id` for `verify_ack`'s
// member-set lookup. This matches the implementation plan Task 3.4 and lets
// unit tests inject a stub `AckRouter` without standing up a full client.
async fn publish_and_await_ack(
    store: &Store,
    net: &NetworkClient,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    topic: TopicHash,
    op: SignedNamespaceOp,
    op_timeout: Duration,
    min_acks: usize,                             // defaults to 1
    required_signers: Option<Vec<PublicKey>>,    // Some(_) for KeyDelivery → ack must come from recipient
) -> Result<DeliveryReport, GovernanceBroadcastError> {
    let start = Instant::now();
    let op_hash = hash_scoped(topic, &op);
    let mut collector = ack_router.subscribe(op_hash);
    net.publish(topic.clone(), NamespaceTopicMsg::Op(op)).await?;

    let deadline = start + op_timeout;
    let mut acked_by: Vec<PublicKey> = Vec::new();
    loop {
        // `saturating_duration_since` returns ZERO past the deadline (no Instant
        // subtraction panic) and `tokio::time::timeout` resolves immediately as
        // `Err(_elapsed)` on a zero duration — same control flow as a separate
        // `if now >= deadline` guard, but expressed in one line.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GovernanceBroadcastError::NoAckReceived {
                waited_ms: start.elapsed().as_millis() as u64,
                op_hash,
            });
        }
        // `tokio::sync::broadcast::Receiver::recv()` returns `Result<T, RecvError>`.
        // The two RecvError variants have OPPOSITE semantics:
        //   - `Lagged(n)`: we missed n messages, channel is still open → continue.
        //   - `Closed`:    all senders dropped, no more messages will EVER arrive.
        //                  `recv()` returns synchronously forever; `continue` would
        //                  burn CPU until the outer deadline. Must terminate.
        match timeout(remaining, collector.recv()).await {
            Ok(Ok(ack)) => {
                if !verify_ack(store, namespace_id, op_hash, &ack) { continue; }
                if let Some(req) = &required_signers {
                    if !req.contains(&ack.signer_pubkey) { continue; }
                }
                if !acked_by.iter().any(|p| *p == ack.signer_pubkey) {
                    acked_by.push(ack.signer_pubkey);
                }
                if acked_by.len() >= min_acks { break; }
            }
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) | Err(_elapsed) => return Err(
                GovernanceBroadcastError::NoAckReceived {
                    waited_ms: start.elapsed().as_millis() as u64,
                    op_hash,
                }
            ),
        }
    }
    Ok(DeliveryReport { op_hash, acked_by, elapsed_ms: start.elapsed().as_millis() as u64 })
}
```

**AckRouter** is a new `ContextManager`-owned component holding a `HashMap<[u8;32], broadcast::Sender<SignedAck>>` keyed by `op_hash`. The `Ack` arm of `handle_namespace_op` / `handle_group_op` demuxes by `op_hash` and fans out. Entries are GC'd on publish completion or timeout.

`verify_ack` checks (a) ed25519 signature over `op_hash`, (b) `signer_pubkey ∈ current_governance_members(namespace_id)` against the verifier's local DAG. Forged acks are dropped at µs cost. No DoS path: gossipsub peer-scoring catches flooders upstream; the verifier never allocates per forged ack.

### 6.3 Phase 3 — Typed outcome

```rust
// crates/context-primitives/src/errors.rs (extend)
pub enum GovernanceBroadcastError {
    NamespaceNotReady { mesh: usize, required: usize },
    NoAckReceived    { waited_ms: u64, op_hash: [u8; 32] },
    PublishError(PublishError),
    LocalApplyError(ApplyError),
}

pub struct DeliveryReport {
    pub op_hash:     [u8; 32],
    pub acked_by:    Vec<PublicKey>,  // informational, not load-bearing for correctness
    pub elapsed_ms:  u64,
}
```

External API boundaries (JSON-RPC, meroctl, SDK) map this enum to typed outward errors.

### 6.4 Receiver-side ack emission

In `crates/node/src/handlers/network_event/namespace.rs`, the existing `Op(op)` arm after a successful `apply_signed_namespace_op`:

```rust
let my_pubkey = namespace_identity.public_key();
let op_hash = hash_scoped(topic, &op);
let ack = SignedAck::sign(&my_sk, my_pubkey, op_hash);
net.publish(topic.clone(), NamespaceTopicMsg::Ack(ack)).await;   // fire-and-forget
```

Ack emission is itself fire-and-forget. It has to be: acks-about-acks is infinite regress. Safe because (a) we have already applied the op — our state is correct — and (b) the publisher's `NoAckReceived` + retry path is the recovery mechanism if our ack is lost.

### 6.5 `NoAckReceived` does not roll back the local apply

The publisher's op is durably applied + DAG-logged before publish. `NoAckReceived` is evidence about *propagation*, not about *correctness*. On the next heartbeat tick, lagging peers observe the divergence and trigger `NamespaceBackfill` to catch up. The op is never "forgotten".

### 6.6 Per-endpoint timeout defaults

Configurable via `crates/node/src/sync/config.rs`. Phase-1 rejection is immediate (no wait).

| Endpoint class | Default |
|---|---|
| Cheap ops (alias sets, metadata tweaks) | 2s |
| Membership changes (`add_group_members`, `remove_group_members`, `MemberJoined`) | 5s |
| Heavy coordination (context creation with app install) | 10s |
| `join_namespace` total deadline | 10s |
| `await_namespace_ready` total deadline | 15s (backfill can dominate) |
| `join_group` wait for group key | 5s |

## 7. Readiness FSM (R2 + boot-grace split)

### 7.1 States

```rust
enum ReadinessTier {
    Bootstrapping,
    LocallyReady,          // SD1: no pending + non-empty local DAG + BOOT_GRACE elapsed + no peer beacons seen
    PeerValidatedReady,    // full SD1: local head ∈ max-heads seen AND heard a fresh peer beacon
    CatchingUp { target_applied_through: u64 },
    Degraded  { reason: DemotionReason },
}

enum DemotionReason { PendingOps(usize), NoRecentPeers, PeerSawHigherThroughput }
```

### 7.2 Transition rules

```
Bootstrapping ──► PeerValidatedReady:
    heard_fresh_beacon AND local.pending == 0
    [DEFERRED head-match] AND local.head ∈ max_heads

Bootstrapping ──► LocallyReady:
    BOOT_GRACE_elapsed AND local.applied_through > 0 AND local.pending == 0
    AND no peer beacons with received_at ≥ now - TTL_HEARTBEAT exist in cache

LocallyReady ──► PeerValidatedReady:
    heard_fresh_beacon
    [DEFERRED head-match] AND local.head ∈ max_heads

LocallyReady ──► CatchingUp:
    heard_fresh_beacon with applied_through > local.applied_through + GRACE

PeerValidatedReady ──► CatchingUp:
    heard_fresh_beacon with applied_through > local.applied_through + GRACE

*Ready ──► Degraded:
    pending_ops > 0
    [DEFERRED] OR no fresh beacons for 2·TTL_HEARTBEAT

Degraded ──► CatchingUp / LocallyReady:
    trigger backfill OR wait for pending to drain

CatchingUp ──► LocallyReady / PeerValidatedReady:
    backfill completes AND SD1 satisfies the destination tier
```

`GRACE` (default 2) on applied_through avoids thrashing when our own in-flight ops have not yet been heard back by peers.

**Note on the `[DEFERRED]` annotations.** Two strictness guarantees are intentionally **not** implemented in the initial plan's `evaluate_readiness`:

1. **`*Ready → Degraded` on no fresh beacons for `2 · TTL_HEARTBEAT`.** The evaluator is currently stateless w.r.t. beacon recency — it only knows whether peers are *currently* fresh-within-TTL via `PeerSummary { heard_recent_beacon }`. Implementing the transition requires extending `PeerSummary` (and `ReadinessCache::peer_summary`) to surface "age of most recent fresh beacon" so the evaluator can compare against `2 · TTL_HEARTBEAT`. While deferred, between `TTL_HEARTBEAT` and `2 · TTL_HEARTBEAT` of silence a previously `*Ready` node sits in `LocallyReady` (still serves, weaker claim) instead of `Degraded`. Operators detect the condition via `governance_publish_outcome{outcome="no_ack"}` rates.

2. **`local.head ∈ max_heads` (head-match) on transitions into `PeerValidatedReady`.** The plan's evaluator currently checks only `local_applied_through + GRACE >= peer.applied_through`. That's a *weaker* claim — a node on a divergent fork with the same `applied_through` would also satisfy the check, even though its `head` differs. To enforce the strict spec semantics, `PeerSummary` would need an `observed_heads: HashSet<[u8; 32]>` field populated from cache entries, and `evaluate_readiness` would compare `state.local_head ∈ observed_heads` before promoting. Behavioural impact while deferred: in a pathological fork scenario, a node on a minority fork could briefly self-report `PeerValidatedReady` before the next ack-driven correction lands. Acks (Phase 2) and the next governance publish surface the divergence; the FSM tier is a hint, not a safety boundary, so the weakened check does not violate any data-correctness invariant.

Track both follow-ups under #2237's implementation issue.

### 7.3 Why split into tiers — the cold-start deadlock

Without the split, a 3-node fleet cold-starting simultaneously enters a cycle: nobody is ready, nobody beacons, nobody hears a beacon, nobody becomes ready. The transition rule "I need a peer beacon to become ready" is a circular dependency.

The `LocallyReady` tier breaks this: after `BOOT_GRACE` (default 10s) with a non-empty local DAG and no pending ops, a node self-promotes and starts beaconing with `strong: false`. Other cold-boot nodes see the beacon, compare `applied_through`, and the lagging ones transition to `CatchingUp`. The fleet converges in `BOOT_GRACE + backfill_ms`, typically <15s.

Safety of `LocallyReady`: it is a weaker claim — "my local DAG is self-consistent", not "I am tip-fresh vs the world". A peer with genuinely higher `applied_through` (`PeerValidatedReady`) wins the picker tiebreak. An empty-DAG joiner (`applied_through == 0`) **never** self-promotes — they have nothing to serve.

### 7.4 Beacon emission triggers

1. **Edge trigger** — on `* → PeerValidatedReady` or `* → LocallyReady`: emit immediately.
2. **Freshness tick** — while `*Ready`, per-namespace timer emits every `BEACON_INTERVAL` (default 5s).
3. **Probe response** — on `ReadinessProbe` received, if in `*Ready`, emit out-of-cycle (resets the freshness tick). Gives joiners sub-second response instead of waiting up to 5s for the next scheduled beacon.

### 7.5 ReadinessCache

```rust
struct ReadinessCache {
    entries: HashMap<(NamespaceId, PublicKey), CacheEntry>,
}
struct CacheEntry {
    head: [u8; 32],
    applied_through: u64,
    received_at: Instant,
    strong: bool,
}
impl ReadinessCache {
    fn insert(&mut self, ns: NamespaceId, beacon: &SignedReadinessBeacon);
    fn fresh_peers(&self, ns: NamespaceId, ttl: Duration) -> Vec<CacheEntry>;
    fn pick_sync_partner(&self, ns: NamespaceId, ttl: Duration) -> Option<PublicKey>;
    fn max_applied_through(&self, ns: NamespaceId, ttl: Duration) -> Option<u64>;
    fn await_first_fresh_beacon(&self, ns: NamespaceId, deadline: Duration) -> Future<Option<CacheEntry>>;
}
```

`pick_sync_partner` orders candidates by `(strong desc, applied_through desc, received_at desc)`. Only fresh (within TTL) entries are considered.

### 7.6 Relationship to existing `NamespaceStateHeartbeat`

Keep `NamespaceStateHeartbeat` for **liveness only** — detecting silent peer departure. Its state-reconciliation arm in `handle_namespace_state_heartbeat` (the branch that enumerates divergence and initiates catch-up) is **deleted**; `ReadinessBeacon` at 5s cadence catches divergence much faster and without the heartbeat's 30s blind window. Roughly 180 lines in `handlers/network_event/namespace.rs` collapse to the liveness-only path.

### 7.7 Default knobs

| Knob | Default | Purpose |
|---|---|---|
| `BEACON_INTERVAL` | 5s | Freshness-tick cadence while `*Ready`. |
| `BOOT_GRACE` | 10s | Minimum time in `Bootstrapping` before eligible for `LocallyReady` fallback. |
| `TTL_HEARTBEAT` | 60s | Freshness window for `ReadinessCache` entries. |
| `GRACE` (applied_through) | 2 | Thrashing buffer on tip comparisons. |

All configurable via `crates/node/src/sync/config.rs`.

## 8. Join flow in detail (J6)

### 8.1 `join_namespace(invitation) -> Result<JoinStarted>`

Blocks through steps 1-4:

```rust
async fn join_namespace(invitation: SignedGroupOpenInvitation, deadline: Duration) -> Result<JoinStarted> {
    // Start the clock at function entry so `JoinStarted.elapsed_ms` and
    // `JoinError::NoReadyPeers.waited_ms` capture the *full* join latency,
    // including the cost of mark_membership_pending + subscribe + publish probe.
    // Anchoring at step 4 would understate latency vs. the §16.2 acceptance
    // criterion ("join + 20 ops + read ≤ 2s on 2-node localhost").
    let start = Instant::now();
    let ns_id = invitation.namespace_id();
    let topic = ns_topic(ns_id);

    store.mark_membership_pending(ns_id, invitation)?;          // step 1
    net.subscribe(topic.clone()).await?;                        // step 2

    let probe = ReadinessProbe { namespace_id: ns_id, nonce: random_nonce() };  // step 3
    net.publish(topic.clone(), NamespaceTopicMsg::ReadinessProbe(probe)).await?;

    let beacon = readiness_cache.await_first_fresh_beacon(ns_id, deadline).await
        .ok_or(JoinError::NoReadyPeers { waited_ms: start.elapsed().as_millis() as u64 })?;

    Ok(JoinStarted {
        namespace_id:     ns_id,
        sync_partner:     beacon.peer_pubkey,
        partner_head:     beacon.dag_head,
        partner_applied:  beacon.applied_through,
        elapsed_ms:       start.elapsed().as_millis() as u64,
    })
}
```

`await_first_fresh_beacon` is dedup-safe: if a fresh beacon is already cached when the caller arrives (common — we just received one from a periodic tick), it returns immediately without waiting for the probe response.

### 8.2 `await_namespace_ready(ns_id, deadline) -> Result<ReadyReport>`

Blocks through steps 5-8:

```rust
async fn await_namespace_ready(ns_id: NamespaceId, deadline: Duration) -> Result<ReadyReport> {
    let start = Instant::now();

    let partner = readiness_cache.pick_sync_partner(ns_id, TTL_HEARTBEAT)       // step 5
        .ok_or(ReadyError::NoReadyPeers)?;

    run_namespace_backfill(partner, ns_id, deadline).await?;                   // step 6

    let my_signed_op = build_member_joined_op(ns_id, invitation, &my_ns_sk);   // step 7
    let delivery_report = publish_and_await_ack(
        store, net, ctx, ns_topic(ns_id), my_signed_op,
        deadline.saturating_sub(start.elapsed()),
        1,
        None,
    ).await?;

    // step 8 is implicit: FSM evaluator transitions us to PeerValidatedReady as a
    // consequence of (a) backfill making local head ∈ max-heads seen, (b) no pending
    // ops. From here we begin emitting our own beacons.

    Ok(ReadyReport {
        namespace_id:    ns_id,
        final_head:      local.head(ns_id),
        applied_through: local.applied_through(ns_id),
        members_learned: local.member_count(ns_id),
        acked_by:        delivery_report.acked_by,
        elapsed_ms:      start.elapsed().as_millis() as u64,
    })
}
```

### 8.3 `join_and_wait_ready` — aggregate for headless callers

```rust
async fn join_and_wait_ready(invitation: ..., deadline: Duration) -> Result<ReadyReport> {
    // join_deadline ∈ [1s, deadline]: floor prevents near-zero on small deadlines,
    // cap prevents the floor from exceeding the caller's total budget (which would
    // let `join_namespace` run past `deadline` and zero out `ready_deadline`).
    let join_deadline = std::cmp::min(
        std::cmp::max(deadline / 3, Duration::from_secs(1)),
        deadline,
    );
    let ready_deadline = deadline.saturating_sub(join_deadline);
    let started = join_namespace(invitation, join_deadline).await?;
    await_namespace_ready(started.namespace_id, ready_deadline).await
}
```

### 8.4 Client SDK retrying wrapper

```rust
// crates/client/src/client/namespace_retry.rs (new)
pub async fn join_namespace_with_retry(
    &self,
    invitation: SignedGroupOpenInvitation,
    max_total: Duration,
) -> Result<JoinStarted, JoinError> {
    let mut delay = Duration::from_secs(3);
    let start = Instant::now();
    loop {
        match self.join_namespace(invitation.clone(), JOIN_ATTEMPT_DEADLINE).await {
            Ok(js) => return Ok(js),
            Err(JoinError::NoReadyPeers { .. }) => {
                if start.elapsed() + delay > max_total { return Err(JoinError::NoReadyPeers { .. }); }
                tokio::time::sleep(delay + jitter_up_to(delay / 4)).await;
                delay = min(delay * 2, Duration::from_secs(30));  // 3s → 6s → 12s → 24s → 30s cap
            }
            Err(other) => return Err(other),   // signature / invitation / deadline errors don't retry
        }
    }
}
```

Shipping retries in an opt-in helper rather than baking them into `join_namespace` itself: tests want fast-fail, frontends want patience, CI jobs want bounded. Keep the primitive honest; put policy in the wrapper.

### 8.5 UX model on the Battleships-style frontend

- User clicks "Join" → call `join_namespace` → render "Joined, syncing…" on `Ok(JoinStarted)` (fast: typically <1s on a warm fleet).
- In parallel, promise-chain `await_namespace_ready` → render "Ready" on `Ok(ReadyReport)`, unlock state-dependent UI.
- If `Err(NoReadyPeers)` from `join_namespace`: render "Waiting for members to come online…" and drive retries via `join_namespace_with_retry`.

## 9. KeyDelivery integration

KeyDelivery is `NamespaceOp::Root(RootOp::KeyDelivery { group_id, envelope })`, published fire-and-forget by `maybe_publish_key_delivery` in `crates/node/src/key_delivery.rs` after a `MemberJoined` apply. Three changes:

1. **Publisher side.** Route through `publish_and_await_ack` with `required_signers: Some(vec![recipient_pubkey])`. The ack MUST come from the specific joiner whose envelope this is — any-member ack is insufficient because "delivered to network" doesn't prove "decrypted by intended recipient".

2. **Recipient side.** The `KeyDelivery` arm of `handle_namespace_op` decrypts the envelope via ECDH, stores the group key via `load_current_group_key` writer path, **then** emits `Ack`. Order is `decrypt → store → ack`. Never ack before store. Decrypt failures (unexpected key, malformed envelope) silently skip ack — sender observes `NoAckReceived` after timeout and can publish a fresh delivery.

3. **`join_group` success contract.** Wait on a watch-channel fired by the group_store when `load_current_group_key(group_id)` first returns `Some`. Times out as `Err(JoinGroupError::NoKeyReceived)` — typed and retryable. If `join_group` returns `Ok`, a valid group key is in local storage.

**Result.** The silent branch at `execute/mod.rs:180` — `identity.sender_key == None && load_current_group_key == None` — becomes unreachable under the new contract. Remove it or replace with `debug_assert_unreachable!("join_group contract guarantees key presence")` + loud error log.

**Idempotence.** If multiple existing members race to deliver keys, the first valid delivery wins at the receiver's store, subsequent ones are idempotent no-ops and still ack. Senders see `Ok(DeliveryReport)` regardless of delivery order.

## 10. Member-list propagation on join

Comment 3 of #2237 describes a freshly-joined node rejecting incoming sync streams because it has not yet learned other members' pubkeys (`unknown context member after governance sync, closing stream` in `sync/manager/mod.rs:2407`). Under J6 this falls out of the design — no new protocol needed:

- Step 6 of `await_namespace_ready` (`run_namespace_backfill`) pulls the joiner's governance DAG up to the sync partner's `applied_through`. That DAG contains every `MemberAdded` op ever applied. Applying them populates local governance state with every current member's pubkey.
- When `await_namespace_ready` returns `Ok`, the joiner has the complete member list. This is an **invariant of the success contract**, not best-effort.
- Incoming sync-stream validators now see a fully-populated member set.

PR #2252's "request governance catch-up on unknown peer instead of closing" remains as a safety net for the "long-offline returning peer" case, which J6 doesn't cover on its own.

## 11. Edge cases

### 11.1 No peers respond to `ReadinessProbe` — subcase 1 (namespace empty / offline)

A joiner with `applied_through == 0` has no DAG to self-promote from. They must genuinely wait for a peer. `join_namespace` returns `Err(NoReadyPeers)` after the deadline. Handle via `join_namespace_with_retry`. Telemetry: `governance.join.no_ready_peers` counter tagged by `namespace_bucket` (per §15 — 1-byte SHA-256 prefix; full `namespace_id` only in structured logs) surfaces dead namespaces to operators.

### 11.2 No peers respond to `ReadinessProbe` — subcase 2 (cold fleet startup)

All nodes boot simultaneously with pre-partition local DAGs. Without the tier split, this deadlocks (§7.3). Under the split: each node has `applied_through > 0`, after `BOOT_GRACE` each self-promotes to `LocallyReady` and beacons. Beacons are compared, highest `applied_through` wins, lagging nodes transition to `CatchingUp` and backfill. Convergence in `BOOT_GRACE + backfill_ms`, typically <15s. Telemetry: `readiness_locally_ready_fallback` counter surfaces how often this path fires; should be 0 in steady state.

### 11.3 Lying beacon (malicious or buggy peer)

A peer M advertises `applied_through: 10_000` with a forged `dag_head`. Joiner J picks M as sync partner, calls `run_namespace_backfill(M, ...)`. M's responses either fail op-signature verification (J rejects and marks M misbehaving) or produce a final head that diverges from M's claimed head (J detects mismatch, marks M bad). J evicts M from the cache, picks the next peer. Per-beacon cost of the lie is one wasted sync attempt, already mitigated upstream by gossipsub peer scoring.

### 11.4 Ack flood from forged signers

One ed25519 verify + one membership-set lookup per incoming ack, ~µs. Signatures from non-members drop at the verify step. Gossipsub rate limits upstream. No DoS path.

### 11.5 In-flight op during FSM demotion

A publisher that is mid-`publish_and_await_ack` when its own FSM transitions to `Degraded` continues collecting acks until deadline — demotion is about advertised readiness to serve others, not about in-flight publishes. The op is already locally applied; acks only confirm propagation.

### 11.6 Probe storm

A pathological client calling `join_namespace` in a tight loop. Each probe triggers at most one out-of-cycle beacon from each `*Ready` peer; beacon emission is rate-limited per peer at `BEACON_INTERVAL / 2` minimum spacing. Probes that arrive faster than that are absorbed — responder has already beaconed recently. Gossipsub peer scoring also applies.

**Sybil amplification edge case.** Because `ReadinessProbe` is unsigned (§5.4 — joiner has no namespace identity at probe time), the per-(peer, namespace) rate limit can in principle be bypassed by an attacker who controls N gossipsub PeerIds (Sybil): each identity gets its own probe-response budget, so the attacker amplifies by ~N. This is bounded by:

1. **Gossipsub connection limits** — libp2p caps inbound connections per peer and the gossipsub mesh has bounded fanout, so beacons leave the responder at a rate dominated by `mesh_n` × `BEACON_INTERVAL/2`, not the attacker's probe rate.
2. **Per-namespace beacon volume cap** — even with infinite probe sources, each `*Ready` peer emits at most one out-of-cycle beacon per `BEACON_INTERVAL/2` per *requesting* peer per namespace; the steady-state outbound rate per namespace is therefore bounded by the size of the connected peer set.
3. **Operator monitoring** — emit beacon rates as `readiness_beacons_emitted_total{namespace_bucket, kind}` so a sudden spike vs. baseline triggers an investigation. Sybil amplification is detectable as an outlier in this metric long before it threatens overall throughput.

A global cross-namespace cap (e.g. "max N out-of-cycle beacons/sec total") is **not** added here — it would lose per-namespace fairness during legitimate cold-fleet bursts. Operators who observe abuse can tighten gossipsub peer-scoring thresholds and add per-PeerId connection limits at the libp2p layer, both of which already exist as configuration knobs.

## 12. Code injection points

### 12.1 New modules

| Path | Purpose |
|---|---|
| `crates/context/src/governance_broadcast.rs` | Phase 1 check, `publish_and_await_ack`, `AckRouter`, `verify_ack`. Single choke-point. |
| `crates/node/src/readiness.rs` | `ReadinessTier` + FSM + `ReadinessCache` + beacon scheduler + SD1 evaluator. |
| `crates/context-client/src/local_governance/wire.rs` | `NamespaceTopicMsg`, `GroupTopicMsg`, `SignedAck`, `SignedReadinessBeacon`, `ReadinessProbe`, `hash_scoped`. |
| `crates/client/src/client/namespace_retry.rs` | `join_namespace_with_retry` SDK helper. |

### 12.2 Modified files

| File | Change |
|---|---|
| `crates/context/src/group_store/namespace_governance.rs` | `sign_apply_and_publish_namespace_op` and `sign_and_publish_namespace_op` return `Result<DeliveryReport>`; delegate to `publish_and_await_ack`. |
| `crates/context/src/group_store/governance_signer.rs` | `publish_group_op`, `publish_namespace_op`, `publish_namespace_op_without_apply` — same return-type change + delegation. |
| `crates/context/src/lib.rs` (`ContextManager::sign_and_publish_group_op`) | Return-type change; all handler callers adopt the new type. |
| All files in `crates/context/src/handlers/*.rs` that publish governance ops | Consume `DeliveryReport` / map to outward-facing API response types. Listed explicitly: `add_group_members.rs`, `remove_group_members.rs`, `create_group.rs`, `delete_group.rs`, `join_group.rs`, `set_group_alias.rs`, `set_default_capabilities.rs`, `set_default_visibility.rs`, `set_member_alias.rs`, `update_group_settings.rs`, `update_member_role.rs`, `upgrade_group.rs`, `admit_tee_node.rs`, `set_tee_admission_policy.rs`. |
| `crates/context/src/handlers/join_group.rs` | Wait for first valid group key in local store (watch-channel) before returning `Ok`; emit `JoinGroupError::NoKeyReceived` on timeout. |
| `crates/node/src/handlers/network_event/namespace.rs` | `borsh::from_slice::<SignedNamespaceOp>` → `NamespaceTopicMsg` match; `Op` arm emits `Ack` on success; `Ack` arm routes to `AckRouter`; `ReadinessBeacon` arm feeds `ReadinessCache`; `ReadinessProbe` arm triggers out-of-cycle beacon if `*Ready`. Collapse the ~180-line state-divergence branch of `handle_namespace_state_heartbeat`. |
| `crates/node/src/handlers/network_event/subscriptions.rs` | Track `known_subscribers` per topic for Phase 1 threshold. |
| `crates/node/src/key_delivery.rs` | `maybe_publish_key_delivery` goes through `publish_and_await_ack` with `required_signers: Some([recipient_pubkey])`. |
| `crates/node/src/sync/manager/mod.rs` | Collapse cross-peer `parent_pull` enumeration to single-peer retry; keep PR #2252's unknown-peer catch-up as safety net. |
| `crates/context-primitives/src/errors.rs` | New: `GovernanceBroadcastError`, `JoinError`, `ReadyError`, `JoinGroupError::NoKeyReceived`. |
| `crates/node/src/sync/config.rs` | New knobs: `op_ack_timeout`, `member_change_timeout`, `heavy_op_timeout`, `join_deadline`, `await_ready_deadline`, `boot_grace`, `beacon_interval`, `ttl_heartbeat`, `group_key_wait`. |
| `crates/client/src/client/namespace.rs` | `join_namespace` (new semantics) and `await_namespace_ready` (new) as two explicit SDK methods. |

### 12.3 Files and lines deleted / collapsed

- `crates/node/src/sync/parent_pull.rs`: `ParentPullBudget` multi-peer enumeration collapses to single-peer retry. ~200 → ~30 lines.
- `crates/node/src/handlers/network_event/namespace.rs`: divergence-reconciliation branch of `handle_namespace_state_heartbeat`. ~180 lines deleted.
- `apps/e2e-kv-store/workflows/group-subgroup-queued-deltas.yml`: `type: wait, seconds: N` at lines 103-104, 146-147, 343-344. Deleted.
- `apps/e2e-kv-store/workflows/group-subgroup-cold-sync.yml`: `type: wait, seconds: 5` at lines 105-106. Deleted.
- `.github/workflows/e2e-rust-apps.yml`: `max_attempts=2` + `merobox nuke --force` retry block. Deleted.

## 13. Migration order

One PR per bullet, each independently mergeable and reversible:

1. **Stage-0: instrumentation.** `governance_publish_mesh_peers_at_publish` gauge per publish, tagged by op kind. ~10 LoC. Baseline before/after measurement.
2. **Three-phase mechanism + wire.** `NamespaceTopicMsg`/`GroupTopicMsg` enum, `SignedAck`, `AckRouter`, `governance_broadcast.rs`. Mechanism present; no endpoint migrated yet. Library change only.
3. **Migrate `add_group_members` + `remove_group_members`.** Acceptance: `group-subgroup-queued-deltas.yml` passes without the `seconds: 3|5|10` waits.
4. **Readiness FSM + `ReadinessBeacon` + `ReadinessProbe`.** Including tier split (§7.3). Still no API contract change.
5. **Migrate `join_namespace` to J6** (`join_namespace` + `await_namespace_ready` + `join_and_wait_ready`). Client-facing API change. Acceptance: `group-subgroup-cold-sync.yml` passes without sleeps; new integration test "join + 20 governance ops + read, ≤2s on 2-node" passes.
6. **Migrate `KeyDelivery` + `join_group`.** `maybe_publish_key_delivery` uses `required_signers`; `join_group` waits for key-present. Acceptance: `fuzzy-handlers-node-4` goes from 100% failure to 0% over 10 consecutive runs.
7. **Migrate remaining endpoints.** `create_group_in_namespace`, `reparent_group`, `set_context_alias`, `update_member_role`, `upgrade_group`, `admit_tee_node`, etc. One PR per 2-3 endpoints.
8. **Cleanup.** Delete `parent_pull` multi-peer enumeration. Collapse `handle_namespace_state_heartbeat` divergence arm. Delete workflow `max_attempts` retries. Acceptance: green CI with single-attempt runs for 5 consecutive master builds.

## 14. Testing

### 14.1 Unit tests

| Target | Coverage |
|---|---|
| `governance_broadcast::publish_and_await_ack` | Happy path (1 ack); multi-ack (`min_acks=3`); timeout → `NoAckReceived`; forged ack dropped (signer not in member set); duplicate acks don't double-count; `required_signers` filter. |
| `readiness::evaluate_readiness` | All transitions: `Bootstrapping → LocallyReady` (boot grace + non-empty DAG), `Bootstrapping → PeerValidatedReady` (heard beacon), `LocallyReady → PeerValidatedReady`, `PeerValidatedReady → Degraded` (pending ops), `Degraded → CatchingUp`. Empty-DAG joiner never self-promotes. |
| `ReadinessCache::pick_sync_partner` | `(strong desc, applied_through desc, ts desc)` ordering; TTL eviction; empty cache → `None`; only-stale-entries → `None`. |
| Wire codec | `NamespaceTopicMsg` round-trip via borsh; forward-compat behaviour when an old reader gets a new variant. |

### 14.2 Integration tests (`tokio::test`, in-process)

| Scenario | Expectation |
|---|---|
| 2-node cold start with `add_group_members` at t=0 on publisher | Succeeds without retry; subscriber observes within `op_ack_timeout`. |
| 3-node cold boot (simultaneous) | All reach `PeerValidatedReady` within `BOOT_GRACE + backfill_ms` (≤15s). |
| Joiner into warm namespace | `join_and_wait_ready` completes ≤2s on localhost. |
| Joiner where all peers are `Bootstrapping` | First attempt `NoReadyPeers`; `join_namespace_with_retry` succeeds after one peer self-promotes. |
| Forged ack flood | Discarded; valid ack still collected; publisher's `elapsed_ms` bounded. |

### 14.3 E2E (existing + new)

- **New: `kv-store-joiner-cold.yml`** — 2 existing members at steady state; new joiner spins up and immediately runs 20 `set` operations; verifies all 20 observed on all 3 nodes. No `wait` steps.
- **Modify `group-subgroup-queued-deltas.yml`**: delete `type: wait, seconds: N` at lines 103-104, 146-147, 343-344. Passes deterministically on slowest CI class for 10 consecutive runs.
- **Modify `group-subgroup-cold-sync.yml`**: delete `type: wait, seconds: 5` at lines 105-106. Same criterion.
- **Modify `.github/workflows/e2e-rust-apps.yml`**: drop `max_attempts=2` + `merobox nuke --force`. Single-attempt green for 5 consecutive master builds.

### 14.4 Fuzzy test validation

- `fuzzy-handlers-node-4` failure rate drops from 100% to 0% over 10 consecutive runs after stage 6.
- `fuzzy-kv-store` no "last-joined node stuck at pre-seed root hash" stalls over 10 consecutive runs.

## 15. Observability

Prometheus metrics, extending `crates/node/src/sync/prometheus_metrics.rs`:

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `governance_publish_mesh_peers_at_publish` | Histogram | `op_kind` | Mesh size at publish time — detects cold-publish regressions (added at stage 0). |
| `governance_publish_outcome` | Counter | `op_kind, outcome={ok, not_ready, no_ack, publish_err}` | Per-endpoint health. |
| `governance_publish_ack_latency_ms` | Histogram | `op_kind` | Time from publish to first valid ack. |
| `readiness_transitions` | Counter | `namespace_bucket, from, to` | FSM transition log — spots thrashing. |
| `readiness_boot_to_ready_ms` | Histogram | `tier={locally, peer_validated}` | Time to become a sync partner after boot. |
| `readiness_locally_ready_fallback` | Counter | `namespace_bucket` | How often `BOOT_GRACE` fallback fires. Zero in steady state; non-zero during cold-fleet events. |
| `join_namespace_outcome` | Counter | `outcome={ok, no_ready_peers, timeout, key_not_received}` | Retry-loop health from the SDK helper. |
| `ack_router_pending_ops` | Gauge | — | In-flight ack-collection map size. Should stay small. |

Three Grafana rows to add:
- **Governance publish health** — `governance_publish_outcome` split by outcome, per `op_kind`.
- **Readiness FSM** — `readiness_transitions` and `readiness_boot_to_ready_ms` over time.
- **Join funnel** — `join_namespace_outcome` + retry counts.

**Cardinality note:** the raw 32-byte `namespace_id` (~2^256 possible values) is *not* used as a Prometheus label — that would explode metric cardinality and is exploitable as a DoS vector by an attacker who can create many namespaces. Instead, emit `namespace_bucket = hex_prefix(sha256(namespace_id), 1)` — a 1-byte SHA-256 prefix giving 256 stable, deterministic buckets. The full `namespace_id` is still logged via structured logging (`tracing::info!(namespace_id = %hex(ns), …)`) for per-namespace debugging without polluting the metric label set. Helper: `fn ns_metric_bucket(ns: [u8; 32]) -> String { format!("{:02x}", sha256(&ns)[0]) }`.

## 16. Acceptance criteria

1. `type: wait, seconds: N` at exact file/line locations in `group-subgroup-queued-deltas.yml` and `group-subgroup-cold-sync.yml` is removed; both workflows pass deterministically on slowest CI class for 10 consecutive runs with `max_attempts=1`.
2. `join_namespace` + 20 governance ops + read on a freshly-joined node ≤ 2s on 2-node localhost, no caller-side retries.
3. `list_group_members` on a freshly-joined namespace returns full state on the first call — no retry loop.
4. `fuzzy-handlers-node-4` failure rate: 100% → 0% over 10 consecutive runs.
5. `fuzzy-kv-store` no "node stuck at pre-seed root hash" 30s stalls over 10 consecutive runs.
6. `governance_publish_outcome{outcome="no_ack"}` < 0.1% of publishes over 24h under normal fleet health.
7. `readiness_locally_ready_fallback` is 0 in steady state; spikes during planned cold-boot events decay to 0 within 60s.

## 17. Appendix — primitives already in place

Noted so implementation doesn't re-derive:

- `network_client.mesh_peers(topic).await` (`crates/network/src/handlers/commands/mesh_peers.rs`) — Phase 1 primitive.
- `network_client.mesh_peer_count(topic).await` — cheaper variant that avoids allocation.
- `handle_subscribed` (`crates/node/src/handlers/network_event/subscriptions.rs`) — natural place to populate `known_subscribers` per topic.
- `gossipsub.all_peers()` — populates in ms after subscribe (much faster than mesh GRAFT ~s). Useful for distinguishing "mesh not warm" from "no subscribers exist" if we ever want to refine the Phase-1 threshold.
- Gossipsub config (`heartbeat_interval`, `mesh_n_low`, `flood_publish`) — tunable in `calimero_network` if mesh convergence is slower than needed. Not required for correctness; complementary to the contract.
- `NamespaceBackfillRequest` / `NamespaceBackfillResponse` protocol — still used for step 6 of `await_namespace_ready`; traffic volume drops to near-zero during normal operation but protocol stays.
- PR #2252's "request governance catch-up on unknown peer instead of closing" — kept as belt-and-braces safety net for long-offline returning peers.

## 18. Out of scope for this design

- DHT-backed member directory (#2236's direction). Orthogonal — addresses ex-member isolation and capability-gated mesh admission, not delivery semantics.
- Rolling upgrade with older nodes. Coordinated cluster-wide upgrade assumed.
- CRDT data-delta sync for user writes. Remains eventually consistent by design; contract explicitly does not apply.
- Peer discovery (rendezvous, kad, mdns) in `crates/network/`. Unchanged.
- Signature verification and DAG structure. Acks are additive; no changes to how ops are signed, hashed, or linked.
