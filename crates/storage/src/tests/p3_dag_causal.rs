//! Tests for phase **P3** of [#2233](https://github.com/calimero-network/core/issues/2233):
//!
//! - **Verifier swap.** When `ApplyContext` carries DAG-causal information,
//!   `Interface::apply_action` validates `Shared` signatures against
//!   `writers_at(causal_parents)` (per ADR 0001) instead of stored writers.
//!   When the context is empty, behavior matches v2 exactly.
//! - **Write hook.** Successful applies of `Shared` rotations append a
//!   [`RotationLogEntry`]. Value-writes (writers unchanged) and ctx without
//!   `delta_id`/`delta_hlc` are no-ops.

use core::num::NonZeroU128;
use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;
use ed25519_dalek::{Signer, SigningKey};

use crate::action::Action;
use crate::address::Id;
use crate::entities::{ChildInfo, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::rotation_log::{self, RotationLogEntry};
use crate::store::{MockedStorage, StorageAdaptor};

// Each test uses a unique mocked-storage scope so they don't bleed into each
// other (the mock store is a thread-local BTreeMap keyed on (scope, key)).
type S<const SCOPE: usize> = MockedStorage<SCOPE>;

// =============================================================================
// Helpers
// =============================================================================

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

/// Returns an HLC nanosecond value rooted at "now" plus `step` seconds.
/// Use this so sequential actions in a test always have strictly-increasing
/// nonces without depending on wall-clock advancement between calls.
fn hlc_at(step: u64) -> u64 {
    env::time_now().saturating_add(step.saturating_mul(1_000_000_000))
}

/// Pre-create the root index entry so non-root child entities can be
/// added/updated by `apply_action` without tripping `IndexNotFound`.
/// Returns the root `ChildInfo` ready to embed in an action's `ancestors`.
fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    ChildInfo::new(root_id, [0; 32], root_meta)
}

/// Construct a signed Shared action. `signer_sk` must be in `writers` (or the
/// caller is intentionally testing a forged action).
///
/// `hlc_ns` is used as BOTH the action's `updated_at` and the signature's
/// `nonce` — matching production where `sign_authorized_actions` stamps both
/// with the action's HLC. Tests must pass strictly increasing values across
/// sequential calls to satisfy the per-entity replay-protection check.
fn build_signed_shared_action(
    add: bool,
    id: Id,
    data: Vec<u8>,
    writers: BTreeSet<PublicKey>,
    hlc_ns: u64,
    signer_sk: &SigningKey,
    ancestors: Vec<ChildInfo>,
) -> Action {
    let metadata = Metadata {
        created_at: hlc_ns,
        updated_at: hlc_ns.into(),
        storage_type: StorageType::Shared {
            writers,
            signature_data: Some(SignatureData {
                signature: [0; 64], // placeholder, filled in below
                nonce: hlc_ns,
                signer: Some(pubkey_of(signer_sk)),
            }),
        },
        crdt_type: None,
        field_name: None,
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

fn empty_ctx<'a>() -> ApplyContext<'a> {
    ApplyContext {
        causal_parents: &[],
        delta_id: None,
        delta_hlc: None,
        happens_before: None,
    }
}

fn dag_ctx<'a>(
    parents: &'a [[u8; 32]],
    delta_id: [u8; 32],
    delta_hlc_ns: u64,
    happens_before: &'a dyn Fn(&[u8; 32], &[u8; 32]) -> bool,
) -> ApplyContext<'a> {
    ApplyContext {
        causal_parents: parents,
        delta_id: Some(delta_id),
        delta_hlc: Some(hlc(delta_hlc_ns)),
        happens_before: Some(happens_before),
    }
}

// Convenient per-test entity id derived from the scope so each test gets a
// distinct, deterministic id (avoids any cross-test interference even if
// scopes were reused).
fn entity_id(seed: u8) -> Id {
    Id::new([seed; 32])
}

// =============================================================================
// Verifier-swap tests
// =============================================================================

/// Baseline: with no DAG context, the verifier behaves exactly like v2 — sig
/// must verify against the stored writer set, falling back to the action's
/// claim when the entity doesn't exist yet (bootstrap).
#[test]
fn verifier_without_dag_context_uses_stored_writers() {
    env::reset_for_testing();
    let root = setup_root::<S<400>>();

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
    Interface::<S<400>>::apply_action(bootstrap, empty_ctx())
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
    Interface::<S<400>>::apply_action(update, empty_ctx()).expect("update by writer accepted");
}

/// Action signed by a non-writer is rejected by the v2 path.
#[test]
fn verifier_without_dag_context_rejects_non_writer() {
    env::reset_for_testing();
    let root = setup_root::<S<401>>();

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
    Interface::<S<401>>::apply_action(bootstrap, empty_ctx()).unwrap();

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
    let result = Interface::<S<401>>::apply_action(forged, empty_ctx());
    assert!(matches!(
        result,
        Err(crate::interface::StorageError::InvalidSignature)
    ));
}

/// **The partition-correctness fix.** A pre-populated rotation log says the
/// writer set as-of `D1` is `{Alice}`. The stored entity (simulating
/// divergent state from a partition) has `{Bob}`. An action signed by Alice
/// with `causal_parents = [D1]` must be accepted: per ADR 0001 the verifier
/// consults the rotation log, not stored.
#[test]
fn verifier_with_dag_context_uses_rotation_log() {
    env::reset_for_testing();
    let root = setup_root::<S<402>>();

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
    Interface::<S<402>>::apply_action(bootstrap, empty_ctx()).unwrap();

    // Pre-populate the rotation log: as-of delta D1, the writer set was {Alice}.
    let d1 = [0xD1; 32];
    rotation_log::append::<S<402>>(
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
    let parents = [d1];
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|a, b| a == &d1 && b == &d1;
    let ctx = dag_ctx(&parents, [0xD2; 32], hlc_at(2), happens_before);

    // Without P3 this would be rejected (sig vs stored {Bob} fails).
    // With P3 it's accepted because writers_at returns {Alice}.
    Interface::<S<402>>::apply_action(action, ctx)
        .expect("DAG-causal verifier accepts Alice — she's the writer as-of D1");
}

/// Even with DAG context, an action signed by someone *outside* the causal
/// writer set is rejected.
#[test]
fn verifier_with_dag_context_rejects_non_causal_writer() {
    env::reset_for_testing();
    let root = setup_root::<S<403>>();

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
    Interface::<S<403>>::apply_action(bootstrap, empty_ctx()).unwrap();

    let d1 = [0xD1; 32];
    rotation_log::append::<S<403>>(
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
    let parents = [d1];
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|a, b| a == &d1 && b == &d1;
    let ctx = dag_ctx(&parents, [0xD2; 32], hlc_at(2), happens_before);

    let result = Interface::<S<403>>::apply_action(forged, ctx);
    assert!(matches!(
        result,
        Err(crate::interface::StorageError::InvalidSignature)
    ));
}

// =============================================================================
// Write-hook tests
// =============================================================================

/// Bootstrap with full DAG context appends one rotation log entry.
#[test]
fn write_hook_appends_on_bootstrap_with_ctx() {
    env::reset_for_testing();
    let root = setup_root::<S<404>>();

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
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    let ctx = dag_ctx(&[], [0xAA; 32], hlc_at(0), happens_before);
    Interface::<S<404>>::apply_action(bootstrap, ctx).unwrap();

    let log = rotation_log::load::<S<404>>(id)
        .unwrap()
        .expect("rotation log exists after Shared apply with delta ctx");
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].delta_id, [0xAA; 32]);
    assert_eq!(log.entries[0].signer, Some(alice));
    assert_eq!(log.entries[0].new_writers, [alice].into_iter().collect());
}

/// Same bootstrap but with empty ctx (no delta_id) — the log stays empty.
/// Production sync paths behave like this until the WASM ABI extension lands.
#[test]
fn write_hook_skips_when_ctx_lacks_delta_id() {
    env::reset_for_testing();
    let root = setup_root::<S<405>>();

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
    Interface::<S<405>>::apply_action(bootstrap, empty_ctx()).unwrap();

    assert_eq!(rotation_log::load::<S<405>>(id).unwrap(), None);
}

/// Value-write (writer set unchanged) does not append an entry.
#[test]
fn write_hook_skips_when_writers_unchanged() {
    env::reset_for_testing();
    let root = setup_root::<S<406>>();

    let alice_sk = make_signing_key(0xA7);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x46);

    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<406>>::apply_action(
        bootstrap,
        dag_ctx(&[], [0xBB; 32], hlc_at(0), happens_before),
    )
    .unwrap();
    assert_eq!(
        rotation_log::load::<S<406>>(id)
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
    Interface::<S<406>>::apply_action(
        value_write,
        dag_ctx(&[], [0xCC; 32], hlc_at(1), happens_before),
    )
    .unwrap();

    let log = rotation_log::load::<S<406>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 1, "value-write did not append");
}

/// Genuine rotation (writer set changes) appends a second entry.
#[test]
fn write_hook_appends_on_writer_set_change() {
    env::reset_for_testing();
    let root = setup_root::<S<407>>();

    let alice_sk = make_signing_key(0xA8);
    let bob_sk = make_signing_key(0xB8);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x47);

    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<407>>::apply_action(
        bootstrap,
        dag_ctx(&[], [0xD0; 32], hlc_at(0), happens_before),
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
    Interface::<S<407>>::apply_action(
        rotation,
        dag_ctx(&[[0xD0; 32]], [0xD1; 32], hlc_at(1), happens_before),
    )
    .unwrap();

    let log = rotation_log::load::<S<407>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[1].delta_id, [0xD1; 32]);
    assert_eq!(
        log.entries[1].new_writers,
        [alice, bob].into_iter().collect()
    );
}
