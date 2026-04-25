# Governance Three-Phase Contract + Readiness Handshake — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fire-and-forget governance and KeyDelivery publishes with an acked, typed-outcome three-phase contract; add a per-namespace readiness FSM with signed `ReadinessBeacon` / `ReadinessProbe` messages so sync-partner selection respects state-readiness; split `join_namespace` into a fast `Ok(JoinStarted)` and an explicit `await_namespace_ready` per the J6 design; eliminate the workaround scaffolding (e2e sleeps, multi-peer parent-pull enumeration, heartbeat divergence reconciliation).

**Architecture:** Single choke-point at `crates/context/src/group_store/namespace_governance.rs` and `governance_signer.rs` — wrap `sign_and_publish_*` to delegate to a new `governance_broadcast::publish_and_await_ack`. New `governance_broadcast.rs` module owns Phase-1 readiness check + `AckRouter` + ack collection. New `crates/node/src/readiness.rs` owns the per-namespace FSM, `ReadinessCache`, beacon scheduler. Wire format on `ns/<id>` and `group/<id>` topics changes from bare `SignedNamespaceOp`/`SignedGroupOp` to discriminated `NamespaceTopicMsg`/`GroupTopicMsg` enums (`Op | Ack | ReadinessBeacon | ReadinessProbe`). Migration is endpoint-by-endpoint, each stage independently mergeable.

**Tech Stack:** Rust 1.88.0, actix actor framework, libp2p gossipsub, tokio async, borsh serialization, blake3 for op hashing, ed25519 for signatures, RocksDB / InMemoryDB for storage. Tests use `#[tokio::test]` with `tempfile::tempdir()` and `Store::new(Arc::new(InMemoryDB::owned()))` for in-memory fixtures.

**Spec:** `docs/superpowers/specs/2026-04-25-governance-three-phase-readiness-design.md`

**Tracking issue:** [#2237](https://github.com/calimero-network/core/issues/2237)

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `crates/context/src/governance_broadcast.rs` | `publish_and_await_ack`, Phase-1 `assert_transport_ready`, `AckRouter`, `verify_ack`, `hash_scoped`. The single broadcast choke-point all endpoints delegate to. |
| `crates/context-client/src/local_governance/wire.rs` | Discriminated wire enums `NamespaceTopicMsg` / `GroupTopicMsg`, `SignedAck`, `SignedReadinessBeacon`, `ReadinessProbe`. |
| `crates/node/src/readiness.rs` | `ReadinessTier` enum, per-namespace FSM, `ReadinessCache`, beacon scheduler, `evaluate_readiness` evaluator. |
| `crates/node/src/handlers/network_event/readiness.rs` | `handle_readiness_beacon`, `handle_readiness_probe` — per-kind branches feeding the cache and triggering out-of-cycle beacons. |
| `crates/client/src/client/namespace_retry.rs` | `join_namespace_with_retry` SDK helper with exponential backoff. |
| `crates/context/src/governance_broadcast/tests.rs` | Unit tests for Phase-1, `publish_and_await_ack` happy/timeout/forged-ack paths. |
| `crates/node/src/readiness/tests.rs` | Unit tests for FSM transitions and `ReadinessCache` ordering. |
| `apps/e2e-kv-store/workflows/kv-store-joiner-cold.yml` | New e2e: 2 warm members + cold joiner + 20 ops, no sleeps, all-3-converge assertion. |

### Modified files

| Path | Change summary |
|---|---|
| `crates/context-client/src/local_governance.rs` | Mount the new `wire` submodule + re-export. |
| `crates/context/src/group_store/namespace_governance.rs` | `sign_apply_and_publish` / `sign_and_publish_without_apply` route through `publish_and_await_ack`; return `EyreResult<DeliveryReport>`. |
| `crates/context/src/group_store/governance_signer.rs` | Same return-type change on the four `publish_*` functions; delegation to `publish_and_await_ack`. |
| `crates/context/src/group_store/mod.rs` | `GroupGovernancePublisher::sign_apply_and_publish` and the two `publish_group_op` wrappers — return-type change + delegation. |
| `crates/context/src/lib.rs` | `ContextManager::sign_and_publish_group_op` returns `DeliveryReport`. New `ContextManager::ack_router` field. |
| `crates/context/src/handlers/add_group_members.rs` | Consume `DeliveryReport`; surface `MemberChangeReport` on response. |
| `crates/context/src/handlers/remove_group_members.rs` | Same shape. |
| `crates/context/src/handlers/create_group.rs` | Consume `DeliveryReport`. |
| `crates/context/src/handlers/delete_group.rs` | Consume `DeliveryReport`. |
| `crates/context/src/handlers/join_group.rs` | Wait for first valid group key on a watch-channel before returning `Ok`; new `JoinGroupError::NoKeyReceived`. |
| `crates/context/src/handlers/set_group_alias.rs`, `set_default_capabilities.rs`, `set_default_visibility.rs`, `set_member_alias.rs`, `update_group_settings.rs`, `update_member_role.rs`, `upgrade_group.rs`, `admit_tee_node.rs`, `set_tee_admission_policy.rs`, `set_member_capabilities.rs`, `update_application/*.rs` | Phase 11 — return-type updates. Compile cleanly under new contract. |
| `crates/context-primitives/src/errors.rs` (or wherever the existing error enums live) | New: `GovernanceBroadcastError`, `JoinError`, `ReadyError`, `JoinGroupError::NoKeyReceived`. |
| `crates/node/src/handlers/network_event/namespace.rs` | Replace `borsh::from_slice::<SignedNamespaceOp>` with `NamespaceTopicMsg` match; emit `Ack` after successful apply; route `ReadinessBeacon`/`Probe` to new submodule; collapse the ~180-line state-divergence branch of `handle_namespace_state_heartbeat`. |
| `crates/node/src/handlers/network_event/subscriptions.rs` | Maintain `HashMap<TopicHash, HashSet<PeerId>>` of known subscribers per topic for Phase-1 threshold. |
| `crates/node/src/key_delivery.rs` | `maybe_publish_key_delivery` routes through `publish_and_await_ack` with `required_signers: Some([recipient])`. |
| `crates/node/src/sync/manager/mod.rs` | Collapse `parent_pull` cross-peer enumeration to single-peer retry; keep PR #2252's unknown-peer catch-up branch as safety net. |
| `crates/node/src/sync/parent_pull.rs` | Reduce from `ParentPullBudget` multi-peer scheduler to a small single-peer helper. |
| `crates/node/src/sync/config.rs` | New knobs: `op_ack_timeout`, `member_change_timeout`, `heavy_op_timeout`, `join_deadline`, `await_ready_deadline`, `boot_grace`, `beacon_interval`, `ttl_heartbeat`, `group_key_wait`, `applied_through_grace`. |
| `crates/node/src/sync/prometheus_metrics.rs` | New metrics per §15 of the spec. |
| `crates/node/src/manager.rs` / `manager/startup.rs` | Wire `readiness::ReadinessManager` into the actor tree alongside the existing heartbeat scheduler. |
| `crates/node/src/lib.rs` | Mount `readiness` module. |
| `crates/client/src/client/namespace.rs` | Two explicit methods: `join_namespace` (J6 step 1-4 semantics) and `await_namespace_ready`; mount retry helper module. |
| `apps/e2e-kv-store/workflows/group-subgroup-queued-deltas.yml` | Delete `type: wait, seconds: N` at lines 103-104, 146-147, 343-344. |
| `apps/e2e-kv-store/workflows/group-subgroup-cold-sync.yml` | Delete `type: wait, seconds: 5` at lines 105-106. |
| `.github/workflows/e2e-rust-apps.yml` | Drop `max_attempts=2` + `merobox nuke --force`. |

---

## Phase 1 — Stage-0 instrumentation

**Goal:** Land the `governance_publish_mesh_peers_at_publish` histogram metric at the existing publish sites so we have a baseline measurement of how often we're publishing into cold meshes today. Independently mergeable; no contract change.

### Task 1.1 — Add `mesh_peers_at_publish` histogram

**Files:**
- Modify: `crates/node/src/sync/prometheus_metrics.rs`
- Modify: `crates/context/src/group_store/governance_signer.rs`
- Modify: `crates/context/src/group_store/namespace_governance.rs`

- [ ] **Step 1: Add the histogram to prometheus_metrics.rs.**

```rust
// crates/node/src/sync/prometheus_metrics.rs — add to the lazy_static block

pub static GOVERNANCE_PUBLISH_MESH_PEERS_AT_PUBLISH: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "calimero_governance_publish_mesh_peers_at_publish",
        "Number of mesh peers visible at the moment a governance op is published",
        &["op_kind"],
        vec![0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0],
    )
    .expect("metric registration")
});
```

- [ ] **Step 2: Emit at the namespace publish site.**

In `crates/context/src/group_store/governance_signer.rs::publish_namespace_op` (and `publish_namespace_op_without_apply`), after computing `topic_hash` but before calling the underlying `publish`, do:

```rust
let mesh_count = node_client.network_client().mesh_peer_count(topic_hash.clone()).await;
calimero_node_metrics::GOVERNANCE_PUBLISH_MESH_PEERS_AT_PUBLISH
    .with_label_values(&[op_kind_label(&op)])
    .observe(mesh_count as f64);
```

Add a small helper at the top of the file:

```rust
fn op_kind_label(op: &NamespaceOp) -> &'static str {
    match op {
        NamespaceOp::Root(RootOp::MemberJoined { .. })       => "member_joined",
        NamespaceOp::Root(RootOp::KeyDelivery { .. })        => "key_delivery",
        NamespaceOp::Root(RootOp::PolicyUpdated { .. })      => "policy_updated",
        NamespaceOp::Group(_)                                => "group_op",
        // extend as variants exist; default fallback for forward compat:
        _                                                    => "other",
    }
}
```

- [ ] **Step 3: Emit at the group publish site.**

Mirror the same emission in `governance_signer.rs::publish_group_op` and `publish_group_removal`, with `op_kind_label_group(&op)` returning labels per `GroupOp` variant (`member_added`, `member_removed`, `member_role_set`, `alias_set`, `default_capabilities_set`, `default_visibility_set`, `context_registered`, `context_alias_set`, `context_detached`, `target_application_set`, `upgrade_policy_set`, `member_joined_via_tee`, `other`).

- [ ] **Step 4: Build + lint.**

```bash
cargo build -p calimero-context -p calimero-node 2>&1 | tail -20
cargo clippy -p calimero-context -p calimero-node -- -A warnings 2>&1 | tail -10
cargo fmt --check 2>&1 | tail -5
```

Expected: clean build, no new clippy warnings, no fmt diff.

- [ ] **Step 5: Commit.**

```bash
git add crates/node/src/sync/prometheus_metrics.rs crates/context/src/group_store/governance_signer.rs crates/context/src/group_store/namespace_governance.rs
git commit -m "feat(node/metrics): instrument mesh_peers_at_publish histogram for #2237 baseline"
```

---

## Phase 2 — Wire protocol enum + new message types

**Goal:** Introduce `NamespaceTopicMsg` / `GroupTopicMsg` discriminated enums and the three new message structs (`SignedAck`, `SignedReadinessBeacon`, `ReadinessProbe`). Receiver-side: temporarily wrap existing `SignedNamespaceOp` deserialization in a backward-compatible adapter so this phase is mergeable independently. Forward all current code paths through `NamespaceTopicMsg::Op` only; new variants are added but not yet emitted.

### Task 2.1 — Add wire types module

**Files:**
- Create: `crates/context-client/src/local_governance/wire.rs`
- Modify: `crates/context-client/src/local_governance.rs`

- [ ] **Step 1: Create the wire module with the three structs and the two enums.**

```rust
// crates/context-client/src/local_governance/wire.rs

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use crate::local_governance::{SignedGroupOp, SignedNamespaceOp};

/// Topic-scoped op hash: blake3(topic_id || borsh(SignedOp)).
/// Topic-scoping prevents cross-namespace replay of (coincidentally identical) acks.
pub fn hash_scoped_namespace(topic_id: &[u8], op: &SignedNamespaceOp) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    hasher.update(&borsh::to_vec(op).expect("borsh"));
    *hasher.finalize().as_bytes()
}

pub fn hash_scoped_group(topic_id: &[u8], op: &SignedGroupOp) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    hasher.update(&borsh::to_vec(op).expect("borsh"));
    *hasher.finalize().as_bytes()
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignedAck {
    pub op_hash: [u8; 32],
    pub signer_pubkey: PublicKey,
    pub signature: [u8; 64],
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignedReadinessBeacon {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    pub dag_head: [u8; 32],
    pub applied_through: u64,
    pub ts_millis: u64,
    pub strong: bool,
    pub signature: [u8; 64],
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct ReadinessProbe {
    pub namespace_id: [u8; 32],
    pub nonce: [u8; 16],
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum NamespaceTopicMsg {
    Op(SignedNamespaceOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum GroupTopicMsg {
    Op(SignedGroupOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
}
```

- [ ] **Step 2: Mount the submodule.**

In `crates/context-client/src/local_governance.rs`, near the top:

```rust
pub mod wire;
pub use wire::{
    hash_scoped_group, hash_scoped_namespace, GroupTopicMsg, NamespaceTopicMsg, ReadinessProbe,
    SignedAck, SignedReadinessBeacon,
};
```

- [ ] **Step 3: Add `blake3` dep if not present.**

```bash
grep -n "blake3" crates/context-client/Cargo.toml || echo "MISSING — add to [dependencies]"
```

If missing, add `blake3 = "1"` (workspace version) under `[dependencies]` in `crates/context-client/Cargo.toml`.

- [ ] **Step 4: Write a borsh round-trip test.**

```rust
// crates/context-client/src/local_governance/wire.rs — append at bottom

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PrivateKey;

    #[test]
    fn signed_ack_roundtrip() {
        let ack = SignedAck {
            op_hash: [7u8; 32],
            signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
            signature: [9u8; 64],
        };
        let bytes = borsh::to_vec(&ack).expect("ser");
        let parsed: SignedAck = borsh::from_slice(&bytes).expect("de");
        assert_eq!(parsed.op_hash, ack.op_hash);
        assert_eq!(parsed.signature, ack.signature);
    }

    #[test]
    fn namespace_topic_msg_discriminates_kinds() {
        let probe = NamespaceTopicMsg::ReadinessProbe(ReadinessProbe {
            namespace_id: [1u8; 32],
            nonce: [2u8; 16],
        });
        let bytes = borsh::to_vec(&probe).expect("ser");
        let parsed: NamespaceTopicMsg = borsh::from_slice(&bytes).expect("de");
        match parsed {
            NamespaceTopicMsg::ReadinessProbe(p) => {
                assert_eq!(p.namespace_id, [1u8; 32]);
                assert_eq!(p.nonce, [2u8; 16]);
            }
            _ => panic!("wrong variant"),
        }
    }
}
```

- [ ] **Step 5: Run the tests.**

```bash
cargo test -p calimero-context-client wire:: 2>&1 | tail -20
```

Expected: 2 tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/context-client/src/local_governance.rs crates/context-client/src/local_governance/wire.rs crates/context-client/Cargo.toml
git commit -m "feat(context-client/wire): add NamespaceTopicMsg/GroupTopicMsg + Ack/ReadinessBeacon/Probe types"
```

### Task 2.2 — Switch publish path to wrap in `NamespaceTopicMsg::Op`

**Files:**
- Modify: `crates/context/src/group_store/namespace_governance.rs`
- Modify: `crates/context/src/group_store/mod.rs` (`publish_group_op`)
- Modify: `crates/node/src/handlers/network_event/namespace.rs` (receiver-side decode)

- [ ] **Step 1: At every place that calls `network_client.publish(topic, borsh(SignedNamespaceOp))`, replace with `borsh(NamespaceTopicMsg::Op(op))`.**

Run this to find all call sites:

```bash
rg -n "publish.*SignedNamespaceOp|publish.*borsh" crates/context/src crates/node/src --no-heading | grep -v test
```

Each call site changes the inner payload from `borsh::to_vec(&signed_op)?` to `borsh::to_vec(&NamespaceTopicMsg::Op(signed_op))?`. Do not yet emit Ack / Beacon / Probe variants — the publish-side change is purely the wrapping.

- [ ] **Step 2: Update receiver in `network_event/namespace.rs` to decode the new wrapper but error cleanly on unknown variants.**

```rust
// crates/node/src/handlers/network_event/namespace.rs — replace the existing
// borsh::from_slice::<SignedNamespaceOp>(&payload) call

let msg: NamespaceTopicMsg = match borsh::from_slice(&payload) {
    Ok(m) => m,
    Err(e) => {
        warn!(?e, "failed to decode NamespaceTopicMsg; dropping message");
        return;
    }
};

match msg {
    NamespaceTopicMsg::Op(op) => {
        // existing path: apply, key delivery, etc. (unchanged from prior code)
        handle_namespace_op(this, ctx, peer_id, topic, op).await;
    }
    NamespaceTopicMsg::Ack(_) | NamespaceTopicMsg::ReadinessBeacon(_) | NamespaceTopicMsg::ReadinessProbe(_) => {
        // Phases 5, 7, 8 — not yet wired. Drop with a debug log so the wire
        // change is forward-compatible while later phases are landing.
        debug!("NamespaceTopicMsg variant not yet handled; dropping");
    }
}
```

Same pattern on the group-topic dispatch in the corresponding handler.

- [ ] **Step 3: Build all touched crates.**

```bash
cargo build -p calimero-context -p calimero-node -p calimero-context-client 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 4: Run any pre-existing context/node tests to confirm no regression.**

```bash
cargo test -p calimero-context -p calimero-node 2>&1 | tail -30
```

Expected: same pass count as baseline (record baseline before this phase; should not drop).

- [ ] **Step 5: Commit.**

```bash
git add crates/context/src/group_store/namespace_governance.rs crates/context/src/group_store/mod.rs crates/node/src/handlers/network_event/namespace.rs
git commit -m "feat(context/wire): wrap published governance ops in NamespaceTopicMsg::Op"
```

---

## Phase 3 — `governance_broadcast` core: Phase-1 gate + AckRouter + publish_and_await_ack

**Goal:** Land the central acked-broadcast helper. This is the new module `crates/context/src/governance_broadcast.rs`. After this phase, the helper exists and is fully unit-tested but is not yet wired into `sign_and_publish_*`.

### Task 3.1 — `AckRouter` skeleton

**Files:**
- Create: `crates/context/src/governance_broadcast.rs`
- Create: `crates/context/src/governance_broadcast/tests.rs`
- Modify: `crates/context/src/lib.rs` (mount module + add `ack_router: Arc<AckRouter>` field on `ContextManager`)

- [ ] **Step 1: Create the module skeleton with `AckRouter` only.**

```rust
// crates/context/src/governance_broadcast.rs

use std::collections::HashMap;
use std::sync::Mutex;

use calimero_context_client::local_governance::SignedAck;
use tokio::sync::broadcast;

#[cfg(test)]
mod tests;

/// Routes incoming Ack messages to in-flight `publish_and_await_ack` callers,
/// keyed by op_hash. Each in-flight publish subscribes to the per-op channel
/// before publishing and unsubscribes (drops the receiver) on completion.
#[derive(Default)]
pub struct AckRouter {
    inner: Mutex<HashMap<[u8; 32], broadcast::Sender<SignedAck>>>,
}

impl AckRouter {
    pub fn subscribe(&self, op_hash: [u8; 32]) -> broadcast::Receiver<SignedAck> {
        let mut g = self.inner.lock().expect("ack_router lock");
        let tx = g.entry(op_hash).or_insert_with(|| broadcast::channel(64).0).clone();
        tx.subscribe()
    }

    /// Called from the wire receiver's Ack arm. Returns `true` if any subscriber
    /// was registered (purely for telemetry — not load-bearing).
    pub fn route(&self, ack: SignedAck) -> bool {
        let g = self.inner.lock().expect("ack_router lock");
        match g.get(&ack.op_hash) {
            Some(tx) => tx.send(ack).is_ok(),
            None => false,
        }
    }

    /// Called when a publish completes (Ok or NoAck) to GC entries with no
    /// remaining receivers. **Consumes the receiver** so it is dropped before we
    /// inspect `receiver_count()`; otherwise the caller's still-live `rx` on the
    /// stack would keep the count ≥ 1 and the entry would never be reaped,
    /// leaking one map entry per publish. Idempotent.
    pub fn release(&self, op_hash: [u8; 32], rx: broadcast::Receiver<SignedAck>) {
        drop(rx);
        let mut g = self.inner.lock().expect("ack_router lock");
        if let Some(tx) = g.get(&op_hash) {
            if tx.receiver_count() == 0 {
                let _ = g.remove(&op_hash);
            }
        }
    }
}
```

- [ ] **Step 2: Write the unit test stub.**

```rust
// crates/context/src/governance_broadcast/tests.rs

use calimero_primitives::identity::PrivateKey;

use super::*;

fn dummy_ack(op_hash: [u8; 32]) -> SignedAck {
    SignedAck {
        op_hash,
        signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
        signature: [0u8; 64],
    }
}

#[tokio::test]
async fn ack_router_subscribe_then_route_delivers() {
    let router = AckRouter::default();
    let mut rx = router.subscribe([1u8; 32]);
    let routed = router.route(dummy_ack([1u8; 32]));
    assert!(routed);
    let got = rx.recv().await.expect("ack received");
    assert_eq!(got.op_hash, [1u8; 32]);
}

#[tokio::test]
async fn ack_router_route_with_no_subscriber_returns_false() {
    let router = AckRouter::default();
    let routed = router.route(dummy_ack([2u8; 32]));
    assert!(!routed);
}

#[tokio::test]
async fn ack_router_release_drops_empty_entry() {
    let router = AckRouter::default();
    let rx = router.subscribe([3u8; 32]);
    router.release([3u8; 32], rx);
    assert!(router.inner.lock().unwrap().get(&[3u8; 32]).is_none());
}

#[tokio::test]
async fn ack_router_release_keeps_entry_when_other_receivers_alive() {
    // A second concurrent publish for the same op_hash must keep its subscription alive.
    let router = AckRouter::default();
    let rx_a = router.subscribe([4u8; 32]);
    let _rx_b = router.subscribe([4u8; 32]);
    router.release([4u8; 32], rx_a);
    assert!(
        router.inner.lock().unwrap().get(&[4u8; 32]).is_some(),
        "entry must survive while another receiver is alive"
    );
}

#[tokio::test]
async fn ack_router_release_does_not_leak_when_caller_holds_rx() {
    // Regression: previously `release(op_hash)` checked `receiver_count() == 0`
    // while the caller's `rx` was still on the stack, leaking one entry per
    // publish. The new signature consumes `rx`, eliminating the leak.
    let router = AckRouter::default();
    for i in 0..16u8 {
        let key = [i; 32];
        let rx = router.subscribe(key);
        router.release(key, rx);
    }
    assert!(
        router.inner.lock().unwrap().is_empty(),
        "release must reap every entry; previously this map would have grown to 16"
    );
}
```

- [ ] **Step 3: Mount the module + add field on `ContextManager`.**

```rust
// crates/context/src/lib.rs

pub mod governance_broadcast;
use governance_broadcast::AckRouter;
use std::sync::Arc;

// Inside `pub struct ContextManager { ... }`:
pub(crate) ack_router: Arc<AckRouter>,

// Inside the constructor / actor::Started / wherever ContextManager is initialized:
ack_router: Arc::new(AckRouter::default()),
```

- [ ] **Step 4: Run the tests.**

```bash
cargo test -p calimero-context governance_broadcast::tests 2>&1 | tail -15
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/context/src/governance_broadcast.rs crates/context/src/governance_broadcast/tests.rs crates/context/src/lib.rs
git commit -m "feat(context/broadcast): AckRouter skeleton with op_hash-keyed subscriptions"
```

### Task 3.2 — `verify_ack` + governance member lookup

**Files:**
- Modify: `crates/context/src/governance_broadcast.rs`
- Modify: `crates/context/src/governance_broadcast/tests.rs`

- [ ] **Step 1: Add `verify_ack` next to `AckRouter`.**

```rust
// crates/context/src/governance_broadcast.rs — append

use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::group_store;

/// Verify an Ack: signature over op_hash AND signer is a current member of
/// the relevant namespace. Returns false on any failure (silently dropped).
pub fn verify_ack(
    store: &Store,
    namespace_id: [u8; 32],
    expected_op_hash: [u8; 32],
    ack: &SignedAck,
) -> bool {
    if ack.op_hash != expected_op_hash {
        return false;
    }
    let Ok(verifying_key) = VerifyingKey::from_bytes(&ack.signer_pubkey.to_bytes()) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&ack.signature) else {
        return false;
    };
    if verifying_key.verify(&ack.op_hash, &sig).is_err() {
        return false;
    }
    // Membership: signer_pubkey ∈ current_governance_members(namespace_id) at this node's local DAG.
    group_store::namespace_member_pubkeys(store, namespace_id)
        .map(|members| members.contains(&ack.signer_pubkey))
        .unwrap_or(false)
}
```

- [ ] **Step 2: Add `namespace_member_pubkeys` to `group_store`.**

The existing `NamespaceMembershipService` in `crates/context/src/group_store/namespace_governance.rs` already iterates members via the membership-key prefix. Expose a free function that wraps it:

```rust
// crates/context/src/group_store/namespace.rs (or wherever namespace store helpers live)
pub fn namespace_member_pubkeys(
    store: &Store,
    namespace_id: [u8; 32],
) -> EyreResult<Vec<PublicKey>> {
    let ns_id = ContextGroupId::from(namespace_id);
    let service = NamespaceMembershipService::new(store, ns_id);
    let members = service.list_members()?;  // returns Vec<NamespaceMember>
    Ok(members.into_iter().map(|m| m.public_key).collect())
}
```

(`list_members` already exists on `NamespaceMembershipService` — verify with `rg -n "fn list_members" crates/context/src/group_store/namespace_governance.rs` before writing; if the method is named differently, rename the call and keep the wrapper signature stable.)

- [ ] **Step 3: Write tests for `verify_ack`.**

```rust
// crates/context/src/governance_broadcast/tests.rs — append

use calimero_store::db::InMemoryDB;
use std::sync::Arc;

#[tokio::test]
async fn verify_ack_rejects_wrong_op_hash() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let ack = dummy_ack([1u8; 32]);
    assert!(!verify_ack(&store, [42u8; 32], [9u8; 32], &ack));
}

#[tokio::test]
async fn verify_ack_rejects_invalid_signature() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let ack = dummy_ack([1u8; 32]); // dummy_ack uses [0u8; 64] sig — won't verify
    assert!(!verify_ack(&store, [42u8; 32], [1u8; 32], &ack));
}

#[tokio::test]
async fn verify_ack_rejects_non_member_signer() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    // Construct a properly-signed ack — but signer is not in the namespace member set.
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let pk = sk.public_key();
    let op_hash = [7u8; 32];
    let signature = sk.sign(&op_hash);
    let ack = SignedAck { op_hash, signer_pubkey: pk, signature: signature.to_bytes() };
    assert!(!verify_ack(&store, [42u8; 32], op_hash, &ack));
}
```

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-context governance_broadcast::tests 2>&1 | tail -15
```

Expected: 6 passing (3 from Task 3.1 + 3 new).

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(context/broadcast): verify_ack — signature + topic-scoped op_hash + member-set check"
```

### Task 3.3 — `assert_transport_ready` (Phase 1 gate)

**Files:**
- Modify: `crates/context/src/governance_broadcast.rs`
- Modify: `crates/context/src/governance_broadcast/tests.rs`
- Modify: `crates/node/src/handlers/network_event/subscriptions.rs`
- Modify: `crates/node/src/state.rs` (or wherever NodeState lives — host the `known_subscribers` map)

- [ ] **Step 1: Track known subscribers per topic.**

In `crates/node/src/handlers/network_event/subscriptions.rs::handle_subscribed`, after the existing parsing, insert `peer_id` into a `KnownSubscribers` map living on `NodeManager`. Match `handle_unsubscribed` to remove. Define the map:

```rust
// crates/node/src/state.rs (or wherever NodeManager state lives)
use std::collections::{HashMap, HashSet};
use libp2p::{gossipsub::TopicHash, PeerId};

#[derive(Default)]
pub struct KnownSubscribers {
    pub by_topic: HashMap<TopicHash, HashSet<PeerId>>,
}

impl KnownSubscribers {
    pub fn add(&mut self, topic: TopicHash, peer: PeerId) {
        self.by_topic.entry(topic).or_default().insert(peer);
    }
    pub fn remove(&mut self, topic: &TopicHash, peer: &PeerId) {
        if let Some(s) = self.by_topic.get_mut(topic) {
            s.remove(peer);
            if s.is_empty() { self.by_topic.remove(topic); }
        }
    }
    pub fn count(&self, topic: &TopicHash) -> usize {
        self.by_topic.get(topic).map(|s| s.len()).unwrap_or(0)
    }
}
```

Add `known_subscribers: KnownSubscribers` to `NodeManager` and wire into `handle_subscribed` / `handle_unsubscribed`.

- [ ] **Step 2: Add `assert_transport_ready` to `governance_broadcast.rs`.**

```rust
// crates/context/src/governance_broadcast.rs — append

use libp2p::gossipsub::TopicHash;

#[derive(Debug, thiserror::Error)]
pub enum GovernanceBroadcastError {
    #[error("namespace not ready: mesh={mesh}, required={required}")]
    NamespaceNotReady { mesh: usize, required: usize },
    #[error("no ack received within {waited_ms}ms (op_hash={op_hash:?})")]
    NoAckReceived { waited_ms: u64, op_hash: [u8; 32] },
    #[error("publish error: {0}")]
    Publish(String),
    #[error("local apply error: {0}")]
    LocalApply(String),
}

pub async fn assert_transport_ready(
    network_client: &calimero_network_primitives::client::NetworkClient,
    topic: TopicHash,
    known_subscribers: usize,
    mesh_n_low: usize,
) -> Result<(), GovernanceBroadcastError> {
    let mesh = network_client.mesh_peer_count(topic).await;
    let required = std::cmp::min(mesh_n_low, known_subscribers);
    if mesh < required {
        return Err(GovernanceBroadcastError::NamespaceNotReady { mesh, required });
    }
    Ok(())
}
```

- [ ] **Step 3: Tests for the gate.**

```rust
// crates/context/src/governance_broadcast/tests.rs — append

#[tokio::test]
async fn assert_transport_ready_passes_when_solo_namespace() {
    // known_subscribers=0 ⇒ required=0 ⇒ pass regardless of mesh size.
    let net = mock_network_client_with_mesh_count(0);
    let topic = TopicHash::from_raw("ns/0000");
    let result = assert_transport_ready(&net, topic, 0, 4).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn assert_transport_ready_rejects_when_mesh_below_threshold() {
    let net = mock_network_client_with_mesh_count(1);
    let topic = TopicHash::from_raw("ns/0000");
    let err = assert_transport_ready(&net, topic, 4, 4).await.unwrap_err();
    assert!(matches!(err, GovernanceBroadcastError::NamespaceNotReady { mesh: 1, required: 4 }));
}

#[tokio::test]
async fn assert_transport_ready_caps_required_by_known_subscribers() {
    // Only 1 subscriber known ⇒ required=1; mesh=1 should pass even though mesh_n_low=4.
    let net = mock_network_client_with_mesh_count(1);
    let topic = TopicHash::from_raw("ns/0000");
    let result = assert_transport_ready(&net, topic, 1, 4).await;
    assert!(result.is_ok());
}
```

(Engineer: implement `mock_network_client_with_mesh_count` as a small shim — wire-protocol parts of `NetworkClient` are actor-based, so prefer a trait extraction or a `mockall`-style mock. If extraction is too invasive, gate the test as `#[ignore]` and cover via integration tests in Phase 6 instead — but extracting a trait `MeshSnapshot { fn mesh_peer_count(&self, topic) -> usize }` is the right call long-term and unblocks all Phase-3 unit tests.)

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-context governance_broadcast::tests 2>&1 | tail -15
cargo test -p calimero-node handle_subscribed 2>&1 | tail -10
```

Expected: tests pass.

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(context/broadcast): assert_transport_ready Phase-1 gate + KnownSubscribers tracking"
```

### Task 3.4 — `publish_and_await_ack`

**Files:**
- Modify: `crates/context/src/governance_broadcast.rs`
- Modify: `crates/context/src/governance_broadcast/tests.rs`

- [ ] **Step 1: Add `DeliveryReport` + `publish_and_await_ack`.**

```rust
// crates/context/src/governance_broadcast.rs — append

use std::time::{Duration, Instant};

use calimero_context_client::local_governance::{
    hash_scoped_namespace, NamespaceTopicMsg, SignedNamespaceOp,
};
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct DeliveryReport {
    pub op_hash: [u8; 32],
    pub acked_by: Vec<PublicKey>,
    pub elapsed_ms: u64,
}

pub async fn publish_and_await_ack_namespace(
    store: &Store,
    network_client: &calimero_network_primitives::client::NetworkClient,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    topic: TopicHash,
    op: SignedNamespaceOp,
    op_timeout: Duration,
    min_acks: usize,
    required_signers: Option<Vec<PublicKey>>,
) -> Result<DeliveryReport, GovernanceBroadcastError> {
    let topic_id = topic.as_str().as_bytes();
    let op_hash = hash_scoped_namespace(topic_id, &op);
    let start = Instant::now();

    // local apply happens at the caller (or via apply_signed_namespace_op) — see Task 4.x
    // for the wrapping in NamespaceGovernance::sign_apply_and_publish.

    let mut rx = ack_router.subscribe(op_hash);
    let payload = borsh::to_vec(&NamespaceTopicMsg::Op(op))
        .map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;
    network_client
        .publish(topic.clone(), payload)
        .await
        .map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;

    let mut acked_by: Vec<PublicKey> = Vec::new();
    let deadline = start + op_timeout;
    loop {
        // `saturating_duration_since` returns ZERO past the deadline (no Instant
        // subtraction panic) — `tokio::time::timeout` then resolves immediately as
        // `Err(_elapsed)` on the zero duration. Spec §6.2 uses the identical pattern.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Move `rx` into release so the receiver is dropped before the count check.
            ack_router.release(op_hash, rx);
            return Err(GovernanceBroadcastError::NoAckReceived {
                waited_ms: start.elapsed().as_millis() as u64,
                op_hash,
            });
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Ok(ack)) => {
                if !verify_ack(store, namespace_id, op_hash, &ack) {
                    continue;
                }
                if let Some(req) = &required_signers {
                    if !req.contains(&ack.signer_pubkey) {
                        continue;
                    }
                }
                if !acked_by.iter().any(|p| *p == ack.signer_pubkey) {
                    acked_by.push(ack.signer_pubkey);
                }
                if acked_by.len() >= min_acks {
                    ack_router.release(op_hash, rx);
                    return Ok(DeliveryReport {
                        op_hash,
                        acked_by,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
            }
            // `Lagged(n)`: we missed n messages but the channel is still open — keep
            // polling, more acks may arrive (n is bounded by broadcast capacity = 64).
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            // `Closed`: all senders dropped (typically because a concurrent flow
            // released the AckRouter entry as the last subscriber). `recv()` will
            // return immediately on every subsequent call — `continue` would burn
            // CPU in a tight loop until the deadline. Treat as a terminal NoAck.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                ack_router.release(op_hash, rx);
                return Err(GovernanceBroadcastError::NoAckReceived {
                    waited_ms: start.elapsed().as_millis() as u64,
                    op_hash,
                });
            }
            Err(_elapsed) => {
                ack_router.release(op_hash, rx);
                return Err(GovernanceBroadcastError::NoAckReceived {
                    waited_ms: start.elapsed().as_millis() as u64,
                    op_hash,
                });
            }
        }
    }
}
```

(Engineer: a parallel `publish_and_await_ack_group` function with `SignedGroupOp` / `GroupTopicMsg` and `hash_scoped_group` is added in Task 3.5 if needed; for now namespace-side is enough to unblock Phase-4 endpoints since group-op publishes funnel through namespace topics in this codebase — verify by checking `GroupGovernancePublisher::sign_apply_and_publish` and confirm the topic used.)

- [ ] **Step 2: Test the timeout path with a stub publisher.**

The `publish_and_await_ack_namespace` happy-path tests are integration-style — they require a working `NetworkClient` and a member-populated store. Cover those in Phase 5's full-stack tests via the existing pattern in `crates/client/src/tests.rs` (look at `async fn join_namespace()` at line 511 for the multi-node test fixture). For Phase 3 we only need to confirm timeout behaviour, which is unit-testable because publishing an `Ok(())` to a no-op transport stub is enough.

Extract a thin trait so we can stub `publish`:

```rust
// crates/context/src/governance_broadcast.rs — add near the top
#[async_trait::async_trait]
pub trait BroadcastTransport: Send + Sync {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize;
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), String>;
}

#[async_trait::async_trait]
impl BroadcastTransport for calimero_network_primitives::client::NetworkClient {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        Self::mesh_peer_count(self, topic).await
    }
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), String> {
        Self::publish(self, topic, bytes).await.map_err(|e| e.to_string())
    }
}
```

Then `publish_and_await_ack_namespace` takes `transport: &dyn BroadcastTransport` instead of `&NetworkClient` directly. Real callers pass the existing `NetworkClient`; tests pass a stub.

```rust
// crates/context/src/governance_broadcast/tests.rs — append

struct StubTransport { mesh: usize }

#[async_trait::async_trait]
impl BroadcastTransport for StubTransport {
    async fn mesh_peer_count(&self, _: TopicHash) -> usize { self.mesh }
    async fn publish(&self, _: TopicHash, _: Vec<u8>) -> Result<(), String> { Ok(()) }
}

#[tokio::test]
async fn publish_and_await_ack_times_out_when_no_ack_arrives() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let router = AckRouter::default();
    let transport = StubTransport { mesh: 4 };
    let topic = TopicHash::from_raw("ns/test");
    let signed_op = mk_test_signed_namespace_op(&store, [42u8; 32]);

    let res = publish_and_await_ack_namespace(
        &store, &transport, &router,
        [42u8; 32], topic, signed_op,
        Duration::from_millis(50), 1, None,
    ).await;

    assert!(matches!(res, Err(GovernanceBroadcastError::NoAckReceived { .. })));
}

#[tokio::test]
async fn publish_and_await_ack_dedups_acks_from_same_signer() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let router = AckRouter::default();
    let transport = StubTransport { mesh: 4 };
    let topic = TopicHash::from_raw("ns/test");
    let (op, op_hash) = mk_test_signed_namespace_op_with_hash(&store, [42u8; 32], topic.as_str());
    let alice_sk = mk_member_in_store(&store, [42u8; 32]);

    let r1 = router.clone();
    let oh = op_hash;
    tokio::spawn(async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            r1.route(sign_ack(&alice_sk, oh));   // same signer 3x
        }
    });

    // min_acks=2 should NOT be satisfied by 3 acks from one signer.
    let res = publish_and_await_ack_namespace(
        &store, &transport, &router,
        [42u8; 32], topic, op,
        Duration::from_millis(200), 2, None,
    ).await;
    assert!(matches!(res, Err(GovernanceBroadcastError::NoAckReceived { .. })));
}
```

`mk_test_signed_namespace_op` and `mk_member_in_store` are tiny helpers — define them in the test module (each ~10 lines) using `NamespaceGovernance::sign` and the existing membership-write path. If the existing membership-write API is hidden behind actor messages, instead use `apply_signed_namespace_op` to plant a `MemberJoined` op directly in the store before running the test.

The remaining behaviours (forged-ack drop, `required_signers` filter, ack from valid signer leading to `Ok`) are covered by the multi-node integration test in Task 5.2 Step 4 (`cargo test --workspace`) — all those code paths fire when an actual two-node fixture publishes and receives.

- [ ] **Step 3: Run.**

```bash
cargo test -p calimero-context governance_broadcast 2>&1 | tail -20
```

Expected: 8 tests pass (3 from 3.1 + 3 from 3.2 + 2 from 3.4).

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(context/broadcast): publish_and_await_ack_namespace + 5 unit tests"
```

---

## Phase 4 — Receiver-side ack emission

**Goal:** When the network receives a `NamespaceTopicMsg::Op(op)` and successfully applies it, emit a `SignedAck` back on the same topic. Also wire the `Ack` arm to route into `AckRouter`. Group-op symmetry follows the same pattern.

### Task 4.1 — Sign and emit an Ack after successful op apply

**Files:**
- Modify: `crates/node/src/handlers/network_event/namespace.rs`
- Create: `crates/context/src/governance_broadcast/ack_signer.rs` (or inline in `governance_broadcast.rs`)

- [ ] **Step 1: Add `sign_ack` helper.**

```rust
// crates/context/src/governance_broadcast.rs — append

use calimero_primitives::identity::PrivateKey;

pub fn sign_ack(signer_sk: &PrivateKey, op_hash: [u8; 32]) -> SignedAck {
    let signature_bytes = signer_sk.sign(&op_hash).to_bytes();
    SignedAck {
        op_hash,
        signer_pubkey: signer_sk.public_key(),
        signature: signature_bytes,
    }
}
```

- [ ] **Step 2: Emit Ack after `apply_signed_namespace_op` succeeds.**

In `crates/node/src/handlers/network_event/namespace.rs`, locate the existing `match context_client.apply_signed_namespace_op(op).await { Ok(outcome) => ...` arm. After the successful-apply branch (and after the existing `maybe_publish_key_delivery` call), add:

```rust
// After successful apply: emit Ack on the same topic.
let topic_id = topic.as_str().as_bytes();
let op_hash = hash_scoped_namespace(topic_id, &op);
let Some((_pk, sk_bytes, _)) =
    calimero_context::group_store::get_namespace_identity(&store, &ns_id).ok().flatten()
else {
    debug!("no namespace identity; cannot ack op {op_hash:?}");
    return;
};
let sender_sk = PrivateKey::from(sk_bytes);
let ack = sign_ack(&sender_sk, op_hash);
let payload = borsh::to_vec(&NamespaceTopicMsg::Ack(ack)).expect("borsh");
if let Err(e) = network_client.publish(topic.clone(), payload).await {
    warn!(?e, "failed to publish Ack");
    // Non-fatal: ack is fire-and-forget; sender will time out and retry.
}
```

- [ ] **Step 3: Wire the `Ack` variant arm in the same dispatch match.**

```rust
NamespaceTopicMsg::Ack(ack) => {
    if !context_client.ack_router().route(ack) {
        // No subscriber for this op_hash — either the op wasn't ours, or we already
        // completed. Either way, drop quietly.
    }
}
```

Add a thin `pub fn ack_router(&self) -> &AckRouter` accessor on `ContextClient` if it doesn't exist, returning `&self.inner.ack_router`.

- [ ] **Step 4: Build + run namespace handler tests.**

```bash
cargo build -p calimero-node 2>&1 | tail -10
cargo test -p calimero-node handlers::network_event::namespace 2>&1 | tail -15
```

Expected: clean build, existing tests still pass (ack emission is additive).

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(node/network_event): emit signed Ack after applying namespace op; route incoming Acks to AckRouter"
```

---

## Phase 5 — Wire `sign_and_publish_*` through the new contract; migrate first endpoints

**Goal:** Make the existing publish helpers use `publish_and_await_ack` end-to-end and migrate `add_group_members` + `remove_group_members` to surface the new typed outcome. Acceptance: `group-subgroup-queued-deltas.yml` passes without the `seconds: N` waits.

### Task 5.1 — Route `sign_apply_and_publish` through `publish_and_await_ack`

**Files:**
- Modify: `crates/context/src/group_store/namespace_governance.rs`
- Modify: `crates/context/src/group_store/governance_signer.rs`
- Modify: `crates/context/src/group_store/mod.rs`
- Modify: `crates/node/src/sync/config.rs` (add timeout knobs)

- [ ] **Step 1: Add timeout knobs.**

```rust
// crates/node/src/sync/config.rs — add to SyncConfig defaults

pub op_ack_cheap_timeout: Duration,        // default 2s — alias sets, metadata
pub op_ack_member_change_timeout: Duration, // default 5s — add/remove members, MemberJoined
pub op_ack_heavy_timeout: Duration,         // default 10s — context creation, app install
```

Mirror in `Default for SyncConfig` with `Duration::from_secs(2)`, `Duration::from_secs(5)`, `Duration::from_secs(10)` and matching `pub const DEFAULT_*_MS` constants for serde.

- [ ] **Step 2: Change return type of `sign_apply_and_publish_namespace_op` to `EyreResult<DeliveryReport>`.**

```rust
// crates/context/src/group_store/namespace_governance.rs

pub async fn sign_apply_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
    op_timeout: Duration,
) -> EyreResult<DeliveryReport> {
    let topic = ns_topic(namespace_id);
    // Phase 1 readiness gate FIRST — before any signing or local DAG mutation.
    //
    // Rationale: signing and `apply_signed_op` durably commit the op to our local
    // DAG. If we apply first and Phase 1 then rejects with `NamespaceNotReady`,
    // the caller is left with an op that exists locally but was never published.
    // Retrying calls this helper again, which signs a *different* op (new
    // op_hash) and applies it a second time — duplicate DAG entries / apply
    // errors / unbounded growth on every retry. Checking readiness before
    // sign+apply makes the rejection side-effect-free and cleanly retryable.
    //
    // known_subscribers: pulled from NodeManager state via NodeClient extension;
    // default to mesh_n_low when no subscriptions have been observed yet.
    let known = node_client.known_subscribers(topic.clone()).await.unwrap_or(0);
    governance_broadcast::assert_transport_ready(
        node_client.network_client(),
        topic.clone(),
        known,
        node_client.gossipsub_mesh_n_low(),
    )
    .await
    .map_err(|e| eyre::eyre!(e))?;

    // Readiness OK — now sign and apply locally (apply-before-publish keeps the
    // publisher's own state immediately consistent for the subsequent publish).
    let signed_op = NamespaceGovernance::new(store, namespace_id).sign(signer_sk, op)?;
    NamespaceGovernance::new(store, namespace_id).apply_signed_op(&signed_op)?;

    let report = governance_broadcast::publish_and_await_ack_namespace(
        store,
        node_client.network_client(),
        ack_router,
        namespace_id,
        topic,
        signed_op,
        op_timeout,
        1,    // min_acks default
        None, // required_signers default
    )
    .await
    .map_err(|e| eyre::eyre!(e))?;

    Ok(report)
}
```

(Engineer: there is a residual narrow window where `assert_transport_ready` passes but the subsequent `publish_and_await_ack_namespace` fails its `network_client.publish()` call — leaving the op locally applied but not on the wire. This is the standard TOCTOU between Phase 1 and Phase 2 transmit; gossipsub's mesh state can shift in microseconds. Recovery is handled by the existing heartbeat reconciliation arm and any subsequent re-publish on retry; document this in the spec as a known asymmetric window rather than trying to make Phase 1+Phase 2 atomic.)

(Engineer: `node_client.known_subscribers(topic).await` and `node_client.gossipsub_mesh_n_low()` are new accessors — add them to `NodeClient` proxying through the actor message system. Keep them tightly scoped to this caller path.)

Mirror the same change in `sign_and_publish_namespace_op` (the variant that does NOT apply locally — used in `join_group.rs` for replaying invitee-side namespace ops).

- [ ] **Step 3: Mirror in group-op publish helpers.**

```bash
rg -n "publish_group_op|sign_apply_and_publish.*group" crates/context/src --no-heading | head
```

Same return-type change on `GovernanceSigner::publish_group_op`, `publish_group_removal`, `GroupGovernancePublisher::sign_apply_and_publish`, `ContextManager::sign_and_publish_group_op`. Group ops route through a parallel `publish_and_await_ack_group` (add to `governance_broadcast.rs` mirroring the namespace function but using `GroupTopicMsg` + `hash_scoped_group` + group-topic + `group_member_pubkeys` for membership lookup).

- [ ] **Step 4: Build everything that depends on these helpers.**

```bash
cargo build -p calimero-context -p calimero-node -p calimero-context-client 2>&1 | tail -30
```

Expect: many compile errors at handler call sites due to the return-type change. **Do not fix them yet** — Task 5.2 sweeps them.

- [ ] **Step 5: Commit (compile-broken state, marked WIP).**

```bash
git add -u
git commit -m "feat(context/governance): route sign_*_publish through publish_and_await_ack [WIP]"
```

(WIP commit is intentional — Task 5.2 immediately unbreaks compile by sweeping callers.)

### Task 5.2 — Sweep callers to consume `DeliveryReport`

**Files:**
- Modify: every file in `crates/context/src/handlers/` that calls a `sign_*_publish_*` function

- [ ] **Step 1: Identify all caller files.**

```bash
rg -ln "sign_and_publish_group_op|sign_apply_and_publish_namespace_op|sign_and_publish_namespace_op|publish_group_op|publish_namespace_op" crates/context/src/handlers/ | sort -u
```

Expected list (from spec §12.2): `add_group_members.rs`, `remove_group_members.rs`, `create_group.rs`, `delete_group.rs`, `join_group.rs`, `set_group_alias.rs`, `set_default_capabilities.rs`, `set_default_visibility.rs`, `set_member_alias.rs`, `update_group_settings.rs`, `update_member_role.rs`, `upgrade_group.rs`, `admit_tee_node.rs`, `set_tee_admission_policy.rs`, `set_member_capabilities.rs`, `update_application/*.rs`, `delete_namespace.rs`.

- [ ] **Step 2: At each call site, bind the `DeliveryReport` and propagate or drop deliberately.**

For endpoints that have a return type users care about (e.g., `add_group_members`), thread the report through. For internal callers (e.g., `create_group` emitting follow-up ops), bind with `let _report = ...` and drop. Do not write TODO comments for "surface this later" — if it's not surfaced now, it's a follow-up issue, not a code comment.

Example `add_group_members.rs`:

```rust
let report = self.sign_and_publish_group_op(
    &group_id, requester, true, GroupOp::MemberAdded { member_pubkey, role },
).await?;
// existing post-publish flow continues; report.acked_by populates the response field
return Ok(AddMemberResponse {
    member_pubkey,
    propagated_to: report.acked_by.len(),
    elapsed_ms: report.elapsed_ms,
});
```

Add `propagated_to` and `elapsed_ms` to `AddMemberResponse` (and the JSON-RPC schema if surfaced).

- [ ] **Step 3: Build the world.**

```bash
cargo build --workspace 2>&1 | tail -30
```

Expected: clean.

- [ ] **Step 4: Run the full test suite.**

```bash
cargo test --workspace 2>&1 | tail -40
```

Expected: same baseline as Phase 1 head + new `governance_broadcast::tests` passes.

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(context/handlers): consume DeliveryReport at all governance publish call sites"
```

### Task 5.3 — Delete the e2e sleeps and verify

**Files:**
- Modify: `apps/e2e-kv-store/workflows/group-subgroup-queued-deltas.yml`
- Modify: `apps/e2e-kv-store/workflows/group-subgroup-cold-sync.yml`

- [ ] **Step 1: Delete the named `wait` steps.**

```bash
# group-subgroup-queued-deltas.yml — remove blocks at lines 103-104, 146-147, 343-344
```

The exact block to remove is:

```yaml
  - type: wait
    seconds: 3      # or 5, or 10 depending on the line
```

Apply the same deletion in `group-subgroup-cold-sync.yml` for lines 105-106 (`seconds: 5`).

- [ ] **Step 2: Run the workflows locally.**

```bash
# whichever runner the project uses; check apps/AGENTS.md
cd apps/e2e-kv-store && ./run-workflow.sh workflows/group-subgroup-queued-deltas.yml 2>&1 | tail -30
cd apps/e2e-kv-store && ./run-workflow.sh workflows/group-subgroup-cold-sync.yml 2>&1 | tail -30
```

Expected: both pass without the sleeps. If they fail intermittently, do NOT re-add the waits — file a follow-up issue and continue. The acceptance criterion is 10 consecutive passes on the slowest CI class, which is captured at PR review time, not locally.

- [ ] **Step 3: Commit.**

```bash
git add -u
git commit -m "test(e2e): remove governance-publish sleeps from queued-deltas and cold-sync workflows"
```

---

## Phase 6 — Readiness FSM, ReadinessCache, beacon types

**Goal:** Land `crates/node/src/readiness.rs` with the FSM, the cache, and the per-namespace evaluator. No emission yet (Phase 7 wires emission + probe handling).

### Task 6.1 — `ReadinessTier` + `ReadinessState` + transition function

**Files:**
- Create: `crates/node/src/readiness.rs`
- Create: `crates/node/src/readiness/tests.rs`
- Modify: `crates/node/src/lib.rs` (mount module)

- [ ] **Step 1: Create the module skeleton.**

```rust
// crates/node/src/readiness.rs

use std::time::{Duration, Instant};

use calimero_primitives::identity::PublicKey;

#[cfg(test)]
mod tests;

// Match spec §7.1: data-carrying variants so demotion reason and target
// applied_through propagate through the FSM and metrics/logs without a
// parallel side-channel struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessTier {
    Bootstrapping,
    LocallyReady,
    PeerValidatedReady,
    CatchingUp { target_applied_through: u64 },
    Degraded { reason: DemotionReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionReason {
    PendingOps(usize),
    NoRecentPeers,
    PeerSawHigherThroughput,
}

// `demotion_reason` is no longer carried separately — it lives inside
// `ReadinessTier::Degraded` and is recovered via pattern matching.
#[derive(Debug, Clone)]
pub struct ReadinessState {
    pub tier: ReadinessTier,
    pub local_applied_through: u64,
    pub local_head: [u8; 32],
    pub local_pending_ops: usize,
    pub subscribed_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct ReadinessConfig {
    pub boot_grace: Duration,
    pub ttl_heartbeat: Duration,
    pub beacon_interval: Duration,
    pub applied_through_grace: u64,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            boot_grace: Duration::from_secs(10),
            ttl_heartbeat: Duration::from_secs(60),
            beacon_interval: Duration::from_secs(5),
            applied_through_grace: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerSummary {
    pub max_applied_through: Option<u64>,
    pub heard_recent_beacon: bool,
}

pub fn evaluate_readiness(
    state: &ReadinessState,
    peers: &PeerSummary,
    cfg: &ReadinessConfig,
    now: Instant,
) -> ReadinessTier {
    // Pending ops always demote — record the count so observability can see
    // *how many* ops are blocking promotion, not just that *some* exist.
    if state.local_pending_ops > 0 {
        return ReadinessTier::Degraded {
            reason: DemotionReason::PendingOps(state.local_pending_ops),
        };
    }

    // Empty-DAG joiners never self-promote (no LocallyReady from local_applied_through=0).
    // If we hear a peer beacon we know there's a target to catch up to → CatchingUp
    // carrying that target; otherwise we don't know whether a network exists yet →
    // stay Bootstrapping. With the atomic `ReadinessCache::peer_summary` snapshot,
    // `heard_recent_beacon == true` implies `max_applied_through.is_some()`, so the
    // `unwrap_or(0)` is a defensive fallback only.
    if state.local_applied_through == 0 {
        return if peers.heard_recent_beacon {
            ReadinessTier::CatchingUp {
                target_applied_through: peers.max_applied_through.unwrap_or(0),
            }
        } else {
            ReadinessTier::Bootstrapping
        };
    }

    let boot_grace_elapsed = now.duration_since(state.subscribed_at) >= cfg.boot_grace;

    match (peers.max_applied_through, peers.heard_recent_beacon, boot_grace_elapsed) {
        // Heard a peer beacon: tip-fresh → PeerValidatedReady; behind → CatchingUp{target}.
        (Some(peer_at), true, _) => {
            if state.local_applied_through + cfg.applied_through_grace >= peer_at {
                ReadinessTier::PeerValidatedReady
            } else {
                ReadinessTier::CatchingUp { target_applied_through: peer_at }
            }
        }
        // No peer beacons but we've waited BOOT_GRACE: self-promote (LocallyReady).
        (None, false, true) => ReadinessTier::LocallyReady,
        // No peer beacons and still in boot grace: stay Bootstrapping.
        (None, false, false) => ReadinessTier::Bootstrapping,
        // Edge cases: peer existed but TTL expired since.
        (Some(_), false, true) => ReadinessTier::LocallyReady,
        (Some(_), false, false) => ReadinessTier::Bootstrapping,
        // Defensive: with an atomic `ReadinessCache::peer_summary` snapshot this arm is
        // unreachable — `heard_recent_beacon == true` ⇔ at least one fresh peer ⇒
        // `max_applied_through.is_some()`. Kept as a safe fallback in case a future
        // call site builds `PeerSummary` from non-atomic reads. Stay Bootstrapping
        // (no self-promotion) and let the next beacon re-evaluate.
        (None, true, _) => {
            debug_assert!(false, "PeerSummary built from non-atomic reads — use ReadinessCache::peer_summary");
            ReadinessTier::Bootstrapping
        }
    }
}
```

- [ ] **Step 2: Write transition tests covering all FSM paths in spec §7.2.**

```rust
// crates/node/src/readiness/tests.rs

use super::*;

fn base_state() -> ReadinessState {
    ReadinessState {
        tier: ReadinessTier::Bootstrapping,
        local_applied_through: 5,
        local_head: [0u8; 32],
        local_pending_ops: 0,
        subscribed_at: Instant::now(),
    }
}

#[test]
fn bootstrapping_to_peer_validated_when_caught_up_with_peer() {
    let state = base_state();
    let peers = PeerSummary { max_applied_through: Some(5), heard_recent_beacon: true };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::PeerValidatedReady);
}

#[test]
fn bootstrapping_to_catching_up_when_behind_peer() {
    let state = base_state();
    let peers = PeerSummary { max_applied_through: Some(10), heard_recent_beacon: true };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    // CatchingUp now carries the target — verify both the variant and the value so
    // a regression that loses the target wouldn't pass with a `matches!(_, _ { .. })` wildcard.
    assert_eq!(result, ReadinessTier::CatchingUp { target_applied_through: 10 });
}

#[test]
fn bootstrapping_to_locally_ready_after_boot_grace_with_no_peers() {
    let state = ReadinessState {
        subscribed_at: Instant::now() - Duration::from_secs(11),
        ..base_state()
    };
    let peers = PeerSummary { max_applied_through: None, heard_recent_beacon: false };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::LocallyReady);
}

#[test]
fn empty_dag_with_no_beacon_stays_bootstrapping() {
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(60),
        ..base_state()
    };
    let peers = PeerSummary { max_applied_through: None, heard_recent_beacon: false };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::Bootstrapping);
}

#[test]
fn empty_dag_with_peer_beacon_transitions_to_catching_up() {
    // Empty-DAG joiner that hears a peer beacon must move to CatchingUp so backfill
    // begins, and the variant must carry the peer's applied_through as the target.
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(60),
        ..base_state()
    };
    let peers = PeerSummary { max_applied_through: Some(7), heard_recent_beacon: true };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::CatchingUp { target_applied_through: 7 });
}

#[test]
fn empty_dag_never_promotes_to_locally_ready_after_boot_grace() {
    // Even after boot grace with no peers, an empty DAG must NOT self-promote.
    let state = ReadinessState {
        local_applied_through: 0,
        subscribed_at: Instant::now() - Duration::from_secs(3600),
        ..base_state()
    };
    let peers = PeerSummary { max_applied_through: None, heard_recent_beacon: false };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::Bootstrapping);
}

#[test]
fn pending_ops_always_demotes_to_degraded() {
    let state = ReadinessState { local_pending_ops: 3, ..base_state() };
    let peers = PeerSummary { max_applied_through: Some(5), heard_recent_beacon: true };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    // Degraded now carries the reason — verify the count flows through verbatim.
    assert_eq!(result, ReadinessTier::Degraded { reason: DemotionReason::PendingOps(3) });
}

#[test]
fn applied_through_grace_prevents_thrashing() {
    // Local at 8, peer at 9, grace=2 → still ready (8 + 2 >= 9).
    let state = ReadinessState { local_applied_through: 8, ..base_state() };
    let peers = PeerSummary { max_applied_through: Some(9), heard_recent_beacon: true };
    let result = evaluate_readiness(&state, &peers, &ReadinessConfig::default(), Instant::now());
    assert_eq!(result, ReadinessTier::PeerValidatedReady);
}
```

- [ ] **Step 3: Mount module.**

```rust
// crates/node/src/lib.rs — add near other pub mod declarations
pub mod readiness;
```

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-node readiness::tests 2>&1 | tail -15
```

Expected: 6 tests pass.

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(node/readiness): ReadinessTier FSM + evaluate_readiness with 6 transition tests"
```

### Task 6.2 — `ReadinessCache` with picker

**Files:**
- Modify: `crates/node/src/readiness.rs`
- Modify: `crates/node/src/readiness/tests.rs`

- [ ] **Step 1: Append cache.**

```rust
// crates/node/src/readiness.rs — append

use std::collections::HashMap;
use std::sync::Mutex;

use calimero_context_client::local_governance::SignedReadinessBeacon;

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub head: [u8; 32],
    pub applied_through: u64,
    /// Peer-signed millis-since-epoch from the beacon itself. Authoritative
    /// per-peer ordering signal — used by `insert` to drop stale beacons that
    /// gossipsub may re-deliver out-of-order on mesh churn / peer reconnect.
    pub ts_millis: u64,
    pub received_at: Instant,
    pub strong: bool,
}

#[derive(Default)]
pub struct ReadinessCache {
    entries: Mutex<HashMap<([u8; 32], PublicKey), CacheEntry>>,
}

impl ReadinessCache {
    /// Insert iff the incoming beacon is *newer* than any cached entry from the
    /// same peer (by `ts_millis`, with `applied_through` as tiebreaker on clock
    /// equality). Gossipsub does not guarantee delivery order — without this
    /// filter, an older re-delivered beacon could overwrite a fresher one,
    /// causing `pick_sync_partner` and `peer_summary` to regress and the FSM to
    /// spuriously demote `PeerValidatedReady → CatchingUp`.
    pub fn insert(&self, beacon: &SignedReadinessBeacon) {
        let mut g = self.entries.lock().expect("readiness cache lock");
        let key = (beacon.namespace_id, beacon.peer_pubkey);
        if let Some(existing) = g.get(&key) {
            // Drop the beacon if it's older or equal-clock-but-not-fresher.
            if beacon.ts_millis < existing.ts_millis
                || (beacon.ts_millis == existing.ts_millis
                    && beacon.applied_through <= existing.applied_through)
            {
                return;
            }
        }
        let _ = g.insert(
            key,
            CacheEntry {
                head: beacon.dag_head,
                applied_through: beacon.applied_through,
                ts_millis: beacon.ts_millis,
                received_at: Instant::now(),
                strong: beacon.strong,
            },
        );
    }

    pub fn fresh_peers(&self, ns: [u8; 32], ttl: Duration) -> Vec<(PublicKey, CacheEntry)> {
        let g = self.entries.lock().expect("readiness cache lock");
        let now = Instant::now();
        g.iter()
            .filter(|((nns, _), e)| *nns == ns && now.duration_since(e.received_at) <= ttl)
            .map(|((_, pk), e)| (*pk, e.clone()))
            .collect()
    }

    /// (strong desc, applied_through desc, received_at desc).
    pub fn pick_sync_partner(&self, ns: [u8; 32], ttl: Duration) -> Option<(PublicKey, CacheEntry)> {
        let mut peers = self.fresh_peers(ns, ttl);
        peers.sort_by(|a, b| {
            b.1.strong
                .cmp(&a.1.strong)
                .then(b.1.applied_through.cmp(&a.1.applied_through))
                .then(b.1.received_at.cmp(&a.1.received_at))
        });
        peers.into_iter().next()
    }

    pub fn max_applied_through(&self, ns: [u8; 32], ttl: Duration) -> Option<u64> {
        self.fresh_peers(ns, ttl)
            .into_iter()
            .map(|(_, e)| e.applied_through)
            .max()
    }

    /// Atomic snapshot — `max_applied_through` and `heard_recent_beacon` are read
    /// under a single lock acquisition so the FSM's match arms cannot observe a
    /// torn state (e.g. `heard_recent_beacon=true` while `max_applied_through=None`).
    /// All call sites that build a `PeerSummary` MUST use this rather than two
    /// separate calls to `max_applied_through` and `fresh_peers`.
    pub fn peer_summary(&self, ns: [u8; 32], ttl: Duration) -> PeerSummary {
        let g = self.entries.lock().expect("readiness cache lock");
        let now = Instant::now();
        let mut max_applied: Option<u64> = None;
        let mut any_fresh = false;
        for ((nns, _), e) in g.iter() {
            if *nns != ns || now.duration_since(e.received_at) > ttl {
                continue;
            }
            any_fresh = true;
            max_applied = Some(max_applied.map_or(e.applied_through, |m| m.max(e.applied_through)));
        }
        PeerSummary {
            max_applied_through: max_applied,
            heard_recent_beacon: any_fresh,
        }
    }
}
```

- [ ] **Step 2: Tests.**

```rust
// crates/node/src/readiness/tests.rs — append

use calimero_context_client::local_governance::SignedReadinessBeacon;
use calimero_primitives::identity::PrivateKey;

fn make_beacon(pk: PublicKey, applied_through: u64, strong: bool) -> SignedReadinessBeacon {
    SignedReadinessBeacon {
        namespace_id: [42u8; 32],
        peer_pubkey: pk,
        dag_head: [9u8; 32],
        applied_through,
        ts_millis: 0,
        strong,
        signature: [0u8; 64],
    }
}

#[test]
fn pick_sync_partner_prefers_strong_over_locally_ready() {
    let cache = ReadinessCache::default();
    let weak_pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let strong_pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(weak_pk, 100, false));
    cache.insert(&make_beacon(strong_pk, 50, true));
    let pick = cache.pick_sync_partner([42u8; 32], Duration::from_secs(60)).unwrap();
    assert_eq!(pick.0, strong_pk, "strong=true beats higher applied_through if strong=false");
}

#[test]
fn pick_sync_partner_among_strong_picks_highest_applied_through() {
    let cache = ReadinessCache::default();
    let pk_a = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let pk_b = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk_a, 5, true));
    cache.insert(&make_beacon(pk_b, 10, true));
    let pick = cache.pick_sync_partner([42u8; 32], Duration::from_secs(60)).unwrap();
    assert_eq!(pick.0, pk_b);
}

#[test]
fn pick_sync_partner_excludes_stale_entries() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 5, true));
    // Wait beyond TTL by setting a very small TTL on the query.
    std::thread::sleep(Duration::from_millis(10));
    let pick = cache.pick_sync_partner([42u8; 32], Duration::from_millis(5));
    assert!(pick.is_none());
}

#[test]
fn pick_sync_partner_empty_cache_returns_none() {
    let cache = ReadinessCache::default();
    assert!(cache.pick_sync_partner([42u8; 32], Duration::from_secs(60)).is_none());
}

#[test]
fn insert_drops_stale_beacon_from_same_peer() {
    // Regression: gossipsub out-of-order delivery must not stale-overwrite a
    // fresher entry. The fresher beacon's `applied_through` and `ts_millis`
    // should remain after the older beacon arrives second.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut fresh = make_beacon(pk, 100, true);
    fresh.ts_millis = 2000;
    let mut stale = make_beacon(pk, 50, true);
    stale.ts_millis = 1000;
    cache.insert(&fresh);
    cache.insert(&stale); // arrives second but is older — must be dropped
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(
        s.max_applied_through,
        Some(100),
        "stale beacon must not overwrite fresher entry from same peer",
    );
}

#[test]
fn insert_accepts_newer_beacon_from_same_peer() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut older = make_beacon(pk, 50, true);
    older.ts_millis = 1000;
    let mut newer = make_beacon(pk, 100, true);
    newer.ts_millis = 2000;
    cache.insert(&older);
    cache.insert(&newer); // arrives second and IS newer — must replace
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(100));
}

#[test]
fn insert_uses_applied_through_to_break_ts_millis_ties() {
    // Same wall-clock millis (rare but possible across reboots / clock skew):
    // the higher applied_through wins.
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let mut a = make_beacon(pk, 10, true);
    a.ts_millis = 1000;
    let mut b = make_beacon(pk, 20, true);
    b.ts_millis = 1000;
    cache.insert(&a);
    cache.insert(&b);
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert_eq!(s.max_applied_through, Some(20));
}

#[test]
fn peer_summary_atomic_when_fresh_peer_present() {
    // Snapshot must always have heard_recent_beacon == true ⇒ max_applied_through.is_some().
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 7, true));
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert!(s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, Some(7));
}

#[test]
fn peer_summary_no_fresh_peers_returns_none_and_false() {
    let cache = ReadinessCache::default();
    let s = cache.peer_summary([42u8; 32], Duration::from_secs(60));
    assert!(!s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, None);
}

#[test]
fn peer_summary_excludes_stale_and_returns_none_after_ttl() {
    let cache = ReadinessCache::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 9, false));
    std::thread::sleep(Duration::from_millis(10));
    let s = cache.peer_summary([42u8; 32], Duration::from_millis(5));
    assert!(!s.heard_recent_beacon);
    assert_eq!(s.max_applied_through, None);
}
```

- [ ] **Step 3: Run.**

```bash
cargo test -p calimero-node readiness::tests 2>&1 | tail -15
```

Expected: 13 tests pass (6 from 6.1 + 4 partner-picker tests + 3 new peer_summary atomicity tests).

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(node/readiness): ReadinessCache with strong-first sync-partner picker"
```

---

## Phase 7 — Beacon emission, probe handling, ReadinessManager actor

**Goal:** Emit beacons on edge-trigger / freshness tick / probe response. Wire receiver-side `ReadinessBeacon` and `ReadinessProbe` arms.

### Task 7.1 — `ReadinessManager` actor

**Files:**
- Modify: `crates/node/src/readiness.rs`
- Modify: `crates/node/src/manager.rs` (or wherever the actix actor tree is wired)

- [ ] **Step 1: Add an actor-shaped manager.**

```rust
// crates/node/src/readiness.rs — append

use actix::{Actor, AsyncContext, Context, Handler, Message};

pub struct ReadinessManager {
    pub cache: std::sync::Arc<ReadinessCache>,
    pub config: ReadinessConfig,
    pub state_per_namespace: HashMap<[u8; 32], ReadinessState>,
    pub network_client: calimero_network_primitives::client::NetworkClient,
    // Identity provider, store handle — wire on construction.
    pub store: calimero_store::Store,
    /// Per-(peer, namespace) timestamp of the last out-of-cycle beacon emitted in
    /// response to a `ReadinessProbe`. Used to rate-limit probe responses at
    /// `BEACON_INTERVAL / 2` and close the unsigned-probe amplification path —
    /// see `Handler<EmitOutOfCycleBeacon>` below for rationale.
    pub last_probe_response_at: HashMap<(libp2p::PeerId, [u8; 32]), Instant>,
}

impl Actor for ReadinessManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Periodic freshness-tick beacon emission.
        ctx.run_interval(self.config.beacon_interval, |this, _ctx| {
            this.emit_periodic_beacons();
        });
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct ApplyBeaconLocal { pub namespace_id: [u8; 32] }

#[derive(Message)]
#[rtype(result = "()")]
pub struct LocalStateChanged {
    pub namespace_id: [u8; 32],
    pub local_applied_through: u64,
    pub local_head: [u8; 32],
    pub local_pending_ops: usize,
}

impl ReadinessManager {
    fn emit_periodic_beacons(&mut self) {
        for (ns_id, state) in self.state_per_namespace.iter() {
            if matches!(state.tier, ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady) {
                self.publish_beacon(*ns_id, state);
            }
        }
    }

    fn publish_beacon(&self, ns_id: [u8; 32], state: &ReadinessState) {
        // Sign + publish ReadinessBeacon. See Task 7.2 for sign body.
        // ... (filled in Task 7.2)
    }
}

impl Handler<LocalStateChanged> for ReadinessManager {
    type Result = ();
    fn handle(&mut self, msg: LocalStateChanged, _ctx: &mut Self::Context) {
        let entry = self.state_per_namespace.entry(msg.namespace_id).or_insert_with(|| {
            ReadinessState {
                tier: ReadinessTier::Bootstrapping,
                local_applied_through: 0,
                local_head: [0u8; 32],
                local_pending_ops: 0,
                subscribed_at: Instant::now(),
            }
        });
        entry.local_applied_through = msg.local_applied_through;
        entry.local_head = msg.local_head;
        entry.local_pending_ops = msg.local_pending_ops;
        // Atomic single-lock snapshot — see ReadinessCache::peer_summary for why.
        let peers = self.cache.peer_summary(msg.namespace_id, self.config.ttl_heartbeat);
        let new_tier = evaluate_readiness(entry, &peers, &self.config, Instant::now());
        if new_tier != entry.tier {
            entry.tier = new_tier;
            // Edge trigger: emit a beacon immediately on transition into a Ready tier.
            if matches!(new_tier, ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady) {
                self.publish_beacon(msg.namespace_id, entry);
            }
        }
    }
}
```

- [ ] **Step 2: Wire `ReadinessManager` into `NodeManager` startup.**

In `crates/node/src/manager/startup.rs` (alongside `setup_hash_heartbeat_interval`), add:

```rust
pub(super) fn setup_readiness_manager(&self, _ctx: &mut actix::Context<Self>) {
    let manager = ReadinessManager {
        cache: self.clients.readiness_cache.clone(),
        config: self.config.readiness.clone(),
        state_per_namespace: HashMap::new(),
        network_client: self.clients.network.clone(),
        store: self.store.clone(),
        last_probe_response_at: HashMap::new(),
    };
    let addr = manager.start();
    self.readiness_addr = Some(addr);
}
```

Add `readiness_addr: Option<actix::Addr<ReadinessManager>>` to `NodeManager` and call `setup_readiness_manager(ctx)` from `Actor::started`.

- [ ] **Step 3: Build.**

```bash
cargo build -p calimero-node 2>&1 | tail -15
```

Expected: clean.

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(node/readiness): ReadinessManager actor with FSM evaluation on LocalStateChanged"
```

### Task 7.2 — Beacon signing + emission

**Files:**
- Modify: `crates/node/src/readiness.rs`

- [ ] **Step 1: Implement `publish_beacon`.**

```rust
// crates/node/src/readiness.rs — replace the empty publish_beacon

fn publish_beacon(&self, ns_id: [u8; 32], state: &ReadinessState) {
    use calimero_context_client::local_governance::{NamespaceTopicMsg, SignedReadinessBeacon};

    let Some((my_pk, my_sk_bytes, _)) = calimero_context::group_store::get_namespace_identity(
        &self.store,
        &calimero_context_config::types::ContextGroupId::from(ns_id),
    ).ok().flatten() else {
        return;  // No identity for this namespace yet; skip.
    };

    let strong = matches!(state.tier, ReadinessTier::PeerValidatedReady);
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);

    let mut payload = Vec::with_capacity(32 + 32 + 32 + 8 + 8 + 1);
    payload.extend_from_slice(&ns_id);
    payload.extend_from_slice(&my_pk.to_bytes());
    payload.extend_from_slice(&state.local_head);
    payload.extend_from_slice(&state.local_applied_through.to_le_bytes());
    payload.extend_from_slice(&ts_millis.to_le_bytes());
    payload.push(if strong { 1 } else { 0 });
    let signing_key = calimero_primitives::identity::PrivateKey::from(my_sk_bytes);
    let signature = signing_key.sign(&payload).to_bytes();

    let beacon = SignedReadinessBeacon {
        namespace_id: ns_id,
        peer_pubkey: my_pk,
        dag_head: state.local_head,
        applied_through: state.local_applied_through,
        ts_millis,
        strong,
        signature,
    };
    let topic = ns_topic(ns_id);
    let msg = NamespaceTopicMsg::ReadinessBeacon(beacon);
    let bytes = match borsh::to_vec(&msg) {
        Ok(b) => b,
        Err(e) => { tracing::warn!(?e, "failed to encode ReadinessBeacon"); return; }
    };
    let net = self.network_client.clone();
    actix::spawn(async move {
        if let Err(e) = net.publish(topic, bytes).await {
            tracing::debug!(?e, "ReadinessBeacon publish failed (non-fatal)");
        }
    });
}
```

- [ ] **Step 2: Update beacon-receive verification to match the sign payload.**

This must round-trip with verification. Add `verify_readiness_beacon` next to `verify_ack`:

```rust
// crates/context/src/governance_broadcast.rs — append
use calimero_context_client::local_governance::SignedReadinessBeacon;

pub fn verify_readiness_beacon(store: &Store, beacon: &SignedReadinessBeacon) -> bool {
    let mut payload = Vec::with_capacity(32 + 32 + 32 + 8 + 8 + 1);
    payload.extend_from_slice(&beacon.namespace_id);
    payload.extend_from_slice(&beacon.peer_pubkey.to_bytes());
    payload.extend_from_slice(&beacon.dag_head);
    payload.extend_from_slice(&beacon.applied_through.to_le_bytes());
    payload.extend_from_slice(&beacon.ts_millis.to_le_bytes());
    payload.push(if beacon.strong { 1 } else { 0 });
    let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&beacon.peer_pubkey.to_bytes()) else { return false; };
    let Ok(sig) = ed25519_dalek::Signature::from_slice(&beacon.signature) else { return false; };
    if vk.verify(&payload, &sig).is_err() { return false; }
    crate::group_store::namespace_member_pubkeys(store, beacon.namespace_id)
        .map(|m| m.contains(&beacon.peer_pubkey))
        .unwrap_or(false)
}
```

- [ ] **Step 3: Round-trip test.**

```rust
// crates/context/src/governance_broadcast/tests.rs — append (keep this with other broadcast tests)

#[tokio::test]
async fn readiness_beacon_signature_round_trip() {
    // Create namespace identity in an in-memory store, sign + verify a beacon.
    // Assert: verify_readiness_beacon returns true.
    // (Engineer: full body — set up an InMemoryDB store, write a namespace member entry
    // with the test pubkey, call publish_beacon's sign logic, then verify.)
    // No todo! — fill in the body.
}
```

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-context governance_broadcast::tests::readiness 2>&1 | tail -10
cargo build -p calimero-node 2>&1 | tail -5
```

Expected: all pass; build clean.

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(node/readiness): sign + publish ReadinessBeacon on edge trigger and freshness tick"
```

### Task 7.3 — Receiver-side beacon + probe arms

**Files:**
- Create: `crates/node/src/handlers/network_event/readiness.rs`
- Modify: `crates/node/src/handlers/network_event/namespace.rs`
- Modify: `crates/node/src/handlers/network_event.rs`

- [ ] **Step 1: New submodule for the two handlers.**

```rust
// crates/node/src/handlers/network_event/readiness.rs

use calimero_context_client::local_governance::{
    NamespaceTopicMsg, ReadinessProbe, SignedReadinessBeacon,
};
use libp2p::PeerId;
use tracing::debug;

use crate::NodeManager;

pub(super) fn handle_readiness_beacon(
    manager: &mut NodeManager,
    _ctx: &mut actix::Context<NodeManager>,
    _peer_id: PeerId,
    beacon: SignedReadinessBeacon,
) {
    if !calimero_context::governance_broadcast::verify_readiness_beacon(&manager.store, &beacon) {
        debug!("ReadinessBeacon failed verification; dropping");
        return;
    }
    manager.clients.readiness_cache.insert(&beacon);
    // Notify FSM that a peer beacon may have changed peer_summary for this namespace.
    if let Some(addr) = &manager.readiness_addr {
        let _ = addr.do_send(crate::readiness::ApplyBeaconLocal {
            namespace_id: beacon.namespace_id,
        });
    }
}

pub(super) fn handle_readiness_probe(
    manager: &mut NodeManager,
    _ctx: &mut actix::Context<NodeManager>,
    peer_id: PeerId,
    probe: ReadinessProbe,
) {
    // Out-of-cycle beacon: only if we're *Ready for this namespace.
    // The (peer_id, namespace_id) tuple is forwarded so the FSM actor can
    // rate-limit per-(peer, namespace) — see EmitOutOfCycleBeacon handler.
    if let Some(addr) = &manager.readiness_addr {
        let _ = addr.do_send(crate::readiness::EmitOutOfCycleBeacon {
            namespace_id: probe.namespace_id,
            requesting_peer: peer_id,
        });
    }
}
```

Add the matching message and handler to `readiness.rs`:

```rust
// crates/node/src/readiness.rs — append

#[derive(Message)]
#[rtype(result = "()")]
pub struct EmitOutOfCycleBeacon {
    pub namespace_id: [u8; 32],
    pub requesting_peer: PeerId,
}

impl Handler<EmitOutOfCycleBeacon> for ReadinessManager {
    type Result = ();
    fn handle(&mut self, msg: EmitOutOfCycleBeacon, _ctx: &mut Self::Context) {
        // Rate-limit probe responses per (peer, namespace) at BEACON_INTERVAL / 2 to
        // close the unsigned-`ReadinessProbe` traffic-amplification path: one ~48-byte
        // probe would otherwise trigger one ~200-byte signed beacon from EVERY *Ready
        // peer on the topic (≈Nx amplification). Bypass via varying `nonce` is blocked
        // because the limit keys on (peer, namespace), not on the probe content.
        let now = Instant::now();
        let min_spacing = self.config.beacon_interval / 2;
        let key = (msg.requesting_peer, msg.namespace_id);
        if let Some(last) = self.last_probe_response_at.get(&key) {
            if now.duration_since(*last) < min_spacing {
                return; // within rate-limit window — drop silently
            }
        }
        if let Some(state) = self.state_per_namespace.get(&msg.namespace_id) {
            if matches!(state.tier, ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady) {
                self.publish_beacon(msg.namespace_id, state);
                self.last_probe_response_at.insert(key, now);
            }
        }
    }
}

impl Handler<ApplyBeaconLocal> for ReadinessManager {
    type Result = ();
    fn handle(&mut self, msg: ApplyBeaconLocal, _ctx: &mut Self::Context) {
        // Re-evaluate FSM with possibly updated peer summary.
        let Some(state) = self.state_per_namespace.get(&msg.namespace_id).cloned() else { return; };
        // Atomic single-lock snapshot — see ReadinessCache::peer_summary.
        let peers = self.cache.peer_summary(msg.namespace_id, self.config.ttl_heartbeat);
        let new_tier = evaluate_readiness(&state, &peers, &self.config, Instant::now());
        if new_tier != state.tier {
            if let Some(s) = self.state_per_namespace.get_mut(&msg.namespace_id) {
                s.tier = new_tier;
                if matches!(new_tier, ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady) {
                    self.publish_beacon(msg.namespace_id, s);
                }
            }
        }
    }
}
```

- [ ] **Step 2: Wire into `network_event/namespace.rs` dispatch.**

```rust
// crates/node/src/handlers/network_event/namespace.rs — replace the placeholder
// drop branch from Phase 2 with real dispatch

match msg {
    NamespaceTopicMsg::Op(op) => { /* existing path */ }
    NamespaceTopicMsg::Ack(ack) => {
        if !this.clients.context.ack_router().route(ack) { /* drop */ }
    }
    NamespaceTopicMsg::ReadinessBeacon(b) => {
        super::readiness::handle_readiness_beacon(this, ctx, peer_id, b);
    }
    NamespaceTopicMsg::ReadinessProbe(p) => {
        super::readiness::handle_readiness_probe(this, ctx, peer_id, p);
    }
}
```

Mount the new submodule in `crates/node/src/handlers/network_event.rs`:

```rust
mod readiness;
```

- [ ] **Step 3: Build + run all node tests.**

```bash
cargo build -p calimero-node 2>&1 | tail -10
cargo test -p calimero-node 2>&1 | tail -15
```

Expected: clean.

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(node/network_event): wire ReadinessBeacon + ReadinessProbe handlers"
```

---

## Phase 8 — J6 join flow: `join_namespace` + `await_namespace_ready` + retry helper

**Goal:** Land the J6 split. After this phase, `join_namespace` returns fast on first beacon; `await_namespace_ready` blocks through backfill + MemberJoined ack.

### Task 8.1 — `await_first_fresh_beacon` future

**Files:**
- Modify: `crates/node/src/readiness.rs`

- [ ] **Step 1: Add the await helper to `ReadinessCache`.**

```rust
// crates/node/src/readiness.rs — append

use tokio::sync::Notify;

#[derive(Default)]
pub struct ReadinessCacheNotify {
    pub waiters: Mutex<HashMap<[u8; 32], std::sync::Arc<Notify>>>,
}

impl ReadinessCacheNotify {
    pub fn waiter_for(&self, ns: [u8; 32]) -> std::sync::Arc<Notify> {
        let mut g = self.waiters.lock().expect("notify lock");
        g.entry(ns).or_insert_with(|| std::sync::Arc::new(Notify::new())).clone()
    }
    pub fn notify(&self, ns: [u8; 32]) {
        let g = self.waiters.lock().expect("notify lock");
        if let Some(n) = g.get(&ns) { n.notify_waiters(); }
    }
}

impl ReadinessCache {
    pub async fn await_first_fresh_beacon(
        &self,
        notify: &ReadinessCacheNotify,
        ns: [u8; 32],
        ttl: Duration,
        deadline: Duration,
    ) -> Option<(PublicKey, CacheEntry)> {
        // Fast-path: already cached.
        if let Some(entry) = self.pick_sync_partner(ns, ttl) {
            return Some(entry);
        }
        let waiter = notify.waiter_for(ns);
        let timeout_fut = tokio::time::sleep(deadline);
        tokio::pin!(timeout_fut);
        loop {
            tokio::select! {
                _ = waiter.notified() => {
                    if let Some(entry) = self.pick_sync_partner(ns, ttl) {
                        return Some(entry);
                    }
                }
                _ = &mut timeout_fut => return None,
            }
        }
    }
}
```

Wire `notify.notify(ns)` into `ReadinessCache::insert` (or call it from `handle_readiness_beacon` after `cache.insert`). The notify struct lives on `NodeManager.clients.readiness_notify`.

- [ ] **Step 2: Tests.**

```rust
// crates/node/src/readiness/tests.rs — append

#[tokio::test]
async fn await_first_fresh_beacon_resolves_immediately_when_cached() {
    let cache = ReadinessCache::default();
    let notify = ReadinessCacheNotify::default();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    cache.insert(&make_beacon(pk, 5, true));
    let got = cache.await_first_fresh_beacon(&notify, [42u8; 32], Duration::from_secs(60), Duration::from_secs(5)).await;
    assert!(got.is_some());
}

#[tokio::test]
async fn await_first_fresh_beacon_resolves_on_late_arrival() {
    let cache = std::sync::Arc::new(ReadinessCache::default());
    let notify = std::sync::Arc::new(ReadinessCacheNotify::default());
    let cache_w = cache.clone();
    let notify_w = notify.clone();
    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();
    let _ = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cache_w.insert(&make_beacon(pk, 7, true));
        notify_w.notify([42u8; 32]);
    });
    let got = cache.await_first_fresh_beacon(&notify, [42u8; 32], Duration::from_secs(60), Duration::from_secs(2)).await;
    assert!(got.is_some());
}

#[tokio::test]
async fn await_first_fresh_beacon_times_out() {
    let cache = ReadinessCache::default();
    let notify = ReadinessCacheNotify::default();
    let got = cache.await_first_fresh_beacon(&notify, [42u8; 32], Duration::from_secs(60), Duration::from_millis(50)).await;
    assert!(got.is_none());
}
```

- [ ] **Step 3: Run.**

```bash
cargo test -p calimero-node readiness::tests::await 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(node/readiness): await_first_fresh_beacon with cache + Notify wakers"
```

### Task 8.2 — `join_namespace` (J6 steps 1-4)

**Files:**
- Modify: `crates/client/src/client/namespace.rs`
- Modify: `crates/context-primitives/src/errors.rs` (add `JoinError`)

- [ ] **Step 1: Add `JoinError` and `JoinStarted`.**

```rust
// crates/context-primitives/src/errors.rs — extend

#[derive(Debug, thiserror::Error)]
pub enum JoinError {
    #[error("no ready peers responded within {waited_ms}ms")]
    NoReadyPeers { waited_ms: u64 },
    #[error("namespace not ready: {0}")]
    NamespaceNotReady(String),
    #[error("invitation invalid: {0}")]
    InvalidInvitation(String),
    #[error("transport: {0}")]
    Transport(String),
}

#[derive(Debug, Clone)]
pub struct JoinStarted {
    pub namespace_id: [u8; 32],
    pub sync_partner: PublicKey,
    pub partner_head: [u8; 32],
    pub partner_applied: u64,
    pub elapsed_ms: u64,
}
```

- [ ] **Step 2: Update `client::namespace::join_namespace`.**

```rust
// crates/client/src/client/namespace.rs

pub async fn join_namespace(
    &self,
    invitation: SignedGroupOpenInvitation,
    deadline: Duration,
) -> Result<JoinStarted, JoinError> {
    // Anchor the clock at function entry so `JoinStarted.elapsed_ms` and
    // `JoinError::NoReadyPeers.waited_ms` capture the FULL join latency
    // (mark_membership_pending + subscribe + publish probe + beacon wait),
    // not just the step-4 beacon wait. Mirrors spec §8.1.
    let start = Instant::now();
    let ns_id = invitation.namespace_id();

    // step 1: mark pending-membership locally (existing behavior)
    self.context_client
        .mark_membership_pending(ns_id, invitation.clone())
        .await
        .map_err(|e| JoinError::InvalidInvitation(e.to_string()))?;

    // step 2: subscribe to namespace topic
    let topic = ns_topic(ns_id);
    self.network_client
        .subscribe(topic.clone())
        .await
        .map_err(|e| JoinError::Transport(e.to_string()))?;

    // step 3: active probe
    let probe = ReadinessProbe { namespace_id: ns_id, nonce: rand::random() };
    let payload = borsh::to_vec(&NamespaceTopicMsg::ReadinessProbe(probe))
        .map_err(|e| JoinError::Transport(e.to_string()))?;
    self.network_client
        .publish(topic.clone(), payload)
        .await
        .map_err(|e| JoinError::Transport(e.to_string()))?;

    // step 4: collect first fresh beacon (start anchored above for full-latency timing)
    let beacon = self.readiness_cache
        .await_first_fresh_beacon(
            &self.readiness_notify,
            ns_id,
            self.config.ttl_heartbeat,
            deadline,
        )
        .await
        .ok_or_else(|| JoinError::NoReadyPeers { waited_ms: start.elapsed().as_millis() as u64 })?;

    Ok(JoinStarted {
        namespace_id: ns_id,
        sync_partner: beacon.0,
        partner_head: beacon.1.head,
        partner_applied: beacon.1.applied_through,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}
```

- [ ] **Step 3: Smoke test.**

```rust
// crates/client/src/tests.rs — extend the existing `async fn join_namespace()` test

#[tokio::test]
async fn join_namespace_returns_no_ready_peers_when_alone() {
    let client = build_test_client_solo().await;
    let invitation = mk_test_invitation();
    let err = client.join_namespace(invitation, Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, JoinError::NoReadyPeers { .. }));
}
```

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-client join_namespace 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(client/namespace): J6 join_namespace returns Ok(JoinStarted) on first ReadinessBeacon"
```

### Task 8.3 — `await_namespace_ready` (J6 steps 5-8)

**Files:**
- Modify: `crates/client/src/client/namespace.rs`

- [ ] **Step 1: Implement.**

```rust
pub async fn await_namespace_ready(
    &self,
    ns_id: [u8; 32],
    deadline: Duration,
) -> Result<ReadyReport, ReadyError> {
    let start = Instant::now();

    // step 5: pick partner
    let (partner_pk, partner_entry) = self.readiness_cache
        .pick_sync_partner(ns_id, self.config.ttl_heartbeat)
        .ok_or(ReadyError::NoReadyPeers)?;

    // step 6: backfill against partner
    self.run_namespace_backfill(partner_pk, ns_id, deadline.saturating_sub(start.elapsed())).await
        .map_err(|e| ReadyError::Backfill(e.to_string()))?;

    // step 7: publish MemberJoined op through three-phase contract
    let invitation = self.context_client.load_pending_invitation(ns_id).await
        .map_err(|e| ReadyError::Local(e.to_string()))?;
    let op = NamespaceOp::Root(RootOp::MemberJoined {
        member: self.identity.public_key(),
        signed_invitation: invitation,
    });
    let report = self.context_client
        .sign_and_publish_namespace_op(
            ns_id,
            &self.identity_sk,
            op,
            self.config.member_change_timeout,
        )
        .await
        .map_err(|e| ReadyError::PublishMemberJoined(e.to_string()))?;

    // step 8: implicit — FSM transitions us to PeerValidatedReady when the local
    // state changes propagate back through ReadinessManager::LocalStateChanged.

    Ok(ReadyReport {
        namespace_id: ns_id,
        final_head: self.context_client.namespace_head(ns_id).await.unwrap_or([0u8; 32]),
        applied_through: self.context_client.namespace_applied_through(ns_id).await.unwrap_or(0),
        members_learned: self.context_client.namespace_member_count(ns_id).await.unwrap_or(0),
        acked_by: report.acked_by,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

pub async fn join_and_wait_ready(
    &self,
    invitation: SignedGroupOpenInvitation,
    deadline: Duration,
) -> Result<ReadyReport, ReadyError> {
    // join_deadline lives in [1s, deadline]: the floor prevents `deadline / 3`
    // from rounding to a near-zero value on small deadlines, the cap stops the
    // floor from EXCEEDING the caller's total budget (which would let
    // `join_namespace` run past the caller's deadline and zero out the ready
    // phase). `min(max(deadline / 3, 1s), deadline)` enforces both bounds.
    let join_deadline = std::cmp::min(
        std::cmp::max(deadline / 3, Duration::from_secs(1)),
        deadline,
    );
    let ready_deadline = deadline.saturating_sub(join_deadline);
    debug_assert!(
        deadline >= Duration::from_secs(2),
        "join_and_wait_ready called with deadline < 2s; ready_deadline saturates to ~zero, \
         await_namespace_ready will fail immediately. Use join_namespace + await_namespace_ready \
         directly if you genuinely need a sub-2s budget."
    );
    let started = self.join_namespace(invitation, join_deadline).await
        .map_err(|e| ReadyError::JoinFailed(e.to_string()))?;
    self.await_namespace_ready(started.namespace_id, ready_deadline).await
}
```

(Engineer: `run_namespace_backfill`, `load_pending_invitation`, `namespace_head`, `namespace_applied_through`, `namespace_member_count` are accessor methods to add — wire each to existing primitives in the context client.)

- [ ] **Step 2: Smoke test using the existing 2-node fixture.**

The existing `async fn join_namespace()` test in `crates/client/src/tests.rs:511` shows how to spin up two `ContextClient` actors connected by an in-memory bus. Reuse it:

```rust
// crates/client/src/tests.rs — append after line ~600

#[tokio::test]
async fn join_and_wait_ready_succeeds_against_warm_namespace() {
    let (node_a, node_b) = make_two_node_fixture().await;     // existing helper, see line ~480
    let ns_id = node_a.create_namespace().await.expect("create ns");
    node_a.publish_one_governance_op(ns_id).await.expect("warm op");
    // Wait for node_a's beacon to propagate to node_b's ReadinessCache.
    wait_for_beacon(&node_b, ns_id, Duration::from_secs(2)).await;

    let invitation = node_a.invite(ns_id, node_b.identity().public_key()).await.expect("invite");
    let report = node_b
        .join_and_wait_ready(invitation, Duration::from_secs(5))
        .await
        .expect("join_and_wait_ready");

    assert_eq!(report.namespace_id, ns_id);
    assert!(report.acked_by.contains(&node_a.identity().public_key()));
    assert!(report.elapsed_ms < 2000, "warm join should converge in <2s");
}
```

`make_two_node_fixture`, `publish_one_governance_op`, and `wait_for_beacon` are small helpers — the first already exists, the latter two are 5-line additions on `ContextClient` test extensions. Define them at the top of the test module if missing.

- [ ] **Step 3: Run.**

```bash
cargo test -p calimero-client join_and_wait_ready 2>&1 | tail -10
```

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(client/namespace): await_namespace_ready + join_and_wait_ready aggregate"
```

### Task 8.4 — Retry wrapper

**Files:**
- Create: `crates/client/src/client/namespace_retry.rs`
- Modify: `crates/client/src/client/namespace.rs` (mount + re-export)

- [ ] **Step 1: Implement.**

```rust
// crates/client/src/client/namespace_retry.rs

use std::time::{Duration, Instant};

use rand::Rng;

use super::namespace::JoinStarted;
use crate::client::Client;

const ATTEMPT_DEADLINE: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub async fn join_namespace_with_retry(
    client: &Client,
    invitation: SignedGroupOpenInvitation,
    max_total: Duration,
) -> Result<JoinStarted, JoinError> {
    let mut delay = Duration::from_secs(3);
    let start = Instant::now();
    loop {
        // Each attempt must respect the *remaining* total budget — otherwise a
        // caller passing `max_total < ATTEMPT_DEADLINE` (e.g. 2s) would block on
        // a single 10s attempt and only "respect" `max_total` between attempts,
        // overshooting the caller's stated budget by up to ATTEMPT_DEADLINE.
        let remaining = max_total.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(JoinError::NoReadyPeers { waited_ms: start.elapsed().as_millis() as u64 });
        }
        let attempt_deadline = std::cmp::min(ATTEMPT_DEADLINE, remaining);
        match client.join_namespace(invitation.clone(), attempt_deadline).await {
            Ok(started) => return Ok(started),
            Err(JoinError::NoReadyPeers { .. }) => {
                if start.elapsed() + delay > max_total {
                    return Err(JoinError::NoReadyPeers { waited_ms: start.elapsed().as_millis() as u64 });
                }
                let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..(delay.as_millis() as u64 / 4)));
                tokio::time::sleep(delay + jitter).await;
                delay = std::cmp::min(delay * 2, MAX_BACKOFF);
            }
            Err(other) => return Err(other),
        }
    }
}
```

- [ ] **Step 2: Tests for backoff and final timeout.**

```rust
// crates/client/src/client/namespace_retry.rs — append #[cfg(test)] mod

#[tokio::test]
async fn join_namespace_with_retry_eventually_succeeds_after_peer_arrives() {
    let (node_a_holder, node_b) = make_two_node_fixture_a_offline().await;
    // Node A is created but not yet started. Spawn a task to start it after 4s.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        node_a_holder.start_and_warm().await;
    });
    let invitation = mk_dummy_invitation_for(&node_b);
    let result = join_namespace_with_retry(&node_b, invitation, Duration::from_secs(30)).await;
    assert!(result.is_ok(), "should succeed once A comes online");
}

#[tokio::test]
async fn join_namespace_with_retry_returns_no_ready_peers_after_budget() {
    let node_b = make_solo_node_fixture().await;   // no peer to find
    let invitation = mk_dummy_invitation_for(&node_b);
    let start = Instant::now();
    let err = join_namespace_with_retry(&node_b, invitation, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(matches!(err, JoinError::NoReadyPeers { .. }));
    assert!(start.elapsed() >= Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(4), "should respect total budget");
}
```

`make_two_node_fixture_a_offline` and `make_solo_node_fixture` are minor variants of the existing `make_two_node_fixture`; either factor a single builder with optional flags or duplicate (5 lines each).

- [ ] **Step 3: Run.**

```bash
cargo test -p calimero-client namespace_retry 2>&1 | tail -10
```

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(client/namespace): join_namespace_with_retry — exponential backoff with jitter"
```

---

## Phase 9 — KeyDelivery + `join_group` integration

**Goal:** KeyDelivery routes through the three-phase contract with `required_signers: Some([recipient])`. `join_group` waits on first valid group key. Acceptance: `fuzzy-handlers-node-4` 100% → 0% failure.

### Task 9.1 — KeyDelivery via `publish_and_await_ack` with `required_signers`

**Files:**
- Modify: `crates/node/src/key_delivery.rs`

- [ ] **Step 1: Replace the fire-and-forget call.**

```rust
// crates/node/src/key_delivery.rs

let report = match calimero_context::group_store::sign_and_publish_namespace_op_with(
    &store,
    node_client,
    namespace_id,
    &sender_sk,
    delivery_op,
    config.member_change_timeout,
    1,                                  // min_acks
    Some(vec![member]),                 // required_signers — only the joiner counts
)
.await {
    Ok(r) => r,
    Err(e) => {
        warn!(?e, ?member, "KeyDelivery publish_and_await_ack failed; will be retried on next MemberJoined event");
        return;
    }
};
info!(group_id = %hex::encode(group_id.to_bytes()), %member, acked = ?report.acked_by.len(), "KeyDelivery acked");
```

(Engineer: `sign_and_publish_namespace_op_with` is a parameterized variant of `sign_and_publish_namespace_op` that accepts `min_acks` and `required_signers`. Add it to `namespace_governance.rs` as a wrapper around `publish_and_await_ack_namespace`. The non-parameterized `sign_and_publish_namespace_op` becomes a thin shim with `min_acks=1, required_signers=None`.)

- [ ] **Step 2: Receiver-side ack-only-after-store.**

In `crates/node/src/handlers/network_event/namespace.rs`, the existing `KeyDelivery` arm decrypts the envelope and stores the group key. The Ack emission added in Task 4.1 happens *after* the apply path returns Ok — confirm this for the KeyDelivery branch specifically:

```bash
rg -n "KeyDelivery|RootOp::KeyDelivery" crates/node/src/handlers/network_event/namespace.rs crates/context/src/handlers/apply_signed_namespace_op.rs --no-heading
```

If decrypt or store fails, `apply_signed_namespace_op` returns `Err`, and the Phase 4 ack-emission code path is skipped — sender will `NoAckReceived`. Verify by reading the existing branches and adding a unit test that an ECDH-decrypt failure path does NOT emit an ack.

- [ ] **Step 3: Test ack-after-store.**

```rust
// crates/node/src/key_delivery_test.rs (new sibling test file)

#[tokio::test]
async fn key_delivery_failed_decrypt_emits_no_ack() {
    let (node_a, node_b) = make_two_node_fixture().await;
    let (ns_id, group_id) = warm_namespace_with_group(&node_a, &node_b).await;

    // Forge a KeyDelivery to node_b but encrypt with the wrong recipient pubkey
    // (use a freshly-generated pk, not node_b's). node_b cannot decrypt.
    let forged_op = make_keydelivery_op_for(ns_id, group_id, &mk_random_pk());
    let acks_before = collect_recent_acks(&node_a).await;
    deliver_namespace_op(&node_a, &node_b, forged_op).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let acks_after = collect_recent_acks(&node_a).await;

    assert_eq!(acks_before.len(), acks_after.len(),
        "node_b must not ack a KeyDelivery it could not decrypt");
}

#[tokio::test]
async fn key_delivery_acked_after_store_completes() {
    let (node_a, node_b) = make_two_node_fixture().await;
    let (ns_id, group_id) = warm_namespace_with_group(&node_a, &node_b).await;

    // node_a delivers a real key to node_b.
    let report = node_a.send_key_delivery(ns_id, group_id, node_b.identity().public_key())
        .await
        .expect("send_key_delivery");

    // 1) node_b stored the key.
    assert!(node_b.has_group_key(group_id).await);
    // 2) report.acked_by contains node_b's pubkey.
    assert!(report.acked_by.contains(&node_b.identity().public_key()));
}
```

`warm_namespace_with_group`, `make_keydelivery_op_for`, `collect_recent_acks`, `deliver_namespace_op`, `send_key_delivery`, `has_group_key` are small test helpers built on the existing two-node fixture — each is 5-15 lines.

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-node key_delivery 2>&1 | tail -15
```

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(node/key_delivery): route through publish_and_await_ack with required_signers=[recipient]"
```

### Task 9.2 — `join_group` waits for first valid group key

**Files:**
- Modify: `crates/context/src/handlers/join_group.rs`
- Modify: `crates/context/src/group_store/mod.rs` (or wherever the group-store mutators live)
- Modify: `crates/context-primitives/src/errors.rs` — add `JoinGroupError::NoKeyReceived`

- [ ] **Step 1: Add a watch-channel fired on key store.**

In the group_store writer for `load_current_group_key`'s setter (look for the `store_group_key` or equivalent):

```rust
// crates/context/src/group_store/keys.rs (or wherever)

use tokio::sync::watch;

pub struct GroupKeyWatchers {
    senders: Mutex<HashMap<[u8; 32] /* group_id */, watch::Sender<bool>>>,
}

impl GroupKeyWatchers {
    pub fn watcher(&self, group_id: [u8; 32]) -> watch::Receiver<bool> {
        let mut g = self.senders.lock().unwrap();
        g.entry(group_id).or_insert_with(|| watch::channel(false).0).subscribe()
    }
    pub fn notify(&self, group_id: [u8; 32]) {
        let g = self.senders.lock().unwrap();
        if let Some(tx) = g.get(&group_id) {
            let _ = tx.send(true);
        }
    }
}
```

Hook `notify(group_id)` into the existing path that writes the first group key for the group (after the rocksdb put succeeds).

- [ ] **Step 2: `join_group` awaits the watcher.**

```rust
// crates/context/src/handlers/join_group.rs

let watcher = self.clients.context.group_key_watchers().watcher(group_id);

// existing local-state mutations...

// Wait until either the watcher fires OR group_key_wait elapses.
match tokio::time::timeout(self.config.group_key_wait, async {
    let mut w = watcher;
    loop {
        if *w.borrow() { return Ok::<(), JoinGroupError>(()); }
        w.changed().await.map_err(|_| JoinGroupError::WatcherClosed)?;
    }
}).await {
    Ok(Ok(())) => {}
    Ok(Err(e)) => return Err(e),
    Err(_) => return Err(JoinGroupError::NoKeyReceived),
}
```

Already-have-key fast path: `if load_current_group_key(group_id)?.is_some() { return Ok(...); }` before subscribing.

- [ ] **Step 3: Test.**

```rust
// crates/context/src/handlers/join_group_test.rs (new sibling test file)

#[tokio::test]
async fn join_group_waits_until_group_key_arrives() {
    let (node_a, node_b) = make_two_node_fixture().await;
    let (ns_id, group_id) = warm_namespace_with_group(&node_a, &node_b).await;

    // Force-clear node_b's group key so join_group has to wait for delivery.
    node_b.clear_group_key(group_id).await;

    let node_a_clone = node_a.clone();
    let _delivery = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        node_a_clone.send_key_delivery(ns_id, group_id, node_b.identity().public_key())
            .await.expect("delivery");
    });

    let invitation = node_a.invite_to_group(group_id, node_b.identity().public_key()).await.unwrap();
    let result = node_b.join_group(invitation, Duration::from_secs(1)).await;
    assert!(result.is_ok());
    assert!(node_b.has_group_key(group_id).await);
}

#[tokio::test]
async fn join_group_times_out_with_no_key_received() {
    let (node_a, node_b) = make_two_node_fixture().await;
    let group_id = node_a.create_group_no_key_delivery().await;  // helper that suppresses key delivery
    let invitation = node_a.invite_to_group(group_id, node_b.identity().public_key()).await.unwrap();

    let result = node_b.join_group(invitation, Duration::from_millis(300)).await;
    assert!(matches!(result, Err(JoinGroupError::NoKeyReceived)));
}
```

`clear_group_key` is a test-only mutation; gate behind `#[cfg(test)]`. `create_group_no_key_delivery` is a helper that wires up a group but skips the existing `maybe_publish_key_delivery` hook (e.g., by toggling a test-only flag on the publisher).

- [ ] **Step 4: Run.**

```bash
cargo test -p calimero-context join_group 2>&1 | tail -15
```

- [ ] **Step 5: Commit.**

```bash
git add -u
git commit -m "feat(context/join_group): block on first valid group key; new JoinGroupError::NoKeyReceived"
```

### Task 9.3 — Remove the silent `InternalError` branch at `execute/mod.rs:180`

**Files:**
- Modify: `crates/context/src/handlers/execute/mod.rs`

- [ ] **Step 1: Replace the silent return with a loud assertion.**

```rust
// crates/context/src/handlers/execute/mod.rs:~180

_ => {
    if let Some(sk) = identity.sender_key { (sk, [0u8; 32]) }
    else {
        // Under the post-#2237 join_group contract, this branch is unreachable:
        // join_group does not return Ok until at least one valid group key is stored.
        error!(
            %context_id, ?identity.public_key,
            "execute reached load_current_group_key=None && sender_key=None — \
             join_group contract violation"
        );
        return ActorResponse::reply(Err(ExecuteError::InternalError));
    }
}
```

(Keep the `return Err` — defense in depth. The `error!` log line is the new bit.)

- [ ] **Step 2: Add the matching `error!` log to `execute/mod.rs:189` (the other silent `InternalError`) for consistency. Same pattern.**

- [ ] **Step 3: Build.**

```bash
cargo build -p calimero-context 2>&1 | tail -5
```

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "fix(context/execute): log before returning InternalError at sender_key=None branches"
```

---

## Phase 10 — Migrate remaining endpoints

**Goal:** Apply the new return-type contract to every governance endpoint not yet migrated. Each is a small mechanical change.

### Task 10.1 — Sweep create / delete / set / update endpoints

**Files:** All files in `crates/context/src/handlers/` not touched in Phase 5.

- [ ] **Step 1: Identify remaining un-swept files.**

```bash
rg -L "DeliveryReport" crates/context/src/handlers/ | grep -v ".rs.bk" | head -30
```

(Using `-L` to find files NOT containing the type — those are the un-swept ones.)

For each: look at the publish call(s), bind the `DeliveryReport`, plumb through to the response type if the endpoint surfaces a result, otherwise `let _report = ...`.

- [ ] **Step 2: Build clean.**

```bash
cargo build --workspace 2>&1 | tail -10
cargo test --workspace 2>&1 | tail -20
```

- [ ] **Step 3: Commit.**

```bash
git add -u
git commit -m "feat(context/handlers): sweep remaining endpoints to consume DeliveryReport"
```

---

## Phase 11 — Cleanup: delete obsolete divergence paths and workflow retries

**Goal:** Now that delivery is acked, the workaround scaffolding is dead weight. Delete carefully.

### Task 11.1 — Collapse `parent_pull` to single-peer retry

**Files:**
- Modify: `crates/node/src/sync/parent_pull.rs`
- Modify: `crates/node/src/sync/manager/mod.rs`
- Modify: `crates/node/src/handlers/network_event/namespace.rs`

- [ ] **Step 1: Reduce `ParentPullBudget` from a multi-peer scheduler to a single-peer helper.**

Find the existing struct and the multi-peer iteration:

```bash
sed -n '1,100p' crates/node/src/sync/parent_pull.rs
```

Replace the `next(&mut self, mesh_peers: &[PeerId]) -> NextPeer` method to return a single peer (the *delivering* peer if known, otherwise picker by mesh) and remove the cross-peer iteration. Keep the deadline check.

- [ ] **Step 2: Update the call site in `network_event/namespace.rs:50-51`** to use the single-peer helper. The existing function will be ~30 lines instead of ~200.

- [ ] **Step 3: Run sync-related tests.**

```bash
cargo test -p calimero-node sync:: 2>&1 | tail -20
```

Expected: existing tests still green; behaviour for non-cold-start cases is unchanged.

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "refactor(node/sync): collapse parent_pull multi-peer enumeration to single-peer retry"
```

### Task 11.2 — Collapse `handle_namespace_state_heartbeat` divergence arm

**Files:**
- Modify: `crates/node/src/handlers/network_event/namespace.rs`

- [ ] **Step 1: Locate and remove the divergence-reconciliation branch.**

```bash
rg -n "NamespaceStateHeartbeat" crates/node/src/handlers/network_event/ --no-heading
```

Inside `handle_namespace_state_heartbeat`, the branch that compares heads and triggers `NamespaceBackfill` becomes a no-op (or is deleted). Keep only the liveness-touch path (record peer last-seen). The ~180 lines of divergence-handling code disappear.

- [ ] **Step 2: Test.**

```bash
cargo test -p calimero-node 2>&1 | tail -15
```

Expected: green. Run the e2e suite locally if your dev environment supports it.

- [ ] **Step 3: Commit.**

```bash
git add -u
git commit -m "refactor(node/heartbeat): heartbeat is liveness-only; ReadinessBeacon owns divergence detection"
```

### Task 11.3 — Drop workflow `max_attempts` retries

**Files:**
- Modify: `.github/workflows/e2e-rust-apps.yml`

- [ ] **Step 1: Find and remove `max_attempts: 2` and the matching `merobox nuke --force` block.**

```bash
grep -n "max_attempts\|merobox nuke" .github/workflows/e2e-rust-apps.yml
```

Delete both. The job spec becomes a single-attempt run.

- [ ] **Step 2: Verify the workflow file is still valid YAML.**

```bash
yamllint .github/workflows/e2e-rust-apps.yml 2>&1 | head -10
```

- [ ] **Step 3: Commit.**

```bash
git add -u
git commit -m "ci(e2e): drop max_attempts=2 retry — three-phase contract removes the flake driver"
```

---

## Phase 12 — Observability + new e2e

**Goal:** Land the metrics from spec §15 and the new `kv-store-joiner-cold.yml` e2e workflow.

### Task 12.1 — Metrics

**Files:**
- Modify: `crates/node/src/sync/prometheus_metrics.rs`
- Modify: `crates/context/src/governance_broadcast.rs` (emit `governance_publish_outcome` + `governance_publish_ack_latency_ms`)
- Modify: `crates/node/src/readiness.rs` (emit `readiness_transitions` + `readiness_boot_to_ready_ms` + `readiness_locally_ready_fallback`)
- Modify: `crates/client/src/client/namespace.rs` (emit `join_namespace_outcome`)

- [ ] **Step 1: Register all metrics from spec §15.**

```rust
// crates/node/src/sync/prometheus_metrics.rs — register

pub static GOVERNANCE_PUBLISH_OUTCOME: Lazy<IntCounterVec> = Lazy::new(|| { /* ... */ });
pub static GOVERNANCE_PUBLISH_ACK_LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| { /* ... */ });
pub static READINESS_TRANSITIONS: Lazy<IntCounterVec> = Lazy::new(|| { /* ... */ });
pub static READINESS_BOOT_TO_READY_MS: Lazy<HistogramVec> = Lazy::new(|| { /* ... */ });
pub static READINESS_LOCALLY_READY_FALLBACK: Lazy<IntCounterVec> = Lazy::new(|| { /* ... */ });
pub static JOIN_NAMESPACE_OUTCOME: Lazy<IntCounterVec> = Lazy::new(|| { /* ... */ });
pub static ACK_ROUTER_PENDING_OPS: Lazy<IntGauge> = Lazy::new(|| { /* ... */ });
```

(Engineer: full bodies follow the histogram in Task 1.1; copy that structure with the appropriate label sets per spec §15.)

- [ ] **Step 2: Wire the emission sites.**

For each metric, add `.with_label_values(...).inc()` / `.observe(...)` at the matching code site. Spec §15 names the site for each.

For `readiness_transitions` and `readiness_locally_ready_fallback`, **do not** pass the raw 32-byte `namespace_id` as a label — that explodes Prometheus cardinality and is exploitable as a DoS vector by an attacker who can create many namespaces. Use the bucketing helper from spec §15 to emit a stable 256-bucket label:

```rust
// crates/node/src/sync/prometheus_metrics.rs — add helper near the metric registrations

use sha2::{Digest, Sha256};

/// Maps a 32-byte namespace_id to one of 256 stable buckets — a 1-byte SHA-256 prefix
/// formatted as 2 hex chars. Use this for any per-namespace Prometheus label;
/// emit the full namespace_id only via structured logging (`tracing::info!`).
pub fn ns_metric_bucket(ns: [u8; 32]) -> String {
    let h = Sha256::digest(ns);
    format!("{:02x}", h[0])
}
```

Emission sites:

```rust
// crates/node/src/readiness.rs — on FSM transition
READINESS_TRANSITIONS
    .with_label_values(&[
        &ns_metric_bucket(namespace_id),
        tier_label(old_tier),
        tier_label(new_tier),
    ])
    .inc();
tracing::info!(
    namespace_id = %hex::encode(namespace_id),
    from = tier_label(old_tier),
    to = tier_label(new_tier),
    "readiness tier transition"
);

// crates/node/src/readiness.rs — on LocallyReady fallback firing
READINESS_LOCALLY_READY_FALLBACK
    .with_label_values(&[&ns_metric_bucket(namespace_id)])
    .inc();
```

- [ ] **Step 3: Build clean.**

```bash
cargo build -p calimero-node -p calimero-context -p calimero-client 2>&1 | tail -10
```

- [ ] **Step 4: Commit.**

```bash
git add -u
git commit -m "feat(metrics): governance/readiness/join metrics per #2237 §15"
```

### Task 12.2 — New e2e: `kv-store-joiner-cold.yml`

**Files:**
- Create: `apps/e2e-kv-store/workflows/kv-store-joiner-cold.yml`

- [ ] **Step 1: Author the workflow.**

```yaml
# apps/e2e-kv-store/workflows/kv-store-joiner-cold.yml

name: "KV-store cold joiner — verifies #2237 acceptance criterion 2"
description: "2 warm members at steady state; new joiner does 20 set ops; all 3 nodes converge in ≤2s, no sleeps."

setup:
  - type: start_nodes
    count: 2
  - type: install_app
    app: kv-store
  - type: create_context
    on: node-1

steps:
  - type: invite_join_namespace
    inviter: node-1
    invitee: node-2
    use_join_and_wait_ready: true
    deadline_ms: 5000

  - type: start_node
    name: node-3

  - type: invite_join_namespace
    inviter: node-1
    invitee: node-3
    use_join_and_wait_ready: true
    deadline_ms: 5000

  - type: loop
    count: 20
    on: node-3
    action:
      type: kv_set
      key: "key_${index}"
      value: "value_${index}"

  - type: wait_for_sync
    expected_root_hash_matches_on: [node-1, node-2, node-3]
    deadline_ms: 2000

assert:
  - type: all_nodes_have_n_keys
    n: 20
```

(Engineer: adjust to the workflow runner's actual schema. The principle: no `wait, seconds: N` steps; use `wait_for_sync` with a deadline; assertion is full convergence.)

- [ ] **Step 2: Run locally.**

```bash
cd apps/e2e-kv-store && ./run-workflow.sh workflows/kv-store-joiner-cold.yml 2>&1 | tail -30
```

Expected: pass within deadline.

- [ ] **Step 3: Commit.**

```bash
git add -u
git commit -m "test(e2e): add kv-store-joiner-cold.yml — #2237 acceptance criterion 2"
```

---

## Final verification

- [ ] **Run the full workspace test suite.**

```bash
cargo test --workspace 2>&1 | tee /tmp/test-output.txt | tail -40
grep -E "FAILED|test result" /tmp/test-output.txt
```

Expected: all green, no failures.

- [ ] **Run lints + format check.**

```bash
cargo clippy -- -A warnings 2>&1 | tail -20
cargo fmt --check 2>&1 | tail -5
```

Expected: no new warnings, no fmt diff.

- [ ] **Run e2e workflows that this PR claims to fix.**

```bash
cd apps/e2e-kv-store
./run-workflow.sh workflows/group-subgroup-queued-deltas.yml 2>&1 | tail -10
./run-workflow.sh workflows/group-subgroup-cold-sync.yml 2>&1 | tail -10
./run-workflow.sh workflows/kv-store-joiner-cold.yml 2>&1 | tail -10
```

Expected: all pass without retries.

- [ ] **Final commit if any drift.**

```bash
git status
# If anything unstaged: git add -u && git commit -m "chore: final cleanup before PR"
```

---

## Acceptance — map plan tasks to spec criteria

| Spec §16 criterion | Tasks proving it |
|---|---|
| 1. Sleep removal in queued-deltas + cold-sync | Task 5.3 |
| 2. join + 20 ops + read ≤ 2s on 2-node | Task 12.2 (`kv-store-joiner-cold.yml`) |
| 3. `list_group_members` first-call returns full state | Falls out of Task 8.3 (`await_namespace_ready` invariant) |
| 4. `fuzzy-handlers-node-4` 100% → 0% | Task 9.1 + 9.2 (KeyDelivery + `join_group` contract) |
| 5. `fuzzy-kv-store` no 30s stalls | Task 8.3 (member-list propagation invariant) |
| 6. `governance_publish_outcome{outcome="no_ack"}` < 0.1% in prod | Tasks 5.1 + 5.2 + 12.1 (mechanism + telemetry); operator validation post-merge |
| 7. `readiness_locally_ready_fallback` zero in steady state | Tasks 6.1 + 7.1 (FSM + fallback gating); validated via metric |

All seven criteria are covered.
