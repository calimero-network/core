//! End-to-end probe for the governance-position `Shared` verifier wiring.
//!
//! Drives `CausalDelta`s through `calimero_dag::DagStore` with a custom
//! [`DeltaApplier`] that mirrors the production `ContextStorageApplier::apply`
//! flow minus the WASM hop:
//!
//!   1. Look up the governance position the delta was signed at, remembered by delta id when
//!      it was handed to the store, so a delta buffered for its parents is judged at its own
//!      position when it finally applies.
//!   2. Resolve `effective_writers` per Shared entity from the governance fold at that
//!      position: a real [`RotationWorld`], fed with real rotation ops.
//!   3. Serialize a [`StorageDelta::CausalActions`] artifact with that map
//!      (Borsh roundtrip, mirroring what `__calimero_sync_next` would receive).
//!   4. On the receive side, deserialize and dispatch each action through
//!      [`Interface::apply_action`] with a per-action `ApplyContext`
//!      whose `effective_writers` is keyed on `action.id()`, exactly what
//!      `Root::sync`'s `CausalActions` branch does in WASM.
//!
//! A rotation is a governance op: it is not a delta in the data DAG, so it cannot be delivered
//! out of order with the writes it affects. What the DAG does reorder is the data deltas, and
//! each is judged against the position it carries, whatever order they arrive in.
//!
//! Tests in this file:
//!
//! - **`update_vs_rotation_race_pre_rotation_write_accepted_through_full_sync_path`**
//!   a writer's write signed before a rotation is accepted even when delivered AFTER this node
//!   has folded the rotation that removes them.
//!
//! - **`post_rotation_forgery_by_revoked_writer_rejected`** a write signed at a position that
//!   includes the rotation is rejected when its signer was removed by it.
//!
//! - **`buffered_pre_rotation_write_resolves_correctly_after_parents_arrive`** the same, with
//!   adversarial DAG delivery that forces `DagStore` to buffer the writes until their parent
//!   arrives.

use std::collections::HashMap;
use std::sync::Arc;

use borsh::{from_slice, to_vec};
use calimero_account::AccountId;
use calimero_context::test_support::RotationWorld;
use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier, DeltaKind};
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{full_mask, ChildInfo, Metadata, StorageType};
use calimero_storage::index::Index;
use calimero_storage::interface::{
    disable_nonce_check_for_testing, ApplyContext, Interface, StorageError,
};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::shared_writers::{CellWriters, Writers};
use calimero_storage::store::MainStorage;
use calimero_storage::tests::common::{build_signed_shared_action, cell_at, pubkey_of};
use core::num::NonZeroU128;
use ed25519_dalek::SigningKey;
use tokio::sync::RwLock;

// =============================================================================
// Helpers
// =============================================================================

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn one_sec(n: u64) -> u64 {
    n.saturating_mul(1_000_000_000)
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU64::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

fn setup_root() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<MainStorage>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    // Return the post-`add_root` full_hash so callers using this as an
    // ancestor pass the v2 `verify_ancestor_integrity` check at apply
    // time. Without this every test using `setup_root` would trip
    // `TreeStateMismatch`.
    let (full_hash, _) = Index::<MainStorage>::get_hashes_for(root_id)
        .unwrap()
        .unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

// `build_signed_shared_action` / `pubkey_of` come from
// `calimero_storage::tests::common`, shared cross-crate via the `testing`
// feature.

// =============================================================================
// SharedRotationApplier: production-flow mirror minus the WASM hop
// =============================================================================

/// `DeltaApplier` that mirrors `ContextStorageApplier::apply` from
/// `crates/node/src/delta_store.rs`, with the WASM execute call replaced by direct dispatch
/// into [`Interface::apply_action`]. This keeps the test fast and dependency-free while still
/// exercising:
///
/// - the per-Shared-entity resolution at the delta's governance position,
/// - Borsh roundtrip of [`StorageDelta::CausalActions`],
/// - the receiver-side variant branching that builds a per-action
///   [`ApplyContext`] from the resolved map.
struct SharedRotationApplier {
    /// The governance this node holds, folded from real ops.
    world: Arc<RotationWorld>,
    /// The governance heads each delta was signed at, by delta id.
    positions: RwLock<HashMap<[u8; 32], Vec<[u8; 32]>>>,
    /// The account each delta's author speaks for, by delta id: the node's at-cut resolution
    /// of the key that signed it.
    authors: RwLock<HashMap<[u8; 32], AccountId>>,
    /// Successful-apply log (id + serialized artifact size and the writers handed to storage)
    /// for assertions.
    applied: Arc<RwLock<Vec<AppliedDelta>>>,
}

#[derive(Debug, Clone)]
struct AppliedDelta {
    delta_id: [u8; 32],
    /// Size of the serialized `StorageDelta::CausalActions` artifact.
    /// Asserts the wire format went through Borsh on both sides.
    artifact_bytes: usize,
    /// What the node resolved for the delta's cells.
    effective_writers: std::collections::BTreeMap<Id, Writers>,
}

impl SharedRotationApplier {
    fn new(world: Arc<RotationWorld>) -> Self {
        Self {
            world,
            positions: RwLock::default(),
            authors: RwLock::default(),
            applied: Arc::default(),
        }
    }

    /// Hand the store `delta` as its author signed it, at `position`.
    async fn accept(
        &self,
        delta: &CausalDelta<Vec<Action>>,
        author: AccountId,
        position: &[[u8; 32]],
    ) {
        let _ = self
            .positions
            .write()
            .await
            .insert(delta.id, position.to_vec());
        let _ = self.authors.write().await.insert(delta.id, author);
    }

    async fn applied(&self) -> Vec<AppliedDelta> {
        self.applied.read().await.clone()
    }

    /// The writers the governance fold gives each Shared entity touched by `delta`, at the
    /// delta's own position. A cell that never rotated is absent: storage holds its set.
    async fn resolve_effective_writers(
        &self,
        delta: &CausalDelta<Vec<Action>>,
    ) -> Result<std::collections::BTreeMap<Id, Writers>, ApplyError> {
        let position = self
            .positions
            .read()
            .await
            .get(&delta.id)
            .cloned()
            .ok_or_else(|| ApplyError::Application("the delta carries no position".to_owned()))?;
        let mut out = std::collections::BTreeMap::new();
        for action in &delta.payload {
            let metadata = match action {
                Action::Add { metadata, .. }
                | Action::Update { metadata, .. }
                | Action::DeleteRef { metadata, .. } => metadata,
            };
            if !matches!(metadata.storage_type, StorageType::Shared { .. }) {
                continue;
            }
            let folded = self
                .world
                .projections
                .read()
                .unwrap()
                .shared_writers_at_cut(
                    &self.world.store,
                    &self.world.context,
                    action.id(),
                    &position,
                )
                .map_err(|e| ApplyError::Application(format!("writers unavailable: {e:?}")))?;
            if let CellWriters::Rotated(writers) = folded {
                let _previous = out.insert(action.id(), writers);
            }
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for SharedRotationApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Step 1: resolve.
        let effective_writers = self.resolve_effective_writers(delta).await?;

        // Step 2: serialize the CausalActions artifact, mirroring what the
        // sender ships across the wire to `__calimero_sync_next`.
        let artifact = to_vec(&StorageDelta::CausalActions {
            actions: delta.payload.clone(),
            delta_id: delta.id,
            delta_hlc: delta.hlc,
            effective_writers: effective_writers.clone(),
            // The applying node's resolution of this delta's author. One delta,
            // one author, so one account answers for the whole batch.
            signer_account: self.authors.read().await.get(&delta.id).copied(),
        })
        .map_err(|e| ApplyError::Application(format!("serialize artifact: {e}")))?;
        let artifact_size = artifact.len();

        // Step 3: receiver side: deserialize and dispatch. Mirrors
        // `Root::sync`'s `CausalActions` branch.
        let storage_delta: StorageDelta = from_slice(&artifact)
            .map_err(|e| ApplyError::Application(format!("deserialize artifact: {e}")))?;
        let (actions, recv_delta_id, recv_hlc, recv_writers, recv_account) = match storage_delta {
            StorageDelta::CausalActions {
                actions,
                delta_id,
                delta_hlc,
                effective_writers,
                signer_account,
            } => (
                actions,
                delta_id,
                delta_hlc,
                effective_writers,
                signer_account,
            ),
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
                signer_account: recv_account,
            };
            Interface::<MainStorage>::apply_action(action.clone(), &ctx)
                .map_err(|e: StorageError| ApplyError::Application(e.to_string()))?;
        }

        // Step 4: tally.
        self.applied.write().await.push(AppliedDelta {
            delta_id: delta.id,
            artifact_bytes: artifact_size,
            effective_writers,
        });
        Ok(())
    }
}

/// Build a `CausalDelta` with explicit HLC.
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
        kind: DeltaKind::Regular,
    }
}

/// Alice and Bob share a cell; `world` is the governance this node holds.
struct Cell {
    world: Arc<RotationWorld>,
    alice_sk: SigningKey,
    bob_sk: SigningKey,
    alice: AccountId,
    bob: AccountId,
    id: Id,
}

impl Cell {
    fn new(seed: u8, key_seed: (u8, u8)) -> Self {
        let alice_sk = make_signing_key(key_seed.0);
        let bob_sk = make_signing_key(key_seed.1);
        let world = Arc::new(RotationWorld::new(&[
            pubkey_of(&alice_sk),
            pubkey_of(&bob_sk),
        ]));
        let (alice, bob) = (
            world.account(&pubkey_of(&alice_sk)),
            world.account(&pubkey_of(&bob_sk)),
        );
        let id = cell_at(seed, &[alice, bob].into_iter().collect());
        Cell {
            world,
            alice_sk,
            bob_sk,
            alice,
            bob,
            id,
        }
    }

    fn joined(&self) -> Vec<[u8; 32]> {
        self.world.joined()
    }

    /// Governance: Alice rotates Bob out, as op `id` on the joined cut.
    fn rotate_bob_out(&self, id: [u8; 32]) {
        self.world.rotate(
            &pubkey_of(&self.alice_sk),
            self.id,
            id,
            &self.joined(),
            full_mask([self.alice, self.bob].into_iter().collect()),
            1,
            full_mask([self.alice].into_iter().collect()),
        );
    }

    /// A write to the cell, claiming the writers its id commits to.
    fn write(
        &self,
        create: bool,
        data: &[u8],
        hlc_ns: u64,
        signer: &SigningKey,
        root: &ChildInfo,
    ) -> Action {
        build_signed_shared_action(
            create,
            self.id,
            data.to_vec(),
            [self.alice, self.bob].into_iter().collect(),
            hlc_ns,
            signer,
            if create { vec![root.clone()] } else { vec![] },
        )
    }
}

// =============================================================================
// Scenario 1: update-vs-rotation race (#2197 motivator 1)
// =============================================================================

/// Bob writes "world" against the writer set he sees ({Alice, Bob}) under a partition.
/// Concurrently, Alice rotates Bob out. This node folds the rotation, then receives Bob's write.
/// It must accept it: Bob signed at a governance position before the rotation, where the
/// writers include him, even though the rotation is in this node's governance now.
#[tokio::test]
async fn update_vs_rotation_race_pre_rotation_write_accepted_through_full_sync_path() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();
    let cell = Cell::new(0x70, (0xA1, 0xB1));
    let applier = SharedRotationApplier::new(Arc::clone(&cell.world));
    let mut dag = DagStore::new([0; 32]);

    // D_root: Alice bootstraps with writers = {Alice, Bob}.
    let d_root_id = [0xD0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![cell.write(true, b"hello", one_sec(10), &cell.alice_sk, &root)],
    );
    applier.accept(&d_root, cell.alice, &cell.joined()).await;
    dag.add_delta(d_root, &applier)
        .await
        .expect("D_root applied");

    // R1: Alice rotates Bob out. This node folds it before Bob's write arrives.
    let r1 = [0xD1; 32];
    cell.rotate_bob_out(r1);

    // D2: Bob's pre-rotation write, signed at the joined position, parent = D_root.
    let d2_id = [0xD2; 32];
    let d2 = delta_with_hlc(
        d2_id,
        vec![d_root_id],
        one_sec(21),
        vec![cell.write(false, b"world", one_sec(21), &cell.bob_sk, &root)],
    );
    applier.accept(&d2, cell.bob, &cell.joined()).await;
    dag.add_delta(d2, &applier)
        .await
        .expect("a pre-rotation write must be accepted at the position it was signed at");

    // D3: Alice's write after the rotation, signed at a position that includes it.
    let d3_id = [0xD3; 32];
    let d3 = delta_with_hlc(
        d3_id,
        vec![d2_id],
        one_sec(30),
        vec![cell.write(false, b"after", one_sec(30), &cell.alice_sk, &root)],
    );
    applier.accept(&d3, cell.alice, &[r1]).await;
    dag.add_delta(d3, &applier)
        .await
        .expect("Alice writes after the rotation");

    // Three deltas applied. The artifact size on each apply is non-zero, proving the
    // StorageDelta::CausalActions wire format went through Borsh on both sides.
    let applied = applier.applied().await;
    assert_eq!(applied.len(), 3);
    assert!(applied.iter().all(|a| a.artifact_bytes > 0));
    assert_eq!(applied[0].delta_id, d_root_id);
    assert_eq!(applied[1].delta_id, d2_id);
    assert_eq!(applied[2].delta_id, d3_id);
    assert!(
        applied[1].effective_writers.is_empty(),
        "at the position Bob signed at, the cell stands at the set its id commits to"
    );
    assert_eq!(
        applied[2].effective_writers[&cell.id],
        full_mask([cell.alice].into_iter().collect()),
        "after the rotation the fold gives the rotated set"
    );
}

// =============================================================================
// Post-rotation forgery rejected
// =============================================================================

/// Inverse of scenario 1: a write signed at a position that *includes* the rotation must be
/// rejected if the signer was removed by it.
#[tokio::test]
async fn post_rotation_forgery_by_revoked_writer_rejected() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();
    let cell = Cell::new(0x71, (0xA2, 0xB2));
    let applier = SharedRotationApplier::new(Arc::clone(&cell.world));
    let mut dag = DagStore::new([0; 32]);

    // D_root: bootstrap with {Alice, Bob}.
    let d_root_id = [0xE0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![cell.write(true, b"v0", one_sec(10), &cell.alice_sk, &root)],
    );
    applier.accept(&d_root, cell.alice, &cell.joined()).await;
    dag.add_delta(d_root, &applier).await.unwrap();

    // R1: Alice rotates Bob out.
    let r1 = [0xE2; 32];
    cell.rotate_bob_out(r1);

    // D3: Bob saw the rotation and tries to write anyway, at a position that includes it.
    let d3 = delta_with_hlc(
        [0xE3; 32],
        vec![d_root_id],
        one_sec(30),
        vec![cell.write(false, b"forgery", one_sec(30), &cell.bob_sk, &root)],
    );
    // Bob authored the forgery, so the node resolves BOB's account, the honest resolution. He
    // was rotated out at R1, so the rejection below is authorization at the cut rather than a
    // mismatched principal.
    applier.accept(&d3, cell.bob, &[r1]).await;
    let result = dag.add_delta(d3, &applier).await;
    // Match on the rejection, not the exact prose, so a change to
    // `StorageError::InvalidSignature`'s Display text does not break this probe.
    assert!(
        matches!(&result, Err(calimero_dag::DagError::ApplyFailed(ApplyError::Application(msg))) if msg.contains("Invalid signature")),
        "post-rotation forgery by revoked writer must be rejected with InvalidSignature; got {result:?}"
    );

    // Only D_root made it through; D3 was rejected before the applier could record it.
    let applied = applier.applied().await;
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].delta_id, d_root_id);
}

// =============================================================================
// Out-of-order buffering: same scenario, adversarial DAG ordering
// =============================================================================

/// Same invariant as scenario 1, but the DAG receives both writes BEFORE their parent D_root
/// has arrived. The `DagStore` buffers them, then once D_root lands applies them in topological
/// order, each resolved at ITS OWN position: Bob's pre-rotation write against the set before the
/// rotation, Alice's later write against the rotated set. The position belongs to the delta, not
/// to the moment it happens to apply, so the order the DAG releases them in cannot change a
/// verdict.
#[tokio::test]
async fn buffered_pre_rotation_write_resolves_correctly_after_parents_arrive() {
    let _nonce_off = disable_nonce_check_for_testing();
    let root = setup_root();
    let cell = Cell::new(0x72, (0xA3, 0xB3));
    let applier = SharedRotationApplier::new(Arc::clone(&cell.world));
    let mut dag = DagStore::new([0; 32]);

    let d_root_id = [0xF0; 32];
    let d_root = delta_with_hlc(
        d_root_id,
        vec![[0; 32]],
        one_sec(10),
        vec![cell.write(true, b"hello", one_sec(10), &cell.alice_sk, &root)],
    );
    let r1 = [0xF1; 32];
    cell.rotate_bob_out(r1);

    let d2_id = [0xF2; 32];
    let d2 = delta_with_hlc(
        d2_id,
        vec![d_root_id],
        one_sec(21),
        vec![cell.write(false, b"world", one_sec(21), &cell.bob_sk, &root)],
    );
    let d3_id = [0xF3; 32];
    let d3 = delta_with_hlc(
        d3_id,
        vec![d_root_id],
        one_sec(22),
        vec![cell.write(false, b"later", one_sec(22), &cell.alice_sk, &root)],
    );
    applier.accept(&d_root, cell.alice, &cell.joined()).await;
    applier.accept(&d2, cell.bob, &cell.joined()).await;
    applier.accept(&d3, cell.alice, &[r1]).await;

    // Adversarial: the writes arrive FIRST, then D_root. Both buffer until it lands.
    let applied_d3 = dag.add_delta(d3, &applier).await.unwrap();
    assert!(!applied_d3, "D3 must buffer (D_root missing)");
    let applied_d2 = dag.add_delta(d2, &applier).await.unwrap();
    assert!(!applied_d2, "D2 must buffer (D_root missing)");
    assert_eq!(applier.applied().await.len(), 0, "nothing applied yet");

    let applied_root = dag.add_delta(d_root, &applier).await.unwrap();
    assert!(applied_root, "D_root applies; cascade flushes pending");

    let applied = applier.applied().await;
    assert_eq!(applied.len(), 3, "D_root, D2, D3 all applied after cascade");
    let by_id = |id: [u8; 32]| applied.iter().find(|a| a.delta_id == id).cloned();
    assert!(by_id(d_root_id).is_some());
    assert!(
        by_id(d2_id).is_some_and(|a| a.effective_writers.is_empty()),
        "Bob's pre-rotation write resolved at the position he signed at, even after buffering"
    );
    assert_eq!(
        by_id(d3_id).map(|a| a.effective_writers[&cell.id].clone()),
        Some(full_mask([cell.alice].into_iter().collect())),
        "Alice's later write resolved against the rotated set"
    );
}
