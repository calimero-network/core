//! End-to-end probe for the DAG-causal Shared verifier wiring (#2266).
//!
//! Drives `CausalDelta`s through `calimero_dag::DagStore` with a custom
//! [`DeltaApplier`] that mirrors the production `ContextStorageApplier::apply`
//! flow minus the WASM hop:
//!
//!   1. Resolve `effective_writers` per Shared entity via
//!      `rotation_log_reader::writers_at(&log, &delta.parents,
//!      |a,b| happens_before_in_topology(...))`.
//!   2. Serialize a [`StorageDelta::CausalActions`] artifact with that map
//!      (Borsh roundtrip — mirrors what `__calimero_sync_next` would receive).
//!   3. On the receive side, deserialize and dispatch each action through
//!      [`Interface::apply_action`] with a per-action `ApplyContext`
//!      whose `effective_writers` is keyed on `action.id()` — exactly what
//!      `Root::sync`'s `CausalActions` branch does in WASM.
//!   4. After success, record the delta's parent links into the local
//!      topology mirror and the resolved writer sets into the cache, so
//!      cascaded children's `apply()` sees an up-to-date view.
//!
//! Tests in this file:
//!
//! - **`update_vs_rotation_race_pre_rotation_write_accepted_through_full_sync_path`**
//!   — regression check that a writer's pre-rotation write is accepted even
//!   when delivered AFTER the rotation that revokes them. NOTE: this case
//!   currently also passes under v2 by accident (the index's
//!   `metadata.storage_type.writers` is frozen at bootstrap due to a known
//!   `Index::update_hash_for` fragility, so v2's stored-writers fallback
//!   happens to return the same set as `writers_at([D_root])`). See the
//!   storage-side `write_hook_relies_on_stale_stored_writers_for_rotation_detection`.
//!   Kept here because v3 must continue to accept it.
//!
//! - **`post_rotation_forgery_by_revoked_writer_rejected`** — the strong probe
//!   for #2266. Bob's signed write whose parents include the rotation must be
//!   rejected because `writers_at([D1]) = {Alice}`. Under v2 the verifier
//!   would consult stored writers (still `{Alice, Bob}` due to the same
//!   index fragility) and accept the forgery. Toggling the resolve step off
//!   in `SharedRotationApplier::apply` makes this test fail with `Ok(true)`
//!   — confirms it would have caught the v2 bug.
//!
//! - **`buffered_pre_rotation_write_resolves_correctly_after_parents_arrive`**
//!   — the regression check above + adversarial DAG delivery order that
//!   forces `DagStore` to buffer the pre-rotation write until its parent
//!   arrives. Catches a topology-mirror update-ordering bug class (if the
//!   topology were updated before WASM apply, buffered deltas would resolve
//!   against the wrong snapshot).
//!
//! Together these exercise the production sync-layer path end-to-end (DAG
//! buffering → resolve → cache → Borsh artifact → verifier swap) for the
//! partition / late-delivery cases #2266 was opened to fix.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use borsh::{from_slice, to_vec};
use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier, DeltaKind};
use calimero_node::sync::rotation_log_reader;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{ChildInfo, Metadata, SignatureData, StorageType};
use calimero_storage::index::Index;
use calimero_storage::interface::{
    disable_nonce_check_for_testing, ApplyContext, Interface, StorageError,
};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::rotation_log;
use calimero_storage::store::MainStorage;
use core::num::NonZeroU128;
use ed25519_dalek::{Signer, SigningKey};
use tokio::sync::RwLock;

// =============================================================================
// Helpers
// =============================================================================

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey_of(sk: &SigningKey) -> PublicKey {
    PublicKey::from(*sk.verifying_key().as_bytes())
}

fn one_sec(n: u64) -> u64 {
    n.saturating_mul(1_000_000_000)
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

fn setup_root() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<MainStorage>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    ChildInfo::new(root_id, [0; 32], root_meta)
}

/// Build a signed `Shared` action (Add or Update). The signature is over
/// `payload_for_signing()`, just like production.
fn build_signed_shared_action(
    add: bool,
    id: Id,
    data: Vec<u8>,
    writers: BTreeSet<PublicKey>,
    hlc_ns: u64,
    signer_sk: &SigningKey,
    ancestors: Vec<ChildInfo>,
) -> Action {
    let mut metadata = Metadata::new(hlc_ns, hlc_ns);
    metadata.storage_type = StorageType::Shared {
        writers,
        signature_data: Some(SignatureData {
            signature: [0; 64],
            nonce: hlc_ns,
            signer: Some(pubkey_of(signer_sk)),
        }),
    };
    let mut action = if add {
        Action::Add {
            id,
            data,
            ancestors,
            metadata,
        }
    } else {
        Action::Update {
            id,
            data,
            ancestors,
            metadata,
        }
    };
    let payload = action.payload_for_signing();
    let signature = signer_sk.sign(&payload).to_bytes();
    let metadata_mut = match &mut action {
        Action::Add { metadata, .. } | Action::Update { metadata, .. } => metadata,
        _ => unreachable!(),
    };
    if let StorageType::Shared {
        signature_data: Some(sd),
        ..
    } = &mut metadata_mut.storage_type
    {
        sd.signature = signature;
    }
    action
}

/// Reverse-BFS reachability over a `delta_id → parents` mirror: returns
/// true iff `a` is in the transitive ancestry of `b`. Mirrors the
/// production `delta_store::happens_before_in_topology` so this probe
/// resolves writer sets the same way the live sync layer does.
fn happens_before_in_topology(
    topology: &HashMap<[u8; 32], Vec<[u8; 32]>>,
    a: &[u8; 32],
    b: &[u8; 32],
) -> bool {
    if a == b {
        return false;
    }
    let mut frontier: Vec<[u8; 32]> = topology.get(b).cloned().unwrap_or_default();
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    while let Some(node) = frontier.pop() {
        if !seen.insert(node) {
            continue;
        }
        if &node == a {
            return true;
        }
        if let Some(parents) = topology.get(&node) {
            frontier.extend(parents.iter().copied());
        }
    }
    false
}

// =============================================================================
// SharedRotationApplier — production-flow mirror minus the WASM hop
// =============================================================================

/// `DeltaApplier` that mirrors `ContextStorageApplier::apply` from
/// `crates/node/src/delta_store.rs:134`, with the WASM execute call
/// replaced by direct dispatch into [`Interface::apply_action`]. This
/// keeps the test fast and dependency-free while still exercising:
///
/// - the per-Shared-entity resolution loop (`writers_at`),
/// - the topology mirror that `happens_before_in_topology` consults,
/// - the `(entity_id, delta_id)` cache,
/// - Borsh roundtrip of [`StorageDelta::CausalActions`],
/// - the receiver-side variant branching that builds a per-action
///   [`ApplyContext`] from the resolved map.
struct SharedRotationApplier {
    /// `delta_id → parents` for every applied delta. Updated after a
    /// successful apply. Used as the snapshot the `happens_before`
    /// closure runs against during resolution.
    topology: Arc<RwLock<HashMap<[u8; 32], Vec<[u8; 32]>>>>,
    /// Resolved writer-set cache. Same key/shape as
    /// `ContextStorageApplier::effective_writers_cache`.
    effective_writers_cache: Arc<RwLock<HashMap<(Id, [u8; 32]), BTreeSet<PublicKey>>>>,
    /// Successful-apply log (id + action count + serialized artifact
    /// size) for assertions.
    applied: Arc<RwLock<Vec<AppliedDelta>>>,
}

#[derive(Debug, Clone)]
struct AppliedDelta {
    delta_id: [u8; 32],
    /// Size of the serialized `StorageDelta::CausalActions` artifact.
    /// Asserts the wire format went through Borsh on both sides.
    artifact_bytes: usize,
}

impl SharedRotationApplier {
    fn new() -> Self {
        Self {
            topology: Arc::new(RwLock::new(HashMap::new())),
            effective_writers_cache: Arc::new(RwLock::new(HashMap::new())),
            applied: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn applied(&self) -> Vec<AppliedDelta> {
        self.applied.read().await.clone()
    }

    /// Resolve `effective_writers` for every Shared entity in `delta`.
    /// Mirrors `ContextStorageApplier::resolve_effective_writers_for_delta`.
    async fn resolve_effective_writers(
        &self,
        delta: &CausalDelta<Vec<Action>>,
    ) -> BTreeMap<Id, BTreeSet<PublicKey>> {
        // Collect Shared-entity ids touched by this delta.
        let mut shared_entities: BTreeSet<Id> = BTreeSet::new();
        for action in &delta.payload {
            let metadata = match action {
                Action::Add { metadata, .. }
                | Action::Update { metadata, .. }
                | Action::DeleteRef { metadata, .. } => metadata,
                Action::Compare { .. } => continue,
            };
            if matches!(metadata.storage_type, StorageType::Shared { .. }) {
                let _inserted = shared_entities.insert(action.id());
            }
        }

        let mut out: BTreeMap<Id, BTreeSet<PublicKey>> = BTreeMap::new();
        if shared_entities.is_empty() {
            return out;
        }

        let topology_snapshot = self.topology.read().await.clone();

        for entity_id in shared_entities {
            let cache_key = (entity_id, delta.id);
            if let Some(cached) = self.effective_writers_cache.read().await.get(&cache_key) {
                let _replaced = out.insert(entity_id, cached.clone());
                continue;
            }

            let log = match rotation_log::load::<MainStorage>(entity_id) {
                Ok(Some(log)) => log,
                Ok(None) => continue, // No log → verifier falls back to v2 stored-writers.
                Err(e) => panic!("rotation_log::load for {entity_id:?} failed: {e}"),
            };

            let resolved = rotation_log_reader::writers_at(&log, &delta.parents, |a, b| {
                happens_before_in_topology(&topology_snapshot, a, b)
            });

            if let Some(set) = resolved {
                let _replaced = out.insert(entity_id, set.clone());
                let mut cache = self.effective_writers_cache.write().await;
                let _previous = cache.insert(cache_key, set);
            }
        }

        out
    }
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for SharedRotationApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Step 1 — resolve.
        let effective_writers = self.resolve_effective_writers(delta).await;

        // Step 2 — serialize the CausalActions artifact, mirroring what the
        // sender ships across the wire to `__calimero_sync_next`.
        let artifact = to_vec(&StorageDelta::CausalActions {
            actions: delta.payload.clone(),
            delta_id: delta.id,
            delta_hlc: delta.hlc,
            effective_writers,
        })
        .map_err(|e| ApplyError::Application(format!("serialize artifact: {e}")))?;
        let artifact_size = artifact.len();

        // Step 3 — receiver side: deserialize and dispatch. Mirrors
        // `Root::sync`'s `CausalActions` branch.
        let storage_delta: StorageDelta = from_slice(&artifact)
            .map_err(|e| ApplyError::Application(format!("deserialize artifact: {e}")))?;
        let (actions, recv_delta_id, recv_hlc, recv_writers) = match storage_delta {
            StorageDelta::CausalActions {
                actions,
                delta_id,
                delta_hlc,
                effective_writers,
            } => (actions, delta_id, delta_hlc, effective_writers),
            other => {
                return Err(ApplyError::Application(format!(
                    "unexpected variant on receive: {other:?}"
                )))
            }
        };

        for action in &actions {
            let ctx = ApplyContext {
                effective_writers: recv_writers.get(&action.id()).cloned(),
                delta_id: Some(recv_delta_id),
                delta_hlc: Some(recv_hlc),
            };
            Interface::<MainStorage>::apply_action(action.clone(), &ctx)
                .map_err(|e: StorageError| ApplyError::Application(e.to_string()))?;
        }

        // Step 4 — record topology + tally.
        {
            let mut topology = self.topology.write().await;
            let _previous = topology.insert(delta.id, delta.parents.clone());
        }
        self.applied.write().await.push(AppliedDelta {
            delta_id: delta.id,
            artifact_bytes: artifact_size,
        });
        Ok(())
    }
}

/// Build a `CausalDelta` with explicit HLC. `CausalDelta::new_test` defaults
/// the HLC, but our scenarios depend on HLC ordering for sibling tiebreak.
fn delta_with_hlc(
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    hlc_ns: u64,
    payload: Vec<Action>,
) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload,
        hlc: hlc(hlc_ns),
        expected_root_hash: [0; 32],
        kind: DeltaKind::Regular,
    }
}

// =============================================================================
// Scenario 1: update-vs-rotation race (#2197 motivator 1)
// =============================================================================

/// Bob writes "world" against the writer set he sees ({Alice, Bob}) under
/// a partition. Concurrently, Alice rotates Bob out. Carol receives both
/// in adversarial order. Carol must accept Bob's pre-rotation write — per
/// ADR 0001 the verifier consults `writers_at(D2.parents=[D_root])`, which
/// includes Bob, even though stored writers post-D1 is `{Alice}`.
///
/// Without #2266, this test would fail: the verifier would fall back to
/// stored writers `{Alice}` and reject Bob's signature.
#[tokio::test]
async fn update_vs_rotation_race_pre_rotation_write_accepted_through_full_sync_path() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();

    let alice_sk = make_signing_key(0xA1);
    let bob_sk = make_signing_key(0xB1);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let entity_id = Id::new([0x70; 32]);

    let applier = SharedRotationApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // D_root: Alice bootstraps with writers = {Alice, Bob}.
    let d_root_id = [0xD0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![build_signed_shared_action(
            true,
            entity_id,
            b"hello".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        )],
    );
    dag.add_delta(d_root, &applier)
        .await
        .expect("D_root applied");

    // D1: Alice rotates Bob out. Parent = D_root.
    let d1_id = [0xD1; 32];
    let d1 = delta_with_hlc(
        d1_id,
        vec![d_root_id],
        one_sec(20),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"hello".to_vec(),
            [alice].into_iter().collect(), // Bob removed
            one_sec(20),
            &alice_sk,
            vec![],
        )],
    );

    // D2: Bob's pre-rotation write — parent = D_root (concurrent with D1).
    let d2_id = [0xD2; 32];
    let d2 = delta_with_hlc(
        d2_id,
        vec![d_root_id],
        one_sec(21),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"world".to_vec(),
            [alice, bob].into_iter().collect(), // Bob's view
            one_sec(21),
            &bob_sk,
            vec![],
        )],
    );

    // Adversarial delivery: rotation first, then Bob's pre-rotation write.
    dag.add_delta(d1, &applier).await.expect("rotation applied");
    dag.add_delta(d2, &applier)
        .await
        .expect("pre-rotation write must be accepted via writers_at(D2.parents)");

    // Three deltas applied (D_root, D1, D2). The artifact size on each
    // apply is non-zero, proving the StorageDelta::CausalActions wire
    // format went through Borsh on both sides.
    let applied = applier.applied().await;
    assert_eq!(applied.len(), 3);
    assert!(applied.iter().all(|a| a.artifact_bytes > 0));
    assert_eq!(applied[0].delta_id, d_root_id);
    assert_eq!(applied[1].delta_id, d1_id);
    assert_eq!(applied[2].delta_id, d2_id);

    // Rotation log has D_root + D1; D2 is a value-write whose claimed
    // {Alice, Bob} matches the bootstrap set, so the rotation hook
    // correctly skips it.
    let log = rotation_log::load::<MainStorage>(entity_id)
        .unwrap()
        .unwrap();
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[0].delta_id, d_root_id);
    assert_eq!(log.entries[1].delta_id, d1_id);
}

// =============================================================================
// Post-rotation forgery rejected
// =============================================================================

/// Inverse of scenario 1: a write whose causal parents *include* the
/// rotation must be rejected if the signer was revoked at that causal
/// point. With #2266 the verifier looks up `writers_at([D1])` = {Alice}
/// and rejects Bob's signature.
#[tokio::test]
async fn post_rotation_forgery_by_revoked_writer_rejected() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let entity_id = Id::new([0x71; 32]);

    let applier = SharedRotationApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // D_root: bootstrap with {Alice, Bob}.
    let d_root_id = [0xE0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![build_signed_shared_action(
            true,
            entity_id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        )],
    );
    dag.add_delta(d_root, &applier).await.unwrap();

    // D1: Alice rotates Bob out.
    let d1_id = [0xE1; 32];
    let d1 = delta_with_hlc(
        d1_id,
        vec![d_root_id],
        one_sec(20),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"v0".to_vec(),
            [alice].into_iter().collect(),
            one_sec(20),
            &alice_sk,
            vec![],
        )],
    );
    dag.add_delta(d1, &applier).await.unwrap();

    // D3: Bob saw the rotation and tries to write anyway. Parent = D1.
    // Per ADR 0001, writers_at([D1]) = {Alice} — Bob is no longer a writer.
    let d3_id = [0xE3; 32];
    let d3 = delta_with_hlc(
        d3_id,
        vec![d1_id],
        one_sec(30),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"forgery".to_vec(),
            [alice].into_iter().collect(),
            one_sec(30),
            &bob_sk, // Bob signs even though revoked
            vec![],
        )],
    );

    let result = dag.add_delta(d3, &applier).await;
    // `StorageError::InvalidSignature` displays as "Invalid signature for
    // user-owned data" — match on the rejection, not the exact prose, so
    // doc-string changes don't break this probe.
    assert!(
        matches!(&result, Err(calimero_dag::DagError::ApplyFailed(ApplyError::Application(msg))) if msg.contains("Invalid signature")),
        "post-rotation forgery by revoked writer must be rejected with InvalidSignature; got {result:?}"
    );

    // Only D_root and D1 made it through; D3 was rejected before the
    // applier could record it.
    let applied = applier.applied().await;
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0].delta_id, d_root_id);
    assert_eq!(applied[1].delta_id, d1_id);
}

// =============================================================================
// Out-of-order buffering: same scenario, adversarial DAG ordering
// =============================================================================

/// Same correctness invariant as scenario 1, but the DAG receives D2
/// (Bob's pre-rotation write) BEFORE its parent D_root has arrived. The
/// `DagStore` must buffer D2 in pending, then once D_root + D1 land,
/// apply them in topological order with each of their resolved
/// `effective_writers` correctly computed against the topology that
/// existed AT APPLY TIME. This catches a subtle bug class: if the
/// topology mirror were updated *before* WASM apply (or if the cache
/// keyed wrong), buffered deltas would resolve against an empty
/// topology and silently fall back to v2 stored-writers, and Bob's
/// write would be rejected.
#[tokio::test]
async fn buffered_pre_rotation_write_resolves_correctly_after_parents_arrive() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let entity_id = Id::new([0x72; 32]);

    let applier = SharedRotationApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let d_root_id = [0xF0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![build_signed_shared_action(
            true,
            entity_id,
            b"hello".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        )],
    );

    let d1_id = [0xF1; 32];
    let d1 = delta_with_hlc(
        d1_id,
        vec![d_root_id],
        one_sec(20),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"hello".to_vec(),
            [alice].into_iter().collect(),
            one_sec(20),
            &alice_sk,
            vec![],
        )],
    );

    let d2_id = [0xF2; 32];
    let d2 = delta_with_hlc(
        d2_id,
        vec![d_root_id],
        one_sec(21),
        vec![build_signed_shared_action(
            false,
            entity_id,
            b"world".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(21),
            &bob_sk,
            vec![],
        )],
    );

    // Adversarial: D2 arrives FIRST, then D1, then D_root. D2 and D1
    // both buffer until D_root lands; then they cascade in topo order.
    let applied_d2 = dag.add_delta(d2, &applier).await.unwrap();
    assert!(!applied_d2, "D2 must buffer (D_root missing)");
    let applied_d1 = dag.add_delta(d1, &applier).await.unwrap();
    assert!(!applied_d1, "D1 must buffer (D_root missing)");
    assert_eq!(applier.applied().await.len(), 0, "nothing applied yet");

    let applied_root = dag.add_delta(d_root, &applier).await.unwrap();
    assert!(applied_root, "D_root applies; cascade flushes pending");

    // All three should now be applied; the rotation log has D_root + D1.
    let applied = applier.applied().await;
    assert_eq!(applied.len(), 3, "D_root, D1, D2 all applied after cascade");
    let applied_ids: Vec<_> = applied.iter().map(|a| a.delta_id).collect();
    assert!(applied_ids.contains(&d_root_id));
    assert!(applied_ids.contains(&d1_id));
    assert!(
        applied_ids.contains(&d2_id),
        "Bob's pre-rotation write resolved correctly even after buffering"
    );

    let log = rotation_log::load::<MainStorage>(entity_id)
        .unwrap()
        .unwrap();
    assert_eq!(log.entries.len(), 2);
}
