//! Tests for phase **P3** of [#2233](https://github.com/calimero-network/core/issues/2233):
//!
//! - **Verifier swap.** When `ApplyContext` carries DAG-causal information
//!   (resolved `effective_writers`), `Interface::apply_action` validates
//!   `Shared` signatures against that set instead of stored writers.
//!   When the context is empty, behavior matches v2 exactly.
//! - **Write hook.** Successful applies of `Shared` rotations append a
//!   [`RotationLogEntry`]. Value-writes (writers unchanged) and ctx without
//!   `delta_id`/`delta_hlc` are no-ops.
//!
//! Migrated from `calimero_storage::tests::p3_dag_causal` per #2266 step 5.
//! The closure-typed `happens_before` ApplyContext field is gone; resolution
//! happens in this crate's `rotation_log_reader::writers_at` against a DAG
//! the test owns. The single storage-layer write-hook stale-writers
//! regression stays in `calimero_storage::tests::write_hook` (it asserts
//! a storage-internal invariant; no DAG needed).

use core::num::NonZeroU128;
use std::collections::{BTreeSet, HashMap, HashSet};

use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata, SignatureData, StorageType};
use calimero_storage::index::Index;
use calimero_storage::interface::{ApplyContext, Interface, StorageError};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::rotation_log::{self, RotationLogEntry};
use calimero_storage::store::{MockedStorage, StorageAdaptor};
use ed25519_dalek::{Signer, SigningKey};

use crate::sync::rotation_log_reader;

// =============================================================================
// Harness
// =============================================================================

/// Minimal DAG mirror used by tests to provide a `happens_before` predicate
/// over delta ids — same shape as P5's `Dag`, scoped here so each test
/// builds the DAG it needs.
struct Dag {
    parents: HashMap<[u8; 32], Vec<[u8; 32]>>,
}

impl Dag {
    fn new() -> Self {
        Self {
            parents: HashMap::new(),
        }
    }

    fn record(&mut self, delta_id: [u8; 32], parents: Vec<[u8; 32]>) {
        self.parents.insert(delta_id, parents);
    }

    fn happens_before(&self, ancestor: &[u8; 32], descendant: &[u8; 32]) -> bool {
        if ancestor == descendant {
            return false;
        }
        let mut frontier: Vec<[u8; 32]> = self.parents.get(descendant).cloned().unwrap_or_default();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if !seen.insert(node) {
                continue;
            }
            if node == *ancestor {
                return true;
            }
            if let Some(ps) = self.parents.get(&node) {
                frontier.extend(ps.iter().copied());
            }
        }
        false
    }
}

// Each test uses a unique mocked-storage scope so they don't bleed into each
// other (the mock store is a thread-local BTreeMap keyed on (scope, key)).
type S<const SCOPE: usize> = MockedStorage<SCOPE>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey_of(sk: &SigningKey) -> PublicKey {
    PublicKey::from(*sk.verifying_key().as_bytes())
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

/// Returns a HLC nanosecond value rooted at "now" plus `step` seconds.
fn hlc_at(step: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    now.saturating_add(step.saturating_mul(1_000_000_000))
}

/// Pre-create the root index entry so non-root child entities can be
/// added/updated by `apply_action` without tripping `IndexNotFound`.
fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    ChildInfo::new(root_id, [0; 32], root_meta)
}

fn entity_id(seed: u8) -> Id {
    Id::new([seed; 32])
}

/// Build a signed `Shared` action — see notes on the storage-side helper
/// (`tests::common::build_signed_shared_action`) for invariants.
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

/// Build an `ApplyContext` for `apply_action` by resolving `effective_writers`
/// against the rotation log + DAG, mirroring the production sync-layer flow.
fn ctx_for<S: StorageAdaptor>(
    entity: Id,
    parents: &[[u8; 32]],
    delta_id: [u8; 32],
    delta_hlc_ns: u64,
    dag: &Dag,
) -> ApplyContext {
    let effective_writers = match rotation_log::load::<S>(entity).unwrap() {
        Some(log) => {
            rotation_log_reader::writers_at(&log, parents, |a, b| dag.happens_before(a, b))
        }
        None => None,
    };
    ApplyContext {
        effective_writers,
        delta_id: Some(delta_id),
        delta_hlc: Some(hlc(delta_hlc_ns)),
    }
}

// =============================================================================
// Verifier-swap tests
// =============================================================================

/// Baseline: with no DAG context, the verifier behaves exactly like v2 — sig
/// must verify against the stored writer set, falling back to the action's
/// claim when the entity doesn't exist yet (bootstrap).
#[test]
fn verifier_without_dag_context_uses_stored_writers() {
    let root = setup_root::<S<6400>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x40);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6400>>::apply_action(bootstrap, &ApplyContext::empty())
        .expect("bootstrap accepted with v2 fallback");

    let update = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<6400>>::apply_action(update, &ApplyContext::empty())
        .expect("update by writer accepted");
}

/// Action signed by a non-writer is rejected by the v2 path.
#[test]
fn verifier_without_dag_context_rejects_non_writer() {
    let root = setup_root::<S<6401>>();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x41);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6401>>::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

    // Bob is not in the stored writer set — must reject.
    let forged = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &bob_sk,
        vec![],
    );
    let result = Interface::<S<6401>>::apply_action(forged, &ApplyContext::empty());
    assert!(matches!(result, Err(StorageError::InvalidSignature)));
}

/// **The partition-correctness fix.** A pre-populated rotation log says the
/// writer set as-of `D1` is `{Alice}`. The stored entity (simulating
/// divergent state from a partition) has `{Bob}`. An action signed by Alice
/// with `causal_parents = [D1]` must be accepted: per ADR 0001 the verifier
/// consults the rotation log resolution, not stored.
#[test]
fn verifier_with_dag_context_uses_rotation_log() {
    let root = setup_root::<S<6402>>();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x42);

    // Bootstrap with Bob as the stored writer.
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"divergent".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(0),
        &bob_sk,
        vec![root.clone()],
    );
    Interface::<S<6402>>::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

    // Pre-populate the rotation log: as-of delta D1, the writer set was {Alice}.
    let d1 = [0xD1; 32];
    rotation_log::append::<S<6402>>(
        id,
        RotationLogEntry {
            delta_id: d1,
            delta_hlc: hlc(hlc_at(0)),
            signer: Some(alice),
            new_writers: [alice].into_iter().collect(),
            writers_nonce: 1,
        },
    )
    .unwrap();

    // Alice signs an Update; ctx points at D1 as a causal parent.
    let action = build_signed_shared_action(
        false,
        id,
        b"alice-update".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(2),
        &alice_sk,
        vec![],
    );
    let mut dag = Dag::new();
    dag.record(d1, vec![]);
    let ctx = ctx_for::<S<6402>>(id, &[d1], [0xD2; 32], hlc_at(2), &dag);

    // Without DAG-causal this would be rejected (sig vs stored {Bob} fails).
    // With DAG-causal it's accepted because writers_at returns {Alice}.
    Interface::<S<6402>>::apply_action(action, &ctx)
        .expect("DAG-causal verifier accepts Alice — she's the writer as-of D1");
}

/// Even with DAG context, an action signed by someone *outside* the causal
/// writer set is rejected.
#[test]
fn verifier_with_dag_context_rejects_non_causal_writer() {
    let root = setup_root::<S<6403>>();

    let alice_sk = make_signing_key(0xA4);
    let bob_sk = make_signing_key(0xB4);
    let mallory_sk = make_signing_key(0xC4);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x43);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(0),
        &bob_sk,
        vec![root.clone()],
    );
    Interface::<S<6403>>::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

    let d1 = [0xD1; 32];
    rotation_log::append::<S<6403>>(
        id,
        RotationLogEntry {
            delta_id: d1,
            delta_hlc: hlc(hlc_at(0)),
            signer: Some(alice),
            new_writers: [alice].into_iter().collect(),
            writers_nonce: 1,
        },
    )
    .unwrap();

    // Mallory is in neither stored {Bob} nor causal {Alice}.
    let forged = build_signed_shared_action(
        false,
        id,
        b"forged".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(2),
        &mallory_sk,
        vec![],
    );
    let mut dag = Dag::new();
    dag.record(d1, vec![]);
    let ctx = ctx_for::<S<6403>>(id, &[d1], [0xD2; 32], hlc_at(2), &dag);

    let result = Interface::<S<6403>>::apply_action(forged, &ctx);
    assert!(matches!(result, Err(StorageError::InvalidSignature)));
}

// =============================================================================
// Write-hook tests
// =============================================================================

/// Bootstrap with full DAG context appends one rotation log entry.
#[test]
fn write_hook_appends_on_bootstrap_with_ctx() {
    let root = setup_root::<S<6404>>();

    let alice_sk = make_signing_key(0xA5);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x44);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let dag = Dag::new();
    let ctx = ctx_for::<S<6404>>(id, &[], [0xAA; 32], hlc_at(0), &dag);
    Interface::<S<6404>>::apply_action(bootstrap, &ctx).unwrap();

    let log = rotation_log::load::<S<6404>>(id)
        .unwrap()
        .expect("rotation log exists after Shared apply with delta ctx");
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].delta_id, [0xAA; 32]);
    assert_eq!(log.entries[0].signer, Some(alice));
    assert_eq!(log.entries[0].new_writers, [alice].into_iter().collect());
}

/// Same bootstrap but with empty ctx (no delta_id) — the log stays empty.
/// Local-apply / snapshot-leaf paths behave like this.
#[test]
fn write_hook_skips_when_ctx_lacks_delta_id() {
    let root = setup_root::<S<6405>>();

    let alice_sk = make_signing_key(0xA6);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x45);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6405>>::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

    assert_eq!(rotation_log::load::<S<6405>>(id).unwrap(), None);
}

/// Value-write (writer set unchanged) does not append an entry.
#[test]
fn write_hook_skips_when_writers_unchanged() {
    let root = setup_root::<S<6406>>();

    let alice_sk = make_signing_key(0xA7);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x46);

    let mut dag = Dag::new();

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let bootstrap_id = [0xBB; 32];
    dag.record(bootstrap_id, vec![]);
    Interface::<S<6406>>::apply_action(
        bootstrap,
        &ctx_for::<S<6406>>(id, &[], bootstrap_id, hlc_at(0), &dag),
    )
    .unwrap();
    assert_eq!(
        rotation_log::load::<S<6406>>(id)
            .unwrap()
            .unwrap()
            .entries
            .len(),
        1
    );

    // Value-write with the same writer set → log stays at 1 entry.
    let value_write = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(), // same set
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    let vw_id = [0xCC; 32];
    dag.record(vw_id, vec![bootstrap_id]);
    Interface::<S<6406>>::apply_action(
        value_write,
        &ctx_for::<S<6406>>(id, &[bootstrap_id], vw_id, hlc_at(1), &dag),
    )
    .unwrap();

    let log = rotation_log::load::<S<6406>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 1, "value-write did not append");
}

/// Genuine rotation (writer set changes) appends a second entry.
#[test]
fn write_hook_appends_on_writer_set_change() {
    let root = setup_root::<S<6407>>();

    let alice_sk = make_signing_key(0xA8);
    let bob_sk = make_signing_key(0xB8);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x47);

    let mut dag = Dag::new();

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let d0 = [0xD0; 32];
    dag.record(d0, vec![]);
    Interface::<S<6407>>::apply_action(
        bootstrap,
        &ctx_for::<S<6407>>(id, &[], d0, hlc_at(0), &dag),
    )
    .unwrap();

    // Alice rotates: now writers = {Alice, Bob}.
    let rotation = build_signed_shared_action(
        false,
        id,
        b"v0".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    let d1 = [0xD1; 32];
    dag.record(d1, vec![d0]);
    Interface::<S<6407>>::apply_action(
        rotation,
        &ctx_for::<S<6407>>(id, &[d0], d1, hlc_at(1), &dag),
    )
    .unwrap();

    let log = rotation_log::load::<S<6407>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[1].delta_id, d1);
    assert_eq!(
        log.entries[1].new_writers,
        [alice, bob].into_iter().collect()
    );
}

// =============================================================================
// ADR Example D coverage (write vs rotate on the same entity)
// =============================================================================

/// ADR Example D: pre-rotation value-write is accepted even after the
/// rotation that removes the signer is applied locally. The verifier must
/// consult `writers_at(value_write.parents)`, NOT the post-merge writer set.
#[test]
fn adr_example_d_pre_rotation_write_accepted_after_rotation() {
    let root = setup_root::<S<6420>>();

    let alice_sk = make_signing_key(0xA9);
    let bob_sk = make_signing_key(0xB9);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x60);

    let mut dag = Dag::new();

    // D_root: writers = {Alice, Bob}. Bootstrap so the entity exists locally.
    let d_root = [0xD0; 32];
    dag.record(d_root, vec![]);
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6420>>::apply_action(
        bootstrap,
        &ctx_for::<S<6420>>(id, &[], d_root, hlc_at(0), &dag),
    )
    .unwrap();

    // D1 (concurrent sibling of D_root from D2's perspective): Alice rotates
    // Bob out → writers = {Alice}. Apply this first.
    let d1 = [0xD1; 32];
    dag.record(d1, vec![d_root]);
    let rotation = build_signed_shared_action(
        false,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<6420>>::apply_action(
        rotation,
        &ctx_for::<S<6420>>(id, &[d_root], d1, hlc_at(1), &dag),
    )
    .unwrap();

    // Sanity: the local stored writer set is now {Alice} and the rotation log
    // has two entries (bootstrap + rotation).
    let log = rotation_log::load::<S<6420>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2);

    // D2 (concurrent sibling of D1): Bob writes "world" against the writer
    // set he saw — {Alice, Bob}. From Bob's local view this is valid; D2's
    // parent is D_root, NOT D1.
    let d2 = [0xD2; 32];
    dag.record(d2, vec![d_root]);
    let bob_write = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice, bob].into_iter().collect(), // Bob's view of writers
        hlc_at(2),
        &bob_sk,
        vec![],
    );
    let ctx = ctx_for::<S<6420>>(id, &[d_root], d2, hlc_at(2), &dag);

    // Crucial: even though stored writers (post-D1) is {Alice}, D2 is causally
    // a sibling of D1 — it never saw the rotation. writers_at(D2.parents=[D_root])
    // returns the bootstrap writer set {Alice, Bob}, so Bob's signature
    // verifies. Without DAG-causal this would fail (sig vs stored {Alice}).
    Interface::<S<6420>>::apply_action(bob_write, &ctx).expect(
        "ADR Example D: pre-rotation write by Bob accepted because writers_at \
         (causal parents of D2) includes Bob, even though stored writers no longer do",
    );
}

/// Inverse of Example D: a write whose causal parents *include* the rotation
/// (i.e., the writer saw the rotation and chose to write anyway) must be
/// rejected if the signer is no longer in the writer set as-of those parents.
#[test]
fn write_post_rotation_by_removed_writer_rejected() {
    let root = setup_root::<S<6421>>();

    let alice_sk = make_signing_key(0xAA);
    let bob_sk = make_signing_key(0xBA);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x61);

    let mut dag = Dag::new();

    let d_root = [0xD0; 32];
    dag.record(d_root, vec![]);
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6421>>::apply_action(
        bootstrap,
        &ctx_for::<S<6421>>(id, &[], d_root, hlc_at(0), &dag),
    )
    .unwrap();

    // D1: Alice rotates Bob out.
    let d1 = [0xD1; 32];
    dag.record(d1, vec![d_root]);
    let rotation = build_signed_shared_action(
        false,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<6421>>::apply_action(
        rotation,
        &ctx_for::<S<6421>>(id, &[d_root], d1, hlc_at(1), &dag),
    )
    .unwrap();

    // D2 has D1 as a parent — Bob saw the rotation and tries to write anyway.
    let d2 = [0xD2; 32];
    dag.record(d2, vec![d1]);
    let bob_write_post = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice].into_iter().collect(), // Bob acknowledges the rotation in his claim
        hlc_at(2),
        &bob_sk,
        vec![],
    );
    let ctx = ctx_for::<S<6421>>(id, &[d1], d2, hlc_at(2), &dag);

    // writers_at(D2.parents=[D1]) returns {Alice} — Bob is no longer a writer
    // and his signature must fail.
    let result = Interface::<S<6421>>::apply_action(bob_write_post, &ctx);
    assert!(
        matches!(result, Err(StorageError::InvalidSignature)),
        "post-rotation write by removed writer must be rejected; got {result:?}",
    );
}
